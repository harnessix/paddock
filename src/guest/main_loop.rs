//! The main loop of the agent: the connection, the hello, the health timer,
//! the window poll, the request dispatch with the `req` ids, and the
//! reconnect.
//!
//! Two threads: the socket thread reads the control line and answers
//! `ping` at once, so a hung Accessibility call never blocks the answer;
//! the agent thread (the caller of [`run`]) serves the run loop, polls the
//! windows and runs every other request. The configure steps run in a
//! thread of their own and reply when they end. The clipboard is in
//! `main_loop_clipboard.rs`, the menu bar in `main_loop_menu.rs`, the
//! Accessibility mirror in `main_loop_ax.rs`.

use super::apps::{Relaunch, CRASH_WAIT};
use super::configure;
use super::pace::MovePace;
use super::platform::{monotonic_ms, Platform, Waker, WindowOp};
use super::windows::{self, Follow, Snapshot, SYSTEM_APP};
use crate::macvm::proto::{
    write_line, Call, ErrorCode, Event, Health, Hello, Reply, ReplyData, Request, DISK_LOW,
    DISK_WARN, HEALTH_INTERVAL, RECONNECT, TOKEN_WAIT, VERSION, WINDOW_POLL,
};
use crate::macvm::text;
use std::collections::BTreeSet;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// What the socket thread and the worker threads pass to the agent thread.
enum Msg {
    Call(Call),
    /// A reply that a worker thread finished.
    Done(Reply),
    /// The reply of the `data.prepare` thread (`main_loop_data.rs`).
    Prepared(Reply),
    Closed(Option<String>),
}

/// How a connection ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// The connection is gone: dial again.
    Lost(String),
    /// The agent must exit with this code (an update, a restart).
    Exit(i32),
}

/// The writer of the control line, shared by the threads.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<UnixStream>>);

impl Writer {
    fn send(&self, ev: &Event) -> Result<(), String> {
        log::debug!("-> {}", ev.kind());
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        write_line(&mut *s, ev).map_err(|e| text::host_write_failed(&format!("{e:#}")))
    }
}

/// A quit the host asked for, answered when the app is gone.
struct PendingQuit {
    req: u64,
    pid: u32,
    deadline: Instant,
}

/// An app that went away: the crash report may still appear.
struct ExitWatch {
    pid: u32,
    app: String,
    since: SystemTime,
    started: Instant,
    /// The host asked for this exit: no crash wait, no relaunch count.
    requested: bool,
}

/// The state that outlives one connection.
#[derive(Default)]
pub struct State {
    pub follow: Follow,
    pub snapshot: Snapshot,
    pub relaunch: Relaunch,
    pub ax_watch: bool,
    /// The session started with `--bench`: the bench ops are honored.
    pub bench: bool,
    /// The host says a session window is key: poll the pasteboard.
    pub clipboard_watch: bool,
    /// The pasteboard change count that the host has, or that the agent
    /// wrote itself.
    pub clip_seen: Option<u64>,
    /// The corner radius measured on each window (`chrome.rs`).
    pub radii: super::chrome::Radii,
    /// The Downloads link to the downloads share is checked and right.
    pub downloads_linked: bool,
    /// An `app.copy` thread runs: a second request gets `Busy`.
    copy_running: bool,
    pub token: Option<String>,
    /// The bundle id of the app, or its file name until a window says.
    pub app: Option<String>,
    observed: BTreeSet<u32>,
    pending_quit: Option<PendingQuit>,
    exit: Option<ExitWatch>,
    disk_band: Option<u8>,
    data_verify: Option<String>,
    last_focus: Option<(String, Option<u32>)>,
    /// The last menu tree sent, so an unchanged one is not sent again,
    /// and whether the next poll reads it (`main_loop_menu.rs`).
    menu_last: Option<Vec<crate::macvm::proto::MenuItem>>,
    menu_dirty: bool,
    /// The watched window of the mirror (`main_loop_ax.rs`).
    ax_tree: ax_tree::AxTreeState,
    /// The disk dialogs tried (`main_loop_dialog.rs`).
    dialogs: super::dialogs::Tracker,
    /// The running `data.prepare`, and the shutdown that waits for its end
    /// (`main_loop_data.rs`).
    prepare: Option<data::PrepareRun>,
    shutdown_after_prepare: Option<(u64, String)>,
    /// The screen lock state the host has (`main_loop_screen.rs`).
    screen_locked: Option<bool>,
}

