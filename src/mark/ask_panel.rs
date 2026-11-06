//! The drawing of one firewall panel, after the anatomy of the macOS
//! system permission alert:
//!
//! - The surface is the system popover material (`NSVisualEffectView`),
//!   corner radius 12, with the system shadow. It follows the appearance
//!   of the system, Light or Dark. No color is fixed: every text uses
//!   `labelColor`, and the buttons are native push buttons of the regular
//!   size. The system alert uses `secondaryLabelColor` for the detail, but
//!   that color is black at about half alpha in Light, about 4:1 on white,
//!   under the 4.5:1 of WCAG AA that this panel keeps
//!   (`panels::contrast` has the arithmetic). The detail tells itself
//!   apart by its size (11 pt against 13 pt semibold). The macOS test
//!   `ask_panel_macos_tests.rs` resolves the semantic colors in Aqua and
//!   Dark Aqua and checks AA.
//! - The accent of the paddock mark shows only as a 3 pt left edge.
//! - Top to bottom, left aligned, 360 pt wide, 16 pt padding: the 32 pt
//!   app icon next to the title (13 pt semibold), the detail (11 pt), the
//!   `and 3 more waiting` line, and the row of buttons at the bottom right:
//!   `Deny` (Esc), `Allow once`, `Always allow` (Return, the default).
//!   The other answers sit in the `More…` menu at the bottom left.
//! - The panel is a non-activating panel at the floating level: it does
//!   not take the keyboard from the user's typing, it takes clicks, and
//!   after a click into it Return and Esc work.
//! - Each button has an accessibility label with the host; VoiceOver
//!   reads the title, then the detail.
//!
//! What the panel says comes from `panels::content`, which has Linux
//! tests. macOS only.

use crate::broker::ConnectAnswer;
use crate::mark::panels::{self, Content, Role};
use crate::mark::{Rect, Tone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSBackingStoreType, NSBezelStyle, NSButton, NSColor,
    NSControlSize, NSFloatingWindowLevel, NSFont, NSFontWeightRegular, NSFontWeightSemibold,
    NSImage, NSImageScaling, NSImageView, NSLineBreakMode, NSPanel, NSTextField, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{ns_string, NSPoint, NSRect, NSSize, NSString};
use std::path::Path;

const EDGE: f64 = 3.0;
const RADIUS: f64 = 12.0;

/// One drawn panel.
pub struct Drawn {
    pub panel: Retained<NSPanel>,
    /// The size of the panel, which the text sets.
    pub size: (f64, f64),
}

/// The tag of a button: the index of its answer in `ConnectAnswer::ALL`,
/// which `answerConnect:` reads back.
pub fn tag_of(a: ConnectAnswer) -> isize {
    ConnectAnswer::ALL
        .iter()
        .position(|x| *x == a)
        .map(|i| i as isize)
        .unwrap_or(-1)
}

fn label(
    mtm: MainThreadMarker,
    text: &str,
    size: f64,
    strong: bool,
    width: f64,
) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    // SAFETY: constants of AppKit.
    let weight = unsafe {
        if strong {
            NSFontWeightSemibold
        } else {
            NSFontWeightRegular
        }
    };
    l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    // labelColor for every text: see the module text for the contrast.
    l.setTextColor(Some(&NSColor::labelColor()));
    // Word wrap: a long host breaks onto the next line, never into "…".
    l.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    l.setMaximumNumberOfLines(0);
    l.setPreferredMaxLayoutWidth(width);
    l.setSelectable(false);
    l
}

fn height_of(l: &NSTextField, width: f64) -> f64 {
    l.sizeThatFits(NSSize::new(width, 10_000.0)).height.ceil()
}

/// The panel window: titled for the system shadow and the key handling,
/// with the title bar hidden and the content under it.
fn window(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let p = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(panels::WIDTH, 120.0)),
        NSWindowStyleMask::Titled
            | NSWindowStyleMask::FullSizeContentView
            | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    // SAFETY: the marker keeps the panel in a Retained.
    unsafe { p.setReleasedWhenClosed(false) };
    p.setTitlebarAppearsTransparent(true);
    p.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    for b in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = p.standardWindowButton(b) {
            button.setHidden(true);
        }
    }
    p.setOpaque(false);
    p.setBackgroundColor(Some(&NSColor::clearColor()));
    p.setHasShadow(true);
    p.setMovable(false);
    p.setFloatingPanel(true);
    p.setLevel(NSFloatingWindowLevel);
    // A click into the panel makes it key, so Return and Esc work; it
    // never becomes key by itself.
    p.setBecomesKeyOnlyIfNeeded(false);
    p.setHidesOnDeactivate(false);
    p.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    p
}

fn icon_image(icon: Option<&Path>, mtm: MainThreadMarker) -> Option<Retained<NSImage>> {
    if let Some(path) = icon {
        let s = NSString::from_str(&path.to_string_lossy());
        if let Some(img) = NSImage::initWithContentsOfFile(NSImage::alloc(), &s) {
            return Some(img);
        }
    }
    NSApplication::sharedApplication(mtm).applicationIconImage()
}

