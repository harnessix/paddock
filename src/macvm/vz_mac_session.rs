//! The booted golden of `paddock macvm setup`, macOS only: the guided
//! step, the configure step and the shutdown for the seal, over the
//! control line of the guest agent.
//!
//! The agent dials in after the account exists (the provisioning options
//! or the person in Setup Assistant) and the agent is installed (the
//! first-boot daemon of the offline seed or the typed command): its
//! `hello` marks the panel steps 1 to 4 done, its `health` with the
//! Accessibility permission marks step 5 (`guided.rs` decides, and each
//! wait on the person goes to the event log). With the offline trust the
//! guided step ends at the first health report and the guest shuts down
//! for the patch. `configure` sends the list of `configure.rs`, prints
//! one line per step result and names the failed ones. The `Continue`
//! button of the panel stands in for the agent's messages when they do
//! not come. `vz_mac.rs` holds the other states and the configuration.

use super::configure;
use super::guided::{Change, Progress, Signal};
use super::layout::Golden;
use super::proto::{self, Hello, Outcome, ReplyData, Request};
use super::provision::Account;
use super::provision::{self, Route};
use super::setup::End;
use super::setup_events;
use super::text;
use super::text::PanelStep;
use super::vz_mac::{
    self, configuration, stub_network, GoldenDelegate, Parts, Sendable, VmEvent, VzBackend,
    SHUTDOWN_TIMEOUT, START_TIMEOUT, WANT_CPUS, WANT_MEMORY,
};
use super::vz_mac_agent::{Agent, AgentEvent, Listener};
use super::vz_mac_provision;
use super::vz_mac_window::{PanelEvent, Window};
use crate::vm::vz::{error_text, url};
use anyhow::{anyhow, bail, Context, Result};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::AllocAnyThread;
use objc2_foundation::{NSData, NSError};
use objc2_virtualization::*;
use std::collections::BTreeMap;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the configure step waits for the agent's hello: the user may
/// have to log in by hand in the window, because auto login is one of
/// the steps that configure sets.
const CONFIGURE_HELLO_WAIT: Duration = Duration::from_secs(600);
/// How long a boot of a seeded disk waits for the agent before the panel
/// shows the typed command: a provisioned macOS 27 guest reaches its
/// desktop in about 50 s.
const SEEDED_HELLO_WAIT: Duration = Duration::from_secs(300);
/// How long the trust check waits for a health report with the
/// permission after the first one without it.
const TRUST_WAIT: Duration = Duration::from_secs(30);
/// How long a `sudo shutdown` of the guest may take before the stop
/// request of the framework.
const SUDO_SHUTDOWN_WAIT: Duration = Duration::from_secs(120);

/// The booted golden of the guided step, the configure step and the seal.
pub(super) struct Session {
    vm: Sendable<Retained<VZVirtualMachine>>,
    rx: Receiver<VmEvent>,
    panel_rx: Receiver<PanelEvent>,
    pub(super) window: Window,
    _delegate: Sendable<Retained<GoldenDelegate>>,
    _listener: Listener,
    /// The other end of the stub network: open while the guest runs.
    _net: OwnedFd,
    /// The control line of the agent, while it is connected.
    agent: Option<Agent>,
    /// The hello of the agent, once it dialed in.
    hello: Option<Hello>,
    /// The token of this setup session, given to the agent at its hello.
    token: String,
    booted: Instant,
}

/// What one round of the loop saw.
pub(super) enum Step {
    Nothing,
    /// The agent dialed in and got the token.
    Hello,
    /// A health report, with the Accessibility permission or not.
    Health(bool),
    Reply(proto::Reply),
    /// The `Continue` button.
    Continue,
    /// The password that the person typed into the panel's question.
    Password(String),
    /// The `Stop` button.
    Stop,
    /// The window closed.
    Closed,
    GuestStopped,
    Failed(String),
    /// The channels are gone.
    Gone,
}

