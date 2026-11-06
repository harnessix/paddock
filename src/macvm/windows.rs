//! The host windows of an app session, macOS only: one borderless
//! `NSWindow` per guest window, each with a `VZVirtualMachineView` of the
//! full guest display at the origin that `crop::view_origin` gives, so
//! the window shows exactly one guest window. The view is the
//! `DropVmView` of `vz_mac_drop.rs`, so a drop on the window reaches the
//! session. Main thread only: the session loop runs on it.
//!
//! `order.rs` decides what changes; this file runs the changes on AppKit
//! and reports what the user does to the windows (key, minimize, Space,
//! the Globe key, Cmd-Q, an Accessibility query or press) as
//! [`HostEvent`]s. The viewport source of `frames.rs` lives here as
//! [`Viewport`], because it needs the window class. The Accessibility
//! children of a window are the mirror elements of `ax_host.rs`. The
//! host-led drag and resize, the view offset, the chrome and the full
//! screen are in `windows_lead.rs`.

use super::ax_host::{Elements, Posted};
use super::axmirror::{Change as AxChange, Mirror};
use super::crop::{self, Display, Rect, ScreenSet};
use super::dock::DockAction;
use super::frames::{Frames, Source};
use super::input::{Globe, GlobeKey, KeyEvent};
use super::lead::Chrome;
use super::order::Change;
use super::proto::{Frame, Level, Window};
use super::run_vm::Vm;
use super::vz_mac_drop::{DropEvent, DropVmView};
use super::windows_lead::{Gesture, Mouse};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags, NSEventType,
    NSFloatingWindowLevel, NSNormalWindowLevel, NSPopUpMenuWindowLevel, NSScreen, NSView, NSWindow,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowOrderingMode,
    NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSNotification, NSPoint, NSRect, NSSize, NSString};
use objc2_virtualization::VZVirtualMachineView;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Two `AxWanted` events of one window are at least this far apart.
const AX_ASK_GAP: Duration = Duration::from_secs(1);

/// What the user did to the host windows, for the session loop.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    BecameKey(u32),
    ResignedKey(u32),
    Miniaturized(u32),
    Deminiaturized(u32),
    /// The active Space changed (`NSWorkspaceActiveSpaceDidChangeNotification`).
    SpaceChanged,
    /// The host screen set changed
    /// (`NSApplicationDidChangeScreenParametersNotification`).
    ScreensChanged,
    /// The host's current input source changed
    /// (`kTISNotifySelectedKeyboardInputSourceChanged`).
    InputSourceChanged,
    /// A lone press of the Globe key on a session window.
    Globe,
    /// Cmd-Q on a session window, or `Quit` of the Dock or the app menu.
    Quit,
    Dock(DockAction),
    /// An item of the menu bar replica, by its
    /// Accessibility path in the guest.
    MenuPress(Vec<usize>),
    /// An Accessibility client asked this session window for its
    /// children: the mirror of its tree is wanted.
    AxWanted(u32),
    /// A press of an Accessibility client on the mirror element with
    /// this id.
    AxPress(u64),
    /// The host window moved or resized in a host-led drag or resize:
    /// its frame in host points, and whether the user still holds it.
    LeadMoved(u32, Rect, bool),
    /// The host window entered (true) or left its full screen.
    FullScreenDone(u32, bool),
    /// A value of an Accessibility client for the mirror element with
    /// this id.
    AxSet(u64, String),
}

/// Where the host events go. The callbacks run on the main thread.
pub type HostSink = Rc<dyn Fn(HostEvent)>;

pub struct WindowIvars {
    id: u32,
    /// `normal`, `floating` and `sheet` windows take key; menus, popovers
    /// and tooltips never do: the candidate window of an input method
    /// takes no focus.
    takes_key: bool,
    /// The Globe key changes the input source on this Mac: catch it.
    catch_globe: bool,
    globe: RefCell<GlobeKey>,
    sink: HostSink,
    view: RefCell<Option<Retained<VZVirtualMachineView>>>,
    /// The mirror elements of the guest window's tree.
    ax: RefCell<Elements>,
    /// When an Accessibility client last asked for children with no
    /// tree: one `AxWanted` per second, not one per query.
    ax_asked: Cell<Option<Instant>>,
    pub(super) level: Level,
    pub(super) display: Display,
    /// The title bar, the buttons and the radius of the guest window.
    pub(super) chrome: RefCell<Chrome>,
    /// The last guest frame the view shows: the view keeps its offset
    /// from it while the host window moves (`windows_lead.rs`).
    pub(super) guest: Cell<Frame>,
    /// The press, drag or resize of the mouse now.
    pub(super) gesture: RefCell<Gesture>,
    /// A host-led move or resize runs: the moves go to the session.
    pub(super) led: Cell<bool>,
    /// The guest window is in full screen: the host window too.
    pub(super) full_screen: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockSessionWindow"]
    #[ivars = WindowIvars]
    pub struct SessionWindow;

