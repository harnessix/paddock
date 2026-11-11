//! The cover over the guest picture of a session window on AppKit, macOS
//! only. `cover.rs` decides what goes where; this file paints it.
//!
//! Each window has a [`CoverView`] above its `VZVirtualMachineView`, in
//! the content view and so inside the corner clip. It hosts its own
//! layer: one sublayer per rectangle, a color or a part of the window's
//! last picture. It never takes a click (`hitTest:` gives nil), so the
//! view below gets every event.
//!
//! The edge colors and the picture come from an image of the host
//! window itself (`CGWindowListCreateImage` of the own window, which
//! needs no Screen Recording). Without an image the cover takes the
//! host's window background color.

use super::cover::{self, Edges, Held, Paint, Rgba, Samples};
use super::proto::Frame;
use super::windows::{HostEvent, HostWindows, SessionWindow};
use objc2::rc::Retained;
use objc2::AllocAnyThread;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBitmapImageRep, NSColor, NSColorSpace, NSEvent,
    NSEventType, NSView,
};
use objc2_core_foundation::{CFRetained, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGImage, CGWindowImageOption, CGWindowListOption};
use objc2_foundation::{NSPoint, NSRect};
use objc2_quartz_core::{CALayer, CATransaction};
use std::time::{Duration, Instant};

extern "C" {
    /// `CGRectNull`: the bounds of the window itself.
    static CGRectNull: CGRect;
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockCoverView"]
    pub struct CoverView;

    impl CoverView {
        /// The cover never takes a click: the view below gets it.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

/// The last picture of a window, from before another window covered it.
struct Picture {
    image: CFRetained<CGImage>,
    /// The window size of the picture, in points.
    w: f64,
    h: f64,
    /// The parts where another window lay on it then.
    tainted: Vec<Frame>,
    /// When the host took it: a second picture within [`FRESH`] keeps the
    /// first, which a click took before the guest raised another window.
    taken: Instant,
}

/// A picture younger than this is not taken again.
const FRESH: Duration = Duration::from_millis(300);

/// The cover state of one window.
#[derive(Default)]
pub struct CoverState {
    view: Option<Retained<CoverView>>,
    edges: Option<Edges>,
    picture: Option<Picture>,
    /// The parts that another guest window covers (`cover::covers`).
    covers: Vec<Frame>,
    /// The parts that any window above it in the guest lies on: the taint
    /// of a picture taken now.
    above: Vec<Frame>,
    /// The windows below it in the guest that it lies on
    /// (`cover::lies_on`): their pictures go first when a click raises it.
    lies_on: Vec<u32>,
    held: Option<Held<Retained<NSEvent>>>,
    /// The undrawn rectangles the cover paints now, for `window frames`.
    undrawn: usize,
    /// The layers the cover has now: a drag step with nothing to paint
    /// and nothing painted does no work.
    painted: usize,
}

/// The cover view of a new window, above the view of the guest.
pub fn attach(w: &SessionWindow) {
    let Some(content) = w.contentView() else {
        return;
    };
    let mtm = MainThreadMarker::from(w);
    let size = content.frame().size;
    let this = CoverView::alloc(mtm).set_ivars(());
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), size);
    let view: Retained<CoverView> = unsafe { msg_send![super(this), initWithFrame: rect] };
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // A layer of its own: the view hosts it, so its sublayers are ours.
    view.setLayer(Some(&CALayer::layer()));
    view.setWantsLayer(true);
    content.addSubview(&view);
    w.ivars().cover.borrow_mut().view = Some(view);
}

/// An image of the host window as it shows now, or None.
fn capture(w: &SessionWindow) -> Option<CFRetained<CGImage>> {
    let number = w.windowNumber();
    if number <= 0 {
        return None;
    }
    // SAFETY: a constant of CoreGraphics.
    let null = unsafe { CGRectNull };
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGWindowListCreateImage(
        null,
        CGWindowListOption::OptionIncludingWindow,
        number as u32,
        CGWindowImageOption::BoundsIgnoreFraming,
    )?;
    (CGImage::width(Some(&image)) > 0).then_some(image)
}

/// The content size of the window in points.
fn content_size(w: &SessionWindow) -> (f64, f64) {
    let s = w
        .contentView()
        .map(|c| c.frame().size)
        .unwrap_or_else(|| w.frame().size);
    (s.width, s.height)
}

/// The corner radius of the clip now.
fn radius(w: &SessionWindow) -> f64 {
    w.contentView()
        .and_then(|c| c.layer())
        .map_or(0.0, |l| l.cornerRadius())
}