impl VzBackend {
    /// The guided state: boot (with the provisioning options when the
    /// route says so), then wait for the agent. With the offline trust it
    /// ends at the agent's first health report and shuts the guest down;
    /// else when the agent reports the Accessibility permission.
    pub(super) fn guided_run(
        &mut self,
        golden: &Golden,
        password: &str,
        route: &Route,
    ) -> Result<End> {
        let mut provisioned = false;
        if self.session.is_none() {
            let start = if route.account == Account::Provision {
                match vz_mac_provision::start_options(password) {
                    Ok(s) => Some(s),
                    Err(why) => {
                        println!("{}", text::route_guided(&why));
                        let line = format!("guided: {}", text::guided_why(&why)); // not user text
                        setup_events::record(&golden.events, provision::ROUTE, false, &line);
                        None
                    }
                }
            } else {
                None
            };
            provisioned = start.is_some();
            self.ensure_session_with(golden, password, start)?;
        }
        let account_made = provisioned || route.account == Account::Made;
        self.seeded = route.seeded;
        self.unattended = account_made && route.seeded;
        if let Some(s) = &self.session {
            s.window.set_unattended(self.unattended);
        }
        if account_made {
            self.signal(golden, Signal::Desktop);
        }
        let booted = Instant::now();
        let mut late = false;
        let mut asked = false;
        loop {
            if self.progress.finished() {
                return Ok(End::Done);
            }
            if route.trust_offline && asked {
                self.stop_guest(golden, password)?;
                return Ok(End::Done);
            }
            let hello = self.session.as_ref().is_some_and(|s| s.hello.is_some());
            if route.seeded && !late && !hello && booted.elapsed() > SEEDED_HELLO_WAIT {
                late = true;
                self.agent_late(golden, booted.elapsed());
            }
            match self.pump(Duration::from_millis(200)) {
                Step::Nothing | Step::Reply(_) | Step::Password(_) => {}
                Step::Hello => self.signal(golden, Signal::Hello),
                Step::Health(a) => {
                    self.signal(golden, Signal::Health(a));
                    asked |= self.session.as_ref().is_some_and(|s| s.hello.is_some());
                }
                // The button stands in for the agent: each press ends the
                // next wait (account, seed, Accessibility).
                Step::Continue => self.signal(golden, Signal::Continue),
                Step::Stop | Step::Closed => {
                    self.stop_session(Duration::from_secs(60));
                    return Ok(End::Stopped);
                }
                Step::GuestStopped => {
                    println!("{}", text::guest_shut_down_in_guided());
                    self.drop_session();
                    return Ok(End::Stopped);
                }
                Step::Failed(e) => {
                    self.drop_session();
                    bail!(text::guest_stopped_with_error(&e));
                }
                Step::Gone => bail!(text::guided_window_gone()),
            }
        }
    }

    /// The agent of a seeded disk did not dial in: the panel shows the
    /// typed command, and the waits go to the person.
    fn agent_late(&mut self, golden: &Golden, waited: Duration) {
        let line =
            text::seeded_agent_late(waited.as_secs(), &golden.console, &super::seed::command());
        println!("{line}");
        setup_events::record(
            &golden.events,
            setup_events::AGENT_LATE,
            false,
            &waited.as_secs().to_string(),
        );
        self.unattended = false;
        self.seeded = false;
        if let Some(s) = &self.session {
            s.window.hand_over(&line);
        }
        let change = self.progress.hand_over(PanelStep::Seed);
        self.apply(golden, change, "the first-boot daemon");
    }