    impl SessionWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            self.ivars().takes_key
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            self.ivars().takes_key
        }

        /// The children for Accessibility clients: the mirror of the
        /// guest window's tree once it is there. The first
        /// query asks the session for the tree and gets AppKit's own
        /// children, so a client sees the window at once and the tree
        /// on its next query.
        #[unsafe(method_id(accessibilityChildren))]
        fn accessibility_children(&self) -> Option<Retained<NSArray>> {
            let mirror = {
                let ax = self.ivars().ax.borrow();
                (!ax.is_empty()).then(|| ax.roots())
            };
            if mirror.is_some() {
                mirror
            } else {
                let asked = self.ivars().ax_asked.get();
                if asked.is_none_or(|t| t.elapsed() >= AX_ASK_GAP) {
                    self.ivars().ax_asked.set(Some(Instant::now()));
                    (self.ivars().sink)(HostEvent::AxWanted(self.ivars().id));
                }
                unsafe { msg_send![super(self), accessibilityChildren] }
            }
        }

        /// The element under a screen point: the deepest mirror element
        /// there, else AppKit's answer.
        #[unsafe(method_id(accessibilityHitTest:))]
        fn accessibility_hit_test(&self, point: NSPoint) -> Option<Retained<AnyObject>> {
            let hit = self.ivars().ax.borrow().hit(self.frame(), point);
            if hit.is_some() {
                hit
            } else {
                unsafe { msg_send![super(self), accessibilityHitTest: point] }
            }
        }

        /// The focused element: the one the guest reports as focused,
        /// else AppKit's answer.
        #[unsafe(method_id(accessibilityFocusedUIElement))]
        fn accessibility_focused_ui_element(&self) -> Option<Retained<AnyObject>> {
            let focused = self.ivars().ax.borrow().focused();
            if focused.is_some() {
                focused
            } else {
                unsafe { msg_send![super(self), accessibilityFocusedUIElement] }
            }
        }

        /// Cmd-Q runs the quit path of the session instead
        /// of going to the guest through the view, and a lone Globe key
        /// switches the guest's input source. Every other
        /// key goes on to the view.
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if is_cmd_q(event) {
                (self.ivars().sink)(HostEvent::Quit);
                return;
            }
            match super::windows_lead::mouse(self, event) {
                Mouse::Pass => {}
                Mouse::Swallow => return,
                Mouse::Replay(down) => {
                    // A title bar click without a move: the press and its
                    // release reach the guest.
                    let _: () = unsafe { msg_send![super(self), sendEvent: &*down] };
                }
            }
            if self.ivars().catch_globe {
                if let Some(k) = key_event(event) {
                    match self.ivars().globe.borrow_mut().step(k) {
                        Globe::Pass => {}
                        Globe::Swallow => return,
                        Globe::Fire => {
                            (self.ivars().sink)(HostEvent::Globe);
                            return;
                        }
                    }
                }
            }
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
        }
    }
);

impl SessionWindow {
    fn new(
        mtm: MainThreadMarker,
        w: &Window,
        catch_globe: bool,
        sink: HostSink,
        frame: NSRect,
        display: Display,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowIvars {
            id: w.id,
            takes_key: matches!(w.level, Level::Normal | Level::Floating | Level::Sheet),
            catch_globe,
            globe: RefCell::new(GlobeKey::default()),
            sink,
            view: RefCell::new(None),
            ax: RefCell::new(Elements::default()),
            ax_asked: Cell::new(None),
            level: w.level,
            display,
            chrome: RefCell::new(super::order::chrome_of(w)),
            guest: Cell::new(w.frame),
            gesture: RefCell::new(Gesture::Idle),
            led: Cell::new(false),
            full_screen: Cell::new(false),
        });
        unsafe {
            msg_send![
                super(this),
                initWithContentRect: frame,
                styleMask: NSWindowStyleMask::Borderless,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        }
    }

    pub fn id(&self) -> u32 {
        self.ivars().id
    }

    /// The view that shows the guest, once a frame source attached one.
    pub fn view(&self) -> Option<Retained<VZVirtualMachineView>> {
        self.ivars().view.borrow().clone()
    }

    /// The mirror elements of the window.
    pub fn ax(&self) -> &RefCell<Elements> {
        &self.ivars().ax
    }

