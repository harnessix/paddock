//! The registry requests of a horse-mode sandbox (`registry.fetch`,
//! `registry.sync`): the approval card, the audit log and the device
//! ledger of the broker, around `horse::device::registry`.

use super::{audit, Answer, Broker, Pending, Request, Response};
use crate::devices::ledger;
use crate::horse::device::registry::{self as reg, Host};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

struct BrokerHost<'a> {
    broker: &'a Broker,
    /// The card id and the decision, for the audit line.
    asked: RefCell<Option<(String, &'static str)>>,
}

impl Host for BrokerHost<'_> {
    fn state_dir(&self) -> PathBuf {
        self.broker.env.paddock_state()
    }

    fn ask(&self, summary: &str, detail: &str) -> reg::Answer {
        let b = self.broker;
        let now = crate::state::now();
        let p = Pending {
            id: super::new_request_id(),
            sandbox: b.ctx.id.clone(),
            harness: b.ctx.harness.clone(),
            project: b.ctx.project.clone(),
            capability: "horse".into(),
            summary: summary.into(),
            detail: detail.into(),
            reason: String::new(),
            created: now,
            expires: now + b.policy.wait,
            // Each fetch is its own card: a grant for the session would
            // let the sandbox pull any DNA without the user.
            session_allowed: false,
            started_by: b.ctx.chain.clone(),
            choices: Vec::new(),
        };
        let verdict = b
            .approver
            .ask_verdict(&b.env, &p, Duration::from_secs(b.policy.wait));
        let (answer, decision) = match verdict.map(|v| (v.answer, v.who)) {
            Some((Answer::Once | Answer::Session, "agent")) => {
                (reg::Answer::Approved, "allowed once by the agent approver")
            }
            Some((Answer::Once | Answer::Session, _)) => (reg::Answer::Approved, "allowed once"),
            Some((_, "agent")) => (reg::Answer::Denied, "denied by the agent approver"),
            Some(_) => (reg::Answer::Denied, "denied"),
            None => (reg::Answer::TimedOut(b.policy.wait), "timed out"),
        };
        *self.asked.borrow_mut() = Some((p.id, decision));
        answer
    }

    fn record(&self, line: &str) {
        let b = self.broker;
        let project = b.ctx.project.to_string_lossy();
        let payer = ledger::Payer {
            sandbox: &b.ctx.id,
            root: ledger::root_of(&b.ctx.id, &b.ctx.chain),
            project: &project,
            device: "horse",
        };
        if let Err(e) =
            ledger::Ledger::open(&ledger::path(&b.env)).and_then(|l| l.record(&payer, line))
        {
            log::warn!("horse ledger: {e:#}");
        }
    }
}

impl Broker {
    /// A `registry.fetch` or `registry.sync` request of this sandbox.
    pub(super) fn handle_registry(&self, req: &Request) -> Response {
        let host = BrokerHost {
            broker: self,
            asked: RefCell::new(None),
        };
        let resp = reg::handle(self.horse.get(), req, &host);
        // Only a request that reached the card is audited: the others
        // ran nothing and asked nobody.
        if let Some((id, decision)) = host.asked.into_inner() {
            let op = req.args.get("op").and_then(|v| v.as_str()).unwrap_or("");
            let subject = ["ref", "index"]
                .iter()
                .filter_map(|k| req.args.get(*k).and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join(" ");
            audit::audit(
                &self.env,
                &self.ctx,
                req,
                &format!("{op} {subject}"),
                Some(&id),
                decision,
                &resp,
                None,
            );
        }
        resp
    }
}