    /// The trust check after the offline patch: boot when the guest is
    /// down, wait for the hello and a health report. `Some(true)` as soon
    /// as one has the permission, `Some(false)` when none has it within
    /// [`TRUST_WAIT`] of the first, `None` when the user stopped.
    pub(super) fn trusted_run(&mut self, golden: &Golden, password: &str) -> Result<Option<bool>> {
        self.ensure_session(golden, password)?;
        let deadline = Instant::now() + CONFIGURE_HELLO_WAIT;
        let mut first: Option<Instant> = None;
        loop {
            if Instant::now() >= deadline {
                bail!(text::hello_timeout(CONFIGURE_HELLO_WAIT, &golden.console));
            }
            if first.is_some_and(|t| t.elapsed() > TRUST_WAIT) {
                return Ok(Some(false));
            }
            match self.pump(Duration::from_millis(200)) {
                Step::Health(a) if self.session.as_ref().is_some_and(|s| s.hello.is_some()) => {
                    if a {
                        self.signal(golden, Signal::Health(true));
                        return Ok(Some(true));
                    }
                    first.get_or_insert_with(Instant::now);
                }
                other => {
                    if self.end_of(golden, other)?.is_some() {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// The configure step: the list of `configure.rs` to the agent, and
    /// every result checked against the step's expected text.
    pub(super) fn configure_run(&mut self, golden: &Golden, password: &str) -> Result<End> {
        self.ensure_session(golden, password)?;
        let deadline = Instant::now() + CONFIGURE_HELLO_WAIT;
        while self.session.as_ref().is_none_or(|s| s.hello.is_none()) {
            if Instant::now() >= deadline {
                bail!(text::hello_timeout(CONFIGURE_HELLO_WAIT, &golden.console));
            }
            if let Some(end) = self.pump_or_end(golden)? {
                return Ok(end);
            }
        }
        let request = Request::Configure {
            password: password.to_string(),
            steps: super::configure_quiet::wire_all(),
        };
        let wait = proto::timeout(&request);
        let session = self.session.as_mut().expect("booted above");
        let agent = session
            .agent
            .as_mut()
            .ok_or_else(|| anyhow!(text::guided_window_gone()))?;
        let req = agent.send(request)?;
        let deadline = Instant::now() + wait;
        loop {
            if Instant::now() >= deadline {
                bail!(text::request_timeout("configure", wait));
            }
            match self.pump(Duration::from_millis(200)) {
                Step::Reply(r) if r.req == req => {
                    return match r.outcome {
                        Outcome::Ok {
                            data: Some(ReplyData::Configure { steps }),
                        } => {
                            if configure::wrong_password(&steps) {
                                return Ok(End::WrongPassword);
                            }
                            let (lines, failed) = configure::report(&steps);
                            for line in &lines {
                                println!("{line}");
                            }
                            // The quiet steps: a warning each, never a
                            // failed golden.
                            let (quiet, unproven) = super::configure_quiet::report(&steps);
                            for line in &quiet {
                                println!("{line}");
                                setup_events::record(
                                    &golden.events,
                                    setup_events::STATE,
                                    false,
                                    line,
                                );
                            }
                            if !unproven.is_empty() {
                                for line in &quiet {
                                    if line.contains("goes on without it") {
                                        log::warn!("macvm: {line}");
                                    }
                                }
                            }
                            for line in lines.iter().filter(|l| l.contains("FAILED")) {
                                setup_events::record(
                                    &golden.events,
                                    setup_events::STATE,
                                    false,
                                    line,
                                );
                            }
                            if failed.is_empty() {
                                let change = self.progress.configured();
                                self.apply(golden, change, "configure");
                                Ok(End::Done)
                            } else {
                                bail!(text::configure_failed(&failed))
                            }
                        }
                        Outcome::Ok { data: _ } => bail!(text::configure_no_result()),
                        Outcome::Error { code, text: detail } => bail!(text::request_failed(
                            "configure",
                            &text::io_failed(code.describe(), &detail)
                        )),
                    };
                }
                other => {
                    if let Some(end) = self.end_of(golden, other)? {
                        return Ok(end);
                    }
                }
            }
        }
    }

    /// The guest refused the password: the panel asks for the one the
    /// person set, and this waits for the answer or a stop.
    pub(super) fn ask_password_run(&mut self, golden: &Golden) -> Result<Option<String>> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| anyhow!(text::guided_window_gone()))?;
        session.window.ask_password();
        loop {
            match self.pump(Duration::from_millis(200)) {
                Step::Password(p) => return Ok(Some(p)),
                other => {
                    if self.end_of(golden, other)?.is_some() {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Shut the guest down and wait for the stop: `sudo shutdown -h now`
    /// through the agent (a configure request of one root step, the
    /// password on stdin), then the stop request of the framework, then a
    /// hard stop. Nothing asks macOS for a permission on the way. After
    /// the stop the disk and the auxiliary storage are free again.
    pub(super) fn stop_guest(&mut self, golden: &Golden, password: &str) -> Result<()> {
        if self.session.is_none() {
            return Ok(());
        }
        let sent = self
            .session
            .as_mut()
            .and_then(|s| s.agent.as_mut())
            .map(|agent| {
                agent.send(Request::Configure {
                    password: password.to_string(),
                    steps: vec![configure::shutdown_step()],
                })
            });
        if let Some(Err(e)) = &sent {
            log::warn!("macvm: {e:#}");
        }
        if matches!(sent, Some(Ok(_))) {
            let deadline = Instant::now() + SUDO_SHUTDOWN_WAIT;
            while Instant::now() < deadline {
                match self.pump(Duration::from_millis(200)) {
                    Step::GuestStopped => {
                        self.drop_session();
                        break;
                    }
                    Step::Failed(e) => {
                        self.drop_session();
                        bail!(text::guest_stopped_with_error(&e));
                    }
                    Step::Gone => break,
                    // sudo refused or failed: the stop request at once.
                    Step::Reply(r) if !shutdown_started(&r) => break,
                    _ => {}
                }
            }
        }
        if self.session.is_some() && !self.stop_session(SHUTDOWN_TIMEOUT) {
            bail!(text::guest_no_shutdown(SHUTDOWN_TIMEOUT));
        }
        wait_aux_free(&golden.disk, AUX_WAIT);
        wait_aux_free(&golden.aux, AUX_WAIT);
        Ok(())
    }

    /// One round of the loop while a state waits for something else than
    /// a reply: the ends that stop the state, else nothing.
    fn pump_or_end(&mut self, golden: &Golden) -> Result<Option<End>> {
        let step = self.pump(Duration::from_millis(200));
        self.end_of(golden, step)
    }

    /// The end of a state that a step means, when it means one.
    pub(super) fn end_of(&mut self, golden: &Golden, step: Step) -> Result<Option<End>> {
        Ok(match step {
            Step::Stop | Step::Closed => {
                self.stop_session(Duration::from_secs(60));
                Some(End::Stopped)
            }
            Step::GuestStopped => {
                println!("{}", text::guest_shut_down_in_guided());
                self.drop_session();
                Some(End::Stopped)
            }
            Step::Failed(e) => {
                self.drop_session();
                bail!(text::guest_stopped_with_error(&e));
            }
            Step::Gone => bail!(text::guided_window_gone()),
            Step::Hello => {
                self.signal(golden, Signal::Hello);
                None
            }
            Step::Health(a) => {
                self.signal(golden, Signal::Health(a));
                None
            }
            _ => None,
        })
    }

    /// One signal of the guided step into the progress.
    pub(super) fn signal(&mut self, golden: &Golden, signal: Signal) {
        let by = match signal {
            Signal::Continue => "the Continue button",
            Signal::Hello | Signal::Health(_) => "the guest agent",
            Signal::Desktop => "the provisioning options",
        };
        let change = self.progress.on(signal);
        self.apply(golden, change, by);
    }

    /// Mark the panel and the event log by a change of the progress.
    /// While no step waits on the person, paddock did the waits: they go
    /// to the log without the person. The seed of a seeded disk is never
    /// the person's.
    pub(super) fn apply(&mut self, golden: &Golden, change: Change, by: &str) {
        for kind in change.person {
            if self.unattended || (self.seeded && kind == setup_events::SEED_INSTALLED) {
                setup_events::record_once(&golden.events, kind, by);
            } else {
                setup_events::record_person_once(&golden.events, kind, by);
            }
        }
        if let (Some(through), Some(s)) = (change.mark, &self.session) {
            s.window.mark_done(through);
        }
    }

    /// One event of the VM, the agent or the panel, within `wait`.
    pub(super) fn pump(&mut self, wait: Duration) -> Step {
        let Some(session) = self.session.as_mut() else {
            return Step::Gone;
        };
        match session.rx.recv_timeout(wait) {
            Ok(VmEvent::GuestStopped) => return Step::GuestStopped,
            Ok(VmEvent::Failed(e)) => return Step::Failed(e),
            Ok(VmEvent::Started(_)) => {}
            Ok(VmEvent::Agent(AgentEvent::Connected(writer))) => {
                session.agent = Some(Agent::new(writer));
            }
            Ok(VmEvent::Agent(AgentEvent::Gone)) => {
                session.agent = None;
                session.hello = None;
            }
            Ok(VmEvent::Agent(AgentEvent::Message(ev))) => return session.message(ev),
            // The setup has no guest-files port.
            Ok(VmEvent::Agent(AgentEvent::File { .. } | AgentEvent::FileRefused(_))) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Step::Gone,
        }
        match session.panel_rx.try_recv() {
            Ok(PanelEvent::Continue) => Step::Continue,
            Ok(PanelEvent::Password(p)) => Step::Password(p),
            Ok(PanelEvent::Stop) => Step::Stop,
            Ok(PanelEvent::Closed) => Step::Closed,
            Err(mpsc::TryRecvError::Empty) => Step::Nothing,
            Err(mpsc::TryRecvError::Disconnected) => Step::Gone,
        }
    }

    /// Boot the golden with the guided window, when it is not up.
    pub(super) fn ensure_session(&mut self, golden: &Golden, password: &str) -> Result<()> {
        self.ensure_session_with(golden, password, None)
    }

    /// Boot the golden with the guided window and, when given, the start
    /// options with the provisioning options. The first boot of the golden
    /// goes to its event log after the start (`provision.rs`).
    pub(super) fn ensure_session_with(
        &mut self,
        golden: &Golden,
        password: &str,
        start: Option<Retained<VZMacOSVirtualMachineStartOptions>>,
    ) -> Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let hw_data = std::fs::read(&golden.hardware_model)
            .with_context(|| text::cannot_read(&golden.hardware_model))?;
        let hw = unsafe {
            VZMacHardwareModel::initWithDataRepresentation(
                VZMacHardwareModel::alloc(),
                &NSData::with_bytes(&hw_data),
            )
        }
        .ok_or_else(|| anyhow!(text::damaged(&golden.hardware_model)))?;
        let id_data = std::fs::read(&golden.machine_id)
            .with_context(|| text::cannot_read(&golden.machine_id))?;
        let machine_id = unsafe {
            VZMacMachineIdentifier::initWithDataRepresentation(
                VZMacMachineIdentifier::alloc(),
                &NSData::with_bytes(&id_data),
            )
        }
        .ok_or_else(|| anyhow!(text::damaged(&golden.machine_id)))?;
        wait_aux_free(&golden.aux, AUX_WAIT);
        let aux_url = url(&golden.aux)?;
        let storage =
            unsafe { VZMacAuxiliaryStorage::initWithURL(VZMacAuxiliaryStorage::alloc(), &aux_url) };
        let (net_guest, net_host) = stub_network()?;
        let parts = Parts {
            disk: golden.disk.clone(),
            console: Some(golden.console.clone()),
            hw,
            machine_id,
            storage,
            cpus: WANT_CPUS.min(unsafe { VZVirtualMachineConfiguration::maximumAllowedCPUCount() }),
            memory: WANT_MEMORY
                .min(unsafe { VZVirtualMachineConfiguration::maximumAllowedMemorySize() }),
            network: Some(net_guest),
            // The guided step needs it; configure and the seal boot an
            // unsealed golden that may have it still, and it does no harm.
            seed: golden.seed.is_dir().then(|| golden.seed.clone()),
        };
        let config = configuration(&parts)?;
        let (tx, rx) = mpsc::channel::<VmEvent>();
        let (panel_tx, panel_rx) = mpsc::channel::<PanelEvent>();
        let delegate = Sendable(GoldenDelegate::new(tx.clone()));
        let token = crate::vm::proto::new_token()?;
        let listener = {
            let tx = tx.clone();
            Listener::new(
                Arc::new(move |ev| {
                    let _ = tx.send(VmEvent::Agent(ev));
                }),
                token.clone(),
                BTreeMap::new(),
                BTreeMap::new(),
            )
        };
        let config = Sendable(config);
        let password = password.to_string();
        let delegate_for_vm = Sendable(delegate.0.clone());
        let provisioned = start.is_some();
        let start = Sendable(start);
        let listener_for_vm = listener.clone();
        let (vtx, vrx) = mpsc::channel::<Sendable<(Retained<VZVirtualMachine>, Window)>>();
        // The VM of the guided step lives on the main queue, where its view is.
        DispatchQueue::main().exec_sync(move || {
            let vm = unsafe {
                VZVirtualMachine::initWithConfiguration_queue(
                    VZVirtualMachine::alloc(),
                    config.get(),
                    DispatchQueue::main(),
                )
            };
            unsafe { vm.setDelegate(Some(ProtocolObject::from_ref(&**delegate_for_vm.get()))) };
            listener_for_vm.install(&vm);
            let window = Window::open(&vm, password, panel_tx);
            let done = RcBlock::new(move |err: *mut NSError| {
                let _ = tx.send(VmEvent::Started(error_text(err)));
            });
            match start.get() {
                // SAFETY: start options of a macOS boot loader.
                Some(o) => unsafe { vm.startWithOptions_completionHandler(o, &done) },
                None => unsafe { vm.startWithCompletionHandler(&done) },
            }
            let _ = vtx.send(Sendable((vm, window)));
        });
        let Sendable((vm, window)) = vrx.recv().with_context(text::main_thread_no_boot)?;
        match rx.recv_timeout(START_TIMEOUT) {
            Ok(VmEvent::Started(Ok(()))) => {}
            Ok(VmEvent::Started(Err(e))) => bail!(text::golden_start_failed(&e)),
            Ok(VmEvent::Failed(e)) => bail!(text::golden_stopped_at_start(Some(&e))),
            Ok(_) => bail!(text::golden_stopped_at_start(None)),
            Err(_) => bail!(text::golden_start_timeout(START_TIMEOUT)),
        }
        provision::record_first_boot(&golden.events, provisioned);
        self.session = Some(Session {
            vm: Sendable(vm),
            rx,
            panel_rx,
            window,
            _delegate: delegate,
            _listener: listener,
            _net: net_host,
            agent: None,
            hello: None,
            token,
            booted: Instant::now(),
        });
        self.progress = Progress::default();
        Ok(())
    }

    /// Ask the guest to stop and wait. True when it stopped. A guest that
    /// does not stop in time is stopped hard after the wait, so the
    /// process can end; the disk is then a crash of the guest, which the
    /// next boot of the unsealed golden repairs itself.
    pub(super) fn stop_session(&mut self, wait: Duration) -> bool {
        let Some(session) = self.session.as_ref() else {
            return true;
        };
        let vm = Sendable(session.vm.0.clone());
        DispatchQueue::main().exec_sync(move || {
            let vm = vm.get();
            if unsafe { vm.canRequestStop() } {
                if let Err(e) = unsafe { vm.requestStopWithError() } {
                    eprintln!("{}", text::stop_request_failed(&vz_mac::describe(&e)));
                }
            }
        });
        let deadline = Instant::now() + wait;
        let mut stopped = false;
        while Instant::now() < deadline {
            match session.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(VmEvent::GuestStopped) | Ok(VmEvent::Failed(_)) => {
                    stopped = true;
                    break;
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if !stopped {
            let vm = Sendable(session.vm.0.clone());
            let (tx, rx) = mpsc::channel();
            DispatchQueue::main().exec_sync(move || {
                let vm = vm.get();
                if unsafe { vm.canStop() } {
                    let done = RcBlock::new(move |err: *mut NSError| {
                        let _ = tx.send(error_text(err));
                    });
                    unsafe { vm.stopWithCompletionHandler(&done) };
                }
            });
            let _ = rx.recv_timeout(Duration::from_secs(10));
        }
        self.drop_session();
        stopped
    }

    pub(super) fn drop_session(&mut self) {
        if let Some(s) = self.session.take() {
            s.window.close();
            let vm = s.vm;
            // Release the VM on its queue.
            DispatchQueue::main().exec_sync(move || drop(vm));
        }
    }
}

impl Session {
    /// One message of the agent: the hello gets the token, the health
    /// tells about the permission, a reply goes to the state.
    fn message(&mut self, ev: proto::Event) -> Step {
        match ev {
            proto::Event::Hello(h) => {
                println!("{}", text::agent_dialed(&h.agent, self.booted.elapsed()));
                if let Err(e) =
                    proto::check_hello(&h, proto::VERSION, super::version::paddock_version())
                {
                    // The golden carries the agent of this paddock: a
                    // refusal is a build error, and the line says so.
                    return Step::Failed(e.to_string());
                }
                if let Some(agent) = self.agent.as_mut() {
                    if let Err(e) = agent.send(Request::Token {
                        token: self.token.clone(),
                        v: proto::VERSION,
                        bench: false,
                        now_ms: Some(crate::macvm::proto::now_ms()),
                    }) {
                        return Step::Failed(format!("{e:#}"));
                    }
                }
                self.hello = Some(h);
                Step::Hello
            }
            proto::Event::Health(h) => Step::Health(h.accessibility),
            proto::Event::Reply(r) => {
                if let Some(agent) = self.agent.as_mut() {
                    agent.take(r.req);
                }
                Step::Reply(r)
            }
            other => {
                log::debug!("macvm: setup guest event {}", other.kind());
                Step::Nothing
            }
        }
    }
}

/// How long a start waits for the auxiliary storage to be free.
const AUX_WAIT: Duration = Duration::from_secs(60);

/// Wait until no VM holds the lock of the auxiliary storage `aux`, at most
/// `max`. The VM of the install keeps it for a moment after it ends, and a
/// start in that moment fails with "Failed to lock auxiliary storage".
/// Virtualization locks the file with `flock`; a probe lock that is taken
/// and released at once tells when it is free. After `max` the start goes
/// on and names the error itself.
fn wait_aux_free(aux: &std::path::Path, max: Duration) {
    use std::os::fd::AsRawFd;
    let deadline = Instant::now() + max;
    let mut said = false;
    loop {
        let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(aux) else {
            return;
        };
        // SAFETY: flock on an fd that `f` owns.
        let free = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if free {
            // SAFETY: as above.
            unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        if !said {
            eprintln!("{}", text::aux_busy_wait());
            said = true;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Whether the reply of the shutdown request says that `sudo shutdown`
/// ran. A guest that stops sends no reply at all; a reply with a failed
/// step means that the guest goes on.
fn shutdown_started(r: &proto::Reply) -> bool {
    match &r.outcome {
        Outcome::Ok {
            data: Some(ReplyData::Configure { steps }),
        } => steps.iter().all(|s| s.ok),
        _ => false,
    }
}
