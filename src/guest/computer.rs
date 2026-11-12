//! Computer use in the guest: the server of the `guest-computer` port
//! (`macvm::computer` has the wire). It captures the target, posts input
//! into the guest session and reads the accessibility snapshot, through a
//! [`Screen`]: ScreenCaptureKit, CGEvent and Accessibility on macOS
//! (`computer_macos.rs`), a fake in the tests.
//!
//! The windows of the app come from the main loop: each poll publishes
//! the [`Scene`] (the app's pids and the windows that the host shows), so
//! the app scope here is the same set of windows that the host forwards.
//!
//! Each connection has its own thread; a subscription adds one more,
//! which captures at most `max_fps` times a second and sends a frame only
//! when the digest of its pixels changed. One mutex keeps the calls on
//! the [`Screen`] one at a time.

use crate::guest::windows::RawWindow;
use crate::macvm::computer::{
    self as cu, frame_interval, Button, Call, Code, Combo, Data, Differ, Done, Element, Failure,
    FrameHeader, Geometry, Hello, Input, MaxSize, Msg, Op, Outcome, Scope, Status, Target,
    WindowInfo,
};
use crate::macvm::computer_input::{self as ci, Low};
use crate::macvm::proto::{Display, Frame, Level, Point, Window};
use crate::macvm::text;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The windows of the app of the session, as the last poll saw them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// The bundle id of the app.
    pub app: Option<String>,
    /// The pid that paddock launched.
    pub root: Option<u32>,
    /// The pids of the app and its helpers.
    pub pids: BTreeSet<u32>,
    /// The windows that the host shows, front first.
    pub windows: Vec<Window>,
}

static SCENE: Mutex<Option<Scene>> = Mutex::new(None);

/// The main loop publishes the scene after each poll.
pub fn publish(scene: Scene) {
    if let Ok(mut s) = SCENE.lock() {
        *s = Some(scene);
    }
}

/// The scene of the last poll, empty before the first.
pub fn scene() -> Scene {
    SCENE
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_default()
}

/// One capture: its size, the digest of its pixels, and the image for
/// [`Screen::png`].
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub digest: [u8; 32],
    pub image: Box<dyn std::any::Any + Send>,
}

/// The windows of an accessibility read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxWindow {
    pub id: u32,
    pub pid: u32,
    pub frame: Frame,
}

