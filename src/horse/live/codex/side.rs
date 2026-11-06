//! A warm side branch of the thread in custody.
//!
//! Codex keys its prompt cache by the root thread id, so a fork under a
//! new id starts cold. An ephemeral `thread/fork` is the exception: Codex
//! derives its cache key from the parent and writes no rollout file for
//! it. paddock opens one at a safe point of the session in custody, at its
//! head, and holds the forked thread like the parent: inbox entries at its
//! safe points, interrupts, approvals.
//!
//! The side branch is a session of its own in the store: a fork of the
//! parent at its head (`fork_of`), bound to the ephemeral thread id. There
//! is no file to ingest, so paddock stores each input that it sent and
//! each `rawResponseItem/completed` item of the thread as a `response_item`
//! row in the rollout row shape. These nodes are marked live
//! ([`crate::horse::store::LIVE`]): Codex never wrote them, so no codec
//! projects the side session, and `paddock horse log` marks them.
//!
//! One branch runs under a cache key at a time: while the side branch is
//! open, the parent thread stays idle and its entries wait in its inbox.
//! The side branch ends with `paddock horse fork --close` or with the run.
//! Its nodes stay in the store.
//!
//! `rawResponseItem/completed` is taken from the app-server protocol, as
//! `thread/unsubscribe` is (reload.rs). When a side turn ends and the
//! server sent no item for it, paddock says so: the side session then holds
//! only the inputs of paddock for that turn.

pub mod request;

use super::{check_version, wire, Codex, Pending, Wait};
use crate::horse::codex::{CONTAINER, HARNESS, VERSION};
use crate::horse::inbox::Entry;
use crate::horse::store::{Hash, Origin};
use anyhow::{Context as _, Result};
use request::{Act, Request, State};
use serde_json::{json, Value};
use std::io::Write;

const ORIGIN: Origin = Origin {
    harness: HARNESS,
    container: CONTAINER,
    version: VERSION,
};

/// The side branch in custody.
pub(super) struct Side {
    /// The store session of the side branch.
    pub(super) session: String,
    /// The ephemeral thread.
    pub(super) thread: String,
    pub(super) head: Option<Hash>,
    pub(super) turns: usize,
    /// Items that came in the running turn.
    items: usize,
    /// The inputs that paddock sent: the server may send them back as
    /// items, before or after its answer, and each is stored once.
    echo: Vec<Sent>,
}

/// An input of paddock to the side thread.
struct Sent {
    /// The inbox entry.
    id: String,
    role: &'static str,
    text: String,
    /// The index of its row in the side session, once it is stored.
    at: Option<usize>,
}

/// The role of the message item of an entry.
fn role_of(e: &Entry) -> &'static str {
    if e.role == "user" {
        "user"
    } else {
        "developer"
    }
}

/// A fork on the way.
pub(super) struct Opening {
    req: Request,
    head: Hash,
    verdict: crate::horse::guard::Verdict,
}

/// A row in the rollout row shape, with the time of now.
fn row(item: &Value) -> Result<Vec<u8>> {
    let ms = crate::horse::now_ms();
    let t = crate::waiting::format_rfc3339(ms / 1000);
    let t = format!("{}.{:03}Z", t.trim_end_matches('Z'), ms % 1000);
    Ok(serde_json::to_vec(
        &json!({ "timestamp": t, "type": "response_item", "payload": item }),
    )?)
}

/// The role and the text of an input message item.
fn input_text(item: &Value) -> Option<(&str, &str)> {
    let role = item["role"]
        .as_str()
        .filter(|r| matches!(*r, "user" | "developer"));
    if item["type"] != "message" {
        return None;
    }
    let text = item["content"]
        .as_array()?
        .iter()
        .find_map(|c| c["text"].as_str());
    role.zip(text)
}

impl<W: Write> Codex<W> {
    /// The store session and the thread that run now: the side branch
    /// while it is open, else the session of the custody.
    pub(super) fn active(&self) -> (&str, Option<&str>) {
        match &self.side {
            Some(s) => (&s.session, Some(&s.thread)),
            None => (&self.ctx.session, self.thread.as_deref()),
        }
    }

    pub(super) fn is_side(&self, thread: &str) -> bool {
        self.side.as_ref().is_some_and(|s| s.thread == thread)
    }

