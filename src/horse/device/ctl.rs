//! The context transforms of a sandbox: one command per write of
//! `self/ctl`, or per `paddock-host horse ctl` over the broker.
//!
//! A command is one line of plain text, so `echo` in a shell is enough:
//!
//! - `fork SESSION [AT_NODE]`: a new session of the horse whose head is a
//!   node of SESSION (default: its head).
//! - `append SESSION ROLE TEXT`: an inbox entry for SESSION; TEXT is the
//!   rest of the line.
//! - `move SESSION HORSE`: as `paddock horse move`, only to a granted horse.
//! - `view SESSION NAME FILTER`: the read-only file `self/views/NAME`.
//! - `disconnect SESSION`: the custody of SESSION ends at its next safe
//!   point.
//!
//! Every command needs the grant of the user for this sandbox
//! ([`grant`]), works only on sessions of the sandbox's own horse, takes
//! one command of the budget, and writes one ledger line: passed,
//! approved, denied, timed out, refused, invalid or failed. The prefix
//! guard judges every command that changes a head before anything is
//! written. A command that breaks the cached prefix asks the user with
//! the break size and the cost, and its lost tokens come from the token
//! budget of the grant. The answers name only the sandbox's own horse and
//! sessions: no path, no key, and nothing of another horse.

pub mod grant;
pub mod view;

use super::registry::{Answer, Host};
use super::Access;
use crate::horse::guard::{self, Op, Verdict};
use crate::horse::{live, Hash, Session, Store};
use anyhow::Result;
use serde_json::json;

/// The broker op of `paddock-host horse ctl`.
pub const OP: &str = "ctl";
/// The device file.
pub const PATH: &str = "self/ctl";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Fork {
        session: String,
        at: Option<String>,
    },
    Append {
        session: String,
        role: String,
        text: String,
    },
    Move {
        session: String,
        horse: String,
    },
    View {
        session: String,
        name: String,
        filter: view::Filter,
    },
    Disconnect {
        session: String,
    },
}

/// Why a command did not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No grant, not granted, out of budget, or not this horse: EACCES.
    Refused,
    /// The command does not parse: EINVAL.
    Invalid,
    /// The user said no: EPERM.
    Denied,
    /// Nobody answered: ETIMEDOUT.
    TimedOut,
    /// The host could not do it: EIO.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    pub message: String,
}

fn failure(kind: Kind, message: impl Into<String>) -> Failure {
    Failure {
        kind,
        message: message.into(),
    }
}

const USAGE: &str = "a ctl command is one of: fork SESSION [AT_NODE], append SESSION ROLE TEXT, move SESSION HORSE, view SESSION NAME FILTER, disconnect SESSION";

impl Command {
    /// One command line. A trailing newline is fine; a second line is not.
    pub fn parse(line: &str) -> Result<Command, String> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.contains('\n') || line.contains('\0') {
            return Err("write one ctl command per write".into());
        }
        let mut it = line.trim_start().splitn(2, char::is_whitespace);
        let word = it.next().unwrap_or("");
        let rest = it.next().unwrap_or("").trim_start();
        let words: Vec<&str> = rest.split_whitespace().collect();
        let s = |i: usize| words.get(i).map(|w| w.to_string());
        let cmd = match (word, words.len()) {
            ("fork", 1 | 2) => Command::Fork {
                session: words[0].into(),
                at: s(1),
            },
            ("append", n) if n >= 3 => {
                let mut p = rest.splitn(3, char::is_whitespace);
                let (session, role) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
                Command::Append {
                    session: session.into(),
                    role: role.into(),
                    text: p.next().unwrap_or("").trim_start().into(),
                }
            }
            ("move", 2) => Command::Move {
                session: words[0].into(),
                horse: words[1].into(),
            },
            ("view", n) if n >= 3 => {
                let mut p = rest.splitn(3, char::is_whitespace);
                let (session, name) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
                Command::View {
                    session: session.into(),
                    name: name.into(),
                    filter: view::Filter::parse(p.next().unwrap_or(""))
                        .map_err(|e| e.to_string())?,
                }
            }
            ("disconnect", 1) => Command::Disconnect {
                session: words[0].into(),
            },
            _ => return Err(USAGE.into()),
        };
        Ok(cmd)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Command::Fork { .. } => "fork",
            Command::Append { .. } => "append",
            Command::Move { .. } => "move",
            Command::View { .. } => "view",
            Command::Disconnect { .. } => "disconnect",
        }
    }

    pub fn session(&self) -> &str {
        match self {
            Command::Fork { session, .. }
            | Command::Append { session, .. }
            | Command::Move { session, .. }
            | Command::View { session, .. }
            | Command::Disconnect { session } => session,
        }
    }
}

