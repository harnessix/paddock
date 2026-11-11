//! The marker process on macOS: AppKit frames, badges and the menu bar item.
//!
//! macOS gives one process no way to recolor the windows of another, so
//! each protected window gets a borderless, click-through panel of its own,
//! ordered right above it. The window server list gives the windows and
//! their frames without any permission. With the Accessibility permission,
//! Accessibility notifications wake the marker on each move, and the menu
//! raises the exact window.

use super::{Rect, Target, Tone, Win};
use crate::env::HostEnv;
use crate::state::Running;
use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    define_class, msg_send, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSBackingStoreType, NSButton, NSCellImagePosition, NSColor, NSEvent, NSFont,
    NSFontWeightMedium, NSFontWeightRegular, NSImage, NSImageSymbolConfiguration, NSMenu,
    NSMenuItem, NSPanel, NSPasteboard, NSPasteboardTypeString, NSRunningApplication, NSScreen,
    NSStatusBar, NSStatusItem, NSTextField, NSVariableStatusItemLength, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask, NSWorkspace,
};
use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXObserver, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    kCFBooleanTrue, kCFRunLoopCommonModes, CFArray, CFRetained, CFRunLoop, CFString, CFType,
    CGPoint, CGSize,
};
use objc2_core_graphics::{CGColor, CGWindowListCopyWindowInfo, CGWindowListOption};
use objc2_foundation::{
    ns_string, NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString, NSTimer, NSURL,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::c_void;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// The timer runs this often. Most ticks do nothing.
const TICK: f64 = 1.0 / 60.0;
/// Without a reason to look sooner, read the window list every this many ticks.
const IDLE_TICKS: u64 = 6;
/// Read the sandbox list and the process table every this many ticks.
const RUNS_TICKS: u64 = 60;
/// After the last app sandbox ends, the marker waits this long, in seconds.
const LINGER: f64 = 3.0;
/// After the start, the marker waits this long for a sandbox, in seconds.
const GRACE: f64 = 15.0;

/// Set by Accessibility notifications: a window moved, so look now.
static DIRTY: AtomicBool = AtomicBool::new(false);

fn lock_path(env: &HostEnv) -> std::path::PathBuf {
    env.paddock_run().join("mark.lock")
}

/// The shortest time between two starts of the marker from one process.
const SPAWN_GAP: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether a marker holds the lock now. A probe lock taken and released
/// at once; an error counts as "not running".
fn marker_runs(env: &HostEnv) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::OpenOptions::new().read(true).open(lock_path(env)) else {
        return false;
    };
    // SAFETY: flock on an fd that `f` owns.
    let free = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if free {
        // SAFETY: as above.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
    }
    !free
}

/// Whether this process may start a marker now: at most one start per
/// `SPAWN_GAP`, whatever number of threads ask.
fn spawn_due() -> bool {
    use std::sync::Mutex;
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| t.elapsed() < SPAWN_GAP) {
        return false;
    }
    *last = Some(Instant::now());
    true
}

/// Start `paddock mark` from the paddock binary `exe` in its own session,
/// with its output in a log file of the run folder. Nothing starts while a
/// marker runs or within `SPAWN_GAP` of the last start of this process, and
/// a thread waits for the child, so a marker that ends never stays a
/// zombie of the caller (a session helper calls this from many threads).
pub fn spawn(env: &HostEnv, exe: &std::path::Path) {
    if marker_runs(env) || !spawn_due() {
        return;
    }
    let me = exe.to_path_buf();
    let log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(env.paddock_run().join("mark.log"))
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    let mut cmd = Command::new(me);
    cmd.arg("mark")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // SAFETY: setsid is async-signal-safe. The marker outlives this start.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => log::warn!("cannot start the window marks: {e}"),
    }
}

/// The windows on screen, front first.
pub fn windows() -> Vec<Win> {
    window_list()
}

/// The name of an app, as the Dock shows it.
pub(super) fn app_name(pid: u32) -> String {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
        .and_then(|a| a.localizedName())
        .map(|n| n.to_string())
        .unwrap_or_else(|| format!("process {pid}"))
}

/// The terminal window of the run that starts now: the front window of
/// the nearest ancestor app, when that app is in front. Else None and one
/// log line that says why.
pub fn terminal_window_now() -> Option<super::TerminalWindow> {
    let procs = super::procs::proc_table();
    let wins = window_list();
    let front = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|a| a.processIdentifier() as u32);
    match super::terminal_window(&procs, std::process::id(), &wins, front) {
        Ok(t) => {
            log::info!("window mark on window {} of {}", t.id, app_name(t.pid));
            Some(t)
        }
        Err(e) => {
            log::info!("{}", e.reason(app_name));
            None
        }
    }
}

