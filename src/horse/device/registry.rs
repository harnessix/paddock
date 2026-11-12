//! The registry requests of a sandbox: `registry.fetch {ref, index?, use?}`
//! and `registry.sync {index}`, from `paddock-host horse registry`.
//!
//! The user approves each request on the host first. Then the host does
//! the work with its own indexes, keys and trust policy, and writes one
//! ledger line. The answer to the sandbox holds the reference, the hash,
//! the manifest fields and the trust verdict: never a path, a key, an
//! index name or the trust list. The full reason of a failure goes to the
//! host log, and `paddock horse registry resolve` shows it to the user.

use super::Access;
use crate::broker::{Request, Response, Status, PROTOCOL};
use crate::horse::dna::{self, graft};
use crate::horse::registry::{host, reference::valid_ident, Reference, Refusal};
use serde_json::json;
use std::path::PathBuf;

/// The ops of this module.
pub const FETCH: &str = "registry.fetch";
pub const SYNC: &str = "registry.sync";

/// Whether `op` is a registry request.
pub fn is_registry_op(op: &str) -> bool {
    op == FETCH || op == SYNC
}

/// The answer of the user to one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Approved,
    Denied,
    /// Nobody answered in this many seconds.
    TimedOut(u64),
    /// The approval store could not establish the outcome.
    Unknown,
}

/// What the broker gives this module: the approval card, the ledger, and
/// the paddock state folder that holds `registry/`.
pub trait Host {
    fn state_dir(&self) -> PathBuf;
    /// Put up an approval card and wait for the answer.
    fn ask(&self, summary: &str, detail: &str) -> Answer;
    /// Write one ledger line.
    fn record(&self, line: &str);
}

fn arg<'a>(req: &'a Request, k: &str) -> Option<&'a str> {
    req.args.get(k).and_then(|v| v.as_str())
}

fn fail(status: Status, msg: impl Into<String>) -> Response {
    Response::new(status, msg)
}

/// Handle a registry request of the sandbox whose horse is `setup`.
pub fn handle(setup: Option<&Access>, req: &Request, host: &dyn Host) -> Response {
    if req.v != PROTOCOL {
        return fail(Status::Failed, "unsupported protocol version");
    }
    let Some(setup) = setup else {
        return fail(Status::Failed, "this sandbox has no horse");
    };
    let index = arg(req, "index");
    if let Some(i) = index.filter(|i| !valid_ident(i)) {
        return fail(
            Status::Invalid,
            format!("{i:?} is not an index name: lower-case letters, digits and dashes"),
        );
    }
    match arg(req, "op").unwrap_or("") {
        FETCH => fetch(setup, req, index, host),
        SYNC => match index {
            Some(i) => sync(setup, i, host),
            None => fail(Status::Invalid, "registry.sync needs an index"),
        },
        _ => fail(Status::Invalid, "unknown horse registry operation"),
    }
}

fn short(horse: &str) -> &str {
    &horse[..horse.len().min(12)]
}

/// The text of a request that was not approved, and its ledger line.
fn not_approved(
    what: &str,
    answer: Answer,
    host: &dyn Host,
    kind: &str,
    subject: &str,
) -> Response {
    match answer {
        Answer::Unknown => {
            host.record(&format!("{kind} approval outcome unknown {subject}"));
            fail(
                Status::Failed,
                format!("{what} was not approved: the approval outcome is unknown"),
            )
        }
        Answer::TimedOut(secs) => {
            host.record(&format!("{kind} timed out {subject}"));
            fail(
                Status::Timeout,
                format!("{what} was not approved: nobody answered within {secs} seconds"),
            )
        }
        _ => {
            host.record(&format!("{kind} denied {subject}"));
            fail(Status::Denied, format!("{what} was not approved"))
        }
    }
}

