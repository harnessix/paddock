//! `paddock-host horse ctl`: the context transforms of a horse-mode
//! sandbox over the broker, with the approval card, the audit log and the
//! device ledger of the broker, around `horse::device::ctl`.

use super::registry::BrokerHost;
use super::{audit, Broker, Request, Response, Status, PROTOCOL};
use crate::horse::device::ctl::{self, Kind};
use std::cell::RefCell;

fn status(kind: Kind) -> Status {
    match kind {
        Kind::Refused | Kind::Denied => Status::Denied,
        Kind::Invalid => Status::Invalid,
        Kind::TimedOut => Status::Timeout,
        Kind::Failed => Status::Failed,
    }
}

impl Broker {
    /// A `ctl {command}` request of this sandbox.
    pub(super) fn handle_ctl(&self, req: &Request) -> Response {
        if req.v != PROTOCOL {
            return Response::new(Status::Failed, "unsupported protocol version");
        }
        let Some(access) = self.horse.get() else {
            return Response::new(Status::Failed, "this sandbox has no horse");
        };
        let Some(line) = req.args.get("command").and_then(|v| v.as_str()) else {
            return Response::new(Status::Invalid, "horse ctl needs a command");
        };
        let host = BrokerHost {
            broker: self,
            asked: RefCell::new(None),
        };
        let resp = match ctl::run(access, &self.ctx.id, line, &host) {
            Ok(out) => Response::done(serde_json::json!({ "bytes": out.into_bytes() })),
            Err(f) => Response::new(status(f.kind), f.message),
        };
        // Only a command that reached the card is audited, like a fetch.
        if let Some((id, decision)) = host.asked.into_inner() {
            let word = line
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            audit::audit(
                &self.env,
                &self.ctx,
                req,
                &format!("ctl {word}"),
                Some(&id),
                decision,
                &resp,
                None,
            );
        }
        resp
    }
}
