//! The AppKit process of one app session, `paddock-vm app`. macOS only.
//!
//! The VM runs on the main dispatch queue, and the main thread takes
//! AppKit's events from the queue of the app, which runs the run loop, so
//! the framework, AppKit (the windows of `windows.rs` and the Dock of
//! `dock.rs`) and the events of this process share one loop. Every call on the
//! `VZVirtualMachine` goes through `exec_async` on that queue, and every
//! answer comes back as an [`Event`] on a channel that the loop drains
//! after each run loop pass.
//!
//! The order of a start: the token from its pipe, the profile lock, the
//! slot, `decide`, `prepare`, `begin`, the configuration, the listeners
//! on the guest ports, then `start` or `restore` and `resume`. A refused
//! start spends nothing: no clone before the locks. The quit path is the
//! state machine of `quit.rs`. The guest agent speaks `proto`: the hello
//! gets the token (and the agent update), every request
//! has its id and its timeout, and every reply names its request. The
//! handlers of the guest messages are in `run_agent.rs`, the framework
//! and process side in `run_vm.rs`, the windows in `run_windows.rs`, the
//! shares, the drops and the clipboard in `run_files.rs`, the input
//! sources in `run_input.rs`, the menu bar replica in `run_menu.rs`, the
//! Accessibility mirror in `run_ax.rs`, the quit path in `run_quit.rs`.

use super::events::Events;
use super::failure::{Step, Watch};
use super::lock::{LockError, ProfileLock, Slot, SlotError};
use super::proto::{self, Request};
use super::quit::{Config as QuitConfig, Input, Quit};
use super::run_vm::{self, say, OnQueue, Vm, VmEvent, VmGuest};
use super::run_windows::Shell;
use super::session::{self, Hooks, Now, Profile, Stamp, Start};
use super::text;
use super::vz_mac_agent::{Agent, AgentEvent, Listener};
use super::vz_mac_config;
use super::windows::HostEvent;
use crate::env::HostEnv;
use crate::vm::bridge;
use crate::vm::helper::AppPlan;
use crate::vm::ports;
use anyhow::{bail, Context, Result};
use dispatch2::DispatchQueue;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};
use objc2_virtualization::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use super::host::EXIT_REFUSED;
pub use super::proto::{HELLO_COLD, HELLO_WARM};

/// One run loop pass.
const TICK: f64 = 0.05;
/// The stub network counters go into the record this often.
const NETWORK_RECORD: Duration = Duration::from_secs(10);

/// What the loop handles: the VM callbacks, the process, the agent.
pub(super) enum Event {
    Started(Result<(), String>),
    Restored(Result<(), String>),
    Resumed(Result<(), String>),
    Paused(Result<(), String>),
    Saved(Result<(), String>),
    Stopped(Result<(), String>),
    /// The framework answered a pause (true) or a resume of the freeze.
    Frozen(bool, Result<(), String>),
    Vm(VmEvent),
    Agent(AgentEvent),
    /// What the user did to the host windows.
    Host(HostEvent),
    /// The password typing into a locked guest screen ended.
    Typed(super::guided_ui::Played),
    /// A connection of a login callback port waits: `login.forward`.
    LoginForward(u16, String),
}

/// Why a request was sent, for its reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Why {
    Token,
    DataPrepare,
    AgentUpdate,
    /// A `drop`: the badge on the reply.
    Drop,
    /// A `clipboard.set` with a spill file: removed on the reply.
    ClipboardSpill,
    /// The `app.copy` of the start: the stamp and the launch follow it.
    AppCopy,
    /// The `app.launch` of the start.
    AppLaunch,
    /// An `input.set`: the missing ids of the reply go to the badge.
    InputSet,
    /// A `menu` request: no reply keeps the last tree (`run_menu.rs`).
    Menu,
    /// A `menu.press`: the reply asks for the tree again.
    MenuPress,
    /// An `ax.tree` request: no reply keeps the last tree (`run_ax.rs`).
    AxTree,
    /// An `ax.press` or `ax.set`: the reply asks for the tree again.
    AxAction,
    /// The `shutdown` of the quit: an error reply stops the VM at once.
    Shutdown,
    /// The `app.quit` of the quit: `no_such_app` goes on to the save.
    AppQuit,
    /// The `log.fetch` of the quit: the exit waits a little for the file.
    LogFetch,
    /// The `keychain.unlock` before the app starts: its reply is an event.
    Keychain,
    /// The `window.move` of a host-led drag of this window (`run_lead.rs`).
    Lead(u32),
    Other,
}