    pub(super) fn sink(&self) -> &HostSink {
        &self.ivars().sink
    }
}

/// The event as the Globe catch sees it: a flags change with its key
/// code and Function flag, a key, or None for a mouse event.
fn key_event(event: &NSEvent) -> Option<KeyEvent> {
    match event.r#type() {
        NSEventType::FlagsChanged => Some(KeyEvent::Flags {
            code: event.keyCode(),
            function: event
                .modifierFlags()
                .contains(NSEventModifierFlags::Function),
        }),
        NSEventType::KeyDown | NSEventType::KeyUp => Some(KeyEvent::Key),
        _ => None,
    }
}

/// True for Cmd-Q with no other modifier.
fn is_cmd_q(event: &NSEvent) -> bool {
    if event.r#type() != NSEventType::KeyDown {
        return false;
    }
    let mods = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
    if mods != NSEventModifierFlags::Command {
        return false;
    }
    event
        .charactersIgnoringModifiers()
        .is_some_and(|s| s.to_string() == "q")
}

pub struct DelegateIvars {
    sink: HostSink,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockSessionWindowDelegate"]
    #[ivars = DelegateIvars]
    pub struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::BecameKey(id));
            }
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::ResignedKey(id));
            }
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::Miniaturized(id));
            }
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, note: &NSNotification) {
            if let Some(id) = id_of(note) {
                (self.ivars().sink)(HostEvent::Deminiaturized(id));
            }
        }

        #[unsafe(method(windowDidMove:))]
        fn did_move(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::moved(&w);
            }
        }

        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::moved(&w);
            }
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::full_screen_done(&w, true);
            }
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, note: &NSNotification) {
            if let Some(w) = window_of(note) {
                super::windows_lead::full_screen_done(&w, false);
            }
        }
    }
);

impl WindowDelegate {
    fn new(mtm: MainThreadMarker, sink: HostSink) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { sink });
        unsafe { msg_send![super(this), init] }
    }
}

/// The session window of a window notification.
fn window_of(note: &NSNotification) -> Option<Retained<SessionWindow>> {
    note.object()?.downcast::<SessionWindow>().ok()
}

/// The guest window id of a window notification.
fn id_of(note: &NSNotification) -> Option<u32> {
    window_of(note).map(|w| w.id())
}

/// The viewport source: one `VZVirtualMachineView` of the full guest
/// display per host window, a `DropVmView` that reports the drops on the
/// window to `drop_tx`.
pub struct Viewport {
    vm: Arc<Vm>,
    display: Display,
    drop_tx: Sender<DropEvent>,
    /// Where the file promises of a drop land.
    promise_dir: PathBuf,
}

impl Frames<SessionWindow> for Viewport {
    fn source(&self) -> Source {
        Source::Viewport
    }

    fn attach(&self, id: u32, window: &SessionWindow) -> Result<(), String> {
        let mtm = MainThreadMarker::from(window);
        let Some(content) = window.contentView() else {
            return Err(super::text::no_content_view());
        };
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(self.display.w, self.display.h),
        );
        let view: Retained<VZVirtualMachineView> = Retained::into_super(DropVmView::new(
            mtm,
            rect,
            id,
            self.promise_dir.clone(),
            self.drop_tx.clone(),
        ));
        // SAFETY: the VM lives as long as the session, and the view is
        // detached before the window closes.
        unsafe {
            view.setVirtualMachine(Some(&self.vm.0));
            view.setAutomaticallyReconfiguresDisplay(false);
            view.setCapturesSystemKeys(true);
        }
        content.addSubview(&view);
        window.makeFirstResponder(Some(&view));
        *window.ivars().view.borrow_mut() = Some(view);
        Ok(())
    }

    fn detach(&self, _id: u32, window: &SessionWindow) {
        if let Some(view) = window.ivars().view.borrow_mut().take() {
            unsafe { view.setVirtualMachine(None) };
            view.removeFromSuperview();
        }
    }
}

/// A host rectangle as AppKit's.
pub(super) fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

/// The host screens now, in points, with their scales. The first screen
/// of AppKit is the main one.
pub fn screens(mtm: MainThreadMarker) -> ScreenSet {
    let all = NSScreen::screens(mtm);
    let screens: Vec<Rect> = all
        .iter()
        .map(|s| {
            let f = s.frame();
            Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height)
        })
        .collect();
    if screens.is_empty() {
        return ScreenSet::one(1440.0, 900.0);
    }
    let scales = all.iter().map(|s| s.backingScaleFactor()).collect();
    ScreenSet {
        screens,
        main: 0,
        scales,
    }
}

