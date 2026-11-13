//! Waiting devices: a read blocks until an event, so an agent never needs
//! `sleep` to wait.
//!
//! - `wait`: the read returns at a time. The request is an RFC 3339 time or
//!   a duration.
//! - `ask`: the agent writes a question; the read returns the user's answer.
//!   The question appears on the approval card of the bar panel (and as a
//!   desktop notification when the bar does not run). It uses the same
//!   pending files as host requests, so `paddock approvals` lists it too.
//!
//! Both take plain text, or a JSON object with a `timeout` in seconds: the
//! longest wait of this one read (default `waiting::DEFAULT_READ_TIMEOUT`,
//! at most `waiting::MAX_READ_TIMEOUT`). When it passes before the event,
//! the read fails with ETIME (still waiting). An agent then sends the same
//! request again: a `wait` for the same time, or an `ask` with the same
//! question, which finds the question that is still open instead of a new
//! card. The answer text goes to the agent only. Logs and the audit log keep
//! sizes and outcomes, never the question or the answer.

use super::{Call, DevError, Device, DeviceCtx, Info, Limits};
use crate::broker;
use crate::waiting;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[path = "blocking_info.rs"]
mod info;
pub use info::ask_info;
#[path = "blocking_request.rs"]
mod request;
use request::{check_question, parse};

/// How often a waiting read checks for its event and for a cancel.
const POLL: Duration = Duration::from_millis(100);
/// The longest question, in characters.
pub const MAX_QUESTION: usize = 4000;
/// Choices of one question, and the length of each.
pub const MAX_CHOICES: usize = 8;
pub const MAX_CHOICE: usize = 64;
/// Questions that one sandbox can have open at the same time.
const MAX_OPEN_QUESTIONS: usize = 4;
// Retain accepted replies until a reader returns, but bound the total
// number of entries (including the still-unanswered questions).
const MAX_RETAINED_QUESTIONS: usize = 8;
/// Questions of all sandboxes on the card at the same time, so that no
/// sandbox can push host requests of others off the card.
pub const MAX_ALL_QUESTIONS: usize = 8;
/// After the user dismisses a question, the sandbox cannot ask a new one
/// for this long.
pub const REASK_AFTER_DISMISS: Duration = Duration::from_secs(30);
/// How often abandoned questions are withdrawn.
const SWEEP_EVERY: Duration = Duration::from_secs(10);
/// A question that no read waited for during this time is withdrawn: the
/// agent stopped asking.
const ABANDONED: Duration = Duration::from_secs(120);

/// The limits of a waiting device: the hard deadline is a little longer than
/// the longest read, so the read itself always reports "still waiting".
fn limits(max_request: usize, max_response: usize) -> Limits {
    Limits {
        max_request,
        max_response,
        deadline_seconds: waiting::MAX_READ_TIMEOUT + 5,
    }
}

#[cfg(test)]
thread_local! {
    static EXPIRE_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// When this read stops waiting: after its own timeout, and never after the
/// deadline of the call.
fn read_end(call: &Call, timeout: u64) -> Instant {
    #[cfg(test)]
    if EXPIRE_READ.with(std::cell::Cell::get) {
        return Instant::now();
    }
    (Instant::now() + Duration::from_secs(timeout)).min(call.deadline - Duration::from_secs(2))
}

/// `/dev/paddock/wait`: the read returns at a time.
pub struct Wait;

impl Device for Wait {
    fn name(&self) -> &str {
        "wait"
    }
    fn info(&self) -> Info {
        Info::builtin(
            format!(
                "the time to wait for: an RFC 3339 time (2026-10-30T15:00:00Z) or a duration (90, 90s, 15m, 2h), at most {} days ahead; or JSON {{\"until\": TIME, \"timeout\": SECONDS}}. One read waits at most timeout seconds (default {}, at most {}), then fails with ETIME (still waiting): send the same time again. For a long wait, send an RFC 3339 time, not a duration, so that each new read waits for the same moment.",
                waiting::MAX_WAIT_AHEAD / 86400,
                waiting::DEFAULT_READ_TIMEOUT,
                waiting::MAX_READ_TIMEOUT
            ),
            "one line at the time: reached TIME (RFC 3339, UTC)",
            limits(1024, 128),
        )
    }
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let p = parse(request, "until")?;
        let at =
            waiting::parse_when(&p.text, crate::state::now()).map_err(|_| DevError::Invalid)?;
        let end = read_end(call, p.timeout);
        loop {
            if crate::state::now() >= at {
                return Ok(format!("reached {}\n", waiting::format_rfc3339(at)).into_bytes());
            }
            if call.cancelled() {
                return Err(DevError::Cancelled);
            }
            if Instant::now() >= end {
                return Err(DevError::Waiting);
            }
            std::thread::sleep(POLL);
        }
    }
}

