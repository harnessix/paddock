//! `OutOfProcess`: a device whose program runs behind a socket that speaks
//! the device-facing contract (program.rs), started by an `InstanceSource`.
//!
//! The source is what differs by run mode (`source_for`): sandbox.rs gives
//! the device sandbox instance (`run = "sandbox"`), host.rs the host
//! instance (`run = "host"`), and the tests a supervisor in a temporary
//! folder with no isolation. Everything else is here: the gate
//! (the card, the session answer), the ledger reserve and settle, the
//! `concurrency` slots with a bounded wait, the health check after a
//! failure, the catalog merge and the audit line with `run`, `shape`,
//! `instance` and the HTTP status. Never a body.

use super::config::{DeviceDef, Run, Scope};
use super::grant::GrantHit;
use super::program::{Caller, Collect, Failure, Outcome, Program};
use super::user::Gate;
use super::{Call, DevError, Device, DeviceCtx, Info, Sink};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Where the instance of a device comes from.
pub trait InstanceSource: Send + Sync {
    /// The socket of a running instance; start one when needed. The error
    /// is for the log and `how_to_fix`.
    fn socket(&self, ctx: &DeviceCtx) -> Result<PathBuf, String>;
    /// The instance for the audit line and the catalog (`test`, a sandbox
    /// id, `host`).
    fn instance(&self) -> String;
    /// The log of the instance, for `how_to_fix`.
    fn log_path(&self) -> Option<PathBuf> {
        None
    }
    /// A number that changes when the instance was stopped or restarted
    /// from outside (`paddock device stop|restart`): the device then
    /// forgets its connection and starts again at the next request.
    fn epoch(&self) -> u64 {
        0
    }
    /// Why the instance is failed for good (its sandbox died three times
    /// within a minute), or None while a start can still help.
    fn failed(&self) -> Option<String> {
        None
    }
    /// What the user does about a failed instance: the log command and the
    /// restart command.
    fn fix_hint(&self) -> String {
        match self.log_path() {
            Some(p) => format!(
                "Its log is {}. Fix the program and restart the sandbox",
                p.display()
            ),
            None => "Fix the program and restart the sandbox".into(),
        }
    }
}

/// The source of `def` by its `run`, from the registry of the run
/// (`ctx.instances`): the device sandbox instance for `run = "sandbox"`,
/// the host instance for `run = "host"`. None for a table that no
/// supervisor serves (the http proxy, `run = "remote"`).
pub fn source_for(def: &DeviceDef, ctx: &DeviceCtx) -> Option<Arc<dyn InstanceSource>> {
    match (&def.run, def.scope) {
        // One instance per user, behind `paddock deviced` (shared.rs).
        (Run::Sandbox, Scope::Shared) => Some(ctx.instances.shared_source(def, ctx)),
        (Run::Sandbox, Scope::Run) => Some(ctx.instances.source(def, ctx)),
        (Run::Host { .. }, _) => Some(ctx.instances.host_source(def, ctx)),
        (Run::Remote { .. } | Run::Broker, _) => None,
    }
}

/// Why an instance is `failed` after the supervisor gave up on restarts.
const FAILED_WHY: &str = "the device program exited three times within a minute";
/// The status of a shared instance that reads another table than this run
/// does (devices/shared.rs).
pub const RECONFIGURED: &str = "reconfigured";

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Stopped,
    Running,
    Failed(String),
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            State::Stopped => "stopped",
            State::Running => "running",
            State::Failed(_) => "failed",
        }
    }
}

struct Inst {
    program: Option<Arc<Program>>,
    state: State,
    catalog: Option<serde_json::Value>,
    /// Check the health before the next request (after a failure).
    recheck: bool,
    /// `InstanceSource::epoch` at the last connect.
    epoch: u64,
}

/// `/dev/paddock/NAME` for a device program behind a socket.
pub struct OutOfProcess {
    def: DeviceDef,
    ctx: DeviceCtx,
    gate: Gate,
    source: Arc<dyn InstanceSource>,
    inst: Mutex<Inst>,
    slots: Mutex<u32>,
    cv: Condvar,
}

impl OutOfProcess {
    pub fn new(
        def: DeviceDef,
        ctx: DeviceCtx,
        source: Arc<dyn InstanceSource>,
        grant: GrantHit,
    ) -> Self {
        let gate = Gate::new(ctx.clone(), &def, grant);
        Self {
            def,
            ctx,
            gate,
            source,
            inst: Mutex::new(Inst {
                program: None,
                state: State::Stopped,
                catalog: None,
                recheck: false,
                epoch: 0,
            }),
            slots: Mutex::new(0),
            cv: Condvar::new(),
        }
    }

