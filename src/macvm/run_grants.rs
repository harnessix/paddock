//! The live grants of a session in the shim, macOS only: a `grant` line
//! of the control socket is planned by `grants_live::Live` (pure, with
//! Linux tests). A change of the shares goes to the framework on the
//! queue of the VM (a new `VZMultipleDirectoryShare` on the running
//! `VZVirtioFileSystemDevice`), and the session takes the new state only
//! when the framework did. The network mode goes to the firewall, the
//! clipboard directions to the clipboard machine, a rule of the session
//! (`grant rule ...`) to the firewall. While a workspace
//! folder is shared, the shim asks the guest for the open file handles
//! every few seconds and writes them into the session record.

use super::grants_live::{Op, Planned, HANDLES_EVERY};
use super::proto::{ErrorCode, Outcome, ReplyData, Request};
use super::run::{Session, Why};
use super::run_vm::VmEvent;
use super::text;
use crate::vm::helper::ShareArg;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// How long the control socket waits for a `grant` answer.
pub const GRANT_TIMEOUT: Duration = Duration::from_secs(15);

/// A message of the grants to the loop of the session.
pub enum GrantMsg {
    /// A `grant` line and where its answer goes.
    Line(String, Sender<Result<String, String>>, Instant),
    /// The framework took the share change, or why not.
    Done(Result<(), String>),
    /// The previous shares were restored after an abandoned request.
    Reverted(Result<(), String>),
}

/// A share change that waits for the framework.
pub struct Pending {
    op: Op,
    planned: Planned,
    reply: Sender<Result<String, String>>,
    deadline: Instant,
}

fn now_secs() -> u64 {
    crate::state::now()
}