/// Run one app session. The exit code: 0 at a clean end, 1 on an error,
/// 2 when the start was refused, 3 after a hard stop.
pub fn run(env: &HostEnv, plan: &AppPlan) -> Result<i32> {
    if !cfg!(target_arch = "aarch64") {
        bail!(text::needs_apple_silicon());
    }
    if !unsafe { VZVirtualMachine::isSupported() } {
        bail!(text::no_virtualization());
    }
    run_vm::ignore_hup();
    let (token, password) = run_vm::read_token(plan.token_fd)?;

    // The locks first, so a refused start spends nothing.
    let lock = match ProfileLock::take(&plan.profile) {
        Ok(l) => l,
        Err(LockError::Held { pid }) => {
            say(&text::profile_running(&plan.app, pid.unwrap_or(0)));
            return Ok(EXIT_REFUSED);
        }
        Err(LockError::Io(e)) => {
            return Err(e).with_context(|| text::cannot_lock(&plan.profile.lock()))
        }
    };
    let holder = super::instance::label(&plan.app, super::instance::of_profile(&plan.profile.dir));
    let slot = match Slot::take(&env.user, &holder, std::process::id()) {
        Ok(s) => s,
        Err(SlotError::Full(full)) => {
            say(&full.to_string());
            return Ok(EXIT_REFUSED);
        }
        Err(SlotError::Io(e)) => return Err(e).with_context(|| text::slot_io("")),
    };

    let golden = super::host::sealed(&plan.golden, &plan.app)?;
    let now = Now {
        golden,
        host_build: run_vm::host_build(),
        vz: run_vm::framework_version(),
        protocol: proto::VERSION,
        paddock: super::version::paddock_version().into(),
        agent: super::version::paddock_version().into(),
        display: plan.display,
    };
    let old = Stamp::read(&plan.profile.stamp()).ok();
    let free = session::free_bytes(&plan.profile.dir);
    let start = match session::decide(&plan.profile, &now, free) {
        Ok(s) => s,
        Err(refusal) => {
            say(&refusal.to_string());
            return Ok(1);
        }
    };
    let events = Events::open(&plan.profile);
    let line = session::line(start, old.as_ref(), &now);
    say(&line);
    events.record(
        "start",
        &format!("{}: {line}", start.word()), // not user text
        &[("free_bytes", free as f64)],
    );
    let clone = |src: &Path, dst: &Path| run_vm::clonefile(src, dst);
    let machine_id = vz_mac_config::new_machine_id;
    let mut backup =
        |profile: &Profile, reason: &str| run_vm::backup_data(profile, reason, &events);
    let format_data = super::data_disk::format_on_mac;
    let mut hooks = Hooks {
        clone: &clone,
        machine_id: &machine_id,
        backup: &mut backup,
        format_data: &format_data,
    };
    let begun = Instant::now();
    session::prepare(&plan.profile, &plan.golden, &now, start, &mut hooks)
        .with_context(|| text::cannot_prepare(&plan.profile.dir))?;
    if start.clones() {
        events.record(
            "prepare",
            "the system part is in place",
            &[("secs", begun.elapsed().as_secs_f64())],
        );
    }
    let stamp = session::begin(&plan.profile, &now, start)?;
    super::logs::rotate(&plan.profile.console_log(), 5, 1 << 20);

    let hw = std::fs::read(&plan.golden.hardware_model)
        .with_context(|| text::cannot_read(&plan.golden.hardware_model))?;
    let id = std::fs::read(plan.profile.machine_id())
        .with_context(|| text::cannot_read(&plan.profile.machine_id()))?;
    let (config, net_fd) = vz_mac_config::session_configuration(plan, &hw, &id)?;
    let mut start = start;
    if start == Start::Warm {
        if let Err(e) = vz_mac_config::save_restore_supported(&config) {
            say(&text::restore_unsupported(&e));
            events.record("restore", &text::restore_unsupported(&e), &[]);
            let _ = session::remove(&plan.profile, session::What::State);
            start = Start::Cold;
        }
    }
    if !plan.save {
        if let Err(e) = vz_mac_config::save_restore_supported(&config) {
            log::debug!("macvm: save is off, and the configuration could not save anyway: {e}");
        }
    }

    // The firewall of the session: the mode, the rules of the
    // profile, the question of mode ask, the connection log.
    let firewall = super::gate::build(
        env,
        &plan.app,
        &plan.profile.dir,
        plan.profile.connections_log(),
        &plan.network,
        &plan.allow,
        std::process::id(),
        super::gate::app_side(
            plan.app_bundle
                .as_ref()
                .and_then(|p| super::appcopy::read(p).ok())
                .as_ref(),
            plan.bench,
        ),
    );
    let proxy_socket = match &plan.proxy_socket {
        Some(p) => p.clone(),
        None => {
            let sock = plan.control.with_file_name("proxy.sock");
            let mut cfg = crate::proxy::HostProxy::from_vars(|k| env.var(k).map(str::to_string));
            for w in cfg.cleartext_warnings() {
                log::warn!("{w}");
            }
            cfg.token = Some(token.clone());
            cfg.gate = Some(firewall.clone().gate());
            let boundary = plan.profile.boundary_log();
            cfg.boundary = Some(crate::proxy::Boundary(Arc::new(
                move |target: &str, rule: &str, id: Option<&crate::proxy::Identity>| {
                    let rule = match id {
                        Some(id) => text::rule_with_identity(rule, &id.to_string()),
                        None => rule.to_string(),
                    };
                    let e = crate::boundary::Entry::new(crate::boundary::NETWORK, target, &rule);
                    if let Err(err) = crate::boundary::record_at(&boundary, e) {
                        log::debug!("macvm: cannot write the boundary log: {err}");
                    }
                },
            )));
            crate::proxy::serve(&sock, cfg)?;
            sock
        }
    };
    let network = {
        let on_end = Events::open(&plan.profile);
        Arc::new(super::stubnet::Network::start(
            net_fd,
            plan.profile.boundary_log(),
            Box::new(move |why| {
                let line = text::network_ended(why);
                log::warn!("macvm: {line}");
                on_end.record("network", &line, &[]);
            }),
        ))
    };

    let mtm = MainThreadMarker::new().with_context(|| text::main_thread_only("paddock-vm app"))?;
    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();

    let (tx, rx) = mpsc::channel::<Event>();
    let (vtx, vrx) = mpsc::channel::<VmEvent>();
    run_vm::watch_signals(vtx.clone())?;
    {
        // The VM and process events join the one channel of the loop.
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = vrx.recv() {
                if tx.send(Event::Vm(ev)).is_err() {
                    break;
                }
            }
        });
    }
    let queue = DispatchQueue::main();
    let vm = Arc::new(Vm(unsafe {
        VZVirtualMachine::initWithConfiguration_queue(VZVirtualMachine::alloc(), &config, queue)
    }));
    let files = super::run_files::Files::new(plan);
    let shell = Shell::new(
        mtm,
        vm.clone(),
        plan.display,
        tx.clone(),
        &plan.app,
        &files,
        &plan.profile,
    );
    let delegate = run_vm::VmDelegate::new(vtx.clone());
    let sink: super::vz_mac_agent::Sink = {
        let tx = tx.clone();
        Arc::new(move |ev| {
            let _ = tx.send(Event::Agent(ev));
        })
    };
    let login = super::run_login::Login::new(env, plan, &token, tx.clone());
    let listener = Listener::new(
        sink.clone(),
        token.clone(),
        BTreeMap::from([(ports::GUEST_PROXY, proxy_socket.clone())]),
        BTreeMap::from([
            (
                ports::GUEST_FILES,
                super::files_port::handler(
                    token.clone(),
                    plan.profile.clone(),
                    files.limiter.clone(),
                    sink,
                ),
            ),
            (ports::GUEST_LOGIN, login.handler()),
        ]),
    );
    {
        let vm = vm.clone();
        let objects = OnQueue((delegate.clone(), listener.clone()));
        queue.exec_async(move || {
            let (delegate, listener) = objects.get();
            let vm = &vm.0;
            unsafe { vm.setDelegate(Some(ProtocolObject::from_ref(&**delegate))) };
            listener.install(vm);
        });
    }

    let agent_dir = plan
        .shares
        .iter()
        .find(|s| s.name == "agent")
        .map(|s| s.path.clone());
    let mut session = Session {
        env,
        plan,
        events,
        stamp,
        start,
        screen: super::run_screen::Screen::new(password.is_some()),
        token,
        password,
        data: Default::default(),
        data_verify_seen: false,
        log_wanted: false,
        agent_dir,
        update_sent: None,
        update_refused: false,
        vm: vm.clone(),
        queue,
        tx: tx.clone(),
        vtx,
        agent: None,
        why: BTreeMap::new(),
        hello_deadline: None,
        last_health: None,
        health_written: None,
        watch: Watch::new(&plan.app),
        disk_level: None,
        relaunch_allowed: true,
        woke_at: None,
        retried_cold: false,
        quit: Quit::new(QuitConfig::new(&plan.app, plan.save, plan.memory_bytes)),
        quit_deadline: None,
        quitting: false,
        exit: None,
        begun: Instant::now(),
        slot_index: slot.index,
        suspend: super::run_suspend::Suspend::new(slot),
        record_done: false,
        failed: false,
        app_started: false,
        copy_begun: None,
        shell,
        network: network.clone(),
        network_written: Instant::now(),
        files,
        firewall,
        sleep: super::gate::SleepWatch::new(crate::state::now()),
        sleep_began: crate::state::now(),
        marker_checked: Instant::now(),
        freeze: Default::default(),
        login,
    };
    session.boot();
    let code = session.run_loop(&rx);
    // The host gets its input source back before the record goes.
    session.input_exit();
    network.stop();
    let _ = std::fs::remove_file(&plan.control);
    if plan.proxy_socket.is_none() {
        let _ = std::fs::remove_file(&proxy_socket);
    }
    if let Some(r) = &plan.record {
        let _ = std::fs::remove_file(r);
    }
    session
        .events
        .record("end", "the session ended", &[("code", code as f64)]);
    // The delegates are weak references of the VM. Keep them to the end.
    drop((delegate, listener));
    // The slot, when the session still holds it.
    drop(session);
    drop(lock);
    Ok(code)
}