/// The three edge colors of `image`, a picture of a window of `size`.
fn edges_of(image: &CGImage, size: (f64, f64), title_bar: f64, r: f64) -> Option<Edges> {
    let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let (pw, ph) = (rep.pixelsWide(), rep.pixelsHigh());
    if pw <= 0 || ph <= 0 || size.0 <= 0.0 {
        return None;
    }
    let scale = pw as f64 / size.0;
    let srgb = NSColorSpace::sRGBColorSpace();
    let colors = |points: &[(f64, f64)]| -> Vec<Rgba> {
        points
            .iter()
            .filter_map(|&(x, y)| {
                let px = ((x * scale) as isize).clamp(0, pw - 1);
                let py = ((y * scale) as isize).clamp(0, ph - 1);
                let c = rep.colorAtX_y(px, py)?.colorUsingColorSpace(&srgb)?;
                let a = c.alphaComponent();
                // A clear pixel (a corner, a shadow) is not the window.
                (a > 0.5).then(|| [c.redComponent(), c.greenComponent(), c.blueComponent(), a])
            })
            .collect()
    };
    let s = Samples::of(size.0, size.1, title_bar, r);
    Edges::from(
        cover::median(&colors(&s.title)),
        cover::median(&colors(&s.side)),
        cover::median(&colors(&s.bottom)),
    )
}

/// Sample the edge colors of `w` from an image of it now: at the press
/// of a resize, before the host window grows. Gives the image.
pub fn sample(w: &SessionWindow) -> Option<CFRetained<CGImage>> {
    let image = capture(w)?;
    let title_bar = w.ivars().chrome.borrow().title_bar;
    let edges = edges_of(&image, content_size(w), title_bar, radius(w));
    if edges.is_some() {
        w.ivars().cover.borrow_mut().edges = edges;
    }
    Some(image)
}

/// Fill `layer` with the color of `paint`: the edge color, else the
/// host's window background color.
fn fill(layer: &CALayer, edges: Option<Edges>, paint: Paint) {
    match edges {
        Some(e) => {
            let [r, g, b, a] = e.of(paint);
            layer.setBackgroundColor(Some(&CGColor::new_srgb(r, g, b, a)));
        }
        None => layer.setBackgroundColor(Some(&NSColor::windowBackgroundColor().CGColor())),
    }
}

/// Paint the cover of `w` again: the undrawn part of a resize, then the
/// covered parts. Nothing in full screen, where the view shows the whole
/// guest display.
pub fn repaint(w: &SessionWindow) {
    let iv = w.ivars();
    let (cw, ch) = content_size(w);
    let r = radius(w);
    let title_bar = iv.chrome.borrow().title_bar;
    let guest = iv.guest.get();
    let mut st = iv.cover.borrow_mut();
    let Some(root) = st.view.as_ref().and_then(|v| v.layer()) else {
        return;
    };
    let mut parts = Vec::new();
    if !iv.full_screen.get() {
        parts = cover::undrawn((cw, ch), (guest.w, guest.h), title_bar, r);
        st.undrawn = parts.len();
        let fits = st
            .picture
            .as_ref()
            .is_some_and(|p| (p.w - guest.w).abs() < 0.5 && (p.h - guest.h).abs() < 0.5);
        let tainted: &[Frame] = st.picture.as_ref().map_or(&[], |p| &p.tainted);
        parts.extend(cover::pieces(&st.covers, fits, tainted));
    } else {
        st.undrawn = 0;
    }
    if parts.is_empty() && st.painted == 0 {
        return;
    }
    st.painted = parts.len();
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    // SAFETY: the layer is the cover view's own.
    unsafe { root.setSublayers(None) };
    for (rect, paint) in parts {
        let layer = CALayer::layer();
        layer.setFrame(CGRect::new(
            CGPoint::new(rect.x, ch - rect.y - rect.h),
            CGSize::new(rect.w, rect.h),
        ));
        let piece = match (paint, &st.picture) {
            (Paint::Picture, Some(p)) => {
                let scale = CGImage::width(Some(&p.image)) as f64 / p.w.max(1.0);
                CGImage::with_image_in_rect(
                    Some(&p.image),
                    CGRect::new(
                        CGPoint::new(rect.x * scale, rect.y * scale),
                        CGSize::new(rect.w * scale, rect.h * scale),
                    ),
                )
            }
            _ => None,
        };
        match piece {
            Some(img) => {
                let cf: &CFType = &img;
                // SAFETY: a CGImage is valid layer contents.
                unsafe { layer.setContents(Some(cf.as_ref())) };
            }
            None => fill(&layer, st.edges, paint),
        }
        root.addSublayer(&layer);
    }
    CATransaction::commit();
}

