//! The side panel of the guided window (`vz_mac_window.rs`), macOS only,
//! main thread only.
//!
//! The rows sit in a scroll view, each as tall as its text, so no label
//! is ever clipped (`guided_ui::stack`). From the top: the title, the
//! question for the password (shown only when the guest refused the one
//! of the keyring), the six steps, the password in a large monospaced
//! selectable field with `Copy` and `Type the password into the guest`,
//! `Type the command into the guest`, the hint, a status line and the
//! help. `Help`, `Continue` and `Stop` stay at the bottom.
//!
//! The guest has no clipboard before its agent runs, so the Type buttons
//! send the text into the guest view as key events: a `keyDown` and a
//! `keyUp` per character with the US ANSI key code, and a flags change of
//! the shift key around a shifted one (`guided_ui::key_actions`). A thread
//! paces them and posts each one to the main queue, where the window
//! sends it to the view, its first responder.

use super::golden::{is_keyring_password, PASSWORD_MAX};
use super::guided_ui::{self, KeyAction, Row, KEY_GAP_MS, PANEL_W};
use super::text::{self, PanelStep};
use super::vz_mac::Sendable;
use super::vz_mac_window::{PanelEvent, ASPECT};
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSPasteboard, NSPasteboardTypeString, NSResponder, NSScrollView,
    NSSecureTextField, NSTextField, NSView, NSWindow, NSWindowDelegate,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSSize, NSString};
use objc2_virtualization::VZVirtualMachineView;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// The height of the bar of `Help`, `Continue` and `Stop`.
const BAR_H: f64 = 60.0;
const MARGIN: f64 = 16.0;
const GAP: f64 = 10.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

define_class!(
    /// The document view of the panel's scroll view: flipped, so the rows
    /// start at the top.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockSetupPanelDocument"]
    pub(crate) struct Document;

    impl Document {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

/// The views of the panel, made once by `build`.
pub(crate) struct Views {
    window: Retained<NSWindow>,
    content: Retained<NSView>,
    vm_view: Retained<VZVirtualMachineView>,
    side: Retained<NSView>,
    scroll: Retained<NSScrollView>,
    doc: Retained<Document>,
    title: Retained<NSTextField>,
    /// The line of the unattended run.
    banner: Retained<NSTextField>,
    ask: Retained<NSTextField>,
    ask_field: Retained<NSSecureTextField>,
    ask_button: Retained<NSButton>,
    steps: Vec<Retained<NSTextField>>,
    password_label: Retained<NSTextField>,
    password_value: Retained<NSTextField>,
    copy: Retained<NSButton>,
    type_password: Retained<NSButton>,
    type_command: Retained<NSButton>,
    hint: Retained<NSTextField>,
    status: Retained<NSTextField>,
    help: Retained<NSTextField>,
}

pub(crate) struct Ivars {
    tx: Sender<PanelEvent>,
    /// The password that the Type and Copy buttons use: the made one, or
    /// the one that the person typed into the question.
    password: RefCell<String>,
    /// The password is the one the person set: the panel hides it.
    password_set: Cell<bool>,
    pub(super) done: Cell<usize>,
    pub(super) closing: Cell<bool>,
    help_on: Cell<bool>,
    asking: Cell<bool>,
    /// The unattended run types into the guest: the Type buttons hide.
    unattended: Cell<bool>,
    /// A Type button is typing: a second press waits.
    typing: Arc<AtomicBool>,
    views: RefCell<Option<Views>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockSetupPanel"]
    #[ivars = Ivars]
    pub(crate) struct Panel;

    unsafe impl NSObjectProtocol for Panel {}

    unsafe impl NSWindowDelegate for Panel {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _note: &NSNotification) {
            if !self.ivars().closing.get() {
                let _ = self.ivars().tx.send(PanelEvent::Closed);
            }
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _note: &NSNotification) {
            self.relayout();
        }
    }

    impl Panel {
        /// `Copy`: the password to the host pasteboard, for a person who
        /// keeps it elsewhere. The guest has no clipboard yet.
        #[unsafe(method(copyPassword:))]
        fn copy_password(&self, _sender: &AnyObject) {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            // SAFETY: a constant of AppKit.
            let kind = unsafe { NSPasteboardTypeString };
            let pw = self.ivars().password.borrow().clone();
            pb.setString_forType(&NSString::from_str(&pw), kind);
            self.set_status(&text::panel_copied());
        }

        #[unsafe(method(typePassword:))]
        fn type_password(&self, _sender: &AnyObject) {
            let pw = self.ivars().password.borrow().clone();
            self.type_text(&pw);
        }

        #[unsafe(method(typeCommand:))]
        fn type_command(&self, _sender: &AnyObject) {
            self.type_text(&crate::macvm::seed::command());
        }

        /// `Use this password`, or Return in the secure field.
        #[unsafe(method(usePassword:))]
        fn use_password(&self, _sender: &AnyObject) {
            self.take_password();
        }

        /// `Help`: the reason of each step, on and off.
        #[unsafe(method(toggleHelp:))]
        fn toggle_help(&self, _sender: &AnyObject) {
            let on = !self.ivars().help_on.get();
            self.ivars().help_on.set(on);
            self.update();
        }

        #[unsafe(method(continueStep:))]
        fn continue_step(&self, _sender: &AnyObject) {
            let _ = self.ivars().tx.send(PanelEvent::Continue);
        }

        #[unsafe(method(stopSetup:))]
        fn stop_setup(&self, _sender: &AnyObject) {
            let _ = self.ivars().tx.send(PanelEvent::Stop);
        }
    }
);

/// A wrapping label of `size` points.
fn label(mtm: MainThreadMarker, size: f64) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&NSString::from_str(""), mtm);
    l.setFont(Some(&NSFont::systemFontOfSize(size)));
    l
}

