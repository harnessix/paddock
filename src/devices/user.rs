//! User devices: what `[devices.NAME]` tables become at run time.
//!
//! config.rs parses the tables. This file holds the parts that every user
//! device shares: the `Gate` (the first-request card, the session answer,
//! the ledger reserve and settle, the audit line), the file of a device
//! that this sandbox does not get (`Disabled`), and the file of a table
//! whose runtime is not in this paddock yet (`NotYet`). http.rs is the
//! loopback proxy; outproc.rs is the runtime behind `paddock device-serve`,
//! with the device sandbox of sandbox.rs (`run = "sandbox"`) and the host
//! instance of host.rs (`run = "host"`).
//!
//! A device is off until its table says `enabled = true`, and a sandbox
//! sees it only when a grant covers it (grant.rs). An off device is still
//! a file in `/dev/paddock`: it answers with a "disabled" status. The
//! first request of a sandbox puts a card on the host ("allow the device
//! `ollama` for this sandbox"). "Allow for this session" lets every later
//! request of this sandbox through without a card. `mode = "allow"` skips
//! the card, and the audit line says which grant allowed it.
//!
//! An `exec` or `serve` table with `run = "sandbox"` is an `OutOfProcess`
//! device (outproc.rs) whose program runs in a device sandbox (sandbox.rs);
//! `kind = "command"` of 0.2.1 is the same path.

use super::config::{DeviceDef, Mode, Run, Shape};
use super::grant::{GrantHit, Why};
use super::ledger::{self, Ledger, Reserve};
use super::{Call, DevError, Device, DeviceCtx, Info, Limits};
use crate::broker::{self, Answer};
use anyhow::Result;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
mod asks;
mod inactive;
pub use inactive::{Disabled, NotYet};

/// The name of 0.2.1, for one release.
pub use super::config::DeviceDef as UserDev;
pub use super::config::{
    check_name, parse_size, DEFAULT_MAX_BODY, DEFAULT_TIMEOUT, MAX_BODY_CAP, MAX_TIMEOUT,
};
#[cfg(test)]
pub(crate) use super::http::{complete, dechunk, Upstream};
pub use super::http::{parse_http, refusal, strip_headers, Http, HttpReq, HEAD_ROOM};

/// The longest wait for the user's answer on the card.
pub const APPROVAL_WAIT: u64 = 120;

/// The limits for the catalog and the file system.
pub fn limits(def: &DeviceDef) -> Limits {
    super::http::catalog_limits(def)
}

/// What the catalog shows about the configuration.
pub fn details(def: &DeviceDef) -> serde_json::Value {
    let mut v = serde_json::json!({
        "kind": def.kind_name(),
        "shape": def.shape.name(),
        "run": def.run.name(),
        "scope": def.scope.name(),
        "stream": def.stream,
        "timeout_seconds": def.timeout(),
    });
    match &def.http {
        Some(p) => {
            v["target"] = p.target.to_string().into();
            v["methods"] = serde_json::json!(p.methods);
            v["paths"] = serde_json::json!(p.paths);
        }
        None if matches!(def.run, Run::Remote { .. }) => {
            if let Run::Remote { target, device } = &def.run {
                v["target"] = target.clone().into();
                v["device"] = device.clone().into();
            }
        }
        None => {
            v["command"] = serde_json::json!(def.command);
            v["network"] = (def.profile().network != super::config::Net::None).into();
            // The program runs on this host (in a device sandbox or as the
            // user), so it builds on the tools of this host's system: an
            // agent in the Linux guest of the VM backend sees `macos`.
            if matches!(def.run, Run::Sandbox | Run::Host { .. }) {
                v["platform"] = std::env::consts::OS.into();
            }
        }
    }
    v
}

/// One line for the approval card.
pub fn describe(def: &DeviceDef) -> String {
    let reach = def.profile().reach_lines();
    match &def.http {
        Some(p) => format!(
            "HTTP to {}: methods {}, paths {}",
            p.target,
            p.methods.join(", "),
            p.paths.join(", ")
        ),
        None => {
            let mut s = match &def.run {
                Run::Host { .. } => super::host::describe(def),
                Run::Remote { .. } => super::forward::describe(def),
                _ => format!(
                    "the command `{}` on the host, the request on its standard input{}",
                    def.command.join(" "),
                    if def.profile().network == super::config::Net::None {
                        ", no network"
                    } else {
                        ", with network"
                    }
                ),
            };
            for line in reach {
                s.push_str("; ");
                s.push_str(&line);
            }
            s
        }
    }
}