/// What computer use needs from the guest system.
pub trait Screen: Send {
    /// The display in points and its pixels per point.
    fn display(&mut self) -> (Display, f64);
    /// Screen Recording and Accessibility.
    fn permissions(&mut self) -> (bool, bool);
    /// The windows on the screen, front first.
    fn on_screen(&mut self) -> Vec<RawWindow>;
    fn locked(&mut self) -> bool;
    fn front_pid(&mut self) -> Option<u32>;
    /// The bundle id of the front app.
    fn front_app(&mut self) -> Option<String>;
    /// The pixels of `g.region()` at `g.width` by `g.height`: every
    /// window, or only `windows` when given.
    fn capture(
        &mut self,
        g: &Geometry,
        windows: Option<&[u32]>,
        cursor: bool,
    ) -> Result<Shot, Failure>;
    fn png(&mut self, shot: &Shot) -> Result<Vec<u8>, Failure>;
    /// Post one low event into the session. Never a wait.
    fn post(&mut self, ev: &Low) -> Result<(), Failure>;
    /// Bring the app of `pid` to the front.
    fn activate(&mut self, pid: u32) -> Result<(), Failure>;
    /// The elements of the windows, and whether a limit cut the read.
    fn ax(
        &mut self,
        windows: &[AxWindow],
        depth: u32,
        max_nodes: u32,
    ) -> Result<(Vec<Element>, bool), Failure>;
    /// `AXPress` on the element at `path` of the window: its role and
    /// label.
    fn ax_press(
        &mut self,
        window: AxWindow,
        path: &[usize],
        role: Option<&str>,
    ) -> Result<(String, String), Failure>;
    /// The character of the key `code` with cmd, and with shift when
    /// `shift`, on the current keyboard layout of the guest; None when the
    /// layout cannot say.
    fn key_char(&mut self, code: u16, shift: bool) -> Option<char>;
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The server: the screen and the source of the scene.
pub struct Server<S: Screen> {
    pub screen: Arc<Mutex<S>>,
    pub scene: Arc<dyn Fn() -> Scene + Send + Sync>,
}

impl<S: Screen> Clone for Server<S> {
    fn clone(&self) -> Self {
        Server {
            screen: self.screen.clone(),
            scene: self.scene.clone(),
        }
    }
}

/// The windows of the target that a capture shows: visible, with an area.
pub fn shown(scene: &Scene) -> Vec<&Window> {
    scene
        .windows
        .iter()
        .filter(|w| !w.minimized && w.level != Level::Hidden && w.frame.w > 0.0 && w.frame.h > 0.0)
        .collect()
}

/// The region of the target on the display, and the windows of the app
/// scope.
pub fn region(
    scope: Scope,
    scene: &Scene,
    display: Display,
) -> Result<(Frame, Option<Vec<u32>>), Failure> {
    let full = Frame::new(0.0, 0.0, f64::from(display.w), f64::from(display.h));
    match scope {
        Scope::Desktop => Ok((full, None)),
        Scope::App => {
            if scene.pids.is_empty() {
                return Err(Failure::new(Code::NoApp, text::cu_no_app()));
            }
            let windows = shown(scene);
            let bbox = cu::union(windows.iter().map(|w| w.frame))
                .and_then(|f| cu::clip(f, display))
                .ok_or_else(|| Failure::new(Code::NoWindow, text::cu_no_window()))?;
            Ok((bbox, Some(windows.iter().map(|w| w.id).collect())))
        }
    }
}

/// Window server layers that never take a click: the menu bar, the Dock
/// and the desktop pictures are under or around every app window.
fn passes_through(w: &RawWindow) -> bool {
    w.alpha <= 0.0 || w.bounds.w <= 0.0 || w.bounds.h <= 0.0
}

/// The window server layer of an open menu (`kCGPopUpMenuWindowLevel`):
/// a menu of the menu bar, the Apple menu, a context menu, a pop-up.
pub const MENU_LAYER: i64 = 101;
/// The layers of the menu bar and of its status items
/// (`kCGMainMenuWindowLevel`, `kCGStatusWindowLevel`).
const MENU_BAR_LAYERS: [i64; 2] = [24, 25];
/// The height of the menu bar in points, from the top of the display: the
/// Apple menu and the app menu are in it.
pub const MENU_BAR_HEIGHT: f64 = 24.0;

/// True when a menu is open on the screen. A menu of the app belongs to
/// the app pid, so the checks of a point allow clicks on it, and the arrow
/// keys go from the app's menus to the Apple menu and Services. In app
/// scope no input and no press goes while a menu is open, except escape.
pub fn menu_open(on_screen: &[RawWindow]) -> bool {
    on_screen
        .iter()
        .any(|w| w.layer == MENU_LAYER && !passes_through(w))
}

/// True when `p` is on the menu bar: the top strip of the display, or a
/// window of the menu bar layers.
fn on_menu_bar(p: Point, on_screen: &[RawWindow]) -> bool {
    p.y < MENU_BAR_HEIGHT
        || on_screen.iter().any(|w| {
            MENU_BAR_LAYERS.contains(&w.layer) && !passes_through(w) && cu::holds(&w.bounds, p)
        })
}

/// True when the action only closes a menu: escape keys and nothing else.
fn closes_menu(action: &Input) -> bool {
    matches!(action, Input::Key { combos } if !combos.is_empty() && combos.iter().all(ci::is_escape))
}

/// Check that input at `p` is allowed for the target: on the display, and
/// in app scope on a window of the app that nothing covers, not on the
/// menu bar.
pub fn check_point(
    scope: Scope,
    p: Point,
    display: Display,
    scene: &Scene,
    on_screen: &[RawWindow],
) -> Result<(), Failure> {
    let full = Frame::new(0.0, 0.0, f64::from(display.w), f64::from(display.h));
    if !cu::holds(&full, p) {
        return Err(Failure::new(
            Code::Outside,
            text::cu_outside_display(p.x, p.y, display.w, display.h),
        ));
    }
    if scope == Scope::Desktop {
        return Ok(());
    }
    if scene.pids.is_empty() {
        return Err(Failure::new(Code::NoApp, text::cu_no_app()));
    }
    if on_menu_bar(p, on_screen) {
        return Err(Failure::new(Code::Outside, text::cu_menu_bar(p.x, p.y)));
    }
    let ids: BTreeSet<u32> = shown(scene).iter().map(|w| w.id).collect();
    let top = on_screen
        .iter()
        .find(|w| !passes_through(w) && cu::holds(&w.bounds, p));
    let on_app = shown(scene).iter().any(|a| cu::holds(&a.frame, p));
    match top {
        Some(w) if ids.contains(&w.id) || scene.pids.contains(&w.pid) => Ok(()),
        Some(w) if on_app => Err(Failure::new(
            Code::Covered,
            text::cu_covered(p.x, p.y, &w.owner),
        )),
        // The window server list may lag a new window of the app.
        None if on_app => Ok(()),
        _ => Err(Failure::new(Code::Outside, text::cu_outside_app(p.x, p.y))),
    }
}

/// How long the guest looks for a menu that a press of the mouse button
/// opens: a hold-to-open menu or a slow window list. Between two looks it
/// waits [`MENU_LOOK`].
pub const MENU_WAIT: Duration = Duration::from_millis(500);
const MENU_LOOK: Duration = Duration::from_millis(50);

/// True when a menu is open. With `poll`, look again each [`MENU_LOOK`]
/// until a menu opens or [`MENU_WAIT`] passes.
fn menu_seen<S: Screen>(s: &mut S, poll: bool) -> bool {
    let mut left = if poll { MENU_WAIT } else { Duration::ZERO };
    loop {
        if menu_open(&s.on_screen()) {
            return true;
        }
        if left.is_zero() {
            return false;
        }
        let d = MENU_LOOK.min(left);
        s.sleep(d);
        left -= d;
    }
}

/// Stop an action after `done` of `total` steps because a menu opened:
/// escape closes it while the button is down, then the rest that the
/// action holds goes up.
fn menu_stop<S: Screen>(s: &mut S, held: &mut Held, done: usize, total: usize) -> Failure {
    if held.button.is_some() {
        held.escape(s);
    }
    held.release(s);
    Failure::new(Code::Covered, text::cu_menu_opened(done, total))
}

/// How long the app may take to come to the front for keys.
const ACTIVATE_WAIT: Duration = Duration::from_millis(1500);

impl<S: Screen> Server<S> {
    pub fn new(screen: S, scene: Arc<dyn Fn() -> Scene + Send + Sync>) -> Self {
        Server {
            screen: Arc::new(Mutex::new(screen)),
            scene,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, S> {
        // A panic in one connection must not end computer use.
        self.screen.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn hello(&self) -> Hello {
        let mut s = self.lock();
        let (display, scale) = s.display();
        let (screen_recording, accessibility) = s.permissions();
        Hello {
            v: cu::VERSION,
            display,
            scale,
            screen_recording,
            accessibility,
        }
    }

    /// The geometry of the target now, and the windows of the app scope.
    fn geometry(
        &self,
        s: &mut S,
        target: Target,
        max: Option<MaxSize>,
    ) -> Result<(Geometry, Option<Vec<u32>>), Failure> {
        let (display, scale) = s.display();
        let (region, windows) = region(target.scope, &(self.scene)(), display)?;
        Ok((Geometry::plan(region, scale, max), windows))
    }

    pub fn status(&self, target: Target) -> Status {
        let scene = (self.scene)();
        let mut s = self.lock();
        let (display, scale) = s.display();
        let (screen_recording, accessibility) = s.permissions();
        let front = s.front_pid();
        let geometry = region(target.scope, &scene, display)
            .ok()
            .map(|(r, _)| Geometry::plan(r, scale, None));
        Status {
            display,
            scale,
            screen_recording,
            accessibility,
            locked: s.locked(),
            front_app: s.front_app(),
            app: scene.app.clone(),
            app_running: !scene.pids.is_empty(),
            app_front: front.is_some_and(|f| scene.pids.contains(&f)),
            windows: scene
                .windows
                .iter()
                .map(|w| WindowInfo {
                    id: w.id,
                    title: w.title.clone(),
                    frame: w.frame,
                    focused: w.focused,
                    minimized: w.minimized,
                })
                .collect(),
            geometry,
        }
    }

    /// One capture of the target as a PNG.
    pub fn screenshot(
        &self,
        target: Target,
        max: Option<MaxSize>,
        cursor: bool,
    ) -> Result<(Geometry, Vec<u8>), Failure> {
        let mut s = self.lock();
        let (g, windows) = self.geometry(&mut s, target, max)?;
        let shot = s.capture(&g, windows.as_deref(), cursor)?;
        let png = s.png(&shot)?;
        Ok((g, png))
    }

    /// One input action, after the checks of the target.
    pub fn input(&self, target: Target, action: &Input) -> Result<Done, Failure> {
        let steps = ci::steps(action).map_err(|e| Failure::new(Code::BadRequest, e))?;
        // Keys in app scope go to the app only: a combo that the system
        // takes would reach every other app of the guest.
        let app_keys = target.scope == Scope::App && action.is_keyboard();
        if let (true, Input::Key { combos }) = (app_keys, action) {
            if let Some(c) = combos.iter().find(|c| ci::system_shortcut(c)) {
                return Err(Failure::new(
                    Code::BadRequest,
                    text::cu_system_shortcut(&ci::combo_text(c)),
                ));
            }
        }
        let in_app = target.scope == Scope::App;
        // A right or ctrl drag opens a context menu at the press, and the
        // let-go picks the item under the end of the drag.
        if in_app && ci::menu_drag(action) {
            return Err(Failure::new(Code::BadRequest, text::cu_menu_drag()));
        }
        // The first right or ctrl click opens a context menu, and macOS
        // can put it under the pointer: the next click picks an item.
        if in_app && ci::menu_clicks(action) {
            return Err(Failure::new(Code::BadRequest, text::cu_menu_clicks()));
        }
        let scene = (self.scene)();
        let mut s = self.lock();
        // The Help menu by the character of its key on the guest's layout.
        if let (true, Input::Key { combos }) = (app_keys, action) {
            if let Some(c) = combos
                .iter()
                .find(|c| ci::help_combo(c, |code, shift| s.key_char(code, shift)))
            {
                return Err(Failure::new(
                    Code::BadRequest,
                    text::cu_system_shortcut(&ci::combo_text(c)),
                ));
            }
        }
        if s.locked() {
            return Err(Failure::new(Code::Locked, text::cu_locked()));
        }
        let (_, accessibility) = s.permissions();
        if !accessibility {
            return Err(Failure::new(
                Code::Accessibility,
                text::cu_no_accessibility(),
            ));
        }
        let (display, _) = s.display();
        let on_screen = s.on_screen();
        for p in action.points() {
            check_point(target.scope, p, display, &scene, &on_screen)?;
        }
        // An open menu takes the keys and the clicks, and from it the
        // arrow keys reach the Apple menu: only escape goes.
        if in_app && menu_open(&on_screen) && !closes_menu(action) {
            return Err(Failure::new(Code::Covered, text::cu_menu_open()));
        }
        let mut activated = false;
        if app_keys {
            activated = self.front(&mut s, &scene)?;
        }
        let mut events = 0;
        let mut held = Held::default();
        for (i, step) in steps.iter().enumerate() {
            // Before each combo and each chunk of text: the app is still
            // in front, or the rest would go to another app.
            if app_keys && i > 0 {
                let now = (self.scene)();
                if !s.front_pid().is_some_and(|f| now.pids.contains(&f)) {
                    return Err(Failure::new(
                        Code::NotFront,
                        text::cu_front_lost(i, steps.len()),
                    ));
                }
            }
            // A combo of this action, or the press of a drag, opened a
            // menu. With the button down, escape closes the menu before
            // the let-go, so the let-go picks no item. A menu of a press
            // can open late (a hold opens it), so with the button down
            // the guest looks again until MENU_WAIT passes.
            if in_app && i > 0 && !closes_menu(action) && menu_seen(&mut *s, held.button.is_some())
            {
                return Err(menu_stop(&mut *s, &mut held, i, steps.len()));
            }
            for ev in step {
                // The menu can also open while the pointer moves: the
                // let-go of a drag picks no item of it.
                if in_app
                    && matches!(ev, Low::Up { .. })
                    && held.button.is_some()
                    && matches!(action, Input::Drag { .. })
                    && menu_seen(&mut *s, true)
                {
                    return Err(menu_stop(&mut *s, &mut held, i, steps.len()));
                }
                match ev {
                    Low::Wait(ms) => s.sleep(Duration::from_millis(u64::from(*ms))),
                    ev => {
                        if let Err(e) = s.post(ev) {
                            held.release(&mut *s);
                            return Err(e);
                        }
                        held.note(ev);
                    }
                }
            }
            events += ci::event_count(step);
        }
        Ok(Done { events, activated })
    }

    /// Bring the app to the front for keys: true when it was not.
    fn front(&self, s: &mut S, scene: &Scene) -> Result<bool, Failure> {
        if s.front_pid().is_some_and(|f| scene.pids.contains(&f)) {
            return Ok(false);
        }
        let pid = scene
            .root
            .or_else(|| scene.windows.first().map(|w| w.pid))
            .ok_or_else(|| Failure::new(Code::NoApp, text::cu_no_app()))?;
        s.activate(pid)?;
        let end = Instant::now() + ACTIVATE_WAIT;
        loop {
            if s.front_pid().is_some_and(|f| scene.pids.contains(&f)) {
                return Ok(true);
            }
            if Instant::now() >= end {
                return Err(Failure::new(Code::NotFront, text::cu_not_front()));
            }
            s.sleep(Duration::from_millis(50));
        }
    }

    /// The windows that an accessibility read of the target covers: the
    /// app's, or every window of an app on the display.
    fn ax_windows(&self, s: &mut S, scope: Scope) -> Result<Vec<AxWindow>, Failure> {
        let scene = (self.scene)();
        match scope {
            Scope::App => {
                if scene.pids.is_empty() {
                    return Err(Failure::new(Code::NoApp, text::cu_no_app()));
                }
                Ok(shown(&scene)
                    .iter()
                    .map(|w| AxWindow {
                        id: w.id,
                        pid: w.pid,
                        frame: w.frame,
                    })
                    .collect())
            }
            Scope::Desktop => Ok(s
                .on_screen()
                .into_iter()
                .filter(|w| w.layer == 0 && w.bundle.is_some() && !passes_through(w))
                .map(|w| AxWindow {
                    id: w.id,
                    pid: w.pid,
                    frame: w.bounds,
                })
                .collect()),
        }
    }

    /// The accessibility snapshot of the target.
    pub fn ax(
        &self,
        target: Target,
        depth: Option<u32>,
        max_nodes: Option<u32>,
    ) -> Result<(Geometry, Vec<Element>, bool), Failure> {
        let depth = depth
            .unwrap_or(cu::AX_DEPTH_DEFAULT)
            .clamp(1, cu::AX_DEPTH_MAX);
        let nodes = max_nodes
            .unwrap_or(cu::AX_NODES_DEFAULT)
            .clamp(1, cu::AX_NODES_MAX);
        let mut s = self.lock();
        let (_, accessibility) = s.permissions();
        if !accessibility {
            return Err(Failure::new(
                Code::Accessibility,
                text::cu_no_accessibility(),
            ));
        }
        let (g, _) = self.geometry(&mut s, target, None)?;
        let windows = self.ax_windows(&mut s, target.scope)?;
        let (elements, cut) = s.ax(&windows, depth, nodes)?;
        Ok((g, elements, cut))
    }

    /// `AXPress` on an element by its id.
    pub fn press(
        &self,
        target: Target,
        element: &str,
        role: Option<&str>,
    ) -> Result<(String, String), Failure> {
        let (window, path) = cu::parse_element_id(element)
            .ok_or_else(|| Failure::new(Code::BadRequest, text::cu_element_id_invalid(element)))?;
        let mut s = self.lock();
        if s.locked() {
            return Err(Failure::new(Code::Locked, text::cu_locked()));
        }
        // From an open menu a press can reach the Apple menu and Services,
        // as a key can.
        if target.scope == Scope::App && menu_open(&s.on_screen()) {
            return Err(Failure::new(Code::Covered, text::cu_menu_open()));
        }
        let windows = self.ax_windows(&mut s, target.scope)?;
        let w = windows
            .into_iter()
            .find(|w| w.id == window)
            .ok_or_else(|| Failure::new(Code::NoElement, text::cu_element_window_gone(window)))?;
        s.ax_press(w, &path, role)
    }
}

/// The keys, the modifiers and the mouse button that an action holds
/// down now, so that a failed post does not leave them down.
#[derive(Default)]
struct Held {
    /// Each modifier down, with the flags after it.
    mods: Vec<(u16, u64)>,
    /// The key down, with its flags.
    key: Option<(u16, u64)>,
    /// The mouse button down: where the pointer is now, and the flags of
    /// the press.
    button: Option<(Point, Button, u64)>,
}

impl Held {
    fn note(&mut self, ev: &Low) {
        match *ev {
            Low::Modifier {
                code,
                down: true,
                flags,
            } => self.mods.push((code, flags)),
            Low::Modifier {
                code, down: false, ..
            } => self.mods.retain(|(c, _)| *c != code),
            Low::Key {
                code,
                down: true,
                flags,
                ..
            } => self.key = Some((code, flags)),
            Low::Key { down: false, .. } => self.key = None,
            Low::Down {
                at, button, flags, ..
            } => self.button = Some((at, button, flags)),
            Low::Drag { at, .. } => {
                if let Some(b) = self.button.as_mut() {
                    b.0 = at;
                }
            }
            Low::Up { .. } => self.button = None,
            _ => {}
        }
    }

    /// Press and let go of escape: it closes the menu that a press
    /// opened. A failed post changes nothing here.
    fn escape<S: Screen>(&mut self, s: &mut S) {
        let mut evs = Vec::new();
        let esc = Combo {
            mods: Vec::new(),
            key: "escape".into(),
        };
        if ci::combo_plan(&esc, &mut evs).is_ok() {
            for ev in evs.iter().filter(|e| !matches!(e, Low::Wait(_))) {
                let _ = s.post(ev);
            }
        }
    }

    /// Release the key, then the mouse button where the pointer is, then
    /// each modifier, the last first. A post that
    /// fails here again changes nothing: each later event of the guest
    /// sets its own flags.
    fn release<S: Screen>(&mut self, s: &mut S) {
        if let Some((code, flags)) = self.key.take() {
            let _ = s.post(&Low::Key {
                code,
                down: false,
                flags,
                chars: None,
            });
        }
        if let Some((at, button, flags)) = self.button.take() {
            let _ = s.post(&Low::Up {
                at,
                button,
                clicks: 1,
                flags,
            });
        }
        while let Some((code, _)) = self.mods.pop() {
            let flags = self.mods.last().map_or(0, |(_, f)| *f);
            let _ = s.post(&Low::Modifier {
                code,
                down: false,
                flags,
            });
        }
    }
}

/// The writer of one connection: a line and its payload go out whole.
#[derive(Clone)]
pub struct Out(Arc<Mutex<UnixStream>>);

impl Out {
    pub fn new(s: UnixStream) -> Self {
        Out(Arc::new(Mutex::new(s)))
    }

    pub fn send(&self, msg: &Msg, payload: &[u8]) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.write_all(&line)?;
        s.write_all(payload)?;
        s.flush()
    }

    fn reply(&self, id: u64, r: Result<(Option<Data>, Vec<u8>), Failure>) -> std::io::Result<()> {
        match r {
            Ok((data, payload)) => {
                let bytes = (!payload.is_empty()).then_some(payload.len() as u64);
                self.send(
                    &Msg::Reply {
                        id,
                        outcome: Outcome::Ok { data, bytes },
                    },
                    &payload,
                )
            }
            Err(f) => self.send(
                &Msg::Reply {
                    id,
                    outcome: Outcome::Error(f),
                },
                &[],
            ),
        }
    }
}

/// A running subscription: its stop flag, its thread and its counter.
struct Sub {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<u64>>,
}

impl Sub {
    /// Stop the stream and give the last sequence number sent.
    fn end(mut self) -> u64 {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or(0)
    }
}

/// The settings of one subscription.
#[derive(Debug, Clone, Copy)]
pub struct Stream {
    pub target: Target,
    pub max_fps: f64,
    pub max: Option<MaxSize>,
    pub cursor: bool,
}

/// One pass of a frame stream: capture, compare, send. The frame or why
/// it did not go (`Ok(None)`: unchanged).
pub fn frame_once<S: Screen>(
    server: &Server<S>,
    st: &Stream,
    differ: &mut Differ,
    out: &Out,
) -> Result<Option<u64>, Failure> {
    let (g, png) = {
        let mut s = server.lock();
        let (g, windows) = server.geometry(&mut s, st.target, st.max)?;
        let shot = s.capture(&g, windows.as_deref(), st.cursor)?;
        let digest = Differ::digest(shot.digest, &g);
        // Compare before the PNG: an unchanged frame costs no encoding.
        if !differ.changed(digest) {
            return Ok(None);
        }
        let png = s.png(&shot)?;
        differ.next(digest);
        (g, png)
    };
    let seq = differ.last_seq();
    let header = FrameHeader {
        seq,
        width: g.width,
        height: g.height,
        geometry: g,
        bytes: png.len() as u64,
    };
    out.send(&Msg::Frame(header), &png)
        .map_err(|e| Failure::new(Code::Internal, e.to_string()))?;
    Ok(Some(seq))
}

/// The loop of a subscription, until the stop flag or a closed
/// connection. A failed capture (no window for a while, a locked screen)
/// waits and tries again; it does not end the stream.
fn stream_loop<S: Screen + 'static>(
    server: Server<S>,
    st: Stream,
    out: Out,
    stop: Arc<AtomicBool>,
) -> u64 {
    let interval = frame_interval(st.max_fps);
    let mut differ = Differ::default();
    let mut logged: Option<Code> = None;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        match frame_once(&server, &st, &mut differ, &out) {
            Ok(_) => logged = None,
            Err(f) if f.code == Code::Internal => break,
            Err(f) => {
                if logged != Some(f.code) {
                    log::info!("computer: frames: {}", f.text);
                    logged = Some(f.code);
                }
            }
        }
        let spent = started.elapsed();
        let wait = interval.saturating_sub(spent);
        // Wake often enough that a stop never waits a whole slow interval.
        let end = Instant::now() + wait;
        while !stop.load(Ordering::SeqCst) && Instant::now() < end {
            std::thread::sleep((end - Instant::now()).min(Duration::from_millis(20)));
        }
    }
    differ.last_seq()
}

/// Serve one connection until it closes.
pub fn serve<S: Screen + 'static>(server: Server<S>, stream: UnixStream) {
    let Ok(read) = stream.try_clone() else {
        return;
    };
    let out = Out::new(stream);
    if out.send(&Msg::Hello(server.hello()), &[]).is_err() {
        return;
    }
    let mut reader = BufReader::new(read);
    let mut sub: Option<Sub> = None;
    loop {
        let line = match read_line(&mut reader) {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                let _ = out.reply(0, Err(Failure::new(Code::BadRequest, e)));
                break;
            }
        };
        let call: Call = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                // The id when the line has one, so the host is not left waiting.
                let id = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_u64()))
                    .unwrap_or(0);
                let f = Failure::new(Code::Unsupported, text::cu_call_unknown(&e.to_string()));
                if out.reply(id, Err(f)).is_err() {
                    break;
                }
                continue;
            }
        };
        let r = match call.op {
            Op::FramesSubscribe {
                target,
                max_fps,
                max,
                cursor,
            } => {
                if let Some(old) = sub.take() {
                    old.end();
                }
                let st = Stream {
                    target,
                    max_fps,
                    max,
                    cursor,
                };
                let r = {
                    let mut s = server.lock();
                    server.geometry(&mut s, target, max)
                };
                match r {
                    Ok((g, _)) => {
                        let stop = Arc::new(AtomicBool::new(false));
                        let (sv, o, st2) = (server.clone(), out.clone(), stop.clone());
                        // The reply goes before the first frame.
                        let sent = out.reply(call.id, Ok((Some(Data::Subscribed(g)), Vec::new())));
                        let thread = std::thread::Builder::new()
                            .name("computer-frames".into())
                            .spawn(move || stream_loop(sv, st, o, st2))
                            .ok();
                        sub = Some(Sub { stop, thread });
                        if sent.is_err() {
                            break;
                        }
                        continue;
                    }
                    Err(f) => Err(f),
                }
            }
            Op::FramesStop => {
                let last_seq = sub.take().map(Sub::end).unwrap_or(0);
                Ok((Some(Data::Stopped { last_seq }), Vec::new()))
            }
            op => dispatch(&server, op),
        };
        if out.reply(call.id, r).is_err() {
            break;
        }
    }
    if let Some(s) = sub.take() {
        s.end();
    }
}