/// The session `id` when it belongs to the sandbox's horse and is not the
/// session of another instance. A hitched child shares the horse of its
/// parent, but it must not reach the parent or a sibling: only results
/// cross between them. So a session at either end of a `SpawnedFrom`
/// edge is out of reach, unless it is the sandbox's own session. A
/// session of another horse, a session of another instance and a session
/// that does not exist look the same.
fn own_session(access: &Access, id: &str) -> Option<Session> {
    let store = &access.store;
    let horse = access.horse.as_str();
    let h = store.horse(horse).ok()?;
    if !h.sessions.iter().any(|s| s == id) {
        return None;
    }
    if id != access.session {
        let edges = crate::horse::hitch::edges(store, horse).ok()?;
        let instance = edges.iter().any(|e| {
            e.kind == crate::horse::hitch::EdgeKind::SpawnedFrom && (e.from == id || e.to == id)
        });
        if instance {
            return None;
        }
    }
    store.session(id).ok().filter(|s| s.horse == horse)
}

/// What the guard says before a command runs.
enum Judged {
    /// No head changes, or the prefix stays.
    Passes(Option<Verdict>),
    /// The cached prefix breaks: the verdict, or the reason that the
    /// guard has none.
    Breaks(Result<Verdict, String>),
}

/// The guard operation of a command; `None` for commands that change no
/// head (`view`, `disconnect`).
fn op_of(store: &Store, s: &Session, cmd: &Command) -> Result<Option<Op>, Failure> {
    Ok(match cmd {
        Command::Fork { at, .. } => {
            let node: Hash = match at {
                Some(a) => store
                    .resolve_node(s, a)
                    .map_err(|e| failure(Kind::Invalid, format!("{e:#}")))?,
                None => s.head.clone().ok_or_else(|| {
                    failure(Kind::Failed, format!("session {} has no node yet", s.id))
                })?,
            };
            Some(Op::Fork { at: node })
        }
        Command::Append { role, text, .. } => {
            if !crate::horse::inbox::ROLES.contains(&role.as_str()) {
                return Err(failure(
                    Kind::Invalid,
                    format!(
                        "role {role} is not known: use one of {}",
                        crate::horse::inbox::ROLES.join(", ")
                    ),
                ));
            }
            if text.is_empty() {
                return Err(failure(Kind::Invalid, "the text of an append is empty"));
            }
            Some(Op::Append { after: None })
        }
        Command::Move { horse, .. } => Some(Op::MoveHorse { to: horse.clone() }),
        Command::View { name, .. } => {
            if !super::valid_name(name) {
                return Err(failure(
                    Kind::Invalid,
                    format!("{name:?} is not a view name"),
                ));
            }
            None
        }
        Command::Disconnect { .. } => None,
    })
}

fn judge(store: &Store, session: &str, op: Option<&Op>) -> Judged {
    let Some(op) = op else {
        return Judged::Passes(None);
    };
    // An entry at the end changes nothing before the head: a pure append
    // passes (invariant 8). The guard can still say that the cache went
    // cold with time, which no command changes, so its verdict is kept.
    let append = matches!(op, Op::Append { after: None } | Op::MoveHorse { .. });
    match guard::check(store, session, op, crate::horse::now_ms()) {
        Ok(v) if v.keeps || append => Judged::Passes(Some(v)),
        Err(_) if append => Judged::Passes(None),
        Ok(v) => Judged::Breaks(Ok(v)),
        // No verdict is no proof that the prefix stays: ask.
        Err(e) => Judged::Breaks(Err(format!("{e:#}"))),
    }
}