/// Dial, talk, dial again every [`RECONNECT`], forever, until a request
/// makes the agent exit. Returns the exit code.
pub fn run(p: &mut impl Platform, mut dial: impl FnMut() -> std::io::Result<UnixStream>) -> i32 {
    let mut st = State::default();
    load_app(p, &mut st);
    loop {
        match dial() {
            Ok(stream) => match Session::new(p, &mut st, stream).run() {
                End::Exit(code) => return code,
                End::Lost(why) => log::warn!("the control connection ended: {why}"),
            },
            Err(e) => log::info!("cannot dial the host: {e}"),
        }
        p.wait(RECONNECT);
    }
}

/// One connection.
struct Session<'a, P: Platform> {
    p: &'a mut P,
    st: &'a mut State,
    stream: UnixStream,
    writer: Writer,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    waker: Waker,
    next_poll: Instant,
    /// The poll around the moves of a live resize (`pace.rs`).
    pace: MovePace,
    /// The first poll after a hello sends every window.
    first: bool,
}

impl<'a, P: Platform> Session<'a, P> {
    fn new(p: &'a mut P, st: &'a mut State, stream: UnixStream) -> Self {
        let (tx, rx) = mpsc::channel();
        let writer = Writer(Arc::new(Mutex::new(
            stream.try_clone().expect("a second handle of the socket"),
        )));
        let waker = p.waker();
        link::set_live(Some(writer.clone()));
        Session {
            p,
            st,
            stream,
            writer,
            tx,
            rx,
            waker,
            next_poll: Instant::now(),
            pace: MovePace::default(),
            first: true,
        }
    }

    fn run(mut self) -> End {
        let end = self.talk();
        link::set_live(None);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        let _ = self
            .writer
            .0
            .lock()
            .map(|s| s.shutdown(std::net::Shutdown::Both));
        end
    }