/// A question on the card.
struct Open {
    id: String,
    chars: usize,
    choices: usize,
    /// Reads that wait for it now.
    readers: usize,
    /// When the last read stopped waiting.
    left: Instant,
    note: Option<broker::notify::Handle>,
    /// The answer and when it came. Reads that still wait get it too.
    done: Option<(Reply, Instant)>,
}

/// `/dev/paddock/ask`: the read returns the user's answer.
pub struct Ask {
    ctx: DeviceCtx,
    open: Mutex<HashMap<String, Open>>,
    /// When the user last dismissed a question of this sandbox.
    dismissed: Mutex<Option<Instant>>,
}

/// What the user answered.
#[derive(Clone)]
enum Reply {
    Text(String),
    Dismissed,
}

impl Ask {
    pub fn new(ctx: DeviceCtx) -> Self {
        Self {
            ctx,
            open: Mutex::new(HashMap::new()),
            dismissed: Mutex::new(None),
        }
    }

    /// The device with a thread that withdraws abandoned questions, also
    /// when the agent never asks again.
    pub fn start(ctx: DeviceCtx) -> std::sync::Arc<Self> {
        let a = std::sync::Arc::new(Self::new(ctx));
        let weak = std::sync::Arc::downgrade(&a);
        std::thread::spawn(move || loop {
            std::thread::sleep(SWEEP_EVERY);
            let Some(a) = weak.upgrade() else { return };
            let mut open = a.open.lock().unwrap();
            a.sweep(&mut open);
        });
        a
    }

    fn answer_file(&self, id: &str) -> std::path::PathBuf {
        broker::approvals_dir(&self.ctx.env).join(format!("{id}.answer"))
    }

    fn close_note(&self, q: &Open) {
        if let Some(n) = &q.note {
            n.close();
        }
    }

    /// One audit line per question: sizes and the outcome, never the text.
    fn audit(&self, q: &Open, decision: &str, reply_chars: Option<usize>) {
        let mut line = serde_json::json!({
            "time": crate::state::now(),
            "sandbox": self.ctx.sandbox,
            "harness": self.ctx.harness,
            "project": self.ctx.project,
            "capability": "ask",
            "request": q.id,
            "detail": format!(
                "a question of {} characters with {} choices (the text is not logged)",
                q.chars, q.choices
            ),
            "decision": decision,
            "status": if reply_chars.is_some() { "done" } else { "denied" },
        });
        if let Some(n) = reply_chars {
            line["message"] = format!("an answer of {n} characters").into();
        }
        if let Some(p) = self.ctx.started_by.last() {
            line["parent"] = p.clone().into();
            line["chain"] = serde_json::json!(self.ctx.started_by);
        }
        broker::audit_line(&self.ctx.env, &line);
    }

    /// Move replies with no active readers into this owner's memory, and
    /// withdraw abandoned unanswered questions. A saved reply remains until
    /// a reader returns or this device is dropped.
    fn sweep(&self, open: &mut HashMap<String, Open>) {
        self.sweep_with(open, |q| q.left.elapsed() > ABANDONED);
    }