/// Run one command line of the sandbox `sandbox`, whose horse is `access`.
pub fn run(access: &Access, sandbox: &str, line: &str, host: &dyn Host) -> Result<String, Failure> {
    if super::peek::grant::has_trainer(&access.store, sandbox).unwrap_or(true) {
        host.record("ctl refused trainer");
        return Err(failure(Kind::Refused, "trainers cannot change horse state"));
    }
    let cmd = match Command::parse(line) {
        Ok(c) => c,
        Err(e) => {
            host.record("ctl invalid");
            return Err(failure(Kind::Invalid, e));
        }
    };
    let name = cmd.name();
    let refuse = |why: &str, message: String| {
        host.record(&format!("ctl {name} refused {why}"));
        Err(failure(Kind::Refused, message))
    };
    let store = &access.store;
    let g = match grant::get(store, sandbox) {
        Ok(Some(g)) => g,
        Ok(None) => {
            return refuse(
                "no grant",
                format!("this sandbox has no ctl grant; the user grants it on the host with `paddock horse grant {sandbox}`"),
            )
        }
        Err(e) => {
            log::warn!("horse ctl of sandbox {sandbox}: {e:#}");
            host.record(&format!("ctl {name} failed grant"));
            return Err(failure(Kind::Failed, "the ctl grant of this sandbox does not read"));
        }
    };
    if !g.allows(name) {
        return refuse(
            "not granted",
            format!(
                "the ctl grant of this sandbox does not allow {name}; it allows {}",
                g.commands.join(", ")
            ),
        );
    }
    let Some(s) = own_session(access, cmd.session()) else {
        return refuse(
            "not this horse",
            format!("{} is not a session of this horse", cmd.session()),
        );
    };
    if let Command::Move { horse, .. } = &cmd {
        if !g.horses.contains(horse) {
            return refuse(
                "horse not granted",
                format!("the ctl grant of this sandbox does not allow a move to {horse}"),
            );
        }
    }
    if g.commands_left() == 0 {
        return refuse(
            "budget",
            format!(
                "the ctl budget of this sandbox is used up: {} of {} commands",
                g.used, g.budget
            ),
        );
    }
    let session = s.id.clone();
    let op = op_of(store, &s, &cmd).inspect_err(|f| {
        let what = if f.kind == Kind::Invalid {
            "invalid"
        } else {
            "failed"
        };
        host.record(&format!("ctl {name} {session} {what}"));
    })?;
    let (verdict, tokens, outcome) = match judge(store, &session, op.as_ref()) {
        Judged::Passes(v) => (v, 0, "passed".to_string()),
        Judged::Breaks(v) => {
            let tokens = v.as_ref().map_or(0, |v| v.lost_tokens);
            if tokens > g.tokens_left() {
                return refuse(
                    "token budget",
                    format!(
                        "{name} of session {session} loses about {tokens} tokens of the cached prefix; the ctl grant of this sandbox has {} tokens left",
                        g.tokens_left()
                    ),
                );
            }
            let (summary, detail) = card(access, &cmd, &v);
            match host.ask(&summary, &detail) {
                Answer::Approved => {}
                Answer::Denied => {
                    host.record(&format!("ctl {name} {session} denied"));
                    return Err(failure(Kind::Denied, not_approved(&cmd, &v, None)));
                }
                Answer::TimedOut(secs) => {
                    host.record(&format!("ctl {name} {session} timed out"));
                    return Err(failure(Kind::TimedOut, not_approved(&cmd, &v, Some(secs))));
                }
            }
            (v.ok(), tokens, format!("approved lost {tokens} tokens"))
        }
    };
    match grant::spend(store, sandbox, tokens) {
        Ok(true) => {}
        Ok(false) => return refuse("budget", "the ctl budget of this sandbox is used up".into()),
        Err(e) => {
            log::warn!("horse ctl of sandbox {sandbox}: {e:#}");
            host.record(&format!("ctl {name} {session} failed"));
            return Err(failure(Kind::Failed, "the ctl budget did not update"));
        }
    }
    match execute(access, &cmd, op.as_ref(), verdict.as_ref()) {
        Ok(out) => {
            host.record(&format!("ctl {name} {session} {outcome}"));
            event(store, &access.horse, sandbox, &cmd);
            Ok(out)
        }
        Err(e) => {
            log::warn!("horse ctl {name} of sandbox {sandbox}: {e:#}");
            if let Err(e) = grant::refund(store, sandbox, tokens) {
                log::warn!("horse ctl of sandbox {sandbox}: {e:#}");
            }
            host.record(&format!("ctl {name} {session} failed"));
            Err(failure(
                Kind::Failed,
                format!("{name} of session {session} failed: {e:#}"),
            ))
        }
    }
}