/// Hold the lock of the one marker, or None when another marker holds it.
fn take_lock(env: &HostEnv) -> Result<Option<std::fs::File>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    crate::privfs::dir(&env.paddock_run())?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(lock_path(env))
        .context("cannot open the lock of the window marks")?;
    // SAFETY: a plain flock on a file that we own.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    Ok((r == 0).then_some(f))
}

pub fn main(env: &HostEnv) -> Result<i32> {
    let Some(_lock) = take_lock(env)? else {
        log::info!("the window marks already run");
        return Ok(0);
    };
    let mtm = MainThreadMarker::new().context("paddock mark must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon and no menu of its own: only the menu bar item.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let marker = Marker::new(mtm, env.clone());
    marker.reload();
    marker.refresh();
    // SAFETY: the marker answers `tick:` and lives as long as the app.
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            TICK,
            &marker,
            sel!(tick:),
            None,
            true,
        );
    }
    app.run();
    Ok(0)
}

/// The panels of one protected window.
struct Mark {
    /// The window server number of the protected window.
    target: u32,
    pid: u32,
    run: usize,
    bounds: Rect,
    tone: Tone,
    alpha: f64,
    frame: Retained<NSPanel>,
    badge: Retained<NSPanel>,
    label: Retained<NSTextField>,
    dot: Retained<NSView>,
    text: String,
    badge_size: (f64, f64),
    badge_shown: bool,
}

struct State {
    env: HostEnv,
    runs: Vec<Running>,
    /// What tells the windows of each run, in the order of `runs`.
    targets: Vec<Target>,
    /// The terminal app of each terminal run, by name, for the menu.
    terminals: Vec<Option<String>>,
    /// The facts of each guest session, in the order of `runs`.
    vm: Vec<Option<super::VmFacts>>,
    marks: BTreeMap<u32, Mark>,
    /// The focused protected window and when it got focus.
    focus: Option<(u32, Instant)>,
    status: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    menu_sig: String,
    observers: HashMap<u32, CFRetained<AXObserver>>,
    tick: u64,
    /// The last time a frame changed: look often for a while after.
    moved: Instant,
    started: Instant,
    empty_since: Option<Instant>,
    /// The `connect` questions of the firewall, read every second.
    questions: Vec<crate::broker::Pending>,
    /// Their panels, by question id.
    panels: BTreeMap<String, AskPanel>,
    panels_sig: String,
    /// The question of the open `More…` menu of a panel.
    menu_for: Option<String>,
}

struct Ivars {
    state: RefCell<Option<State>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockWindowMarker"]
    #[ivars = Ivars]
    struct Marker;

    unsafe impl NSObjectProtocol for Marker {}

    impl Marker {
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &AnyObject) {
            self.on_tick();
        }

        /// A window entry of the menu bar item: raise that window.
        #[unsafe(method(raise:))]
        fn raise(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            self.raise_window(tag as u32);
        }

        /// A click on a badge: the details of its sandbox.
        #[unsafe(method(badge:))]
        fn badge(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            self.show_details(tag as u32, sender);
        }

        /// Copy `paddock stop ID` for the sandbox of a window.
        #[unsafe(method(copyStop:))]
        fn copy_stop(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            self.copy_stop_command(tag as u32);
        }

        /// A button of a firewall panel: its answer into the store.
        #[unsafe(method(answerConnect:))]
        fn answer_connect(&self, sender: &AnyObject) {
            self.with(|st| answer_from_button(st, sender));
            self.refresh();
        }

        /// The `More…` button of a firewall panel: the other answers.
        #[unsafe(method(askMore:))]
        fn ask_more(&self, sender: &AnyObject) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let me: &AnyObject = self.as_ref();
            // The menu runs its own loop: the state is let go first.
            let menu = self.with(|st| more_menu(st, mtm, sender, me));
            if let Some(menu) = menu {
                // SAFETY: the sender is the `More…` button, a view.
                let view: &NSView = unsafe { &*(sender as *const AnyObject).cast::<NSView>() };
                menu.popUpMenuPositioningItem_atLocation_inView(
                    None,
                    NSPoint::new(0.0, -4.0),
                    Some(view),
                );
            }
            self.refresh();
        }

        /// An item of the `More…` menu: its answer into the store.
        #[unsafe(method(answerConnectMenu:))]
        fn answer_connect_menu(&self, sender: &AnyObject) {
            self.with(|st| answer_from_menu(st, sender));
        }

        #[unsafe(method(openAccessibility:))]
        fn open_accessibility(&self, _sender: &AnyObject) {
            if let Some(url) = NSURL::URLWithString(ns_string!(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            )) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            std::process::exit(0);
        }
    }
);