fn push_button(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: Sel,
) -> Retained<NSButton> {
    // SAFETY: the target answers the action and outlives the button.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    button.setBezelStyle(NSBezelStyle::Push);
    button.setControlSize(NSControlSize::Regular);
    button.sizeToFit();
    button
}

fn frame(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

/// Draw a panel for `content`. `target` answers `action` (the answer
/// buttons, tag = [`tag_of`]) and `more_action` (the `More…` button).
pub fn draw(
    mtm: MainThreadMarker,
    content: &Content,
    icon: Option<&Path>,
    target: &AnyObject,
    action: Sel,
    more_action: Sel,
) -> Drawn {
    let mut d = Drawn {
        panel: window(mtm),
        size: (panels::WIDTH, 0.0),
    };
    redraw(&mut d, mtm, content, icon, target, action, more_action);
    d
}

/// Draw new content into the same panel: no new window, nothing that
/// leaves the screen and comes back, when the count of the queue changes.
/// Where each part goes comes from `panels::layout`.
pub fn redraw(
    d: &mut Drawn,
    mtm: MainThreadMarker,
    content: &Content,
    icon: Option<&Path>,
    target: &AnyObject,
    action: Sel,
    more_action: Sel,
) {
    let surface = NSVisualEffectView::new(mtm);
    surface.setMaterial(NSVisualEffectMaterial::Popover);
    surface.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    surface.setState(NSVisualEffectState::Active);
    surface.setWantsLayer(true);
    if let Some(layer) = surface.layer() {
        layer.setCornerRadius(RADIUS);
        layer.setMasksToBounds(true);
    }
    // The buttons first: their widths set the width of the panel.
    let buttons: Vec<Retained<NSButton>> = content
        .buttons
        .iter()
        .map(|b| {
            let button = push_button(mtm, &b.title, target, action);
            button.setTag(tag_of(b.answer));
            match b.role {
                Role::Default => button.setKeyEquivalent(ns_string!("\r")),
                Role::Cancel => button.setKeyEquivalent(ns_string!("\u{1b}")),
                Role::Plain => {}
            }
            button.setAccessibilityLabel(Some(&NSString::from_str(&b.label)));
            button
        })
        .collect();
    let more_button = (!content.menu.is_empty()).then(|| {
        push_button(
            mtm,
            crate::macvm::text::ask_more_button(),
            target,
            more_action,
        )
    });
    let widths: Vec<f64> = buttons.iter().map(|b| b.frame().size.width).collect();
    let more_w = more_button.as_ref().map(|b| b.frame().size.width);
    let w = panels::width_for(&widths, more_w);
    let text_w = panels::text_width(w);
    let title = label(mtm, &content.title, 13.0, true, text_w);
    let detail = label(mtm, &content.detail, 11.0, false, text_w);
    let more = (!content.more.is_empty()).then(|| label(mtm, &content.more, 11.0, false, text_w));
    let l = panels::layout(
        w,
        height_of(&title, text_w),
        height_of(&detail, text_w),
        more.as_ref().map(|m| height_of(m, text_w)),
        &widths,
        more_w,
    );
    if let Some(img) = icon_image(icon, mtm) {
        let view = NSImageView::imageViewWithImage(&img, mtm);
        view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        view.setFrame(frame(l.icon));
        surface.addSubview(&view);
    }
    // VoiceOver reads the views in this order: the title, then the detail.
    title.setFrame(frame(l.title));
    surface.addSubview(&title);
    detail.setFrame(frame(l.detail));
    surface.addSubview(&detail);
    if let (Some(m), Some(r)) = (&more, l.more) {
        m.setFrame(frame(r));
        surface.addSubview(m);
    }
    for (b, r) in buttons.iter().zip(l.buttons.iter()) {
        b.setFrame(frame(*r));
        surface.addSubview(b);
    }
    if let (Some(b), Some(r)) = (&more_button, l.more_button) {
        b.setFrame(frame(r));
        surface.addSubview(b);
    }

    // The accent of the mark: a 3 pt edge on the left, above the surface.
    let edge = NSView::new(mtm);
    edge.setWantsLayer(true);
    if let Some(layer) = edge.layer() {
        let (r, g, b) = Tone::Amber.rgb();
        layer.setBackgroundColor(Some(&CGColor::new_srgb(r, g, b, 1.0)));
    }
    edge.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(EDGE, l.h)));
    surface.addSubview(&edge);

    let view: &NSView = &surface;
    d.panel.setContentView(Some(view));
    let origin = d.panel.frame().origin;
    d.panel
        .setFrame_display(NSRect::new(origin, NSSize::new(l.w, l.h)), true);
    d.size = (l.w, l.h);
}

#[cfg(test)]
#[path = "ask_panel_macos_tests.rs"]
mod tests;