    /// A slot of `concurrency`, or None after `wait`.
    fn acquire(&self, wait: Duration) -> Option<Slot<'_>> {
        let max = self.def.limits.concurrency;
        let end = Instant::now() + wait;
        let mut g = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        while *g >= max {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *g += 1;
        Some(Slot(self))
    }

    /// The program of a running, healthy instance.
    fn ensure(&self, call: &Call) -> Result<Arc<Program>, Failure> {
        let mut g = self.inst.lock().unwrap_or_else(|e| e.into_inner());
        // Stopped or restarted from outside: forget the old connection and
        // the failed state.
        let epoch = self.source.epoch();
        if g.epoch != epoch {
            g.epoch = epoch;
            g.program = None;
            g.state = State::Stopped;
            g.recheck = false;
        }
        if let State::Failed(_) = g.state {
            return Err(Failure::new(DevError::Failed, "failed"));
        }
        let ready = Duration::from_secs(self.def.ready);
        let left = call.deadline.saturating_duration_since(Instant::now());
        if g.program.is_some() && g.recheck {
            let p = g.program.clone().expect("checked");
            match p.health(ready.min(left.max(Duration::from_secs(1)))) {
                Ok(()) => g.recheck = false,
                Err(e) if e.contains("(failed)") => {
                    log::warn!("device {}: {e}", self.def.name);
                    g.state = State::Failed(FAILED_WHY.to_string());
                    return Err(Failure::new(DevError::Failed, "failed"));
                }
                Err(e) => {
                    // The instance may be gone: ask the source again, which
                    // starts a new one or says `failed`.
                    log::warn!("device {}: {e}", self.def.name);
                    g.program = None;
                    g.recheck = false;
                    g.state = State::Stopped;
                }
            }
        }
        if g.program.is_none() {
            let sock = match self.source.socket(&self.ctx) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("device {}: cannot start the instance: {e}", self.def.name);
                    g.epoch = self.source.epoch();
                    g.state = match self.source.failed() {
                        Some(why) => State::Failed(why),
                        None => State::Stopped,
                    };
                    return Err(Failure::new(DevError::Failed, "cannot start"));
                }
            };
            let p = match Program::connect(&sock) {
                Ok(p) => p.with_caller(Caller::for_device(&self.def, &self.ctx)),
                Err(e) => {
                    log::warn!("device {}: {e:#}", self.def.name);
                    return Err(Failure::new(DevError::Failed, "cannot start"));
                }
            };
            if let Err(e) = p.health(ready.min(left.max(Duration::from_secs(1)))) {
                log::warn!("device {}: {e}", self.def.name);
                g.state = State::Failed(e);
                return Err(Failure::new(DevError::Failed, "unhealthy"));
            }
            g.catalog = p.catalog();
            g.program = Some(Arc::new(p));
            g.state = State::Running;
            g.recheck = false;
        }
        Ok(g.program.clone().expect("set above"))
    }

    /// After a failed call: the next call checks the health first, and a
    /// `failed` program marks the instance.
    fn note_failure(&self, f: &Failure) {
        let mut g = self.inst.lock().unwrap_or_else(|e| e.into_inner());
        if f.status == RECONFIGURED {
            // A shared instance serves the table it read at its start. The
            // user edited the table since: every request of this run fails
            // until the instance is restarted.
            log::warn!(
                "device {}: the table changed since the shared instance started",
                self.def.name
            );
            g.state = State::Failed(super::shared::RECONFIGURED_WHY.to_string());
        } else if f.status == "failed" {
            g.state = State::Failed(FAILED_WHY.to_string());
        } else if f.error == DevError::Failed {
            g.recheck = true;
        }
    }

    fn run(&self, call: &Call, request: &[u8], sink: &mut dyn Sink) -> Result<Outcome, Failure> {
        let p = self.ensure(call)?;
        match p.call(call, &self.def, request, sink) {
            Ok(o) => Ok(o),
            Err(f) => {
                self.note_failure(&f);
                Err(f)
            }
        }
    }

    /// One request: the gate, the budget, a `concurrency` slot, the
    /// program, the audit line. The response goes to `sink` as it comes
    /// from the program.
    fn call_into(&self, call: &Call, request: &[u8], sink: &mut dyn Sink) -> Result<(), DevError> {
        let start = Instant::now();
        if request.len() > self.def.limits.max_request {
            return Err(DevError::TooLarge);
        }
        let bytes_in = format!("{} bytes in", request.len());
        let decision = match self.gate.check(call) {
            Ok(d) => d,
            Err(e) => {
                self.gate
                    .audit_with(&bytes_in, "denied", "denied", start, self.extra(call, None));
                return Err(e);
            }
        };
        let charge = match self.gate.reserve() {
            Ok(c) => c,
            Err(e) => {
                let status = match e {
                    DevError::BudgetUsed => super::ledger::BUDGET_USED,
                    DevError::Denied => "no budget",
                    _ => "failed",
                };
                self.gate
                    .audit_with(&bytes_in, &decision, status, start, self.extra(call, None));
                return Err(e);
            }
        };
        let wait = call
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(self.def.timeout()));
        let Some(_slot) = self.acquire(wait) else {
            self.gate.settle_cost(charge, None, false, "busy");
            self.gate
                .audit_with(&bytes_in, &decision, "busy", start, self.extra(call, None));
            return Err(DevError::Busy);
        };
        match self.run(call, request, sink) {
            Ok(o) => {
                let detail = format!("{} bytes in, {} bytes out", request.len(), o.bytes);
                self.gate.settle_cost(charge, o.cost, true, &o.status);
                self.gate.audit_with(
                    &detail,
                    &decision,
                    &o.status,
                    start,
                    self.extra(call, Some(o.http)),
                );
                Ok(())
            }
            Err(f) => {
                let detail = match f.error {
                    DevError::TooLarge => format!("{} bytes in, too much output", request.len()),
                    _ => bytes_in,
                };
                // A request that never reached the program costs nothing,
                // and one that comes again (waiting, busy) is not done.
                let ran = !matches!(
                    f.status.as_str(),
                    "unreachable" | "cannot start" | "unhealthy" | "busy" | "waiting"
                );
                self.gate.settle_cost(charge, None, ran, &f.status);
                self.gate.audit_with(
                    &detail,
                    &decision,
                    &f.status,
                    start,
                    self.extra(call, f.http),
                );
                Err(f.error)
            }
        }
    }

    fn extra(&self, call: &Call, http: Option<u16>) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert("shape".into(), self.def.shape.name().into());
        m.insert("instance".into(), self.source.instance().into());
        if let Some(h) = http {
            m.insert("http".into(), h.into());
        }
        // A request that another paddock forwarded: its caller as text.
        super::forward::remote_fields(call, &mut m);
        m
    }
}