impl Marker {
    fn new(mtm: MainThreadMarker, env: HostEnv) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            state: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let status =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        status.setMenu(Some(&menu));
        if let Some(button) = status.button(mtm) {
            if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("checkmark.shield"),
                Some(ns_string!("paddock protected windows")),
            ) {
                img.setTemplate(true);
                button.setImage(Some(&img));
            }
            button.setImagePosition(NSCellImagePosition::ImageLeft);
        }
        let now = Instant::now();
        *this.ivars().state.borrow_mut() = Some(State {
            env,
            runs: Vec::new(),
            vm: Vec::new(),
            targets: Vec::new(),
            terminals: Vec::new(),
            marks: BTreeMap::new(),
            focus: None,
            status,
            menu,
            menu_sig: String::new(),
            observers: HashMap::new(),
            tick: 0,
            moved: now,
            started: now,
            empty_since: None,
            questions: Vec::new(),
            panels: BTreeMap::new(),
            panels_sig: String::new(),
            menu_for: None,
        });
        this
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut st = self.ivars().state.borrow_mut();
        f(st.as_mut().expect("the marker state"))
    }

    fn on_tick(&self) {
        let (tick, moved) = self.with(|st| {
            st.tick += 1;
            (st.tick, st.moved)
        });
        if tick % RUNS_TICKS == 0 {
            self.reload();
        }
        let busy = NSEvent::pressedMouseButtons() != 0
            || DIRTY.swap(false, Ordering::Relaxed)
            || moved.elapsed().as_secs_f64() < 0.5;
        if busy || tick % IDLE_TICKS == 0 {
            self.refresh();
        }
    }

    /// Read the marked runs, the process trees of app runs, the windows
    /// of terminal runs and the firewall questions again. End the marker
    /// a while after the last run and the last question are gone.
    fn reload(&self) {
        let quit = self.with(|st| {
            st.runs = super::marked_runs(Running::list(&st.env));
            st.vm = super::vm::facts(&st.env, &st.runs);
            st.questions = load_questions(&st.env);
            let procs = if st.runs.iter().any(|r| r.gui == "app") {
                super::procs::proc_table()
            } else {
                BTreeMap::new()
            };
            st.targets = st.runs.iter().map(|r| Target::of(r, &procs)).collect();
            st.terminals = st
                .targets
                .iter()
                .map(|t| match t {
                    Target::Window(_, pid) => Some(app_name(*pid)),
                    Target::Tree(_) => None,
                })
                .collect();
            if st.runs.is_empty() && st.questions.is_empty() {
                let since = *st.empty_since.get_or_insert_with(Instant::now);
                st.started.elapsed().as_secs_f64() > GRACE && since.elapsed().as_secs_f64() > LINGER
            } else {
                st.empty_since = None;
                false
            }
        });
        if quit {
            std::process::exit(0);
        }
        self.observe();
    }

    /// With the Accessibility permission: a notification on each move,
    /// resize, minimize and close of a protected window.
    fn observe(&self) {
        // SAFETY: a plain query.
        if !unsafe { AXIsProcessTrusted() } {
            return;
        }
        self.with(|st| {
            let pids: BTreeSet<u32> = st.marks.values().map(|m| m.pid).collect();
            st.observers.retain(|pid, obs| {
                let keep = pids.contains(pid);
                if !keep {
                    // SAFETY: the source of an observer that we made.
                    let src = unsafe { obs.run_loop_source() };
                    if let Some(rl) = CFRunLoop::main() {
                        rl.remove_source(Some(&src), unsafe { kCFRunLoopCommonModes });
                    }
                }
                keep
            });
            for pid in pids {
                if st.observers.contains_key(&pid) {
                    continue;
                }
                if let Some(obs) = ax_observer(pid) {
                    st.observers.insert(pid, obs);
                }
            }
        });
    }

    /// Read the window list and bring every frame and badge up to date.
    fn refresh(&self) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let wins = window_list();
        let own = std::process::id();
        let main_h = main_height(mtm);
        let screens = visible_screens(mtm, main_h);
        let mouse = NSEvent::mouseLocation();
        let (mx, my) = (mouse.x, main_h - mouse.y);
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier() as u32);
        let others: Vec<Win> = wins.iter().filter(|w| w.pid != own).cloned().collect();
        let focused = super::focused_window(&others, front);
        let target_sel = sel!(badge:);
        let me: &AnyObject = self.as_ref();

        self.with(|st| {
            let matched = super::match_windows(&wins, &st.targets, own);
            match (st.focus, focused) {
                (Some((id, _)), Some(f)) if id == f => {}
                (_, Some(f)) if matched.iter().any(|(w, _)| w.id == f) => {
                    st.focus = Some((f, Instant::now()))
                }
                _ => st.focus = None,
            }
            let now = crate::state::now();
            let mut seen = BTreeSet::new();
            for (w, run) in &matched {
                let r = &st.runs[*run];
                // A guest session window keeps its badge, without a frame.
                let framed = super::framed(r);
                seen.insert(w.id);
                let tone = Tone::of(&r.network);
                let text = super::run_badge(r, st.vm.get(*run).and_then(|f| f.as_ref()), now);
                let m = st
                    .marks
                    .entry(w.id)
                    .or_insert_with(|| Mark::new(mtm, w, *run, tone, me, target_sel));
                m.run = *run;
                m.pid = w.pid;
                if m.tone != tone {
                    m.set_tone(tone);
                }
                if m.text != text {
                    m.set_text(&text);
                }
                let mut place = false;
                if m.bounds != w.bounds {
                    m.bounds = w.bounds;
                    st.moved = Instant::now();
                    place = true;
                }
                let alpha = if !framed {
                    0.0
                } else if focused == Some(w.id) {
                    1.0
                } else {
                    super::DIM_ALPHA
                };
                if m.alpha != alpha {
                    m.alpha = alpha;
                    m.frame.setAlphaValue(alpha);
                }
                let badge = super::badge_rect(w.bounds, m.badge_size.0, m.badge_size.1, &screens);
                let hover = badge.outset(super::BADGE_HOVER_MARGIN).contains(mx, my);
                let focused_for = st
                    .focus
                    .filter(|(id, _)| *id == w.id)
                    .map(|(_, t)| t.elapsed().as_secs_f64());
                let show = super::badge_visible(focused_for, hover);
                if place || show != m.badge_shown {
                    m.frame
                        .setFrame_display(ns_rect(super::frame_rect(w.bounds).flip(main_h)), true);
                    m.badge.setFrame_display(ns_rect(badge.flip(main_h)), true);
                }
                // The frame goes right above its window, and the badge above
                // the frame. Only when something else got in between.
                let above = above_of(&wins, w.id);
                let frame_no = m.frame.windowNumber() as u32;
                let badge_no = m.badge.windowNumber() as u32;
                if above != Some(frame_no) && above != Some(badge_no) || show != m.badge_shown {
                    m.frame
                        .orderWindow_relativeTo(NSWindowOrderingMode::Above, w.id as isize);
                    if show {
                        m.badge.orderWindow_relativeTo(
                            NSWindowOrderingMode::Above,
                            m.frame.windowNumber(),
                        );
                    } else {
                        m.badge.orderOut(None);
                    }
                    m.badge_shown = show;
                }
            }
            st.marks.retain(|id, m| {
                let keep = seen.contains(id);
                if !keep {
                    m.frame.orderOut(None);
                    m.badge.orderOut(None);
                }
                keep
            });
            update_menu(st, mtm, me);
            refresh_panels(st, mtm, &wins, &screens, main_h, me);
        });
    }

    fn raise_window(&self, id: u32) {
        let Some((pid, bounds)) = self.with(|st| st.marks.get(&id).map(|m| (m.pid, m.bounds)))
        else {
            return;
        };
        // SAFETY: plain Accessibility calls on an element that we own.
        if unsafe { AXIsProcessTrusted() } {
            unsafe { ax_raise(pid, bounds) };
        }
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
        {
            app.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }

    fn show_details(&self, id: u32, sender: &AnyObject) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let me: &AnyObject = self.as_ref();
        let menu = self.with(|st| {
            let m = st.marks.get(&id)?;
            let r = st.runs.get(m.run)?;
            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            let terminal = st.terminals.get(m.run).cloned().flatten();
            for line in super::details(r, crate::state::now(), terminal.as_deref()) {
                menu.addItem(&item(mtm, &line, None, 0, me));
            }
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&item(
                mtm,
                "Copy the stop command",
                Some(sel!(copyStop:)),
                id as isize,
                me,
            ));
            Some(menu)
        });
        let Some(menu) = menu else {
            return;
        };
        // SAFETY: the sender of `badge:` is the badge button, a view.
        let view: &NSView = unsafe { &*(sender as *const AnyObject).cast::<NSView>() };
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(0.0, -4.0), Some(view));
    }

    fn copy_stop_command(&self, id: u32) {
        let Some(sandbox) = self.with(|st| {
            st.marks
                .get(&id)
                .and_then(|m| st.runs.get(m.run))
                .map(|r| r.id.clone())
        }) else {
            return;
        };
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: a constant of AppKit.
        let kind = unsafe { NSPasteboardTypeString };
        pb.setString_forType(
            &NSString::from_str(&format!("paddock stop {sandbox}")),
            kind,
        );
    }
}

