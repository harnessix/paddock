//! Automatic suspend in the shim (`suspend.rs`). macOS only; the decisions
//! are the pure functions of `suspend.rs` and the suspend path of the
//! quit machine (`quit.rs`), both tested on Linux.
//!
//! A `suspend REASON` line on the control socket starts the save of the
//! running VM without `app.quit`. Once the state is saved and the VM is
//! stopped, the shim gives its slot back and stays in the Dock: the record
//! says `suspended`, the Dock menu offers `Resume`. A click on the Dock
//! icon starts `paddock-vm install --app NAME --instance N
//! --resume-from-dock`, which ends this shim and starts a new one from
//! the saved state. A quit or a stop signal of a suspended shim ends it at
//! once: the state is on disk already.

use super::dock::DockAction;
use super::lock::Slot;
use super::quit::Input;
use super::run::{Event, Session};
use super::run_vm::{say, VmEvent};
use super::suspend::{self, Busy};
use super::text;
use super::windows::HostEvent;

/// The suspend state of the shim.
#[derive(Default)]
pub(super) struct Suspend {
    /// The VM slot, given back at the suspend.
    pub(super) slot: Option<Slot>,
    /// The reason of the suspend that runs or ran.
    pub(super) reason: Option<String>,
    /// The quit machine refused the suspend: the line.
    pub(super) refused: Option<String>,
    /// The state is saved and the slot is free.
    pub(super) active: bool,
    /// A resume from the Dock was started.
    pub(super) resuming: bool,
    /// What the record says now, so it is written on a change only.
    pub(super) busy_written: Option<Option<Busy>>,
    pub(super) key_written: u64,
}

impl Suspend {
    pub(super) fn new(slot: Slot) -> Self {
        Self {
            slot: Some(slot),
            ..Default::default()
        }
    }
}

