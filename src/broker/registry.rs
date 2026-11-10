//! The registry requests of a horse-mode sandbox (`registry.fetch`,
//! `registry.sync`): the approval card, the audit log and the device
//! ledger of the broker, around `horse::device::registry`.

use super::{audit, Answer, Broker, Pending, Request, Response};
use crate::devices::ledger;
use crate::horse::device::registry::{self as reg, Host};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

pub(super) struct BrokerHost<'a> {
    pub(super) broker: &'a Broker,
    /// The card id and the decision, for the audit line.
    pub(super) asked: RefCell<Option<(String, &'static str)>>,
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
    /// Trainers have no host capabilities except the scoped peek reader.
    pub(super) fn handle_trainer(&self, req: &Request) -> Option<Response> {
        let Some(access) = self.horse.get() else {
            return (req.cap == "train")
                .then(|| Response::new(super::Status::Disabled, "this run has no horse session"));
        };
        let trainer = crate::horse::device::peek::grant::has_trainer(&access.store, &self.ctx.id)
            .unwrap_or(true);
        if trainer
            && !(req.cap == "horse" && req.args.get("op").and_then(|v| v.as_str()) == Some("peek"))
        {
            return Some(Response::new(
                super::Status::Denied,
                "trainer sandboxes can only request peek reads",
            ));
        }
        if req.cap == "train" {
            return Some(self.request_trainer(req, access));
        }
        if req.cap == "done"
            && req
                .args
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("ok")
                == "ok"
        {
            let text = req.args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            match crate::trainer::completion::ready(&self.env, &self.ctx.id, text) {
                Ok(true) => {},
                Ok(false) => return Some(Response::new(super::Status::Waiting, "a trainer is reviewing this final report; wait for advice and then submit done again")),
                Err(error) => return Some(Response::new(super::Status::Failed, format!("trainer review: {error:#}"))),
            }
        }
        None
    }

    fn request_trainer(&self, req: &Request, access: &crate::horse::device::Access) -> Response {
        use crate::trainer::triggers::{Config, RequestReservation};
        if req.v != super::PROTOCOL {
            return Response::new(super::Status::Invalid, "unsupported protocol version");
        }
        if req.args.get("op").and_then(|v| v.as_str()) == Some("feedback") {
            let Some(id) = req.args.get("advice").and_then(|v| v.as_str()) else {
                return Response::new(super::Status::Invalid, "feedback needs an advice id");
            };
            let Some(adopted) = req.args.get("adopted").and_then(|v| v.as_bool()) else {
                return Response::new(
                    super::Status::Invalid,
                    "feedback needs an adoption decision",
                );
            };
            let note = req.args.get("note").and_then(|v| v.as_str()).unwrap_or("");
            return match crate::trainer::learning::feedback(
                &access.store,
                &self.ctx.id,
                id,
                adopted,
                note,
            ) {
                Ok(()) => Response::done(serde_json::json!({"recorded": true})),
                Err(error) => Response::new(super::Status::Failed, format!("{error:#}")),
            };
        }
        let Some(id) = req.args.get("request").and_then(|v| v.as_u64()) else {
            return Response::new(super::Status::Invalid, "trainer request needs an id");
        };
        let deep = req
            .args
            .get("deep")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let config = match Config::load(&self.env) {
            Ok(config) => config,
            Err(error) => return Response::new(super::Status::Failed, format!("{error:#}")),
        };
        let reserved = crate::trainer::triggers::transact(&access.store, &self.ctx.id, |state| {
            state.reserve_request(id, crate::horse::now_ms(), &config)
        });
        match reserved {
            Ok(RequestReservation::Already) => Response::done(serde_json::json!({"queued":true})),
            Ok(RequestReservation::Capped) => Response::new(
                super::Status::Denied,
                "trainer frequency cap or budget reached",
            ),
            Err(error) => Response::new(super::Status::Failed, format!("{error:#}")),
            Ok(RequestReservation::Reserved) => {
                let env = self.env.clone();
                let run = self.ctx.id.clone();
                std::thread::spawn(move || {
                    if let Err(error) = crate::trainer::sandbox::run_reserved(
                        &env,
                        &run,
                        None,
                        deep,
                        "worker requested review",
                    ) {
                        log::warn!("requested trainer failed: {error:#}");
                        let _ = crate::mail::append(
                            &env,
                            &run,
                            crate::mail::Dir::Up,
                            crate::mail::Message::event(
                                "trainer-failed",
                                format!("trainer request failed: {error:#}"),
                            ),
                        );
                    }
                });
                Response::done(serde_json::json!({"queued":true}))
            }
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