    fn sweep_with(&self, open: &mut HashMap<String, Open>, abandoned: impl Fn(&Open) -> bool) {
        for q in open
            .values_mut()
            .filter(|q| q.readers == 0 && q.done.is_none())
        {
            if !self.answer_file(&q.id).is_file() {
                continue;
            }
            match broker::take_answer(&self.ctx.env, &q.id) {
                Ok(Some(text)) => {
                    let reply = Self::parse_reply(&text);
                    self.close_note(q);
                    self.record_reply(q, &reply);
                    q.done = Some((reply, Instant::now()));
                }
                Ok(None) => {}
                Err(e) => log::warn!("cannot claim answer for question {}: {e}", q.id),
            }
        }
        let stale: Vec<String> = open
            .iter()
            .filter(|(_, q)| q.done.is_none() && q.readers == 0 && abandoned(q))
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            let Some(q) = open.get_mut(&k) else { continue };
            match broker::withdraw_unanswered(&self.ctx.env, &q.id) {
                Ok(true) => {
                    self.close_note(q);
                    self.audit(q, "withdrawn: the agent stopped waiting", None);
                    open.remove(&k);
                }
                // Keep an accepted answer for the next reader, however long
                // the reader takes to return.
                Ok(false) => {}
                Err(e) => log::warn!("cannot withdraw question {}: {e}", q.id),
            }
        }
    }

    /// The desktop notification when the bar panel does not run. Its buttons
    /// write the same answer file as the card.
    fn notify(&self, p: &broker::Pending) -> Option<broker::notify::Handle> {
        if broker::approval_ui_active(&self.ctx.env) {
            return None;
        }
        let mut actions: Vec<(String, String)> = p
            .choices
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("choice:{i}"), c.clone()))
            .collect();
        actions.push(("deny".into(), "Dismiss".into()));
        let note = broker::notify::Note {
            what: "question".into(),
            action: Some(p.id.clone()),
            summary: format!("paddock: {} asks you a question", p.harness),
            body: format!(
                "{}{}\n\nType an answer in a terminal: paddock approvals answer {} --text -\n(type the answer, then Ctrl-D)",
                crate::spawn::chain_text(&p.started_by)
                    .map(|c| format!("{c}\n\n"))
                    .unwrap_or_default(),
                p.detail,
                p.id
            ),
            actions,
            critical: true,
        };
        let (env, id, choices) = (self.ctx.env.clone(), p.id.clone(), p.choices.clone());
        let on_action = move |key: &str| {
            if let Some(i) = key
                .strip_prefix("choice:")
                .and_then(|i| i.parse::<usize>().ok())
            {
                if let Some(c) = choices.get(i) {
                    let _ = broker::answer_reply(&env, &id, c);
                }
            } else if key == "deny" {
                let _ = broker::answer(&env, &id, broker::Answer::Deny);
            }
        };
        broker::notify::ask_or_show(&self.ctx.env, &note, on_action)
    }

    /// The open question for this text, or a new one on the card.
    fn attach(&self, question: &str, choices: &[String]) -> Result<String, DevError> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(question.as_bytes());
        for c in choices {
            h.update([0u8]);
            h.update(c.as_bytes());
        }
        let key = crate::manifest::hex(&h.finalize());
        let p = {
            let mut open = self.open.lock().unwrap();
            self.sweep(&mut open);
            if let Some(q) = open.get_mut(&key) {
                q.readers += 1;
                return Ok(key);
            }
            if open.len() >= MAX_RETAINED_QUESTIONS {
                return Err(DevError::Failed);
            }
            let unanswered = open
                .values()
                .filter(|q| q.done.is_none() && !self.answer_file(&q.id).is_file())
                .count();
            if unanswered >= MAX_OPEN_QUESTIONS {
                return Err(DevError::Failed);
            }
            if self
                .dismissed
                .lock()
                .unwrap()
                .is_some_and(|t| t.elapsed() < REASK_AFTER_DISMISS)
            {
                return Err(DevError::Denied);
            }
            let all = broker::pending(&self.ctx.env)
                .iter()
                .filter(|p| p.capability == broker::ASK_CAPABILITY)
                .count();
            if all >= MAX_ALL_QUESTIONS {
                return Err(DevError::Failed);
            }
            let now = crate::state::now();
            let p = broker::Pending {
                id: broker::new_request_id(),
                sandbox: self.ctx.sandbox.clone(),
                harness: self.ctx.harness.clone(),
                project: self.ctx.project.clone(),
                capability: broker::ASK_CAPABILITY.into(),
                summary: "asks you a question".into(),
                detail: question.to_string(),
                reason: String::new(),
                created: now,
                // A question has no time limit; it waits until the agent stops.
                expires: 0,
                session_allowed: false,
                started_by: self.ctx.started_by.clone(),
                choices: choices.to_vec(),
                connect: None,
            };
            // In the list before the card exists: a read of the same question
            // that comes now attaches instead of making a second card.
            open.insert(
                key.clone(),
                Open {
                    id: p.id.clone(),
                    chars: question.chars().count(),
                    choices: choices.len(),
                    readers: 1,
                    left: Instant::now(),
                    note: None,
                    done: None,
                },
            );
            p
        };
        // The card and the notification, without holding the lock: a slow
        // session bus must not block the other reads of this sandbox.
        if let Err(e) = broker::write_pending(&self.ctx.env, &p) {
            log::warn!("cannot put the question on the card: {e}");
            self.open.lock().unwrap().remove(&key);
            return Err(DevError::Failed);
        }
        let note = self.notify(&p);
        match self.open.lock().unwrap().get_mut(&key) {
            Some(q) => q.note = note,
            None => {
                if let Some(n) = note {
                    n.close();
                }
            }
        }
        Ok(key)
    }

    fn parse_reply(text: &str) -> Reply {
        let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
        if let Some(r) = v.get("reply").and_then(|r| r.as_str()) {
            return Reply::Text(r.to_string());
        }
        // `paddock approvals answer ID deny`, or a notification's Dismiss.
        Reply::Dismissed
    }

    fn record_reply(&self, q: &Open, reply: &Reply) {
        match reply {
            Reply::Text(t) => self.audit(q, "answered", Some(t.chars().count())),
            Reply::Dismissed => {
                self.audit(q, "dismissed", None);
                *self.dismissed.lock().unwrap() = Some(Instant::now());
            }
        }
    }
}