struct Slot<'a>(&'a OutOfProcess);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut g = self.0.slots.lock().unwrap_or_else(|e| e.into_inner());
        *g -= 1;
        self.0.cv.notify_one();
    }
}

impl Device for OutOfProcess {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn info(&self) -> Info {
        let mut i = super::user::info(&self.def);
        let g = self.inst.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = &g.catalog {
            if let Some(r) = c.get("request").and_then(|v| v.as_str()) {
                // The host mark stays in front of what the program says.
                let mark = match self.def.run {
                    Run::Host { .. } => super::host::HOST_MARK,
                    _ => "",
                };
                i.request = format!("{mark}{}", crate::broker::clean_text(r));
            }
            if let Some(r) = c.get("response").and_then(|v| v.as_str()) {
                i.response = crate::broker::clean_text(r);
            }
        }
        i
    }

    fn details(&self) -> Option<serde_json::Value> {
        let mut v = super::user::details(&self.def);
        self.gate.details(&mut v);
        let g = self.inst.lock().unwrap_or_else(|e| e.into_inner());
        v["instance"] = g.state.name().into();
        v["instance_name"] = self.source.instance().into();
        if let State::Failed(why) = &g.state {
            v["how_to_fix"] = format!(
                "the device program of {} is failed: {why}. {}",
                self.def.name,
                self.source.fix_hint()
            )
            .into();
        }
        if let Some(c) = &g.catalog {
            if let Some(e) = c.get("examples").filter(|e| e.is_array()) {
                v["examples"] = e.clone();
            }
            if let Some(cost) = c.get("cost").filter(|_| !self.def.paid()) {
                v["program_cost"] = cost.clone();
            }
        }
        Some(v)
    }

    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let mut sink = Collect::default();
        self.call_into(call, request, &mut sink)?;
        Ok(sink.0)
    }

    /// The program's response as it comes. With `stream = true` the
    /// supervisor sends the raw output chunk by chunk, and each chunk
    /// reaches `sink` at once; a non-zero exit after the first chunk is
    /// the error of the last read.
    fn call_stream(
        &self,
        call: &Call,
        request: &[u8],
        sink: &mut dyn Sink,
    ) -> Result<(), DevError> {
        self.call_into(call, request, sink)
    }
}

#[cfg(test)]
#[path = "outproc_tests.rs"]
mod tests;