/// What `.catalog` says about an `exec` device.
pub fn exec_info(def: &DeviceDef) -> Info {
    use super::supervise::exec::STDERR_LINES;
    let network = def.profile().network != super::config::Net::None;
    Info {
        request: format!(
            "bytes for the standard input of the command `{}` in its device sandbox (at most {} bytes). The request never adds arguments. The command runs isolated, {}, for at most {} seconds.",
            def.command.join(" "),
            def.limits.max_request,
            if network { "with network" } else { "without network" },
            def.timeout()
        ),
        response: if def.stream {
            format!(
                "the standard output as the command writes it (at most {} bytes), read in order to the end of file. A non-zero exit after the first byte fails the last read with EIO. EACCES: the user declined the device. EDQUOT: the budget of a paid device is used.",
                def.limits.max_response
            )
        } else {
            format!(
                "header lines, an empty line, then the standard output (at most {} bytes). Header lines: status: exit CODE, signal NUMBER, timeout, error or disabled; stderr: LINE (the first {STDERR_LINES} lines of the standard error); message: TEXT. EACCES: the user declined the device. EDQUOT: the budget of a paid device is used.",
                def.limits.max_response
            )
        },
        limits: limits(def),
        shape: "exec",
        stream: def.stream,
    }
}

/// What `.catalog` says about the device.
pub fn info(def: &DeviceDef) -> Info {
    if let Run::Host { .. } = def.run {
        return super::host::info(def);
    }
    if let Run::Remote { .. } = def.run {
        return super::forward::info(def);
    }
    match (&def.http, def.shape) {
        (Some(p), _) => super::http::info(def, p),
        (None, Shape::Exec) => exec_info(def),
        (None, Shape::Serve | Shape::HttpProxy) => Info {
            request: format!(
                "bytes for the service `{}` (at most {} bytes)",
                def.command.join(" "),
                def.limits.max_request
            ),
            response: format!(
                "the bytes of its answer (at most {} bytes)",
                def.limits.max_response
            ),
            limits: limits(def),
            shape: def.shape.name(),
            stream: def.stream,
        },
    }
}

impl DeviceDef {
    /// The device to serve: the device itself with the grant that gave it,
    /// or the file of one that this sandbox does not get.
    pub fn build(&self, ctx: &DeviceCtx, state: Result<GrantHit, Why>) -> Arc<dyn Device> {
        let hit = match state {
            Ok(h) => h,
            Err(why) => return Arc::new(Disabled(self.clone(), why)),
        };
        if let Some(reason) = self.not_yet() {
            return Arc::new(NotYet(self.clone(), reason));
        }
        match (self.shape, &self.run) {
            (Shape::HttpProxy, _) => Arc::new(Http::new(self.clone(), ctx.clone(), hit)),
            (Shape::Exec | Shape::Serve, Run::Sandbox | Run::Host { .. }) => {
                let source = super::outproc::source_for(self, ctx)
                    .expect("a sandbox or host table has a source");
                Arc::new(super::outproc::OutOfProcess::new(
                    self.clone(),
                    ctx.clone(),
                    source,
                    hit,
                ))
            }
            (_, Run::Remote { .. }) => {
                Arc::new(super::forward::Forward::new(self.clone(), ctx.clone(), hit))
            }
            (_, run) => Arc::new(NotYet(
                self.clone(),
                format!(
                    "[devices.{}]: run = \"{}\" is not available in this paddock yet",
                    self.name,
                    run.name()
                ),
            )),
        }
    }
}

/// The approval of a device for one sandbox: one card at the first
/// request, and the session answer for the rest.
pub struct Gate {
    ctx: DeviceCtx,
    name: String,
    /// The first line of the card.
    summary: String,
    detail: String,
    run: &'static str,
    /// Never `Allow` for a host device: the card shows every session.
    mode: Mode,
    /// How this sandbox got the device.
    grant: GrantHit,
    /// The session answer: allow (true) or deny (false) without a card.
    session: Mutex<Option<bool>>,
    /// The cost of a paid device: declared cost, cost header, budget.
    paid: Option<(u64, Option<String>, Option<u64>)>,
    /// The peer of a remote caller (forward.rs): the session answer then
    /// lives in the session file too, so a new process per request
    /// (the ssh transport) does not show the card again.
    remote_peer: Option<String>,
}