fn fetch(setup: &Access, req: &Request, index: Option<&str>, host: &dyn Host) -> Response {
    let Some(text) = arg(req, "ref") else {
        return fail(Status::Invalid, "registry.fetch needs a reference");
    };
    let r: Reference = match text.parse() {
        Ok(r) => r,
        Err(e) => return fail(Status::Invalid, format!("{e:#}")),
    };
    let graft_it = req.args.get("use").and_then(|v| v.as_bool()) == Some(true);
    let summary = format!("fetch DNA {r} for horse {}", short(&setup.horse));
    let from = match index {
        Some(i) => format!("the index {i}"),
        None => "the registry indexes of this host".into(),
    };
    let then = if graft_it {
        "then graft it into the DNA of the horse; the running session gets it at its next safe point"
    } else {
        "then store it for the horse, without a change of its DNA"
    };
    let detail = format!(
        "Horse {}.\nThe host resolves {r} from {from}, checks it with the trust policy, {then}.",
        setup.horse
    );
    let answer = host.ask(&summary, &detail);
    if answer != Answer::Approved {
        return not_approved(
            &format!("fetch of {r}"),
            answer,
            host,
            FETCH,
            &r.to_string(),
        );
    }
    let store = &setup.store;
    let got = match dna::registry::resolve_on_host(store, &host.state_dir(), &r, index) {
        Ok(g) => g,
        Err(e) => {
            log::warn!("horse {}: fetch of {r}: {e:#}", setup.horse);
            let refusal = e.downcast_ref::<Refusal>();
            if let Some(v) = refusal.and_then(|x| x.verdict.as_ref()) {
                host.record(&format!("{FETCH} refused {r} {}", v.name()));
                return fail(
                    Status::Failed,
                    format!(
                        "{r} was refused by the trust policy of the host: {}",
                        v.name()
                    ),
                );
            }
            host.record(&format!("{FETCH} failed {r}"));
            if refusal.is_some_and(|x| x.missing) {
                return fail(
                    Status::Failed,
                    format!("{r} is not in any index of the host"),
                );
            }
            return fail(
                Status::Failed,
                format!("fetch of {r} failed on the host; the user sees why with `paddock horse registry resolve {r}`"),
            );
        }
    };
    let pinned = dna::registry::pinned(&got);
    let verdict = if got.signed_by.is_some() {
        "trusted"
    } else {
        "unsigned"
    };
    // An incompatible DNA is refused with its conflicts: trait names and
    // capabilities, nothing of the host.
    if graft_it {
        let conflicts = (|| -> anyhow::Result<Option<Vec<String>>> {
            let h = store.horse(&setup.horse)?;
            let host_dna = dna::get_or_default(store, &h.dna);
            let donor = dna::parse(std::str::from_utf8(&got.bytes)?)?;
            Ok(match graft::check(&host_dna, &donor) {
                graft::Compat::Incompatible(list) => Some(list),
                _ => None,
            })
        })();
        if let Ok(Some(list)) = conflicts {
            host.record(&format!("{FETCH} refused {pinned} incompatible"));
            return fail(
                Status::Failed,
                format!(
                    "{pinned} cannot graft into this horse: the DNAs are incompatible: {}",
                    list.join(", ")
                ),
            );
        }
    }
    let done = if graft_it {
        dna::registry::use_in(store, &setup.horse, &got).map(Some)
    } else {
        dna::registry::import(store, &got, Some(&setup.horse)).map(|_| None)
    };
    let new_dna = match done {
        Ok(d) => d,
        Err(e) => {
            log::warn!("horse {}: fetch of {pinned}: {e:#}", setup.horse);
            host.record(&format!("{FETCH} failed {pinned}"));
            return fail(
                Status::Failed,
                format!("fetch of {pinned} failed on the host; the host log has the reason"),
            );
        }
    };
    let m = &got.manifest;
    host.record(&format!("{FETCH} {pinned} {} {verdict}", m.hash));
    let parents: Vec<&str> = m.parents.iter().map(|p| p.as_str()).collect();
    let mut out = format!(
        "{}\n{pinned} hash {} parents {} created_ms {} verdict {}\n",
        m.hash,
        m.hash,
        if parents.is_empty() {
            "-".to_string()
        } else {
            parents.join(",")
        },
        m.created_ms,
        if got.signed_by.is_some() {
            "Trusted"
        } else {
            "Unsigned"
        }
    );
    if let Some(d) = new_dna {
        out.push_str(&format!("the DNA of this horse is now {d}\n"));
    }
    Response::done(json!({"bytes": out.into_bytes()}))
}

fn sync(setup: &Access, index: &str, host: &dyn Host) -> Response {
    let summary = format!("sync DNA index {index} for horse {}", short(&setup.horse));
    let detail = format!(
        "Horse {}.\nThe host runs git pull --ff-only in the checkout of the index {index}, with your git credentials. A plain folder is not changed.",
        setup.horse
    );
    let answer = host.ask(&summary, &detail);
    if answer != Answer::Approved {
        return not_approved(&format!("sync of {index}"), answer, host, SYNC, index);
    }
    match host::sync(&host.state_dir(), index) {
        Ok(host::Synced::Pulled(_)) => {
            host.record(&format!("{SYNC} {index} pulled"));
            Response::done(json!({"bytes": format!("index {index} is up to date\n").into_bytes()}))
        }
        Ok(host::Synced::Plain) => {
            host.record(&format!("{SYNC} {index} plain"));
            Response::done(json!({"bytes":
                format!("index {index} is a plain folder: nothing to sync\n").into_bytes()}))
        }
        Err(e) => {
            log::warn!("horse {}: sync of {index}: {e:#}", setup.horse);
            host.record(&format!("{SYNC} failed {index}"));
            fail(
                Status::Failed,
                format!("sync of {index} failed on the host; the user sees why with `paddock horse registry sync {index}`"),
            )
        }
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