    /// Serve the side branch request of the session, at a safe point.
    pub(super) fn serve_side(&mut self) -> Result<()> {
        if self.state() != Wait::Idle {
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let Some(r) = request::read(&st, &self.ctx.session)? else {
            return Ok(());
        };
        if r.state != State::Asked || self.side_asked.as_deref() == Some(r.id.as_str()) {
            return Ok(());
        }
        if !request::alive(r.pid) {
            log::info!(
                "horse: the side branch request {} of session {} has no asking process; dropped",
                r.id,
                r.session
            );
            return request::remove(&st, &r.session, &r.id);
        }
        self.side_asked = Some(r.id.clone());
        match r.act {
            Act::Open => self.open_side(r),
            Act::Close => self.close_side(r),
        }
    }

    fn side_answer(&mut self, mut r: Request, error: Option<String>) -> Result<()> {
        if let Some(e) = &error {
            self.say(&format!("paddock: no side branch change: {e}"));
        }
        r.state = if error.is_some() {
            State::Failed
        } else {
            State::Done
        };
        r.error = error;
        request::write(&self.ctx.store, &r)
    }

    fn open_side(&mut self, r: Request) -> Result<()> {
        if let Some(s) = &self.side {
            let e = format!(
                "side branch {} of session {} is open; close it first (one branch runs under a cache key at a time)",
                s.session, self.ctx.session
            );
            return self.side_answer(r, Some(e));
        }
        let Some(thread) = self.thread.clone() else {
            return self.side_answer(r, Some("no thread is open".into()));
        };
        // The fork copies the thread as it is: its head must be current.
        self.ingest()?;
        let st = self.ctx.store.clone();
        let s = st.session(&self.ctx.session)?;
        let Some(head) = s.head.clone() else {
            let e = format!("session {} has no rows to fork", s.id);
            return self.side_answer(r, Some(e));
        };
        let verdict =
            match crate::horse::guard::codex::warm_fork(&st, &s, &head, crate::horse::now_ms()) {
                Ok(v) => v,
                Err(e) => return self.side_answer(r, Some(format!("{e:#}"))),
            };
        let p = wire::thread_fork_ephemeral(&thread);
        let o = Box::new(Opening {
            req: r,
            head,
            verdict,
        });
        self.send(wire::THREAD_FORK, p, Pending::Fork(o))
    }

    /// The answer to `thread/fork`.
    pub(super) fn forked(
        &mut self,
        o: Box<Opening>,
        res: std::result::Result<Value, String>,
    ) -> Result<()> {
        let Opening { req, head, verdict } = *o;
        let v = match res {
            Ok(v) => v,
            Err(e) => {
                let e = format!("codex app-server cannot fork the thread: {e}");
                return self.side_answer(req, Some(e));
            }
        };
        let Some((id, cli)) = wire::thread_of(&v) else {
            let e = "codex app-server answered thread/fork without a thread".into();
            return self.side_answer(req, Some(e));
        };
        let fail = match cli {
            None => Some("codex app-server did not say its version".to_string()),
            Some(c) => check_version(&c).err().map(|e| format!("{e:#}")),
        }
        .or_else(|| {
            crate::horse::claude::check_session_id(&id)
                .err()
                .map(|e| format!("codex app-server forked thread {id}: {e:#}"))
        })
        .or_else(|| {
            (self.thread.as_deref() == Some(id.as_str()))
                .then(|| format!("codex app-server answered the fork with the parent thread {id}"))
        })
        .or_else(|| {
            let file = super::find_rollout(&self.ctx.sessions_root, &id).is_some();
            (v["thread"]["ephemeral"] != true || file).then(|| {
                format!(
                    "codex app-server made thread {id} a persisted fork, which has a new cache key; paddock does not hold it"
                )
            })
        });
        if let Some(e) = fail {
            return self.side_answer(req, Some(e));
        }
        let st = self.ctx.store.clone();
        st.save_verdict(&verdict)?;
        let f = st.fork(&self.ctx.session, &head, None)?;
        st.update_session(&f.id, |s| s.binding.native_id = Some(id.clone()))?;
        self.say(&format!(
            "paddock: warm side branch {} of session {} on ephemeral codex thread {id}; session {} waits until the branch closes",
            f.id, self.ctx.session, self.ctx.session
        ));
        self.side = Some(Side {
            session: f.id.clone(),
            thread: id.clone(),
            head: Some(head),
            turns: 0,
            items: 0,
            echo: Vec::new(),
        });
        let mut req = req;
        req.side = Some(f.id);
        req.thread = Some(id);
        req.verdict = Some(verdict);
        self.side_answer(req, None)
    }

    fn close_side(&mut self, r: Request) -> Result<()> {
        let Some(s) = &self.side else {
            let e = format!("session {} has no open side branch", self.ctx.session);
            return self.side_answer(r, Some(e));
        };
        if r.side.as_deref().is_some_and(|x| x != s.session) {
            let e = format!(
                "session {} is not the open side branch of session {}; that is {}",
                r.side.as_deref().unwrap_or_default(),
                self.ctx.session,
                s.session
            );
            return self.side_answer(r, Some(e));
        }
        let p = wire::thread_unsubscribe(&s.thread);
        self.send(wire::THREAD_UNSUBSCRIBE, p, Pending::SideClose(Box::new(r)))
    }

    /// The answer to the `thread/unsubscribe` of a close. The custody of
    /// the side branch ends also when the server keeps the thread.
    pub(super) fn side_closed(
        &mut self,
        mut r: Box<Request>,
        res: std::result::Result<Value, String>,
    ) -> Result<()> {
        if let Err(e) = &res {
            self.say(&format!(
                "paddock: codex app-server keeps the ephemeral thread in memory ({e}); paddock sends it nothing more"
            ));
        }
        r.turns = self.end_side("closed").unwrap_or_default();
        self.side_answer(*r, None)
    }

    /// End the custody of the side branch. Its turns come back.
    pub(super) fn end_side(&mut self, how: &str) -> Option<usize> {
        let s = self.side.take()?;
        self.say(&format!(
            "paddock: side branch {} {how} after {} turns; its nodes stay in the store, marked live",
            s.session, s.turns
        ));
        Some(s.turns)
    }

    /// Store one item of the side branch as a live row on its head.
    fn put_side(&mut self, item: &Value) -> Result<usize> {
        let st = self.ctx.store.clone();
        let s = self.side.as_mut().context("no side branch is open")?;
        let raw = row(item)?;
        let k = st.chain(s.head.as_ref())?.len();
        let key = match item["id"].as_str() {
            Some(id) => format!("response:{id}"),
            None => format!("live:{k}"),
        };
        let h = st.put_live(&ORIGIN, s.head.as_ref(), &key, &raw)?;
        s.head = Some(h.clone());
        st.update_session(&s.session, |x| x.head = Some(h))?;
        Ok(k)
    }

    /// An input goes out to the side thread.
    pub(super) fn side_sent(&mut self, e: &Entry) {
        if let Some(s) = self.side.as_mut() {
            s.echo.push(Sent {
                id: e.id.clone(),
                role: role_of(e),
                text: e.wire_text(),
                at: None,
            });
        }
    }

    /// The side thread refused an input.
    pub(super) fn side_refused(&mut self, id: &str) {
        if let Some(s) = self.side.as_mut() {
            s.echo.retain(|x| x.id != id || x.at.is_some());
        }
    }

    /// An input of paddock that the side thread took: its row goes into
    /// the store now, unless the server sent it back already, so the
    /// delivery has landed.
    pub(super) fn side_input(&mut self, e: &Entry) -> Result<()> {
        let s = self.side.as_mut().context("no side branch is open")?;
        let at = match s.echo.iter().position(|x| x.id == e.id) {
            Some(i) if s.echo[i].at.is_some() => s.echo.remove(i).at,
            _ => None,
        };
        let k = match at {
            Some(k) => k,
            None => {
                let k = self.put_side(&wire::input_message(role_of(e), &e.wire_text()))?;
                if let Some(x) = self
                    .side
                    .as_mut()
                    .and_then(|s| s.echo.iter_mut().find(|x| x.id == e.id))
                {
                    x.at = Some(k);
                }
                k
            }
        };
        if self
            .ctx
            .store
            .delivery_landed(&e.session, &e.id, k as u64)?
        {
            self.report.landed += 1;
        }
        Ok(())
    }

    /// A raw item of the side thread. An input that paddock stored
    /// already is not stored twice.
    pub(super) fn side_item(&mut self, item: Value) -> Result<()> {
        let Some(s) = self.side.as_mut() else {
            return Ok(());
        };
        s.items += 1;
        let sent = input_text(&item)
            .and_then(|(role, text)| s.echo.iter().position(|x| x.role == role && x.text == text));
        match sent {
            // paddock stored it already: an echo.
            Some(i) if s.echo[i].at.is_some() => {
                s.echo.remove(i);
            }
            // Before the answer: the item of the server is the row.
            Some(i) => {
                let k = self.put_side(&item)?;
                if let Some(s) = self.side.as_mut() {
                    s.echo[i].at = Some(k);
                }
            }
            None => {
                self.put_side(&item)?;
            }
        }
        Ok(())
    }

    /// A turn of the side thread ended.
    pub(super) fn side_turn_done(&mut self, turn: &str) {
        let Some(s) = self.side.as_mut() else { return };
        s.turns += 1;
        // An input stored and not sent back by the end of a turn is not
        // sent back.
        s.echo.retain(|x| x.at.is_none());
        let none = std::mem::take(&mut s.items) == 0;
        let session = s.session.clone();
        self.report.side_turns += 1;
        if none {
            self.say(&format!(
                "paddock: codex sent no item of side turn {turn}; side session {session} holds only the inputs of paddock for it"
            ));
        }
    }

    /// `thread/closed` of the side thread: a close, or the server dropped
    /// the thread.
    pub(super) fn side_thread_closed(&mut self) {
        let closing = self
            .waiting
            .values()
            .any(|w| matches!(w.pending, Pending::SideClose(_)));
        if !closing {
            self.busy = None;
            self.end_side("ended: codex app-server closed its thread");
        }
    }
}