/// The ledger row of a request that a budget allowed.
pub struct Charge(Option<i64>);

impl Gate {
    /// The gate of `def`, with the run mode of the table: a host device,
    /// and a remote device over ssh, get a summary that says so and never
    /// `mode = "allow"`.
    pub fn new(ctx: DeviceCtx, def: &DeviceDef, grant: GrantHit) -> Self {
        let (summary, host) = match &def.run {
            Run::Host { .. } => (
                format!(
                    "allow the device `{}` for this sandbox: it runs ON THE HOST as you",
                    def.name
                ),
                true,
            ),
            Run::Remote { .. } => (
                super::forward::summary(def),
                super::forward::target_of(def).is_some_and(|(t, _)| t.host_action()),
            ),
            _ => (
                format!("allow the device `{}` for this sandbox", def.name),
                false,
            ),
        };
        let remote_peer = match &grant {
            GrantHit::Remote(p) => Some(p.clone()),
            _ => None,
        };
        Self {
            ctx,
            name: def.name.clone(),
            summary,
            detail: describe(def),
            run: def.run.name(),
            mode: if host { Mode::Ask } else { def.mode },
            grant,
            session: Mutex::new(None),
            paid: def
                .paid()
                .then(|| (def.cost, def.cost_header.clone(), def.budget)),
            remote_peer,
        }
    }

    /// The card of a request that another paddock forwarded: it names
    /// the peer and what the caller said about itself, and says that the
    /// text is the other side's word.
    fn remote_card(&self, call: &Call) -> Option<(String, String)> {
        let peer = self.remote_peer.as_deref()?;
        let who = call
            .remote
            .as_ref()
            .map(|r| r.text())
            .unwrap_or_else(|| "an unnamed sandbox".into());
        Some((
            format!(
                "allow the device `{}` for a remote caller from {peer} ({who})",
                self.name
            ),
            format!(
                "{}; the request comes from the paddock on {peer} over the connection you allowed, and \"{who}\" is what that paddock says about its caller, not something this machine can check",
                self.detail
            ),
        ))
    }

    /// The session answer of a remote caller from the session file, when
    /// this process has none yet.
    fn stored_session(&self) -> Option<bool> {
        let peer = self.remote_peer.as_deref()?;
        super::forward::session::load(&self.ctx.env, peer, &self.name)
    }

    fn store_session(&self, allow: bool) {
        if let Some(peer) = &self.remote_peer {
            super::forward::session::save(&self.ctx.env, peer, &self.name, allow);
        }
    }

    /// The sandbox whose budget this sandbox spends: the root of its tree.
    fn root(&self) -> &str {
        ledger::root_of(&self.ctx.sandbox, &self.ctx.started_by)
    }

    fn ledger(&self) -> Result<Ledger, DevError> {
        Ledger::open(&ledger::path(&self.ctx.env)).map_err(|e| {
            log::warn!("device {}: {e:#}", self.name);
            DevError::Failed
        })
    }