impl Session<'_> {
    /// Why this session cannot be suspended now.
    fn busy_now(&self) -> Option<Busy> {
        let order = &self.shell.order;
        let dialog = order
            .ids()
            .iter()
            .filter_map(|id| order.get(*id))
            .any(|e| e.level == super::proto::Level::Sheet);
        // A home on the system disk is never saved (`data_start.rs`): a
        // session whose data disk is not prepared yet cannot be suspended.
        let preparing =
            self.data.phase() == super::data_start::Phase::Waiting || !self.stamp.data_prepared;
        suspend::busy(dialog, self.copy_begun.is_some(), preparing)
    }

    /// The `suspend` line of the control socket: `Ok` once the save began.
    pub(super) fn begin_suspend(&mut self, reason: &str) -> Result<String, String> {
        if self.quitting || self.suspend.active {
            return Err(text::suspend_while_quitting());
        }
        if self.files.grant_pending.is_some() || self.files.fresh_handles.is_some() {
            return Err(text::grant_busy());
        }
        if self.freeze.frozen() {
            return Err(text::suspend_frozen());
        }
        if let Some(b) = self.busy_now() {
            return Err(b.text());
        }
        self.events
            .record("suspend", &text::suspend_event(reason), &[]);
        self.suspend.reason = Some(reason.to_string());
        self.suspend.refused = None;
        self.quitting = true;
        let agent = self.agent.is_some();
        self.feed(Input::Suspend { agent });
        match self.suspend.refused.take() {
            Some(line) => Err(line),
            None => Ok("ok".into()),
        }
    }

    /// The quit machine refused the suspend: the session runs on, unless
    /// a quit came meanwhile and goes on now.
    pub(super) fn suspend_refused(&mut self, line: String) {
        say(&line);
        self.quitting = self.quit.phase() != super::quit::Phase::Idle;
        self.suspend.reason = None;
        self.suspend.refused = Some(line);
    }

    /// The state is saved and the VM stopped: free the slot, keep the Dock.
    pub(super) fn enter_suspended(&mut self) {
        self.suspend.active = true;
        self.input_exit();
        self.agent = None;
        self.why.clear();
        self.hello_deadline = None;
        self.quit_deadline = None;
        // The slot goes now: another session takes it.
        drop(self.suspend.slot.take());
        let n = super::instance::of_profile(&self.plan.profile.dir);
        let label = super::instance::label(&self.plan.app, n);
        say(&text::suspended_line(
            &label,
            &super::instance::arg(&self.plan.app, n),
        ));
        self.shell.dock.update(|s| {
            s.suspended = true;
            s.windows.clear();
        });
        let reason = self.suspend.reason.clone();
        self.update_record(|v| {
            v["suspended"] = serde_json::json!(true);
            v["slot"] = serde_json::Value::Null;
            v["busy"] = serde_json::Value::Null;
            v["suspend_reason"] = serde_json::json!(reason);
        });
    }

    /// The events of a suspended shim, or of one whose suspend runs.
    /// `None` when the event was handled here.
    pub(super) fn suspended_filter(&mut self, ev: Event) -> Option<Event> {
        let pending = self.suspend.reason.is_some() && !self.suspend.active;
        if pending {
            // A quit while the save runs ends as a quit after the save.
            if let Event::Host(HostEvent::Quit) | Event::Vm(VmEvent::ControlStop) = ev {
                let agent = self.agent.is_some();
                self.feed(Input::Begin { agent });
                return None;
            }
            return Some(ev);
        }
        if !self.suspend.active {
            return Some(ev);
        }
        match ev {
            Event::Host(HostEvent::Quit)
            | Event::Vm(VmEvent::Signal)
            | Event::Vm(VmEvent::ControlStop) => {
                self.events
                    .record("stop", "the suspended session ended", &[]);
                self.exit = Some(0);
            }
            Event::Host(HostEvent::Dock(DockAction::Resume | DockAction::OpenAgain)) => {
                self.resume_from_dock()
            }
            Event::Vm(VmEvent::Window(_, reply) | VmEvent::Freeze(_, reply)) => {
                let _ = reply.send(Err(text::suspend_while_quitting()));
            }
            Event::Vm(VmEvent::Suspend(_, reply)) => {
                let _ = reply.send(Err(text::suspend_while_quitting()));
            }
            Event::Vm(VmEvent::Grant(super::run_grants::GrantMsg::Line(_, reply, _))) => {
                let _ = reply.send(Err(text::suspend_while_quitting()));
            }
            _ => {}
        }
        None
    }

    /// A click on the Dock icon of a suspended session: a new start of this
    /// instance, which ends this shim and restores the saved state.
    fn resume_from_dock(&mut self) {
        if self.suspend.resuming {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => return say(&text::resume_spawn_failed(&e.to_string())),
        };
        let n = super::instance::of_profile(&self.plan.profile.dir);
        let spawned = std::process::Command::new(exe)
            .args(["install", "--app", &self.plan.app, "--instance"])
            .arg(n.to_string())
            .arg("--resume-from-dock")
            .stdin(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(_) => self.suspend.resuming = true,
            Err(e) => say(&text::resume_spawn_failed(&e.to_string())),
        }
    }

    /// A window became key: the time goes into the record, at most once a
    /// second, for the choice of the least recently used session.
    pub(super) fn note_key(&mut self) {
        let now = crate::state::now();
        if now > self.suspend.key_written {
            self.suspend.key_written = now;
            self.update_record(|v| v["last_key"] = serde_json::json!(now));
        }
    }

    /// The busy state into the record, on a change.
    pub(super) fn write_busy(&mut self) {
        let busy = self.busy_now();
        if self.suspend.busy_written == Some(busy) {
            return;
        }
        let word = busy.map(|b| b.word());
        if self.update_record(|v| v["busy"] = serde_json::json!(word)) {
            self.suspend.busy_written = Some(busy);
        }
    }
}