/// The host windows of the session, by guest window id.
pub struct HostWindows {
    mtm: MainThreadMarker,
    frames: Viewport,
    display: Display,
    screens: ScreenSet,
    /// The guest's macOS major version: the corner radius of a window
    /// without a measured one.
    pub(super) macos: u32,
    /// The Globe key changes the input source on this Mac.
    catch_globe: bool,
    sink: HostSink,
    delegate: Retained<WindowDelegate>,
    pub(super) windows: BTreeMap<u32, Retained<SessionWindow>>,
}

impl HostWindows {
    /// No windows yet. `display` is the guest display; the corner radius
    /// is the one of macOS 15 until the hello names the guest's macOS. A drop on a
    /// window goes to `drop_tx`, its file promises under `promise_dir`.
    pub fn new(
        mtm: MainThreadMarker,
        vm: Arc<Vm>,
        display: Display,
        sink: HostSink,
        drop_tx: Sender<DropEvent>,
        promise_dir: PathBuf,
    ) -> Self {
        HostWindows {
            mtm,
            frames: Viewport {
                vm,
                display,
                drop_tx,
                promise_dir,
            },
            display,
            screens: screens(mtm),
            macos: 15,
            catch_globe: super::tis::globe_switches_sources(),
            sink: sink.clone(),
            delegate: WindowDelegate::new(mtm, sink),
            windows: BTreeMap::new(),
        }
    }

    pub fn screens(&self) -> &ScreenSet {
        &self.screens
    }

    pub fn display(&self) -> Display {
        self.display
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// The guest's macOS major version, from the hello: the corner radius
    /// of every window without a measured one.
    pub fn set_macos(&mut self, major: u32) {
        self.macos = major;
        for w in self.windows.values() {
            self.clip(w);
        }
    }

    /// Run the changes of the fold, in order. A `Refuse` is the session's
    /// work (the request back and the badge), not a window change.
    pub fn apply(&mut self, changes: &[Change]) {
        for c in changes {
            match c {
                Change::Open(w) => self.open(w),
                Change::Close(id) => self.close(*id),
                Change::Move(id, frame) => self.move_to(*id, *frame),
                Change::FullScreen(id, on) => self.full_screen(*id, *on),
                Change::Chrome(id, chrome) => self.set_chrome(*id, chrome),
                Change::Retitle(id, title) => {
                    if let Some(w) = self.windows.get(id) {
                        w.setTitle(&NSString::from_str(title));
                    }
                }
                Change::Reorder(ids) => self.reorder(ids),
                Change::Minimize(id) => {
                    if let Some(w) = self.windows.get(id) {
                        if !w.isMiniaturized() {
                            w.miniaturize(None);
                        }
                    }
                }
                Change::Restore(id) => {
                    if let Some(w) = self.windows.get(id) {
                        if w.isMiniaturized() {
                            w.deminiaturize(None);
                        }
                    }
                }
                Change::Focus(id) => {
                    if let Some(w) = self.windows.get(id) {
                        if !w.isKeyWindow() && w.ivars().takes_key {
                            w.makeKeyAndOrderFront(None);
                        }
                    }
                }
                // The session's work: the request back to the guest.
                Change::Refuse { .. } | Change::Correct(..) => {}
            }
        }
    }

    /// One new host window for guest window `w`.
    fn open(&mut self, w: &Window) {
        if self.windows.contains_key(&w.id) {
            self.close(w.id);
        }
        let frame = crop::host_frame(w.frame, &self.screens);
        let rect = ns_rect(frame);
        let window = SessionWindow::new(
            self.mtm,
            w,
            self.catch_globe,
            self.sink.clone(),
            rect,
            self.display,
        );
        // SAFETY: the map keeps the window; `close` releases it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setOpaque(false);
        window.setHasShadow(matches!(w.level, Level::Normal | Level::Floating));
        window.setTitle(&NSString::from_str(&w.title));
        window.setMovableByWindowBackground(false);
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setHidesOnDeactivate(false);
        window.setExcludedFromWindowsMenu(w.level != Level::Normal);
        let parent = w.parent.and_then(|p| self.windows.get(&p).cloned());
        let level = match w.level {
            Level::Normal | Level::Hidden => NSNormalWindowLevel,
            Level::Floating | Level::Sheet => match &parent {
                Some(p) => p.level() + 1,
                None => NSFloatingWindowLevel,
            },
            Level::Menu | Level::Popover | Level::Tooltip => NSPopUpMenuWindowLevel,
        };
        window.setLevel(level);
        window.setCollectionBehavior(if w.level == Level::Normal {
            // A guest window in full screen takes the host's full screen.
            NSWindowCollectionBehavior::Managed
                | NSWindowCollectionBehavior::ParticipatesInCycle
                | NSWindowCollectionBehavior::FullScreenPrimary
        } else {
            NSWindowCollectionBehavior::Transient | NSWindowCollectionBehavior::IgnoresCycle
        });
        // The content view clips the full-display view to the rounded
        // rectangle of the guest window.
        let content = NSView::initWithFrame(
            self.mtm.alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(frame.w, frame.h)),
        );
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setMasksToBounds(true);
        }
        window.setContentView(Some(&content));
        window.setDelegate(Some(ProtocolObject::from_ref(&*self.delegate)));
        // The view registers the dragged types: a drag over a session
        // window shows the copy cursor, and a drop reaches the session.
        if let Err(e) = self.frames.attach(w.id, &window) {
            log::warn!("macvm: window {}: {e}", w.id);
        }
        self.clip(&window);
        super::windows_lead::place_view(&window);
        if let Some(p) = &parent {
            // SAFETY: both windows are on the main thread and in the map.
            unsafe { p.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        }
        window.orderWindow_relativeTo(NSWindowOrderingMode::Above, 0);
        if w.minimized {
            window.miniaturize(None);
        }
        self.windows.insert(w.id, window);
        if super::order::full_screen_of(w) {
            self.full_screen(w.id, true);
        }
    }