    fn talk(&mut self) -> End {
        let reader = match self.stream.try_clone() {
            Ok(s) => s,
            Err(e) => return End::Lost(text::socket_clone_failed(&e.to_string())),
        };
        let (tx, writer, waker) = (self.tx.clone(), self.writer.clone(), self.waker.clone());
        std::thread::Builder::new()
            .name("control-reader".into())
            .spawn(move || read_loop(reader, tx, writer, waker))
            .expect("a thread");
        if let Err(e) = self.hello() {
            return End::Lost(e);
        }
        if let Some(text) = self.p.data_unmountable() {
            if let Err(e) = self.writer.send(&Event::DataUnmountable { text }) {
                return End::Lost(e);
            }
        }
        if let Some(v) = self.p.data_check() {
            self.st.data_verify = Some(v);
        }
        // The first window event after a hello carries every window, and
        // a locked screen is named again.
        self.st.snapshot = Snapshot::default();
        self.st.last_focus = None;
        self.st.screen_locked = None;
        // The first poll before the first request: a disk dialog of the
        // boot is pressed and named before `data.prepare` changes the disk.
        if let Err(e) = self.poll() {
            return End::Lost(e);
        }
        self.next_poll = Instant::now() + WINDOW_POLL;
        let mut next_health = Instant::now();
        loop {
            let now = Instant::now();
            let mut deadline = self.next_poll.min(next_health);
            if let Some(q) = &self.st.pending_quit {
                deadline = deadline.min(q.deadline);
            }
            if deadline > now {
                self.p.wait(deadline - now);
            }
            loop {
                match self.rx.try_recv() {
                    Ok(Msg::Call(call)) => match self.dispatch(call) {
                        Ok(None) => {}
                        Ok(Some(end)) => return end,
                        Err(e) => return End::Lost(e),
                    },
                    Ok(Msg::Done(reply)) => {
                        // A worker thread ended: the copy slot is free.
                        self.st.copy_running = false;
                        if let Err(e) = self.reply(reply) {
                            return End::Lost(e);
                        }
                    }
                    Ok(Msg::Prepared(reply)) => {
                        if let Err(e) = self.prepared(reply) {
                            return End::Lost(e);
                        }
                    }
                    Ok(Msg::Closed(why)) => {
                        return End::Lost(why.unwrap_or_else(|| "the host closed it".into()))
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return End::Lost("the reader ended".into()),
                }
            }
            let now = Instant::now();
            if self.p.take_dirty() {
                self.next_poll = self.pace.dirty(now, self.next_poll);
            }
            if now >= self.next_poll {
                if let Err(e) = self.poll() {
                    return End::Lost(e);
                }
                self.next_poll = Instant::now() + WINDOW_POLL;
            }
            if now >= next_health {
                if let Err(e) = self.health() {
                    return End::Lost(e);
                }
                next_health = now + HEALTH_INTERVAL;
            }
        }
    }

    /// Send `hello`, wait for `token`.
    fn hello(&mut self) -> Result<(), String> {
        let info = self.p.hello();
        let hello = Hello {
            v: VERSION,
            agent: info.agent,
            macos: info.macos,
            build: info.build,
            scale: info.scale,
            display: info.display,
            can_update: info.can_update,
            sha256: super::update::own_sha256(),
        };
        self.writer.send(&Event::Hello(hello))?;
        match self.rx.recv_timeout(TOKEN_WAIT) {
            Ok(Msg::Call(Call {
                req,
                request:
                    Request::Token {
                        token,
                        v,
                        bench,
                        now_ms: _,
                    },
            })) => {
                log::info!("req {req} token from host v{v} bench {bench}");
                super::proxy::set_token(&token);
                self.st.token = Some(token);
                self.st.bench = bench;
                self.reply(Reply::ok(req))
            }
            Ok(Msg::Call(call)) => {
                self.reply(Reply::error(
                    call.req,
                    ErrorCode::BadRequest,
                    "the first request must be token",
                ))?;
                Err(text::no_token_first(&call.request.kind()))
            }
            Ok(Msg::Done(_) | Msg::Prepared(_)) => {
                Err("a stale worker reply before the token".into())
            }
            Ok(Msg::Closed(why)) => Err(why.unwrap_or_else(|| "closed before the token".into())),
            Err(_) => Err(text::no_token_timeout(TOKEN_WAIT)),
        }
    }

    fn reply(&mut self, r: Reply) -> Result<(), String> {
        self.writer.send(&Event::Reply(r))
    }

    /// Read the window list, the process table and the front app, and
    /// send what changed: windows, focus, app exits, the quit reply.
    fn poll(&mut self) -> Result<(), String> {
        let raw = self.p.raw_windows();
        self.dialogs(&raw)?;
        self.st.follow.procs = self.p.procs();
        let front = self.p.front_pid();
        let app_pids = self.st.follow.app_pids();
        self.st.follow.app_focused = front.is_some_and(|f| app_pids.contains(&f));
        if let Some(root) = self.st.follow.root {
            if let Some(w) = raw.iter().find(|w| w.pid == root) {
                if let Some(b) = &w.bundle {
                    self.st.app = Some(b.clone());
                }
            }
            if !self.st.follow.procs.contains_key(&root) {
                let requested = self.st.pending_quit.as_ref().is_some_and(|q| q.pid == root);
                self.st.exit = Some(ExitWatch {
                    pid: root,
                    app: self.st.app.clone().unwrap_or_default(),
                    since: SystemTime::now() - Duration::from_secs(2),
                    started: Instant::now(),
                    requested,
                });
                self.st.follow.root = None;
                save_app(self.p, self.st);
            }
        }
        let key = self.st.snapshot.key();
        let followed = windows::followed(&raw, &self.st.follow);
        if followed != self.st.observed {
            self.p.observe(&followed);
            self.st.observed = followed;
        }
        let p = &mut *self.p;
        let mut snap = Snapshot::build(&raw, &self.st.follow, key, |w| p.ax_info(w));
        let radii = &mut self.st.radii;
        super::chrome::measure(radii, &mut snap.windows, &raw, p, Instant::now());
        if self.first {
            self.first = false;
            self.writer.send(&Event::Windows(windows::full(&snap)))?;
        } else if let Some(ev) = windows::diff(&self.st.snapshot, &snap) {
            self.writer.send(&Event::Windows(ev))?;
        }
        self.st.snapshot = snap;
        // Computer use reads the same windows from its own threads.
        super::computer::publish(super::computer::scene_of(
            self.st.app.as_ref(),
            &self.st.follow,
            &self.st.snapshot.windows,
        ));
        // The windows go first, the exit after them.
        self.exits()?;
        let app = match front {
            Some(f) if app_pids.contains(&f) => self.st.app.clone().unwrap_or_default(),
            Some(f) => raw
                .iter()
                .find(|w| w.pid == f)
                .map(|w| {
                    if windows::SYSTEM_PROMPTS.contains(&w.owner.as_str()) {
                        SYSTEM_APP.to_string()
                    } else {
                        w.bundle.clone().unwrap_or_else(|| w.owner.clone())
                    }
                })
                .unwrap_or_else(|| "other".into()),
            None => "none".into(),
        };
        let focus = (app, self.st.snapshot.key());
        if self.st.last_focus.as_ref() != Some(&focus) {
            self.writer.send(&Event::Focus {
                app: focus.0.clone(),
                window: focus.1,
            })?;
            self.st.last_focus = Some(focus);
            self.st.menu_dirty = true;
        }
        self.screen_poll()?;
        self.menu_poll()?;
        self.ax_tree_poll()?;
        if self.st.ax_watch {
            if let Some((window, text)) = self.p.ax_value() {
                self.writer.send(&Event::AxValue { window, text })?;
            }
        }
        self.clipboard_poll()
    }
    /// The app that went away: its event once the crash report had its
    /// time, and the reply to a pending quit.
    fn exits(&mut self) -> Result<(), String> {
        if let Some(q) = &self.st.pending_quit {
            if !self.st.follow.procs.contains_key(&q.pid) {
                let req = q.req;
                self.st.pending_quit = None;
                self.reply(Reply::ok(req))?;
            } else if Instant::now() >= q.deadline {
                let req = q.req;
                self.st.pending_quit = None;
                self.reply(Reply::error(
                    req,
                    ErrorCode::Timeout,
                    "the app did not end in time",
                ))?;
            }
        }
        let Some(w) = &self.st.exit else {
            return Ok(());
        };
        let report = if w.requested {
            None
        } else {
            self.p.crash_report(w.pid, w.since)
        };
        if !w.requested && report.is_none() && w.started.elapsed() < CRASH_WAIT {
            return Ok(());
        }
        let w = self.st.exit.take().expect("the exit watch");
        let relaunch = if w.requested {
            self.st.relaunch.reset();
            true
        } else {
            self.st.relaunch.record(Instant::now())
        };
        let code = if report.is_some() { 1 } else { 0 };
        log::info!(
            "the app {} (pid {}) exited{}",
            w.app,
            w.pid,
            if w.requested { " on request" } else { "" }
        );
        self.writer.send(&Event::AppExited {
            app: w.app,
            code,
            crash_report: report,
            relaunch,
        })
    }

    fn health(&mut self) -> Result<(), String> {
        if !self.st.downloads_linked && self.p.downloads_link() {
            self.st.downloads_linked = true;
        }
        let h = self.p.health();
        if let Some(free) = h.data_disk_free {
            let band = if free < DISK_LOW {
                2
            } else if free < DISK_WARN {
                1
            } else {
                0
            };
            if self.st.disk_band != Some(band) {
                if self.st.disk_band.is_some() || band > 0 {
                    self.writer.send(&Event::Disk {
                        data_free: free,
                        warn: band > 0,
                    })?;
                }
                self.st.disk_band = Some(band);
            }
        }
        self.writer.send(&Event::Health(Health {
            uptime: h.uptime,
            load: h.load,
            mem_free: h.mem_free,
            data_disk_free: h.data_disk_free,
            app_running: self.st.follow.app_alive(),
            input_source: h.input_source,
            data_verify: self.st.data_verify.clone(),
            accessibility: h.accessibility,
            locked: self.p.screen_locked(),
        }))
    }

    /// One request. Some(End) when the agent must exit.
    fn dispatch(&mut self, call: Call) -> Result<Option<End>, String> {
        log::info!("<- {}", call.kind());
        let req = call.req;
        let timeout = crate::macvm::proto::timeout(&call.request);
        let busy = self.st.follow.app_alive();
        if let Some(r) = crate::macvm::bench::op_gate(req, &call.request, self.st.bench) {
            return self.reply(r).map(|_| None);
        }
        match self.prepare_gate(req, &call.request) {
            data::Gate::Go => {}
            data::Gate::Reply(r) => return self.reply(r).map(|_| None),
            data::Gate::Later => return Ok(None),
        }
        let reply = match call.request {
            Request::Token { .. } => {
                Reply::error(req, ErrorCode::BadRequest, "the token was already given")
            }
            Request::ClockSet { unix_ms, password } => {
                if busy {
                    Reply::error(req, ErrorCode::NotInSession, "an app runs in the guest")
                } else {
                    match super::clock::follow_host(unix_ms, &password, super::configure::run_plan)
                    {
                        Ok(()) => Reply::ok(req),
                        Err(e) => Reply::error(req, ErrorCode::Io, e),
                    }
                }
            }
            Request::WindowMove { id, frame } => self.window(req, id, WindowOp::Move(frame)),
            Request::WindowRaise { id } => self.window(req, id, WindowOp::Raise),
            Request::WindowMinimize { id } => self.window(req, id, WindowOp::Minimize),
            Request::WindowClose { id } => self.window(req, id, WindowOp::Close),
            Request::WindowFocus { id } => self.window(req, id, WindowOp::Focus),
            Request::AppLaunch { path, args, env } => {
                if busy {
                    Reply::error(req, ErrorCode::Busy, "an app already runs in the guest")
                } else {
                    match self.p.launch(&path, &args, &env) {
                        Ok(pid) => {
                            self.st.follow.root = Some(pid);
                            // The table of the last poll lacks the new pid. Without
                            // it, `app_alive` is false until the next poll, and a
                            // password request queued behind this one passes the guard.
                            self.st.follow.procs.entry(pid).or_insert(0);
                            self.st.exit = None;
                            self.st.app = Some(
                                path.file_stem()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            );
                            self.next_poll = Instant::now();
                            save_app(self.p, self.st);
                            Reply::data(req, ReplyData::Launched { pid })
                        }
                        Err((code, text)) => Reply::error(req, code, text),
                    }
                }
            }
            Request::AppQuit { app, force } => match self.st.follow.root {
                None => Reply::error(req, ErrorCode::NoSuchApp, text::app_not_running(&app)),
                Some(pid) => match self.p.quit(pid, force) {
                    Ok(()) => {
                        self.st.pending_quit = Some(PendingQuit {
                            req,
                            pid,
                            deadline: Instant::now()
                                + timeout.saturating_sub(Duration::from_secs(1)),
                        });
                        self.next_poll = Instant::now();
                        return Ok(None);
                    }
                    Err((code, text)) => Reply::error(req, code, text),
                },
            },
            Request::ClipboardSet {
                items, transfer, ..
            } => self.clipboard_set(req, items, transfer),
            Request::ClipboardWatch { on } => {
                self.st.clipboard_watch = on;
                if on {
                    // What is on the pasteboard now stays in the guest.
                    self.st.clip_seen = Some(self.p.clipboard_count());
                }
                Reply::ok(req)
            }
            Request::Drop { paths, .. } => {
                match self.p.open_with_app(&paths, self.st.follow.root) {
                    Ok(count) => {
                        self.st.clip_seen = Some(count);
                        Reply::ok(req)
                    }
                    Err((code, text)) => Reply::error(req, code, text),
                }
            }
            Request::AppCopy {
                from,
                to,
                unquarantine,
                ..
            } => match self.app_copy(req, &from, &to, unquarantine, busy) {
                Some(reply) => reply,
                // The copy thread answers.
                None => return Ok(None),
            },
            Request::DisplaySet { .. } => Reply::error(
                req,
                ErrorCode::Unsupported,
                "this agent version does not implement the request",
            ),
            Request::Menu { .. } => self.menu_request(req)?,
            Request::MenuPress { path } => self.menu_press(req, &path),
            Request::DataPrepare { password, data_id } => {
                if busy {
                    Reply::error(req, ErrorCode::NotInSession, "an app runs in the guest")
                } else {
                    self.data_prepare(req, &password, &data_id);
                    return Ok(None);
                }
            }
            Request::Configure { password, steps } => {
                if busy {
                    Reply::error(req, ErrorCode::NotInSession, "an app runs in the guest")
                } else {
                    let (tx, waker) = (self.tx.clone(), self.waker.clone());
                    let done = configure::agent_steps(&steps, |a| self.p.agent_step(a));
                    std::thread::Builder::new()
                        .name("configure".into())
                        .spawn(move || {
                            let service = configure::network_service();
                            let steps = configure::run_all(
                                &steps,
                                &password,
                                service.as_deref(),
                                done,
                                configure::spawn_step,
                            );
                            let _ = tx
                                .send(Msg::Done(Reply::data(req, ReplyData::Configure { steps })));
                            waker();
                        })
                        .expect("a thread");
                    return Ok(None);
                }
            }
            Request::Ping { .. } => {
                self.writer.send(&Event::Pong { t: monotonic_ms() })?;
                Reply::ok(req)
            }
            Request::AgentUpdate {
                path,
                sha256,
                version,
            } => match self.p.update(&path, &sha256) {
                Ok(()) => {
                    log::info!("updated to {version}: exit 0, launchd restarts the agent");
                    self.reply(Reply::ok(req))?;
                    return Ok(Some(End::Exit(0)));
                }
                Err((code, text)) => Reply::error(req, code, text),
            },
            Request::AgentRestart => {
                self.reply(Reply::ok(req))?;
                return Ok(Some(End::Exit(0)));
            }
            Request::InputSet {
                sources,
                current,
                reduce_motion,
                increase_contrast,
            } => match self
                .p
                .input_set(&sources, &current, reduce_motion, increase_contrast)
            {
                Ok(missing) => Reply::data(req, ReplyData::InputSet { missing }),
                Err((code, text)) => Reply::error(req, code, text),
            },
            Request::LogFetch => match self.st.token.clone() {
                Some(token) => match self.p.send_log(&token) {
                    Ok(()) => Reply::ok(req),
                    Err((code, text)) => Reply::error(req, code, text),
                },
                None => Reply::error(req, ErrorCode::Internal, "no token"),
            },
            Request::DiskLow => {
                self.p.disk_low_alert();
                Reply::ok(req)
            }
            Request::AxWatch { on } => {
                self.st.ax_watch = on;
                Reply::ok(req)
            }
            Request::AxTree { window } => self.ax_tree_request(req, window)?,
            Request::AxPress { window, path, role } => {
                self.ax_tree_press(req, window, &path, &role)
            }
            Request::AxSet {
                window,
                path,
                role,
                value,
            } => self.ax_tree_set(req, window, &path, &role, &value),
            Request::BenchAnimate { window, secs } => self.bench_animate(req, window, secs),
            Request::BenchText { window } => self.bench_text(req, window),
            Request::BenchHang => bench::hang(),
            Request::BenchSpawnChild { argv } => bench::spawn_child(req, &argv),
            Request::BenchPasteboard { action, text } => self.bench_pasteboard(req, &action, &text),
            Request::BenchEnv => bench::env(req),
            Request::BenchDockMenu => bench::dock_menu(req, self.st.app.as_deref()),
            Request::Shutdown { password } => self.shutdown(req, &password),
            Request::SharesOpen => return self.shares_open(req).map(|_| None),
            Request::KeychainUnlock { password } => self.keychain_unlock(req, &password),
            Request::BenchLock => self.bench_lock(req),
            Request::BenchKeychain => self.bench_keychain(req),
            Request::LoginForward { port, ticket } => match self.login_forward(req, port, ticket) {
                Some(r) => r,
                None => return Ok(None),
            },
        };
        self.reply(reply)?;
        Ok(None)
    }

    fn window(&mut self, req: u64, id: u32, op: WindowOp) -> Reply {
        let Some(w) = self.st.snapshot.get(id).cloned() else {
            return Reply::error(req, ErrorCode::NoSuchWindow, text::no_window(id));
        };
        let started = Instant::now();
        let r = self.p.window_op(&w, op);
        let now = Instant::now();
        self.next_poll = match op {
            WindowOp::Move(_) => self.pace.moved(now, self.next_poll),
            _ => now,
        };
        match r {
            Ok(Some(frame)) => {
                let ms = now.saturating_duration_since(started).as_secs_f64() * 1000.0;
                Reply::data(req, ReplyData::Moved { frame, ms })
            }
            Ok(None) => Reply::ok(req),
            Err((code, text)) => Reply::error(req, code, text),
        }
    }
}

#[path = "main_loop_app.rs"]
mod app_memory;
#[path = "main_loop_ax.rs"]
mod ax_tree;
use app_memory::{load_app, save_app};
#[path = "main_loop_bench.rs"]
mod bench;
#[path = "main_loop_clipboard.rs"]
mod clip;
#[path = "main_loop_data.rs"]
mod data;
#[path = "main_loop_dialog.rs"]
mod dialog;
#[path = "main_loop_link.rs"]
mod link;
pub use link::send_link;
#[path = "main_loop_menu.rs"]
mod menu;
#[path = "main_loop_read.rs"]
mod read;
use read::read_loop;
#[path = "main_loop_screen.rs"]
mod screen;
#[path = "main_loop_shares.rs"]
mod shares;

#[cfg(test)]
#[path = "main_loop_guard_tests.rs"]
mod guard_tests;
#[cfg(test)]
#[path = "main_loop_shares_tests.rs"]
mod shares_tests;
#[cfg(test)]
#[path = "main_loop_tests.rs"]
mod tests;