impl Mark {
    fn new(
        mtm: MainThreadMarker,
        w: &Win,
        run: usize,
        tone: Tone,
        target: &AnyObject,
        action: objc2::runtime::Sel,
    ) -> Self {
        let frame = panel(mtm, false);
        // A new window is opaque: the frame starts hidden, and only a run
        // that gets a frame raises it.
        frame.setAlphaValue(0.0);
        let view = NSView::new(mtm);
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setBorderWidth(super::FRAME_WIDTH);
            layer.setCornerRadius(super::WINDOW_RADIUS + super::FRAME_WIDTH);
        }
        frame.setContentView(Some(&view));

        let badge = panel(mtm, true);
        // The system popover material, which follows Light and Dark; the
        // tone of the mark shows only in the border and the dot.
        let surface = NSVisualEffectView::new(mtm);
        surface.setMaterial(NSVisualEffectMaterial::Popover);
        surface.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        surface.setState(NSVisualEffectState::Active);
        surface.setWantsLayer(true);
        if let Some(layer) = surface.layer() {
            layer.setCornerRadius(11.0);
            layer.setMasksToBounds(true);
            layer.setBorderWidth(1.0);
        }
        let dot = NSView::new(mtm);
        dot.setWantsLayer(true);
        if let Some(layer) = dot.layer() {
            layer.setCornerRadius(3.0);
        }
        dot.setFrame(NSRect::new(NSPoint::new(10.0, 8.0), NSSize::new(6.0, 6.0)));
        let label = NSTextField::labelWithString(ns_string!(""), mtm);
        // SAFETY: a constant of AppKit.
        label.setFont(Some(&NSFont::systemFontOfSize_weight(11.0, unsafe {
            NSFontWeightMedium
        })));
        label.setTextColor(Some(&NSColor::labelColor()));
        surface.addSubview(&dot);
        surface.addSubview(&label);
        // A transparent button over the whole badge takes the click.
        // SAFETY: the target answers the action and outlives the button.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!(""), Some(target), Some(action), mtm)
        };
        button.setBordered(false);
        button.setTransparent(true);
        button.setTag(w.id as isize);
        surface.addSubview(&button);
        let surface_view: &NSView = &surface;
        badge.setContentView(Some(surface_view));

        let mut m = Mark {
            target: w.id,
            pid: w.pid,
            run,
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            tone,
            alpha: 0.0,
            frame,
            badge,
            label,
            dot,
            text: String::new(),
            badge_size: (0.0, 22.0),
            badge_shown: false,
        };
        m.set_tone(tone);
        let _ = m.target;
        m
    }

    fn set_tone(&mut self, tone: Tone) {
        self.tone = tone;
        let (r, g, b) = tone.rgb();
        if let Some(layer) = self.frame.contentView().and_then(|v| v.layer()) {
            layer.setBorderColor(Some(&CGColor::new_srgb(r, g, b, 1.0)));
        }
        if let Some(layer) = self.badge.contentView().and_then(|v| v.layer()) {
            layer.setBorderColor(Some(&CGColor::new_srgb(r, g, b, 0.85)));
        }
        if let Some(layer) = self.dot.layer() {
            layer.setBackgroundColor(Some(&CGColor::new_srgb(r, g, b, 1.0)));
        }
    }

    fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.label.setStringValue(&NSString::from_str(text));
        self.label.sizeToFit();
        let size = self.label.frame().size;
        let h = 22.0;
        let w = (10.0 + 6.0 + 6.0 + size.width + 10.0).ceil();
        self.label
            .setFrameOrigin(NSPoint::new(22.0, ((h - size.height) / 2.0).round()));
        if let Some(view) = self.badge.contentView() {
            for sub in view.subviews().iter() {
                if sub.isKindOfClass(NSButton::class()) {
                    sub.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
                }
            }
        }
        self.badge_size = (w, h);
    }
}

#[path = "ask.rs"]
mod ask;
#[path = "ask_panel.rs"]
mod ask_panel;
#[path = "ui.rs"]
mod ui;
use ask::*;
use ui::*;