/// Keep the picture of `w` now, unless one is fresh. Only while the host
/// window has the guest window's size.
fn picture(w: &SessionWindow) {
    let iv = w.ivars();
    let guest = iv.guest.get();
    let (cw, ch) = content_size(w);
    if w.isMiniaturized()
        || iv.full_screen.get()
        || (cw - guest.w).abs() > 0.5
        || (ch - guest.h).abs() > 0.5
    {
        return;
    }
    let fresh = iv.cover.borrow().picture.as_ref().is_some_and(|p| {
        p.taken.elapsed() < FRESH && (p.w - guest.w).abs() < 0.5 && (p.h - guest.h).abs() < 0.5
    });
    if fresh {
        return;
    }
    if let Some(image) = sample(w) {
        let mut st = iv.cover.borrow_mut();
        let tainted = st.above.clone();
        st.picture = Some(Picture {
            image,
            w: guest.w,
            h: guest.h,
            tainted,
            taken: Instant::now(),
        });
    }
}

/// A press on `w` while it is not key: the guest raises its window next,
/// so the windows that it lies on keep their pictures now, before the
/// press reaches the guest.
pub fn before_press(w: &SessionWindow, event: &NSEvent) {
    if !matches!(
        event.r#type(),
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown
    ) || w.isKeyWindow()
    {
        return;
    }
    let ids = w.ivars().cover.borrow().lies_on.clone();
    if ids.is_empty() {
        return;
    }
    let app = NSApplication::sharedApplication(MainThreadMarker::from(w));
    for other in app.windows().iter() {
        if let Ok(s) = other.downcast::<SessionWindow>() {
            if ids.contains(&s.id()) {
                picture(&s);
            }
        }
    }
}

/// A mouse event of `w` while a click is held, or a press on a covered
/// part: true when the cover took it.
pub fn held_event(w: &SessionWindow, event: &NSEvent) -> bool {
    let iv = w.ivars();
    if iv.replaying.get() {
        return false;
    }
    let kind = event.r#type();
    {
        let mut st = iv.cover.borrow_mut();
        if let Some(h) = st.held.as_mut() {
            match kind {
                NSEventType::LeftMouseUp => h.up = Some(event.retain()),
                NSEventType::LeftMouseDragged => {}
                _ => return false,
            }
            return true;
        }
    }
    if kind != NSEventType::LeftMouseDown || iv.full_screen.get() {
        return false;
    }
    let (_, ch) = content_size(w);
    let p = event.locationInWindow();
    let at = (p.x, ch - p.y);
    {
        let mut st = iv.cover.borrow_mut();
        if !cover::hit(&st.covers, at.0, at.1) {
            return false;
        }
        st.held = Some(Held::new(event.retain(), at, Instant::now()));
    }
    // The window takes the key as a click would, and the guest raises it.
    if !w.isKeyWindow() && w.canBecomeKeyWindow() {
        NSApplication::sharedApplication(MainThreadMarker::from(w)).activate();
        w.makeKeyAndOrderFront(None);
    }
    (w.sink())(HostEvent::HeldClick(w.id()));
    true
}

/// Send the held click of `w` on when it is ready.
fn release_held(w: &SessionWindow, now: Instant) {
    let iv = w.ivars();
    let held = {
        let mut st = iv.cover.borrow_mut();
        match &st.held {
            Some(h) if h.ready(&st.covers, now) => st.held.take(),
            _ => None,
        }
    };
    let Some(h) = held else {
        return;
    };
    iv.replaying.set(true);
    w.sendEvent(&h.down);
    if let Some(up) = &h.up {
        w.sendEvent(up);
    }
    iv.replaying.set(false);
}

impl HostWindows {
    /// The covered parts of `id` changed: paint them, and send a held
    /// click on once its point is free. `above` and `lies_on` are for the
    /// pictures of the next click.
    pub fn set_covers(&self, id: u32, covers: Vec<Frame>, above: Vec<Frame>, lies_on: Vec<u32>) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        let changed = {
            let mut st = w.ivars().cover.borrow_mut();
            let changed = st.covers != covers;
            st.covers = covers;
            st.above = above;
            st.lies_on = lies_on;
            changed
        };
        if changed {
            repaint(w);
        }
        release_held(w, Instant::now());
    }

    /// Keep the picture of `id` now, before another window goes above it
    /// in the guest.
    pub fn take_picture(&self, id: u32) {
        if let Some(w) = self.windows.get(&id) {
            picture(w);
        }
    }

    /// Each pass of the session loop: the held clicks that waited long
    /// enough go on.
    pub fn held_tick(&self) {
        let now = Instant::now();
        for w in self.windows.values() {
            release_held(w, now);
        }
    }

    /// The undrawn rectangles, the covered parts and the side color of
    /// `id`, for `window frames`.
    pub fn cover_of(&self, id: u32) -> Option<(usize, usize, Option<Rgba>)> {
        let w = self.windows.get(&id)?;
        let st = w.ivars().cover.borrow();
        Some((st.undrawn, st.covers.len(), st.edges.map(|e| e.side)))
    }
}

#[cfg(test)]
#[path = "windows_cover_macos_tests.rs"]
mod tests;