    /// The budget card of a paid device without a budget: the user picks
    /// the budget of the run, or denies. None when no card is needed.
    fn budget_card(&self, wait: Duration) -> Option<Result<(), DevError>> {
        let (cost, header, None) = self.paid.as_ref()? else {
            return None;
        };
        let l = match self.ledger() {
            Ok(l) => l,
            Err(e) => return Some(Err(e)),
        };
        if matches!(l.budget(self.root(), &self.name), Ok(Some(_))) {
            return None;
        }
        let now = crate::state::now();
        let mut cost_text = format!("Each request costs {}", ledger::money(*cost));
        if let Some(h) = header {
            cost_text.push_str(&format!(" or the {h} header of the response, the larger"));
        }
        let p = broker::Pending {
            id: broker::new_request_id(),
            sandbox: self.ctx.sandbox.clone(),
            harness: self.ctx.harness.clone(),
            project: self.ctx.project.clone(),
            capability: broker::BUDGET_CAPABILITY.into(),
            summary: format!("allow the paid device `{}` with a budget", self.name),
            detail: format!(
                "{}. {cost_text}. Pick the budget for this run: {}, or deny.",
                self.detail,
                ledger::CARD_CHOICES.join(", ")
            ),
            reason: String::new(),
            created: now,
            expires: now + wait.as_secs(),
            session_allowed: false,
            started_by: self.ctx.started_by.clone(),
            choices: ledger::CARD_CHOICES.iter().map(|c| c.to_string()).collect(),
            connect: None,
        };
        let got = self
            .ctx
            .approver
            .choose(&self.ctx.env, &p, wait)
            .and_then(|c| ledger::parse_amount(&c).ok());
        Some(match got {
            Some(b) if l.init_budget(self.root(), &self.name, b).is_ok() => Ok(()),
            _ => Err(DevError::Denied),
        })
    }

    /// Check the budget of a paid device and reserve the declared cost of
    /// one request. A device without a cost passes with no ledger row.
    pub fn reserve(&self) -> Result<Charge, DevError> {
        let Some((cost, _, config)) = &self.paid else {
            return Ok(Charge(None));
        };
        let mut l = self.ledger()?;
        let root = self.root().to_string();
        if let Some(b) = config {
            l.init_budget(&root, &self.name, *b)
                .map_err(|_| DevError::Failed)?;
        }
        let project = self.ctx.project.display().to_string();
        let payer = ledger::Payer {
            sandbox: &self.ctx.sandbox,
            root: &root,
            project: &project,
            device: &self.name,
        };
        match l.reserve(&payer, *cost) {
            Ok(Reserve::Ok(id)) => Ok(Charge(Some(id))),
            Ok(Reserve::NoBudget) => Err(DevError::Denied),
            Ok(Reserve::Over {
                spent,
                budget,
                notify,
            }) => {
                if notify {
                    self.notify_used(&root, spent, budget);
                }
                Err(DevError::BudgetUsed)
            }
            Err(e) => {
                log::warn!("device {}: {e:#}", self.name);
                Err(DevError::Failed)
            }
        }
    }

    /// Write the final cost of a request: the larger of the declared cost
    /// and the cost header of `response`. `ran` is false when the request
    /// never reached the service: it costs nothing.
    pub fn settle(&self, charge: Charge, response: Option<&[u8]>, ran: bool, status: &str) {
        let reported = match (&self.paid, response) {
            (Some((_, Some(h), _)), Some(r)) => ledger::header_cost(r, h),
            _ => None,
        };
        self.settle_cost(charge, reported, ran, status);
    }

    /// `settle` with the cost that the program reported (`Paddock-Cost`),
    /// in micro units.
    pub fn settle_cost(&self, charge: Charge, reported: Option<u64>, ran: bool, status: &str) {
        let (Charge(Some(id)), Some((cost, _, _))) = (charge, &self.paid) else {
            return;
        };
        let total = if ran {
            ledger::charge(*cost, reported)
        } else {
            0
        };
        if let Err(e) = self.ledger().map(|l| l.settle(id, total, status)) {
            log::warn!("device {}: cannot write the ledger: {e:?}", self.name);
        }
    }

    /// One notification when a request would pass the budget.
    fn notify_used(&self, root: &str, spent: u64, budget: u64) {
        let note = broker::notify::Note {
            what: "budget".into(),
            action: None,
            summary: format!("paddock: budget used on {}", self.name),
            body: format!(
                "{root} spent {} of {} on {}. Raise it with: paddock device budget {root} {} --add 1.0",
                ledger::money(spent),
                ledger::money(budget),
                self.name,
                self.name
            ),
            actions: Vec::new(),
            critical: false,
        };
        if let Err(e) = broker::notify::show(&self.ctx.env, &note) {
            log::debug!("cannot show the budget notification: {e:#}");
        }
    }