/// One session, from the boot to the exit. `run_agent.rs` holds the
/// handlers of the guest messages.
pub(super) struct Session<'a> {
    pub(super) env: &'a HostEnv,
    pub(super) plan: &'a AppPlan,
    pub(super) events: Events,
    pub(super) stamp: Stamp,
    pub(super) start: Start,
    pub(super) token: String,
    pub(super) password: Option<String>,
    /// The data disk of this start (`data_start.rs`), and whether the
    /// verify of the health is in the events already.
    pub(super) data: super::data_start::DataStart,
    pub(super) data_verify_seen: bool,
    /// A `log.fetch` went out and its file has not come (`run_quit.rs`).
    pub(super) log_wanted: bool,
    /// The host folder of the agent share, for `agent.update`.
    pub(super) agent_dir: Option<PathBuf>,
    /// An `agent.update` went out: the agent version before it and the
    /// time, until the next hello says whether it took.
    pub(super) update_sent: Option<(String, Instant)>,
    /// The hello after an update carried the old version: the guest fell
    /// back to its previous agent. No second update in this session.
    pub(super) update_refused: bool,
    pub(super) vm: Arc<Vm>,
    pub(super) queue: &'static DispatchQueue,
    pub(super) tx: Sender<Event>,
    vtx: Sender<VmEvent>,
    /// The control connection of the guest agent, while one is up.
    pub(super) agent: Option<Agent>,
    /// Why each pending request was sent.
    pub(super) why: BTreeMap<u64, Why>,
    pub(super) hello_deadline: Option<Instant>,
    pub(super) last_health: Option<Instant>,
    /// When the record last got a `health`, for `paddock app status`.
    pub(super) health_written: Option<Instant>,
    /// The failure matrix fold; `run_failure.rs` runs it.
    pub(super) watch: Watch,
    /// The disk space level of the volume, once a level was reached.
    pub(super) disk_level: Option<super::space::Level>,
    /// False after three app crashes in a minute.
    pub(super) relaunch_allowed: bool,
    /// The wall time of the last wake, for the firewall questions.
    pub(super) woke_at: Option<u64>,
    pub(super) retried_cold: bool,
    pub(super) quit: Quit,
    pub(super) quit_deadline: Option<Instant>,
    pub(super) quitting: bool,
    pub(super) exit: Option<i32>,
    pub(super) begun: Instant,
    pub(super) slot_index: usize,
    /// The slot and the automatic suspend (`run_suspend.rs`).
    pub(super) suspend: super::run_suspend::Suspend,
    pub(super) record_done: bool,
    /// The session failed before its end: the exit code is 1.
    pub(super) failed: bool,
    /// The copy and the launch of the app happened, or were skipped.
    pub(super) app_started: bool,
    /// When the `app.copy` of this start went out, for its seconds.
    pub(super) copy_begun: Option<Instant>,
    /// The host windows, the order fold and the Dock.
    pub(super) shell: Shell,
    /// The stub network of the session.
    pub(super) network: Arc<super::stubnet::Network>,
    /// When the counters last went into the record.
    network_written: Instant,
    /// The shares, the drops and the clipboard (`run_files.rs`).
    pub(super) files: super::run_files::Files,
    /// The firewall of the session (`gate.rs`).
    pub(super) firewall: Arc<super::gate::Firewall>,
    /// The watch for a sleep of the host, for the questions of the firewall.
    pub(super) sleep: super::gate::SleepWatch,
    /// The wall time of the last tick, so the sleep length is known.
    pub(super) sleep_began: u64,
    /// The marker row: `paddock mark` is brought back once a minute at most.
    pub(super) marker_checked: Instant,
    /// `paddock freeze` of the session (`run_calls.rs`).
    pub(super) freeze: super::run_calls::Freeze,
    /// The lock of the guest screen and its unlock (`run_screen.rs`).
    pub(super) screen: super::run_screen::Screen,
    /// The links and the login callbacks (`run_login.rs`).
    pub(super) login: super::run_login::Login,
}