/// A push button with its title, sized to fit.
fn button(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: Sel,
) -> Retained<NSButton> {
    // SAFETY: the panel answers the action and outlives the button.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    b.sizeToFit();
    b
}

/// The height of a text field for `width`: wrapped text grows down.
fn text_height(t: &NSTextField, width: f64) -> f64 {
    if t.isHidden() {
        return 0.0;
    }
    t.setPreferredMaxLayoutWidth(width);
    t.cell()
        .map(|c| c.cellSizeForBounds(rect(0.0, 0.0, width, 100_000.0)).height)
        .unwrap_or(20.0)
        .ceil()
}

/// One row of the panel, for the layout.
enum Item<'a> {
    Text(&'a NSTextField, f64),
    Field(&'a NSView, f64, f64),
    Buttons(Vec<&'a NSButton>, f64),
}

impl Item<'_> {
    fn row(&self, width: f64) -> Row {
        match self {
            Item::Text(t, gap) => Row {
                h: text_height(t, width),
                gap: *gap,
            },
            Item::Field(v, h, gap) => Row {
                h: if v.isHidden() { 0.0 } else { *h },
                gap: *gap,
            },
            Item::Buttons(bs, gap) => Row {
                h: bs
                    .iter()
                    .filter(|b| !b.isHidden())
                    .map(|b| b.frame().size.height)
                    .fold(0.0, f64::max),
                gap: *gap,
            },
        }
    }

    fn place(&self, top: f64, width: f64, h: f64) {
        match self {
            Item::Text(t, _) => t.setFrame(rect(MARGIN, top, width, h)),
            Item::Field(v, _, _) => v.setFrame(rect(MARGIN, top, width, h)),
            Item::Buttons(bs, _) => {
                let mut x = MARGIN;
                for b in bs.iter().filter(|b| !b.isHidden()) {
                    let w = b.frame().size.width.max(60.0);
                    b.setFrame(rect(x, top, w, h));
                    x += w + 8.0;
                }
            }
        }
    }
}

impl Panel {
    pub(super) fn new(
        mtm: MainThreadMarker,
        password: String,
        tx: Sender<PanelEvent>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            tx,
            password: RefCell::new(password),
            password_set: Cell::new(false),
            done: Cell::new(0),
            closing: Cell::new(false),
            help_on: Cell::new(false),
            asking: Cell::new(false),
            unattended: Cell::new(false),
            typing: Arc::new(AtomicBool::new(false)),
            views: RefCell::new(None),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Make the panel's views right of `vm_view` in `content`.
    pub(super) fn build(
        &self,
        mtm: MainThreadMarker,
        window: &NSWindow,
        content: &NSView,
        vm_view: &VZVirtualMachineView,
    ) {
        let target: &AnyObject = self.as_ref();
        let zero = rect(0.0, 0.0, 0.0, 0.0);
        let side = NSView::initWithFrame(mtm.alloc(), zero);
        let scroll = NSScrollView::initWithFrame(mtm.alloc(), zero);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let doc: Retained<Document> =
            unsafe { msg_send![Document::alloc(mtm), initWithFrame: zero] };
        scroll.setDocumentView(Some(&doc));
        side.addSubview(&scroll);
        content.addSubview(&side);

        let title = label(mtm, 16.0);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(16.0)));
        title.setStringValue(&NSString::from_str(&text::panel_title()));
        let banner = label(mtm, 13.0);
        banner.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        banner.setStringValue(&NSString::from_str(&text::unattended_banner()));
        banner.setHidden(true);
        let ask = label(mtm, 13.0);
        ask.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
        ask.setTextColor(Some(&NSColor::systemRedColor()));
        ask.setStringValue(&NSString::from_str(&text::panel_wrong_password()));
        let ask_field = NSSecureTextField::initWithFrame(mtm.alloc(), zero);
        ask_field.setPlaceholderString(Some(&NSString::from_str(&text::panel_password_field())));
        // SAFETY: the panel answers the action and outlives the field.
        unsafe {
            ask_field.setTarget(Some(target));
            ask_field.setAction(Some(sel!(usePassword:)));
        }
        let ask_button = button(mtm, &text::panel_use_password(), target, sel!(usePassword:));
        for v in [&*ask as &NSView, &ask_field, &ask_button] {
            v.setHidden(true);
        }
        let steps: Vec<_> = PanelStep::ALL
            .iter()
            .map(|s| {
                let l = label(mtm, 13.0);
                // The account step and the seed command can be selected.
                l.setSelectable(matches!(s, PanelStep::Account | PanelStep::Seed));
                l
            })
            .collect();
        let password_label = label(mtm, 12.0);
        let password_value = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        password_value.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(20.0, 0.0)));
        password_value.setSelectable(true);
        let copy = button(mtm, &text::panel_copy(), target, sel!(copyPassword:));
        let type_password = button(
            mtm,
            &text::panel_type_password(),
            target,
            sel!(typePassword:),
        );
        let type_command = button(mtm, &text::panel_type_command(), target, sel!(typeCommand:));
        let hint = label(mtm, 11.0);
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));
        hint.setStringValue(&NSString::from_str(&text::panel_type_hint()));
        let status = label(mtm, 12.0);
        let help = label(mtm, 12.0);
        help.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let rows: Vec<&NSView> = [&*title as &NSView, &banner, &ask, &ask_field, &ask_button]
            .into_iter()
            .chain(steps.iter().map(|s| &**s as &NSView))
            .chain([
                &*password_label as &NSView,
                &password_value,
                &copy,
                &type_password,
                &type_command,
                &hint,
                &status,
                &help,
            ])
            .collect();
        for v in rows {
            doc.addSubview(v);
        }
        let mut bx = MARGIN;
        for (t, action) in [
            ("Help", sel!(toggleHelp:)),
            ("Continue", sel!(continueStep:)),
            ("Stop", sel!(stopSetup:)),
        ] {
            let b = button(mtm, t, target, action);
            let w = b.frame().size.width.max(70.0);
            b.setFrame(rect(bx, 16.0, w, 32.0));
            bx += w + 8.0;
            side.addSubview(&b);
        }
        *self.ivars().views.borrow_mut() = Some(Views {
            window: window.retain(),
            content: content.retain(),
            vm_view: vm_view.retain(),
            side,
            scroll,
            doc,
            title,
            banner,
            ask,
            ask_field,
            ask_button,
            steps,
            password_label,
            password_value,
            copy,
            type_password,
            type_command,
            hint,
            status,
            help,
        });
        self.update();
        self.relayout();
    }

    /// The frames after a resize: the guest view keeps its aspect left of
    /// the panel, the panel keeps its width at the right edge.
    fn relayout(&self) {
        let views = self.ivars().views.borrow();
        let Some(v) = views.as_ref() else {
            return;
        };
        let b = v.content.bounds().size;
        let f = guided_ui::view_frame(b.width, b.height, ASPECT);
        v.vm_view.setFrame(rect(f.x, f.y, f.w, f.h));
        v.side
            .setFrame(rect(b.width - PANEL_W, 0.0, PANEL_W, b.height));
        v.scroll
            .setFrame(rect(0.0, BAR_H, PANEL_W, (b.height - BAR_H).max(0.0)));
        layout_doc(v);
    }

    /// The texts of the steps for the current progress, the password and
    /// the help, then the layout of the rows.
    pub(super) fn update(&self) {
        let iv = self.ivars();
        let views = iv.views.borrow();
        let Some(v) = views.as_ref() else {
            return;
        };
        let done = iv.done.get();
        for (label, step) in v.steps.iter().zip(PanelStep::ALL) {
            let n = step.number() as usize;
            // The last step shows once the others are done.
            let last = PanelStep::Done.number() as usize;
            let shown = n < last || done >= last - 1;
            let t = if shown {
                text::panel_step(step, n <= done)
            } else {
                String::new()
            };
            label.setStringValue(&NSString::from_str(&t));
            label.setHidden(!shown);
        }
        let (head, value) = if iv.password_set.get() {
            let n = iv.password.borrow().chars().count().min(20);
            (text::panel_password_set_label(), "•".repeat(n))
        } else {
            (text::panel_password_label(), iv.password.borrow().clone())
        };
        v.password_label.setStringValue(&NSString::from_str(&head));
        v.password_value.setStringValue(&NSString::from_str(&value));
        // A hidden password is not copied by a selection.
        v.password_value.setSelectable(!iv.password_set.get());
        let t = if iv.help_on.get() {
            PanelStep::ALL
                .iter()
                .map(|s| text::panel_help_entry(s.number(), &text::panel_help(*s)))
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            text::panel_note()
        };
        v.help.setStringValue(&NSString::from_str(&t));
        layout_doc(v);
    }

    /// The unattended run starts (`on`) or ends: the banner shows while it
    /// runs, and the Type buttons, the password and the hint hide. After a
    /// hand-over, `status` says where the person continues.
    pub(super) fn set_unattended(&self, on: bool, status: Option<&str>) {
        self.ivars().unattended.set(on);
        {
            let views = self.ivars().views.borrow();
            let Some(v) = views.as_ref() else {
                return;
            };
            v.banner.setHidden(!on);
            for view in [
                &*v.password_label as &NSView,
                &v.password_value,
                &v.copy,
                &v.type_password,
                &v.type_command,
                &v.hint,
            ] {
                view.setHidden(on);
            }
        }
        if let Some(line) = status {
            self.set_status(line);
        }
        self.update();
    }

    /// One line under the Type buttons.
    fn set_status(&self, line: &str) {
        let views = self.ivars().views.borrow();
        if let Some(v) = views.as_ref() {
            v.status.setStringValue(&NSString::from_str(line));
            layout_doc(v);
        }
    }

    /// Show the question for the password, at the top, with the keyboard
    /// in its field.
    pub(super) fn ask_password(&self) {
        self.ivars().asking.set(true);
        let views = self.ivars().views.borrow();
        let Some(v) = views.as_ref() else {
            return;
        };
        for view in [&*v.ask as &NSView, &v.ask_field, &v.ask_button] {
            view.setHidden(false);
        }
        layout_doc(v);
        v.doc.scrollPoint(NSPoint::new(0.0, 0.0));
        v.window.makeFirstResponder(Some(&v.ask_field));
    }

    /// The answer of the question: a password that the keyring takes goes
    /// to the driver, the question goes away, and the Type and Copy
    /// buttons use it from now on.
    fn take_password(&self) {
        let iv = self.ivars();
        if !iv.asking.get() {
            return;
        }
        let typed = {
            let views = iv.views.borrow();
            let Some(v) = views.as_ref() else {
                return;
            };
            let typed = v.ask_field.stringValue().to_string();
            if !is_keyring_password(&typed) {
                self.set_status(&text::keyring_password_invalid(PASSWORD_MAX));
                return;
            }
            v.ask_field.setStringValue(&NSString::from_str(""));
            for view in [&*v.ask as &NSView, &v.ask_field, &v.ask_button] {
                view.setHidden(true);
            }
            v.window.makeFirstResponder(Some(&v.vm_view));
            typed
        };
        iv.asking.set(false);
        *iv.password.borrow_mut() = typed.clone();
        iv.password_set.set(true);
        let _ = iv.tx.send(PanelEvent::Password(typed));
        self.set_status("");
        self.update();
    }

    /// Type `text` into the guest view, paced on a thread of its own so
    /// the guest keeps running. Nothing is typed when a character has no
    /// US key, or while an earlier text still types.
    fn type_text(&self, text: &str) {
        let strokes = match guided_ui::key_strokes(text) {
            Ok(s) => s,
            Err(c) => return self.set_status(&text::panel_cannot_type(c)),
        };
        let iv = self.ivars();
        if iv.typing.swap(true, Ordering::SeqCst) {
            return self.set_status(&text::panel_typing_busy());
        }
        let window = {
            let views = iv.views.borrow();
            let Some(v) = views.as_ref() else {
                iv.typing.store(false, Ordering::SeqCst);
                return;
            };
            // The person clicked into the guest field, then this button:
            // the view gets the keys whatever holds the keyboard now.
            v.window.makeFirstResponder(Some(&v.vm_view));
            (Sendable(v.window.clone()), Sendable(v.vm_view.clone()))
        };
        let (window, view) = window;
        let count = strokes.len();
        self.set_status(&text::panel_typing(count));
        let panel = Sendable(self.retain());
        let typing = iv.typing.clone();
        let actions = guided_ui::key_actions(&strokes);
        std::thread::spawn(move || {
            guided_ui::play_keys(
                &actions,
                || true,
                |a| {
                    let w = Sendable(window.0.clone());
                    let v = Sendable(view.0.clone());
                    DispatchQueue::main().exec_sync(move || send_key(w.get(), v.get(), a));
                    true
                },
                || std::thread::sleep(Duration::from_millis(KEY_GAP_MS)),
            );
            // The window and the panel are released on the main thread.
            DispatchQueue::main().exec_async(move || {
                let (_window, _view, panel) = (window, view, panel);
                typing.store(false, Ordering::SeqCst);
                panel.get().set_status(&text::panel_typed(count));
            });
        });
    }
}