impl Drop for Ask {
    /// The sandbox ended: its open questions leave the card.
    fn drop(&mut self) {
        let open = std::mem::take(&mut *self.open.lock().unwrap());
        for q in open.values().filter(|q| q.done.is_none()) {
            match broker::close_pending(&self.ctx.env, &q.id) {
                Ok(answer) => {
                    self.close_note(q);
                    if answer.is_none() {
                        self.audit(q, "withdrawn: the sandbox ended", None);
                    }
                }
                Err(e) => {
                    log::warn!("cannot withdraw ended question {}: {e}", q.id);
                    broker::abandon_owner(&self.ctx.env, &q.id);
                }
            }
        }
    }
}

impl Device for Ask {
    fn name(&self) -> &str {
        "ask"
    }
    fn info(&self) -> Info {
        ask_info()
    }
    fn asks(&self, _call: &Call) -> bool {
        true
    }
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let p = parse(request, "question")?;
        let (question, choices) = check_question(&p)?;
        let key = self.attach(&question, &choices)?;
        let end = read_end(call, p.timeout);
        let leave = |outcome: Result<Vec<u8>, DevError>| {
            let mut open = self.open.lock().unwrap();
            if let Some(q) = open.get_mut(&key) {
                q.readers = q.readers.saturating_sub(1);
                q.left = Instant::now();
                if q.readers == 0 && q.done.is_some() {
                    open.remove(&key);
                }
            }
            outcome
        };
        loop {
            let (id, done) = match self.open.lock().unwrap().get(&key) {
                Some(q) => (q.id.clone(), q.done.as_ref().map(|(r, _)| r.clone())),
                None => return Err(DevError::Cancelled),
            };
            if let Some(reply) = done {
                return leave(match reply {
                    Reply::Text(t) => Ok(t.into_bytes()),
                    Reply::Dismissed => Err(DevError::Denied),
                });
            }
            if self.answer_file(&id).is_file() {
                let mut open = self.open.lock().unwrap();
                if let Some(q) = open.get_mut(&key) {
                    if q.done.is_none() {
                        match broker::take_answer(&self.ctx.env, &q.id) {
                            Ok(Some(text)) => {
                                let reply = Self::parse_reply(&text);
                                self.close_note(q);
                                self.record_reply(q, &reply);
                                q.done = Some((reply, Instant::now()));
                            }
                            Ok(None) => {}
                            Err(e) => log::warn!("cannot claim answer for question {}: {e}", q.id),
                        }
                    }
                    if q.done.is_some() {
                        continue;
                    }
                }
            }
            if call.cancelled() {
                return leave(Err(DevError::Cancelled));
            }
            if Instant::now() >= end {
                return leave(Err(DevError::Waiting));
            }
            std::thread::sleep(POLL);
        }
    }
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod tests;