impl Session<'_> {
    pub(super) fn on_grant(&mut self, m: GrantMsg) {
        match m {
            GrantMsg::Line(line, reply, deadline) => self.grant_line(&line, reply, deadline),
            GrantMsg::Done(r) => self.grant_done(r),
            GrantMsg::Reverted(r) => {
                self.files.grant_pending = None;
                if let Err(e) = r {
                    let line = text::grant_revert_failed(&e);
                    log::error!("macvm: {line}");
                    self.events.record("error", &line, &[]);
                }
            }
        }
    }

    fn grant_line(&mut self, line: &str, reply: Sender<Result<String, String>>, deadline: Instant) {
        if Instant::now() >= deadline {
            return;
        }
        let op = match Op::parse(line) {
            Ok(op) => op,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        if self.quitting {
            let _ = reply.send(Err(text::grant_quitting()));
            return;
        }
        // A rule of the session goes to the firewall, not to the VM.
        if let Some(r) = self.firewall.rule_op(&op) {
            let _ =
                reply.send(r.and_then(|a| serde_json::to_string(&a).map_err(|e| e.to_string())));
            return;
        }
        if self.files.grant_pending.is_some() || self.files.fresh_handles.is_some() {
            let _ = reply.send(Err(text::grant_busy()));
            return;
        }
        if matches!(op, Op::ListFresh) {
            if self.files.fresh_handles.is_some() {
                let _ = reply.send(Err(text::grant_busy()));
                return;
            }
            self.files.grants.handles = None;
            if self.agent.is_none()
                || self.files.handles_off
                || self.stamp.agent != super::version::paddock_version()
                || !self.send(Request::SharesOpen, Why::SharesOpenFresh)
            {
                self.grant_commit(
                    Op::List,
                    self.files
                        .grants
                        .plan(&Op::List, now_secs())
                        .expect("list is valid"),
                    reply,
                    deadline,
                );
            } else {
                self.files.fresh_handles = Some((reply, deadline));
            }
            return;
        }
        let env = match crate::env::HostEnv::detect() {
            Ok(env) => env,
            Err(e) => {
                let _ = reply.send(Err(format!("{e:#}")));
                return;
            }
        };
        let planned = match self.files.grants.plan_checked(
            &op,
            now_secs(),
            &env.home,
            &super::grants::paddock_dirs(&env),
        ) {
            Ok(p) => p,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        let Some(set) = planned.vm_shares.clone() else {
            self.grant_commit(op, planned, reply, deadline);
            return;
        };
        if Instant::now() >= deadline {
            return;
        }
        let args: Vec<ShareArg> = set
            .iter()
            .map(|s| ShareArg {
                name: s.name.clone(),
                path: s.host.clone(),
                read_only: s.read_only,
            })
            .collect();
        let vm = self.vm.clone();
        let tx = self.vtx.clone();
        self.queue.exec_async(move || {
            let r =
                super::vz_mac_config::replace_shares(&vm.0, &args).map_err(|e| format!("{e:#}"));
            let _ = tx.send(VmEvent::Grant(GrantMsg::Done(r)));
        });
        self.files.grant_pending = Some(Pending {
            op,
            planned,
            reply,
            deadline,
        });
    }

    fn grant_done(&mut self, r: Result<(), String>) {
        let Some(p) = self.files.grant_pending.take() else {
            return;
        };
        match r {
            Ok(()) => {
                let answer = serde_json::to_string(&p.planned.answer).map_err(|e| e.to_string());
                let args: Vec<ShareArg> = self
                    .files
                    .grants
                    .shares
                    .iter()
                    .map(|s| ShareArg {
                        name: s.name.clone(),
                        path: s.host.clone(),
                        read_only: s.read_only,
                    })
                    .collect();
                let vm = self.vm.clone();
                let tx = self.vtx.clone();
                let accepted = super::grants_live::deliver_before_deadline(
                    &p.reply,
                    p.deadline,
                    answer,
                    || {
                        self.queue.exec_async(move || {
                            let result = super::vz_mac_config::replace_shares(&vm.0, &args)
                                .map_err(|e| format!("{e:#}"));
                            let _ = tx.send(VmEvent::Grant(GrantMsg::Reverted(result)));
                        });
                    },
                );
                if accepted {
                    self.grant_apply(p.op, p.planned);
                } else {
                    self.files.grant_pending = Some(p);
                }
            }
            Err(e) => {
                let line = text::grant_vm_refused(&e);
                super::run_vm::say(&line);
                self.events.record("error", &line, &[]);
                let _ = p.reply.send(Err(line));
            }
        }
    }

    fn grant_commit(
        &mut self,
        op: Op,
        planned: Planned,
        reply: Sender<Result<String, String>>,
        deadline: Instant,
    ) {
        if Instant::now() >= deadline {
            return;
        }
        let answer = self.grant_apply(op, planned);
        let _ = reply.send(serde_json::to_string(&answer).map_err(|e| e.to_string()));
    }

    fn grant_apply(&mut self, op: Op, planned: Planned) -> super::grants_live::Answer {
        let mut answer = planned.answer;
        match &op {
            Op::Network { mode } => {
                let m = super::gate::startup_mode(mode);
                answer.closed = Some(self.firewall.set_mode(m));
                let word = mode.clone();
                self.update_record(|v| v["network_mode"] = serde_json::json!(word));
            }
            Op::Clipboard(c) => {
                let actions = self.files.sync.set_directions(c.to_app, c.from_app);
                self.act_clipboard(actions);
            }
            _ => {}
        }
        let shares_changed = planned.vm_shares.is_some();
        self.files.grants = planned.live;
        if shares_changed {
            let shares = self.files.grants.shares.clone();
            self.files.set_shares(shares);
            self.write_grant_shares();
            // A fresh count soon after the change.
            self.files.handles_asked = None;
        }
        if !matches!(op, Op::List | Op::ListFresh) {
            log::info!("macvm: {}", op.line());
        }
        answer
    }

    /// The workspace folders with their counts into the session record.
    fn write_grant_shares(&mut self) {
        let shares = self.files.grants.record(now_secs());
        let value = serde_json::to_value(&shares).unwrap_or_default();
        self.update_record(|v| v["shares"] = value);
    }

    /// Ask the guest for the open file handles, every [`HANDLES_EVERY`],
    /// while a workspace folder is shared and the agent is of this paddock
    /// (an older agent does not know `shares.open`).
    pub(super) fn grants_tick(&mut self, now: Instant) {
        if self
            .files
            .fresh_handles
            .as_ref()
            .is_some_and(|(_, deadline)| now + Duration::from_secs(1) >= *deadline)
        {
            // A late reply must not satisfy a later refresh request.
            self.why.retain(|_, why| *why != Why::SharesOpenFresh);
            self.fresh_shares_answer(Outcome::Error {
                code: ErrorCode::Timeout,
                text: "open file count timed out".into(),
            });
        }
        if self.files.handles_off
            || self.agent.is_none()
            || !self.files.grants.has_workspace()
            || self.stamp.agent != super::version::paddock_version()
            || self.why.values().any(|w| *w == Why::SharesOpen)
            || self
                .files
                .handles_asked
                .is_some_and(|t| now.duration_since(t) < HANDLES_EVERY)
        {
            return;
        }
        self.files.handles_asked = Some(now);
        self.send(Request::SharesOpen, Why::SharesOpen);
    }

    pub(super) fn fresh_shares_answer(&mut self, outcome: Outcome) {
        let Some((reply, deadline)) = self.files.fresh_handles.take() else {
            return;
        };
        self.files.grants.handles = None;
        self.shares_open_answer(outcome);
        if let Ok(plan) = self.files.grants.plan(&Op::List, now_secs()) {
            self.grant_commit(Op::List, plan, reply, deadline);
        }
    }

    pub(super) fn shares_open_answer(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Ok {
                data: Some(ReplyData::OpenFiles { per_share }),
            } => {
                self.files.grants.on_handles(per_share, now_secs());
                self.write_grant_shares();
            }
            Outcome::Error {
                code: ErrorCode::Unsupported | ErrorCode::BadRequest,
                text: detail,
            } => {
                log::warn!("macvm: {}", text::open_files_off(&detail));
                self.files.handles_off = true;
            }
            other => log::debug!("macvm: shares.open: {other:?}"),
        }
    }
}