/// Place the rows of the scroll view from the top, each as tall as its
/// text, and size the document so all of them scroll into view.
fn layout_doc(v: &Views) {
    let clip = v.scroll.contentSize();
    let width = (clip.width - 2.0 * MARGIN).max(40.0);
    let field_h = 24.0;
    let value_h = text_height(&v.password_value, width).max(28.0);
    let mut items = vec![
        Item::Text(&v.title, GAP),
        Item::Text(&v.banner, GAP),
        Item::Text(&v.ask, 6.0),
        Item::Field(&v.ask_field, field_h, 6.0),
        Item::Buttons(vec![&v.ask_button], GAP * 2.0),
    ];
    items.extend(v.steps.iter().map(|s| Item::Text(s, GAP)));
    items.extend([
        Item::Text(&v.password_label, 4.0),
        Item::Field(&v.password_value, value_h, 6.0),
        Item::Buttons(vec![&v.copy, &v.type_password], 6.0),
        Item::Buttons(vec![&v.type_command], 6.0),
        Item::Text(&v.hint, 6.0),
        Item::Text(&v.status, GAP),
        Item::Text(&v.help, GAP),
    ]);
    let rows: Vec<Row> = items.iter().map(|i| i.row(width)).collect();
    let (tops, total) = guided_ui::stack(&rows, MARGIN);
    for ((item, top), row) in items.iter().zip(tops).zip(&rows) {
        if row.h > 0.0 {
            item.place(top, width, row.h);
        }
    }
    v.doc
        .setFrame(rect(0.0, 0.0, clip.width, total.max(clip.height)));
}

/// Send one key event to the window, which hands it to its first
/// responder, the guest view. Nothing goes once the window is closed.
fn send_key(window: &NSWindow, view: &VZVirtualMachineView, action: KeyAction) {
    if !window.isVisible() {
        return;
    }
    let (code, down) = action.code_down();
    // The keyboard of the VM (vz_mac_input.rs). Nothing is posted to the
    // system.
    super::vz_mac_input::key(view, code, down);
}