/// One call that is not about the frame stream.
pub fn dispatch<S: Screen>(server: &Server<S>, op: Op) -> Result<(Option<Data>, Vec<u8>), Failure> {
    match op {
        Op::Status { target } => Ok((Some(Data::Status(server.status(target))), Vec::new())),
        Op::Screenshot {
            target,
            max,
            cursor,
        } => {
            let (g, png) = server.screenshot(target, max, cursor)?;
            Ok((Some(Data::Shot(g)), png))
        }
        Op::Input { target, action } => {
            let done = server.input(target, &action)?;
            Ok((Some(Data::Input(done)), Vec::new()))
        }
        Op::Ax {
            target,
            depth,
            max_nodes,
        } => {
            let (geometry, elements, cut) = server.ax(target, depth, max_nodes)?;
            let json = serde_json::to_vec(&elements)
                .map_err(|e| Failure::new(Code::Internal, e.to_string()))?;
            Ok((
                Some(Data::Ax {
                    geometry,
                    count: elements.len() as u32,
                    cut,
                }),
                json,
            ))
        }
        Op::AxPress {
            target,
            element,
            role,
        } => {
            let (role, label) = server.press(target, &element, role.as_deref())?;
            Ok((Some(Data::Pressed { role, label }), Vec::new()))
        }
        Op::FramesSubscribe { .. } | Op::FramesStop => Err(Failure::new(
            Code::BadRequest,
            text::cu_call_unknown("frames"),
        )),
    }
}

/// One line of at most [`cu::LINE_MAX`] bytes, None at the end.
fn read_line(r: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut buf = Vec::new();
    let n = std::io::Read::take(&mut *r, cu::LINE_MAX as u64 + 1)
        .read_until(b'\n', &mut buf)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        if buf.len() > cu::LINE_MAX {
            return Err(text::cu_line_too_long(cu::LINE_MAX));
        }
        // The last line without a newline: the peer closed after it.
    } else {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|e| e.to_string())
}

/// The scene of the main loop's state: the app, its pids and the windows
/// that the host shows.
pub fn scene_of(
    app: Option<&String>,
    follow: &crate::guest::windows::Follow,
    windows: &[Window],
) -> Scene {
    Scene {
        app: app.cloned(),
        root: follow.root,
        pids: if follow.app_alive() {
            follow.app_pids()
        } else {
            BTreeSet::new()
        },
        windows: windows.to_vec(),
    }
}

#[cfg(test)]
#[path = "computer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "computer_menu_tests.rs"]
mod menu_tests;