impl Session<'_> {
    /// One request to the agent. False when no agent is connected or the
    /// write failed.
    pub(super) fn send(&mut self, request: Request, why: Why) -> bool {
        let kind = request.kind();
        let Some(agent) = &mut self.agent else {
            log::warn!("macvm: no guest agent for {kind}");
            return false;
        };
        if matches!(request, Request::AppLaunch { .. }) {
            self.shell.last_launch = Some(request.clone());
        }
        match agent.send(request) {
            Ok(req) => {
                self.why.insert(req, why);
                true
            }
            Err(e) => {
                log::warn!("macvm: {e:#}");
                false
            }
        }
    }

    /// The main loop: AppKit's events of one pass (`run_appkit.rs`), then
    /// every pending event of the session.
    fn run_loop(&mut self, rx: &mpsc::Receiver<Event>) -> i32 {
        let Some(mtm) = MainThreadMarker::new() else {
            return 1;
        };
        let app = NSApplication::sharedApplication(mtm);
        loop {
            if let Some(code) = self.exit {
                self.wait_for_log(rx);
                return code;
            }
            let until = NSDate::dateWithTimeIntervalSinceNow(TICK);
            // SAFETY: a run loop mode constant of Foundation.
            let mode = unsafe { NSDefaultRunLoopMode };
            super::run_appkit::pass(&app, &until, mode);
            while let Ok(ev) = rx.try_recv() {
                self.handle(ev);
                if self.exit.is_some() {
                    break;
                }
            }
            self.tick();
        }
    }

    fn handle(&mut self, ev: Event) {
        let Some(ev) = self.suspended_filter(ev) else {
            return;
        };
        match ev {
            Event::Restored(Ok(())) => {
                self.events.record(
                    "restore",
                    "the saved state is restored",
                    &[("secs", self.begun.elapsed().as_secs_f64())],
                );
                self.resume(true);
            }
            Event::Restored(Err(e)) => {
                say(&text::restore_failed(&e));
                self.events
                    .record("restore", &text::restore_failed(&e), &[]);
                let _ = session::remove(&self.plan.profile, session::What::State);
                self.start = Start::Cold;
                self.boot();
            }
            Event::Started(Ok(())) => self.started(),
            Event::Started(Err(e)) => {
                let other = e.contains("maximum") || e.contains("limit") || e.contains("too many");
                say(&text::vm_start_failed(&e, other));
                self.events
                    .record("error", &text::vm_start_failed(&e, false), &[]);
                self.end(false, 1);
            }
            Event::Resumed(r) => self.feed(Input::Resumed(r)),
            Event::Paused(r) => self.feed(Input::Paused(r)),
            Event::Saved(r) => {
                let partial = session::partial(&self.plan.profile.state());
                let r = r.and_then(|_| {
                    let len = std::fs::metadata(&partial)
                        .map(|m| m.len())
                        .map_err(|e| e.to_string())?;
                    let _ =
                        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600));
                    Ok(len)
                });
                self.feed(Input::Saved(r));
            }
            Event::Stopped(r) => {
                if !self.quitting {
                    // A stop that the hello timeout asked for.
                    if let Err(e) = r {
                        say(&text::stop_failed(&e));
                    }
                    self.after_hello_timeout_stop();
                    return;
                }
                self.feed(Input::Stopped(r));
            }
            Event::Vm(VmEvent::GuestStopped) => {
                say(&text::guest_stopped_after(self.begun.elapsed()));
                self.quitting = true;
                self.feed(Input::GuestStopped);
            }
            Event::Vm(VmEvent::Failed(e)) => {
                say(&text::vm_failed(&e));
                self.events.record("error", &text::vm_failed(&e), &[]);
                self.end(false, 1);
            }
            Event::Vm(VmEvent::Signal) => {
                if self.quitting {
                    say(&text::second_signal());
                    self.feed(Input::Signal);
                } else {
                    self.begin_quit("signal");
                }
            }
            Event::Vm(VmEvent::ControlStop) => {
                if self.quitting {
                    self.feed(Input::Signal);
                } else {
                    self.begin_quit("stop request");
                }
            }
            Event::Agent(AgentEvent::Connected(writer)) => {
                self.agent = Some(Agent::new(writer));
                self.why.clear();
                self.watch.connected(Instant::now());
            }
            Event::Agent(AgentEvent::Gone) => {
                self.agent = None;
                self.why.clear();
                let steps = self.watch.agent_gone();
                self.execute(steps);
                self.menu_agent_gone();
                self.ax_agent_gone();
                self.screen_agent_gone();
            }
            Event::Agent(AgentEvent::Message(ev)) => self.guest_message(ev),
            Event::LoginForward(port, ticket) => self.login_forward(port, ticket),
            Event::Agent(AgentEvent::File { kind, name, path }) => {
                self.guest_file(kind, name, path)
            }
            Event::Agent(AgentEvent::FileRefused(line)) => {
                self.events.record("files", &line, &[]);
            }
            Event::Vm(VmEvent::Window(args, reply)) => {
                let _ = reply.send(self.on_window_request(&args));
            }
            Event::Vm(VmEvent::Bench(b)) => self.on_bench(b),
            Event::Vm(VmEvent::Freeze(pause, reply)) => self.freeze_line(pause, reply),
            Event::Vm(VmEvent::Suspend(reason, reply)) => {
                let _ = reply.send(self.begin_suspend(&reason));
            }
            Event::Frozen(pause, r) => self.freeze_done(pause, r),
            Event::Host(ev) => self.on_host(ev),
            Event::Typed(played) => self.screen_typed(played),
        }
    }

    /// The VM runs: the hello deadline, the record, the control socket.
    fn started(&mut self) {
        let warm = self.start == Start::Warm;
        say(&text::vm_running(self.start.word(), self.begun.elapsed()));
        self.events.record(
            "running",
            if warm { "resumed" } else { "booting" },
            &[("secs", self.begun.elapsed().as_secs_f64())],
        );
        self.hello_deadline = Some(Instant::now() + if warm { HELLO_WARM } else { HELLO_COLD });
        self.write_record();
        match bridge::bind_control(&self.plan.control) {
            Ok(control) => {
                let guest = VmGuest {
                    vm: self.vm.clone(),
                    queue: self.queue,
                    tx: self.vtx.clone(),
                };
                bridge::serve_control(control, Arc::new(guest));
            }
            Err(e) => say(&text::no_control_socket(&format!("{e:#}"))),
        }
    }

    /// The deadlines: hello, health silence, the requests, the quit step,
    /// and the clipboard and drops of `run_files.rs`.
    fn tick(&mut self) {
        let now = Instant::now();
        if self.suspend.active {
            return;
        }
        if self.freeze.frozen() {
            // Paused by paddock freeze: no deadline runs (`run_calls.rs`).
            return;
        }
        if !self.quitting {
            self.files_tick();
            self.lead_tick();
        }
        if let Some(d) = self.hello_deadline {
            if now >= d {
                self.hello_deadline = None;
                self.hello_timeout();
            }
        }
        self.failure_tick(now);
        let mut timed_out = Vec::new();
        let mut expired = Vec::new();
        if let Some(agent) = &mut self.agent {
            for (req, p) in agent.expired(now) {
                timed_out.extend(self.why.remove(&req));
                let line = text::request_timeout(&p.kind, Agent::waited(&p));
                log::warn!("macvm: {line}");
                self.events.record("timeout", &line, &[]);
                expired.push(p.kind);
            }
        }
        for kind in expired {
            // Three in a row: the agent crash row of the failure matrix.
            let steps = self.watch.timeout(&kind);
            self.execute(
                steps
                    .into_iter()
                    .filter(|s| {
                        !matches!(
                            s,
                            Step::Event {
                                kind: "timeout",
                                ..
                            }
                        )
                    })
                    .collect(),
            );
        }
        self.request_timeouts(&timed_out);
        self.input_tick();
        self.screen_tick(now);
        if let Some(d) = self.quit_deadline {
            if now >= d {
                self.quit_deadline = None;
                self.feed(Input::Timeout);
            }
        }
        if now.duration_since(self.network_written) >= NETWORK_RECORD {
            self.network_written = now;
            self.write_network();
            self.write_busy();
            self.disk_tick();
            // The shim log is this process's stderr: 5 files of 1 MiB The
            // new file goes onto fds 1 and 2.
            if let Err(e) = super::logs::reopen_if_large(
                &self.plan.profile.shim_log(),
                super::logs::FIVE_BY_MIB,
                &[1, 2],
            ) {
                log::debug!("macvm: shim log rotation: {e}");
            }
        }
    }
}
