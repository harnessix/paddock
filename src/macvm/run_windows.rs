//! The windows of the session in the shim `paddock-vm app`, macOS only:
//! the guest's `windows` and `focus` events go through `order::apply`
//! (or `order::rebuild` after a new `hello`), the changes go to
//! `windows.rs`, the host's own events (key, minimize, Space, the Dock,
//! the input source and the Globe key, which `run_input.rs` handles)
//! become requests to the guest, a refusal sends the window back and
//! sets the badge, each host window goes to its guest frame at its own
//! offset (`place.rs`), and the `window` and `bench` lines of the control
//! socket drive the Mac checks (the `window menu` lines are in
//! `run_menu.rs`, the `window ax` lines in `run_ax.rs`). `run.rs` holds
//! the session.

use super::crop::Display;
use super::dock::{AppDelegate, DockAction};
use super::order::{Change, Reason, Windows};
use super::proto::{Request, WindowsEvent};
use super::run::{Event, Session, Why};
use super::run_input::InputShell;
use super::run_vm::{Bench, Vm};
use super::text;
use super::windows::{HostEvent, HostSink, HostWindows};
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

/// The host side of the windows: the AppKit windows, the fold, the Dock.
pub(super) struct Shell {
    mtm: MainThreadMarker,
    pub(super) windows: HostWindows,
    pub(super) order: Windows,
    pub(super) dock: Retained<AppDelegate>,
    /// The next `windows` event is the full list after a `hello`.
    expect_full: bool,
    /// When each `window.move` of the control socket was sent, for the
    /// latency line of the event log.
    move_sent: BTreeMap<u32, Instant>,
    /// The `bench ax` connection, while one listens.
    ax_bench: Option<Sender<String>>,
    /// `bench axt`: each line starts with the host's epoch milliseconds.
    ax_stamped: bool,
    /// The last `app.launch`, for `Open <App> again`.
    pub(super) last_launch: Option<Request>,
    badge: Option<String>,
    /// The badge of a refusal, until a window event without one.
    refusal: Option<String>,
    /// The input sources (`run_input.rs`).
    pub(super) input: InputShell,
    /// The menu bar replica (`run_menu.rs`).
    pub(super) menu: super::run_menu::MenuShell,
    /// The Accessibility mirror (`run_ax.rs`).
    pub(super) ax: super::run_ax::AxShell,
    /// The host-led drags and resizes (`run_lead.rs`).
    pub(super) lead: super::lead::Leads,
    /// The offset of each host window (`place.rs`).
    pub(super) places: super::place::Places,
    /// The timings of each resize that runs (`trace.rs`).
    pub(super) traces: BTreeMap<u32, super::trace::Trace>,
    /// The `window.move` requests sent for each window, for `window
    /// frames`.
    pub(super) moves_sent: BTreeMap<u32, u32>,
    /// The guest layout that keeps the windows apart (`apart.rs`).
    pub(super) apart: super::apart::Apart,
}

impl Shell {
    /// `files` gives the drop channel of the windows and the folder of
    /// the file promises.
    pub(super) fn new(
        mtm: MainThreadMarker,
        vm: Arc<Vm>,
        display: super::session::Display,
        tx: Sender<Event>,
        app: &str,
        files: &super::run_files::Files,
        profile: &super::session::Profile,
    ) -> Shell {
        let sink: HostSink = Rc::new(move |ev| {
            let _ = tx.send(Event::Host(ev));
        });
        let windows = HostWindows::new(
            mtm,
            vm,
            Display::from_pixels(display),
            sink.clone(),
            files.drop_tx.clone(),
            super::vz_mac_drop::promise_dir(&profile.drop_dir()),
        );
        let order = Windows::with_screens(windows.screens().clone());
        let dock = AppDelegate::install(mtm, sink, app);
        Shell {
            mtm,
            windows,
            order,
            dock,
            expect_full: true,
            move_sent: BTreeMap::new(),
            ax_bench: None,
            ax_stamped: false,
            last_launch: None,
            badge: None,
            refusal: None,
            input: InputShell::new(),
            menu: super::run_menu::MenuShell::default(),
            ax: super::run_ax::AxShell::default(),
            lead: super::lead::Leads::default(),
            places: super::place::Places::default(),
            traces: BTreeMap::new(),
            moves_sent: BTreeMap::new(),
            apart: super::apart::Apart::default(),
        }
    }