    /// What `.catalog` shows of the grant, the cost and the budget left for
    /// this run.
    pub fn details(&self, v: &mut serde_json::Value) {
        v["grant"] = self.grant.to_string().into();
        let Some((cost, header, config)) = &self.paid else {
            return;
        };
        v["cost"] = ledger::money(*cost).into();
        if let Some(h) = header {
            v["cost_header"] = h.clone().into();
        }
        let file = ledger::path(&self.ctx.env);
        let known = file
            .is_file()
            .then(|| Ledger::open(&file).ok())
            .flatten()
            .and_then(|l| {
                let b = l.budget(self.root(), &self.name).ok().flatten();
                Some((b, l.spent(self.root(), &self.name).ok()?))
            });
        let (budget, spent) = match known {
            Some((Some(b), s)) => (Some(b), s),
            _ => (*config, 0),
        };
        v["budget_left"] = match budget {
            Some(b) => ledger::money(b.saturating_sub(spent)).into(),
            None => "no budget yet: the first request asks the user for one".into(),
        };
    }

    /// Ask the user when no session answer exists. Requests that come
    /// during the card wait for the same answer. The decision text is for
    /// the audit log.
    pub fn check(&self, call: &Call) -> Result<Cow<'static, str>, DevError> {
        let mut s = self.session.lock().unwrap();
        if s.is_none() {
            *s = self.stored_session();
        }
        match *s {
            Some(true) => return Ok("allowed for the session".into()),
            Some(false) => return Err(DevError::Denied),
            None => {}
        }
        let left = call.deadline.saturating_duration_since(Instant::now());
        let wait = left
            .saturating_sub(Duration::from_secs(1))
            .min(Duration::from_secs(APPROVAL_WAIT));
        if let Some(r) = self.budget_card(wait) {
            // The budget is for the run: the device needs no other card.
            if r.is_ok() {
                *s = Some(true);
            }
            return r.map(|()| "allowed with a budget".into());
        }
        if self.mode == Mode::Allow {
            *s = Some(true);
            return Ok(format!("allowed by grant {}", self.grant).into());
        }
        let now = crate::state::now();
        let (summary, detail) = self
            .remote_card(call)
            .unwrap_or_else(|| (self.summary.clone(), self.detail.clone()));
        let p = broker::Pending {
            id: broker::new_request_id(),
            sandbox: self.ctx.sandbox.clone(),
            harness: self.ctx.harness.clone(),
            project: self.ctx.project.clone(),
            capability: "device".into(),
            summary,
            detail,
            reason: String::new(),
            created: now,
            expires: now + wait.as_secs(),
            session_allowed: true,
            started_by: self.ctx.started_by.clone(),
            choices: Vec::new(),
            connect: None,
        };
        match self.ctx.approver.ask(&self.ctx.env, &p, wait) {
            Some(Answer::Session) => {
                *s = Some(true);
                self.store_session(true);
                Ok("allowed for the session".into())
            }
            Some(Answer::Once) => Ok("allowed once".into()),
            Some(Answer::DenySession) => {
                *s = Some(false);
                self.store_session(false);
                Err(DevError::Denied)
            }
            Some(Answer::Deny) | None => Err(DevError::Denied),
        }
    }

    /// One audit line per request: never a body.
    pub fn audit(&self, detail: &str, decision: &str, status: &str, start: Instant) {
        self.audit_with(detail, decision, status, start, serde_json::Map::new());
    }

    /// `audit` with more fields (`shape`, `instance`, `http`).
    pub fn audit_with(
        &self,
        detail: &str,
        decision: &str,
        status: &str,
        start: Instant,
        extra: serde_json::Map<String, serde_json::Value>,
    ) {
        let mut line = serde_json::json!({
            "time": crate::state::now(),
            "sandbox": self.ctx.sandbox,
            "harness": self.ctx.harness,
            "project": self.ctx.project,
            "capability": "device",
            "device": self.name,
            "run": self.run,
            "grant": self.grant.to_string(),
            "detail": detail,
            "decision": decision,
            "status": status,
            "duration_ms": start.elapsed().as_millis() as u64,
        });
        if let Some(p) = self.ctx.started_by.last() {
            line["parent"] = p.clone().into();
            line["chain"] = serde_json::json!(self.ctx.started_by);
        }
        for (k, v) in extra {
            line[k] = v;
        }
        broker::audit_line(&self.ctx.env, &line);
    }
}

#[cfg(test)]
#[path = "user_tests.rs"]
pub(crate) mod tests;