    /// Close the host window of `id`: the view lets go of the VM first.
    fn close(&mut self, id: u32) {
        let Some(window) = self.windows.remove(&id) else {
            return;
        };
        self.frames.detach(id, &window);
        if let Some(children) = window.childWindows() {
            for child in children.iter() {
                window.removeChildWindow(&child);
            }
        }
        if let Some(parent) = window.parentWindow() {
            parent.removeChildWindow(&window);
        }
        window.setDelegate(None);
        window.close();
    }

    /// The order among the session's windows only: each
    /// window goes right below the one before it. Minimized windows and
    /// child windows keep their place.
    fn reorder(&self, ids: &[u32]) {
        let mut above: Option<&Retained<SessionWindow>> = None;
        for id in ids {
            let Some(w) = self.windows.get(id) else {
                continue;
            };
            if w.isMiniaturized() || w.parentWindow().is_some() {
                continue;
            }
            if let Some(a) = above {
                w.orderWindow_relativeTo(NSWindowOrderingMode::Below, a.windowNumber());
            }
            above = Some(w);
        }
    }

    /// Bring the window of `id` to the front and make it key, as the
    /// Dock or the Window menu does.
    pub fn raise(&self, id: u32) {
        if let Some(w) = self.windows.get(&id) {
            NSApplication::sharedApplication(self.mtm).activate();
            if w.isMiniaturized() {
                w.deminiaturize(None);
            }
            w.makeKeyAndOrderFront(None);
        }
    }

    /// The ids whose host window is on the active Space.
    pub fn active_space_ids(&self) -> Vec<u32> {
        self.windows
            .iter()
            .filter(|(_, w)| w.isOnActiveSpace())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Run the changes of the mirror on the elements of the window of
    /// `id`.
    pub fn ax_apply(&self, id: u32, mirror: &Mirror, changes: &[AxChange]) {
        if let Some(w) = self.windows.get(&id) {
            let posted = w.ax().borrow_mut().apply(w, mirror, changes, &self.sink);
            posted.post(w);
        }
    }

    /// The elements of the window of `id` go.
    pub fn ax_clear(&self, id: u32) {
        if let Some(w) = self.windows.get(&id) {
            let had = {
                let mut ax = w.ax().borrow_mut();
                let had = !ax.is_empty();
                ax.clear();
                had
            };
            if had {
                Posted {
                    layout: true,
                    focus: false,
                }
                .post(w);
            }
        }
    }

    /// The mirror elements of every window, for the control socket.
    pub fn ax_len(&self) -> usize {
        self.windows.values().map(|w| w.ax().borrow().len()).sum()
    }

    /// The key window of the session, when one is.
    pub fn key_id(&self) -> Option<u32> {
        self.windows
            .iter()
            .find(|(_, w)| w.isKeyWindow())
            .map(|(id, _)| *id)
    }

    /// The host window number of each guest window, for `window list`.
    pub fn numbers(&self) -> BTreeMap<u32, isize> {
        self.windows
            .iter()
            .map(|(id, w)| (*id, w.windowNumber()))
            .collect()
    }

    /// Close every window (the quit path, a guest restart).
    pub fn close_all(&mut self) {
        let ids: Vec<u32> = self.windows.keys().copied().collect();
        for id in ids {
            self.close(id);
        }
    }
}

#[path = "windows_state.rs"]
mod state;