    /// The badge the windows show now.
    pub(super) fn badge(&self) -> Option<&str> {
        self.badge.as_deref()
    }
}

impl Session<'_> {
    /// A new `hello`: the corner radius of the guest's macOS, and the
    /// next window event rebuilds the map (an agent restart keeps the
    /// host windows that still match).
    pub(super) fn hello_windows(&mut self, macos: &str) {
        let major: u32 = macos
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
            .unwrap_or(15);
        self.shell.windows.set_macos(major);
        self.shell.lead.clear();
        self.shell.traces.clear();
        self.shell.apart.clear();
        self.shell.expect_full = true;
    }

    /// A `windows` event of the guest, or an injected one of the bench.
    pub(super) fn on_windows(&mut self, ev: WindowsEvent) {
        let arrived = Instant::now();
        // A browser or sign-in window that the app opened may then ask
        // for its own connections.
        for w in &ev.changed {
            self.firewall.saw_window_of(&w.app);
        }
        let changes = if self.shell.expect_full {
            self.shell.expect_full = false;
            let before = self.shell.order.len();
            let changes = self.shell.order.rebuild(&ev);
            self.events.record(
                "windows",
                "rebuilt after hello",
                &[
                    ("before", before as f64),
                    ("after", self.shell.order.len() as f64),
                    ("changes", changes.len() as f64),
                ],
            );
            changes
        } else {
            self.shell.order.apply(&ev)
        };
        self.apply_changes(changes);
        if self.plan.bench {
            // `bench move`: the shim's own clock from the arrival of the
            // event to the return of every frame set.
            let ms = arrived.elapsed().as_secs_f64() * 1000.0;
            self.events
                .record("bench", "windows applied", &[("ms", ms)]);
        }
    }

    /// The guest's `focus` event.
    pub(super) fn on_focus(&mut self, window: Option<u32>) {
        if let Some(c) = self.shell.order.on_guest_focus(window) {
            self.apply_changes(vec![c]);
        }
    }

    /// Run the changes: the refusals go back to the guest with a badge,
    /// the rest to AppKit, then the Dock menu learns the windows.
    fn apply_changes(&mut self, changes: Vec<Change>) {
        let changes = self.lead_filter(changes);
        self.apart_filter(&changes);
        let mut refused = false;
        for c in &changes {
            log::debug!("macvm: window change {}", c.kind());
            match c {
                Change::Refuse { id, reason, back } => {
                    if let Some(frame) = back {
                        self.send(
                            Request::WindowMove {
                                id: *id,
                                frame: *frame,
                            },
                            Why::Other,
                        );
                    }
                    let badge = match reason {
                        Reason::FullScreen => text::full_screen_refused(),
                        Reason::TooLarge(level) => text::size_refused(*level),
                    };
                    self.events
                        .record("window", &c.kind(), &[("id", *id as f64)]);
                    refused = true;
                    self.shell.refusal = Some(badge.clone());
                    self.set_badge(Some(badge));
                }
                Change::Open(w) => {
                    self.events
                        .record("window", "opened", &[("id", w.id as f64)]);
                }
                Change::Close(id) => {
                    self.events
                        .record("window", "closed", &[("id", *id as f64)]);
                    self.ax_window_closed(*id);
                }
                Change::Move(id, _) => {
                    if let Some(sent) = self.shell.move_sent.remove(id) {
                        self.events.record(
                            "window",
                            "moved after window.move",
                            &[
                                ("id", *id as f64),
                                ("ms", sent.elapsed().as_secs_f64() * 1000.0),
                            ],
                        );
                    }
                }
                _ => {}
            }
        }
        if !refused && !changes.is_empty() {
            // The guest followed the move back: the refusal badge goes.
            if let Some(r) = self.shell.refusal.take() {
                if self.shell.badge.as_deref() == Some(r.as_str()) {
                    self.set_badge(None);
                }
            }
        }
        let again = self.shell.places.follow(
            &changes,
            &self.shell.order,
            self.shell.windows.screens(),
            self.shell.windows.display(),
        );
        self.shell.windows.apply(&changes, &self.shell.places);
        self.shell.windows.reposition(&again, &self.shell.places);
        self.update_covers();
        let titles = self.shell.order.titles();
        self.shell.dock.update(|s| s.windows = titles);
    }

    /// What the user did to the host windows.
    pub(super) fn on_host(&mut self, ev: HostEvent) {
        log::debug!("macvm: host event {ev:?}");
        let requests = match ev {
            HostEvent::BecameKey(id) => {
                self.pictures_before_raise(id);
                self.note_key();
                self.input_became_key();
                self.ax_became_key(id);
                self.shell.order.on_host_focus(id)
            }
            HostEvent::ResignedKey(_) => {
                self.input_resigned();
                Vec::new()
            }
            HostEvent::InputSourceChanged => {
                self.input_host_changed();
                Vec::new()
            }
            HostEvent::Globe => {
                self.input_globe();
                Vec::new()
            }
            HostEvent::Miniaturized(id) => self.shell.order.on_host_minimize(id, true),
            HostEvent::Deminiaturized(id) => self.shell.order.on_host_minimize(id, false),
            HostEvent::SpaceChanged => {
                let active = self.shell.windows.active_space_ids();
                let requests = self.shell.order.on_space_change(&active);
                for r in &requests {
                    if let Request::WindowRaise { id } = r {
                        self.events.record(
                            "window",
                            "raised after Space change",
                            &[("id", *id as f64)],
                        );
                    }
                }
                requests
            }
            HostEvent::ScreensChanged => self.on_screens_changed(),
            HostEvent::Quit => {
                if !self.quitting {
                    self.begin_quit("quit");
                }
                Vec::new()
            }
            HostEvent::Dock(a) => {
                self.dock_action(a);
                Vec::new()
            }
            HostEvent::MenuPress(path) => {
                self.menu_press(path);
                Vec::new()
            }
            HostEvent::AxWanted(id) => {
                self.ax_wanted(id);
                Vec::new()
            }
            HostEvent::AxPress(id) => {
                self.ax_press(id);
                Vec::new()
            }
            HostEvent::AxSet(id, value) => {
                self.ax_set(id, value);
                Vec::new()
            }
            HostEvent::LeadMoved(step) => {
                self.lead_moved(step);
                Vec::new()
            }
            HostEvent::HeldClick(id) => {
                self.held_click(id);
                Vec::new()
            }
            HostEvent::FullScreenDone(id, entered) => {
                self.full_screen_done(id, entered);
                Vec::new()
            }
        };
        for r in requests {
            self.send(r, Why::Other);
        }
    }

    /// An entry of the Dock menu or the app menu.
    fn dock_action(&mut self, a: DockAction) {
        let app = NSApplication::sharedApplication(self.shell.mtm);
        match a {
            DockAction::About => app.orderFrontStandardAboutPanel(None),
            DockAction::Hide => app.hide(None),
            // There is no firewall panel yet, so the entry is disabled.
            DockAction::FirewallRules => {}
            // A running session has nothing to resume (`run_suspend.rs`).
            DockAction::Resume => {}
            DockAction::OpenAgain => {
                if !self.relaunch_allowed {
                    self.events
                        .record("app", "open again refused: three crashes in a minute", &[]);
                    return;
                }
                if let Some(req) = self.shell.last_launch.clone() {
                    self.events.record("app", "open again from the Dock", &[]);
                    self.send(req, Why::Other);
                    self.shell.dock.update(|s| s.app_exited = false);
                }
            }
            DockAction::RestartAgent => {
                self.events.record("agent", "restart from the Dock", &[]);
                if self.send(Request::AgentRestart, Why::Other) {
                    self.hello_deadline = Some(Instant::now() + super::proto::HELLO_WARM);
                }
            }
            DockAction::RestartGuest => {
                // The same steps as the 120 s stage of the health gap.
                let steps = super::failure::respond(
                    &super::failure::Failure::GuestHang(super::failure::Stage::Restart),
                    self.watch.ctx(),
                );
                self.execute(steps);
            }
            DockAction::Window(id) => {
                self.shell.windows.raise(id);
                for r in self.shell.order.on_host_focus(id) {
                    self.send(r, Why::Other);
                }
            }
        }
    }

    pub(super) fn app_exited_dock(&mut self) {
        let can = self.shell.last_launch.is_some() && self.relaunch_allowed;
        self.shell.dock.update(|s| s.app_exited = can);
    }

    /// The quit path and a guest restart: every host window goes.
    pub(super) fn close_windows(&mut self) {
        self.ax_app_exited();
        self.shell.windows.close_all();
        self.shell.lead.clear();
        self.shell.traces.clear();
        self.shell.places.clear();
        self.shell.apart.clear();
        self.shell.order = Windows::with_screens(self.shell.windows.screens().clone());
        self.shell.dock.update(|s| s.windows.clear());
    }

    /// A change of the host screen set: the host windows that are off
    /// the new set move onto the main screen. Only their offsets change:
    /// the guest windows stay where they are. The badge says that a
    /// restart gives the full size.
    fn on_screens_changed(&mut self) -> Vec<Request> {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return Vec::new();
        };
        let new = super::windows::screens(mtm);
        let old = self.shell.windows.screens().clone();
        let display = self.shell.windows.display();
        let shown: Vec<super::screens::Shown> = self
            .shell
            .order
            .ids()
            .iter()
            .filter_map(|id| {
                let e = self.shell.order.get(*id)?;
                Some(super::screens::Shown {
                    id: *id,
                    frame: self.shell.places.host(*id, e.frame, display),
                    level: e.level,
                    child: e.parent.is_some() || self.shell.places.is_child(*id),
                    minimized: e.minimized,
                })
            })
            .collect();
        let r = super::screens::rescue(&shown, &old, &new, display);
        let line = text::screens_changed(new.screens.len(), r.moves.len());
        self.events
            .record("screens", &line, &[("moved", r.moves.len() as f64)]);
        let mut moved = Vec::new();
        for (id, host) in &r.moves {
            let Some(guest) = self.shell.windows.guest_of(*id) else {
                continue;
            };
            self.shell
                .places
                .set(*id, super::place::Offset::between(*host, guest));
            moved.push(*id);
            moved.extend(self.shell.places.anchored(*id));
        }
        self.shell.windows.reposition(&moved, &self.shell.places);
        self.update_covers();
        if r.badge {
            self.set_badge(Some(text::display_changed()));
        }
        Vec::new()
    }

    /// The badge text of the session's windows, into the record of the
    /// parent so the marker shows it, and into the event log once per
    /// change.
    pub(super) fn set_badge(&mut self, badge: Option<String>) {
        if self.shell.badge == badge {
            return;
        }
        self.shell.badge = badge.clone();
        self.events
            .record("badge", badge.as_deref().unwrap_or(""), &[]);
        let Some(path) = &self.plan.record else {
            return;
        };
        let Ok(bytes) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&bytes) else {
            return;
        };
        match badge {
            Some(b) => v["badge"] = serde_json::json!(b),
            None => {
                if let Some(o) = v.as_object_mut() {
                    o.remove("badge");
                }
            }
        }
        let _ = crate::privfs::write_atomic(path, v.to_string());
    }

    /// An `ax.value` event, to the `bench ax` connection while one
    /// listens. The text never goes to a log.
    pub(super) fn on_ax_value(&mut self, window: u32, value: String) {
        let Some(tx) = &self.shell.ax_bench else {
            return;
        };
        let line = if self.shell.ax_stamped {
            format!("{} {window} {value}", epoch_ms()) // not user text
        } else {
            format!("{window} {value}") // not user text
        };
        if tx.send(line).is_err() {
            self.shell.ax_bench = None;
            self.send(Request::AxWatch { on: false }, Why::Other);
        }
    }

    /// A `window` line of the control socket: `move ID X Y W H`, `raise
    /// ID` or `list`. The answer is one line for the script.
    pub(super) fn on_window_request(&mut self, args: &str) -> Result<String, String> {
        let words: Vec<&str> = args.split(' ').collect();
        if let Some(answer) = self.menu_control(&words) {
            return answer;
        }
        if let Some(answer) = self.ax_control(&words) {
            return answer;
        }
        if let Some(answer) = self.lead_control(&words) {
            return answer;
        }
        if let Some(answer) = self.apart_control(&words) {
            return answer;
        }
        match words.as_slice() {
            ["move", id, x, y, w, h] => {
                let id: u32 = id
                    .parse()
                    .map_err(|_| text::control_request_unknown(args))?;
                let nums: Result<Vec<f64>, _> = [x, y, w, h].iter().map(|s| s.parse()).collect();
                let n = nums.map_err(|_| text::control_request_unknown(args))?;
                let frame = super::proto::Frame::new(n[0], n[1], n[2], n[3]);
                self.shell.move_sent.insert(id, Instant::now());
                self.events
                    .record("window", "window.move sent", &[("id", id as f64)]);
                if self.send(Request::WindowMove { id, frame }, Why::Other) {
                    Ok("sent".into())
                } else {
                    Err(text::no_agent_for_request())
                }
            }
            ["raise", id] => {
                let id: u32 = id
                    .parse()
                    .map_err(|_| text::control_request_unknown(args))?;
                self.shell.windows.raise(id);
                self.send(Request::WindowRaise { id }, Why::Other);
                Ok("raised".into())
            }
            ["state"] => {
                Ok(serde_json::to_string(&self.shell.windows.states()).unwrap_or_default())
            }
            ["list"] => {
                let numbers = self.shell.windows.numbers();
                let key = self.shell.windows.key_id();
                let list: Vec<serde_json::Value> = self
                    .shell
                    .order
                    .ids()
                    .iter()
                    .filter_map(|id| self.shell.order.get(*id).map(|e| (*id, e)))
                    .map(|(id, e)| {
                        // Flat, so a script splits the entries on `{`.
                        serde_json::json!({
                            "id": id,
                            "title": e.title,
                            "level": e.level,
                            "parent": e.parent,
                            "x": e.frame.x,
                            "y": e.frame.y,
                            "w": e.frame.w,
                            "h": e.frame.h,
                            "minimized": e.minimized,
                            "focused": e.focused,
                            "full_screen": e.full_screen,
                            "number": numbers.get(&id),
                            "key": key == Some(id),
                        })
                    })
                    .collect();
                Ok(serde_json::Value::Array(list).to_string())
            }
            // The Dock menu, so the Mac check of Sample Notes reads the entries
            // the user would see.
            ["dock"] => {
                let state = self.shell.dock.state();
                let mut entries: Vec<String> = state
                    .windows
                    .iter()
                    .map(|(_, title)| {
                        if title.is_empty() {
                            state.app.clone()
                        } else {
                            title.clone()
                        }
                    })
                    .collect();
                if state.app_exited {
                    entries.push(text::open_again(&state.app));
                }
                entries.push(text::restart_agent());
                if state.silent {
                    entries.push(text::restart_guest());
                }
                entries.push(text::firewall_rules());
                entries.push(text::quit());
                Ok(serde_json::Value::Array(
                    entries.into_iter().map(serde_json::Value::String).collect(),
                )
                .to_string())
            }
            // Press a Dock entry: `dock-press open-again`.
            ["dock-press", what] => {
                let action = match *what {
                    "open-again" => DockAction::OpenAgain,
                    "restart-agent" => DockAction::RestartAgent,
                    "restart-guest" => DockAction::RestartGuest,
                    _ => return Err(text::control_request_unknown(args)),
                };
                self.dock_action(action);
                Ok("pressed".into())
            }
            _ => Err(text::control_request_unknown(args)),
        }
    }

    /// A `bench` line of the control socket.
    pub(super) fn on_bench(&mut self, b: Bench) {
        match b {
            Bench::Windows(ev, reply) => {
                self.events.record(
                    "bench",
                    "windows event injected",
                    &[("changed", ev.changed.len() as f64)],
                );
                self.on_windows(ev);
                let _ = reply.send(Ok("applied".into()));
            }
            Bench::AxWatch(tx, stamped) => {
                self.shell.ax_bench = Some(tx);
                self.shell.ax_stamped = stamped;
                self.send(Request::AxWatch { on: true }, Why::Other);
            }
            Bench::AxStop => {
                self.shell.ax_bench = None;
                self.send(Request::AxWatch { on: false }, Why::Other);
            }
            Bench::Update(reply) => {
                let _ = reply.send(match self.update_request() {
                    Some(req) => {
                        self.events.record("bench", &req.kind(), &[]);
                        self.hello_deadline = Some(Instant::now() + super::proto::HELLO_WARM);
                        if self.send(req, Why::Other) {
                            Ok("sent".into())
                        } else {
                            Err(text::no_agent_for_request())
                        }
                    }
                    None => Err(text::no_agent_for_request()),
                });
            }
            Bench::Request(req, reply) => {
                self.events.record("bench", &req.kind(), &[]);
                if matches!(req, Request::AgentRestart) {
                    self.hello_deadline = Some(Instant::now() + super::proto::HELLO_WARM);
                }
                let _ = reply.send(if self.send(req, Why::Other) {
                    Ok("sent".into())
                } else {
                    Err(text::no_agent_for_request())
                });
            }
        }
    }
}

/// Milliseconds since the epoch, the clock of the `bench axt` lines and
/// of the key helper of `bench input`.
fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