/// The approval card of a cache-breaking command.
fn card(access: &Access, cmd: &Command, v: &Result<Verdict, String>) -> (String, String) {
    let short = &access.horse[..access.horse.len().min(12)];
    let what = describe(cmd);
    let (summary, guard) = match v {
        Ok(v) => (
            format!(
                "{what} in horse {short} loses about {} tokens of cached prefix",
                v.lost_tokens
            ),
            v.message(),
        ),
        Err(e) => (
            format!("{what} in horse {short}: the prefix guard has no verdict"),
            format!("The prefix guard cannot judge this command, so the cost is not known: {e}"),
        ),
    };
    let detail = format!(
        "Horse {}. The sandbox asks for {what} through its ctl grant.\n{guard}",
        access.horse
    );
    (summary, detail)
}

fn describe(cmd: &Command) -> String {
    match cmd {
        Command::Fork { session, at } => match at {
            Some(a) => format!("a fork of session {session} at {a}"),
            None => format!("a fork of session {session} at its head"),
        },
        Command::Append { session, role, .. } => format!("a {role} entry for session {session}"),
        Command::Move { session, horse } => format!("a move of session {session} to horse {horse}"),
        Command::View { session, name, .. } => format!("view {name} of session {session}"),
        Command::Disconnect { session } => format!("the end of the custody of session {session}"),
    }
}

/// The answer to the sandbox for a command that the user did not approve.
fn not_approved(cmd: &Command, v: &Result<Verdict, String>, secs: Option<u64>) -> String {
    let mut s = match secs {
        Some(secs) => format!(
            "{} was not approved: nobody answered within {secs} seconds; nothing changed",
            describe(cmd)
        ),
        None => format!("{} was not approved; nothing changed", describe(cmd)),
    };
    if let Some(x) = v.as_ref().ok().and_then(|v| v.suggestion.as_ref()) {
        s += &format!("\ninstead: {x}");
    }
    s
}

fn execute(
    access: &Access,
    cmd: &Command,
    op: Option<&Op>,
    verdict: Option<&Verdict>,
) -> Result<String> {
    let store = &access.store;
    // As `paddock horse fork` does. A move saves its own verdict.
    if let Some(v) = verdict.filter(|_| matches!(cmd, Command::Fork { .. })) {
        store.save_verdict(v)?;
    }
    Ok(match cmd {
        Command::Fork { session, .. } => {
            let Some(Op::Fork { at }) = op else {
                anyhow::bail!("a fork needs its node");
            };
            let f = store.fork(session, at, Some(&access.horse))?;
            format!("{}\n", f.id)
        }
        Command::Append {
            session,
            role,
            text,
        } => {
            let e = store.queue_append(session, role, text)?;
            format!("{}\n", e.id)
        }
        Command::Move { session, horse } => {
            crate::horse::lineage::move_to(store, session, horse)?;
            format!("session {session} moved to horse {horse}\n")
        }
        Command::View {
            session,
            name,
            filter,
        } => {
            let v = view::View {
                session: session.clone(),
                filter: filter.clone(),
            };
            view::save(store, &access.horse, name, &v)?;
            format!("self/views/{name}\n")
        }
        Command::Disconnect { session } => {
            live::disconnect::ask(store, session)?;
            format!("the custody of session {session} ends at its next safe point\n")
        }
    })
}

/// One event in the horse for a command that ran: the explicit, recorded
/// edge of invariant 2.
fn event(store: &Store, horse: &str, sandbox: &str, cmd: &Command) {
    let body = json!({
        "type": format!("ctl.{}", cmd.name()),
        "session": cmd.session(),
        "sandbox": sandbox,
        "at_ms": crate::horse::now_ms(),
    });
    let r = store
        .lock()
        .and_then(|l| crate::horse::run::append_horse_event_locked(&l, horse, None, &body));
    if let Err(e) = r {
        log::warn!("horse ctl event of sandbox {sandbox}: {e:#}");
    }
}

#[cfg(test)]
mod hitch_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
pub(crate) mod tests;
