//! Custody of one Grok Build session over ACP (`grok agent stdio`).
//!
//! paddock initializes the agent, loads the session (`session/load`, for a
//! session with a projected session folder) or starts a new one
//! (`session/new`, for a session without rows), and then holds the turns.
//! A turn runs from its `session/prompt` until the answer to it: that
//! answer is the safe point. Only then does an inbox entry go out, always
//! as the next `session/prompt`: a `user` entry as its text, a `system`,
//! `developer` or `note` entry with its role tag (`Entry::wire_text`).
//! `x.ai/interject` is not used: Grok drains it inside a running turn,
//! which is not a safe point. `session/cancel` is the interrupt.
//!
//! ACP gives a client no way to name the id of a new session, so Grok
//! picks it and paddock binds it to the store session, as for a Codex
//! thread. A load that answers for another session id ends the custody
//! before the store changes. After every turn paddock ingests the session
//! folder by record identity (the Grok codec). The cache of a Grok
//! session is keyed by its id, which is the native id of its guard tuple.
//!
//! The custodian is a [`Live`]: the driver of harness.rs runs the loop. A
//! typed line goes into the inbox like an append ([`Typed::Inbox`]).

mod approve;
pub mod wire;

pub use approve::{args, ledger_line, policy, Policy};

use super::harness::{ChannelIo, Driver, End, Ends, Input, Live, LiveSession, Pump, Typed};
use crate::horse::grok::{self as codec, check_version, HARNESS, VERSION};
use crate::horse::inbox::Entry;
use crate::horse::store::Hash;
use anyhow::{bail, Context as _, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use wire::{Client, In, Incoming};

/// The wait before a refused entry is offered again.
const RETRY: Duration = Duration::from_secs(30);

/// What a custody needs from the run.
pub struct CustodyCtx {
    pub store: crate::horse::store::Store,
    pub horse: String,
    /// The store session that this custody holds.
    pub session: String,
    /// The working directory of the agent, as the agent sees it.
    pub cwd: String,
    /// The Grok home (`~/.grok` of the sandbox), as paddock sees it.
    pub grok_home: PathBuf,
    /// The sandbox id: the name of the live record.
    pub run: String,
    /// The longest wait of one loop pass.
    pub tick: Duration,
    /// The longest wait for the answer to a request of paddock, but a
    /// `session/prompt`, which runs as long as its turn.
    pub reply_timeout: Duration,
    /// The longest wait for the agent to exit after its input closed.
    pub close_timeout: Duration,
    /// Where paddock's lines for the user go.
    pub out: Box<dyn Write + Send>,
}

/// What a custody did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub native_id: Option<String>,
    /// Turns that ended while paddock held the channel.
    pub turns: usize,
    /// Turns that ended with the stop reason `cancelled`.
    pub cancelled: usize,
    pub deliveries: usize,
    /// Deliveries whose row came into the store.
    pub landed: usize,
    pub interrupts: usize,
    /// Permission requests that paddock answered.
    pub answers: usize,
    /// Requests and notifications that paddock does not know.
    pub unknown: usize,
    /// Lines that were not JSON-RPC.
    pub malformed: usize,
    /// Ingest passes that failed (each one is logged).
    pub ingest_errors: usize,
    /// The agent closed its output before the custody ended.
    pub exited: bool,
}

/// Run the custody until it ends (`ends`) or the agent exits.
pub fn run<R, W>(io: ChannelIo<R, W>, ctx: CustodyCtx, policy: Policy, ends: Ends) -> Result<Report>
where
    R: Read + Send + 'static,
    W: Write,
{
    let (tx, rx) = std::sync::mpsc::channel();
    wire::read_agent(io.from, tx);
    let mut d = Driver::new(ctx.store.clone(), &ctx.horse, &ctx.run, ctx.tick)
        .close_wait(ctx.close_timeout)
        .ends(ends);
    let mut g = Grok::start(io.to, rx, ctx, policy)?;
    d.drive(&mut g, &io.control)?;
    Ok(g.report())
}

enum Pending {
    Init,
    Open { load: Option<String> },
    Prompt { entry: Entry },
}

struct Waiting {
    pending: Pending,
    method: &'static str,
    sent: Instant,
}

/// A delivered entry whose row is not in the store yet.
struct Unlanded {
    id: String,
    text: String,
    /// The length of the chain when it went out.
    from: usize,
}

/// The state of the session after a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// A safe point: no turn runs and no request is on the way.
    Idle,
    Busy,
    Closed,
}

/// One Grok session in custody.
pub struct Grok<W: Write> {
    ctx: CustodyCtx,
    policy: Policy,
    client: Client<W>,
    rx: Receiver<In>,
    /// The id of the open session.
    id: Option<String>,
    /// The id that a `session/load` asked for, until it answered.
    loading: Option<String>,
    head: Option<Hash>,
    waiting: HashMap<i64, Waiting>,
    unlanded: Vec<Unlanded>,
    /// After a refused entry, no entry goes out until this time.
    retry_at: Option<Instant>,
    logged: HashSet<String>,
    closed: bool,
    report: Report,
}

impl<W: Write> Grok<W> {
    /// Initialize the agent and open the session.
    pub fn start(w: W, rx: Receiver<In>, ctx: CustodyCtx, policy: Policy) -> Result<Grok<W>> {
        let s = ctx.store.session(&ctx.session)?;
        if s.horse != ctx.horse {
            bail!("session {} is not of horse {}", s.id, ctx.horse);
        }
        if s.binding.harness != HARNESS {
            bail!(
                "session {} is a {} session, not a Grok session",
                s.id,
                s.binding.harness
            );
        }
        check_version(&s.binding.version)?;
        let load = match &s.binding.native_id {
            Some(id) => {
                let dir = codec::session_dir(&ctx.grok_home, &ctx.cwd, id);
                if !dir.join(codec::SUMMARY_FILE).is_file() {
                    bail!(
                        "session {} has no Grok session folder {}: project it first",
                        s.id,
                        dir.display()
                    );
                }
                Some(id.clone())
            }
            None if s.head.is_some() => bail!(
                "session {} has rows but no Grok session id: project it first",
                s.id
            ),
            None => None,
        };
        let mut g = Grok {
            ctx,
            policy,
            client: Client::new(w),
            rx,
            id: None,
            loading: load.clone(),
            head: s.head.clone(),
            waiting: HashMap::new(),
            unlanded: Vec::new(),
            retry_at: None,
            logged: HashSet::new(),
            closed: false,
            report: Report::default(),
        };
        g.send(wire::INITIALIZE, wire::initialize(), Pending::Init)?;
        g.settle()?;
        let cwd = g.ctx.cwd.clone();
        let (method, p) = match &load {
            Some(id) => (wire::SESSION_LOAD, wire::session_load(id, &cwd)),
            None => (wire::SESSION_NEW, wire::session_new(&cwd)),
        };
        g.send(method, p, Pending::Open { load })?;
        g.settle()?;
        Ok(g)
    }

    /// What the custody did so far.
    pub fn report(&self) -> Report {
        let mut r = self.report.clone();
        r.native_id = self.id.clone();
        r
    }

    fn say(&mut self, line: &str) {
        let _ = writeln!(self.ctx.out, "{line}");
        let _ = self.ctx.out.flush();
    }

    fn say_once(&mut self, key: &str, line: &str) {
        if self.logged.insert(key.to_string()) {
            self.say(line);
        }
    }

    fn send(&mut self, method: &'static str, params: Value, pending: Pending) -> Result<()> {
        let id = self.client.request(method, params)?;
        let sent = Instant::now();
        self.waiting.insert(
            id,
            Waiting {
                pending,
                method,
                sent,
            },
        );
        Ok(())
    }

    /// Read until no request of the start is open.
    fn settle(&mut self) -> Result<()> {
        while self
            .waiting
            .values()
            .any(|w| matches!(w.pending, Pending::Init | Pending::Open { .. }))
        {
            if self.read(self.ctx.tick)? == Wait::Closed {
                bail!("grok agent exited before the session was open");
            }
        }
        Ok(())
    }

    /// Read what came for up to `timeout`, then what is there already.
    fn read(&mut self, timeout: Duration) -> Result<Wait> {
        if self.closed {
            return Ok(Wait::Closed);
        }
        let mut first = Some(timeout);
        loop {
            let m = match first.take() {
                Some(t) => self.rx.recv_timeout(t),
                None => self.rx.try_recv().map_err(|e| match e {
                    std::sync::mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                    std::sync::mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
                }),
            };
            match m {
                Ok(In::Msg(m)) => self.handle(m)?,
                Ok(In::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                    self.report.exited = true;
                    return Ok(Wait::Closed);
                }
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        let late = self.waiting.values().find(|w| {
            !matches!(w.pending, Pending::Prompt { .. })
                && w.sent.elapsed() > self.ctx.reply_timeout
        });
        if let Some(w) = late {
            bail!(
                "grok agent did not answer {} within {} s",
                w.method,
                self.ctx.reply_timeout.as_secs()
            );
        }
        Ok(self.state())
    }

    fn state(&self) -> Wait {
        if self.closed {
            Wait::Closed
        } else if self.id.is_none() || !self.waiting.is_empty() {
            Wait::Busy
        } else {
            Wait::Idle
        }
    }

    fn handle(&mut self, m: Incoming) -> Result<()> {
        match m {
            Incoming::Response { id, result } => {
                let Some(w) = id.as_i64().and_then(|i| self.waiting.remove(&i)) else {
                    return Ok(());
                };
                self.response(w, result)
            }
            Incoming::Request { id, method, params } => self.answer(&id, &method, &params),
            Incoming::Note { method, params } => self.note(&method, &params),
            Incoming::Malformed => {
                self.report.malformed += 1;
                self.say_once(
                    "malformed",
                    "paddock: grok wrote a line that is not JSON-RPC; such lines are skipped",
                );
                Ok(())
            }
        }
    }

    fn response(&mut self, w: Waiting, r: std::result::Result<Value, String>) -> Result<()> {
        match (w.pending, r) {
            (Pending::Init, Ok(v)) => match wire::agent_version(&v) {
                Some(found) => check_version(&found),
                // The binary said its version before the sandbox started.
                None => Ok(()),
            },
            (Pending::Init, Err(e)) => bail!("grok agent refused initialize: {e}"),
            (Pending::Open { load }, Ok(v)) => self.opened(load, &v),
            (Pending::Open { load: Some(id) }, Err(e)) => {
                bail!("grok agent cannot load session {id}: {e}")
            }
            (Pending::Open { load: None }, Err(e)) => {
                bail!("grok agent cannot start a session: {e}")
            }
            (Pending::Prompt { .. }, Ok(v)) => {
                self.report.turns += 1;
                let why = wire::stop_reason(&v);
                if why == "cancelled" {
                    self.report.cancelled += 1;
                }
                if why != "end_turn" {
                    self.say(&format!("paddock: the grok turn ended: {why}"));
                }
                self.ingest()
            }
            (Pending::Prompt { entry, .. }, Err(e)) => self.refused(&entry, &e),
        }
    }

    /// The answer to `session/new` or `session/load`. A loaded session
    /// must have the id that paddock asked for; a new one is bound to the
    /// store session.
    fn opened(&mut self, load: Option<String>, v: &Value) -> Result<()> {
        let id = match load {
            Some(want) => {
                if let Some(got) = wire::session_id(v).filter(|g| *g != want) {
                    bail!(
                        "grok agent loaded session {got} instead of {want}: custody ends, the store is unchanged"
                    );
                }
                want
            }
            None => {
                let id = wire::session_id(v).context("grok agent answered without a session id")?;
                crate::horse::claude::check_session_id(&id)
                    .with_context(|| format!("grok agent started session {id}"))?;
                let st = &self.ctx.store;
                if let Some(other) = st.session_by_native(HARNESS, &id)? {
                    bail!(
                        "Grok session {id} is bound to store session {} already",
                        other.id
                    );
                }
                st.lock()?.bind_native(&self.ctx.session, &id)?;
                id
            }
        };
        self.loading = None;
        self.say(&format!(
            "paddock: grok session {id} in custody for session {}",
            self.ctx.session
        ));
        self.id = Some(id);
        Ok(())
    }

    fn refused(&mut self, e: &Entry, why: &str) -> Result<()> {
        self.ctx.store.retract_delivery(&e.session, &e.id)?;
        self.unlanded.retain(|u| u.id != e.id);
        self.report.deliveries = self.report.deliveries.saturating_sub(1);
        self.retry_at = Some(Instant::now() + RETRY);
        self.say(&format!(
            "paddock: grok did not take append {}: {why}; it stays in the inbox",
            e.id
        ));
        Ok(())
    }

    fn answer(&mut self, id: &Value, method: &str, params: &Value) -> Result<()> {
        if method != wire::REQUEST_PERMISSION {
            self.report.unknown += 1;
            self.client.respond_error(
                id,
                wire::NO_METHOD,
                &format!("paddock does not answer {method}"),
            )?;
            self.say_once(
                &format!("request:{method}"),
                &format!("paddock: grok asked {method}, which paddock does not answer"),
            );
            return Ok(());
        }
        let (result, decision) = self.policy.answer(params);
        self.client.respond(id, result)?;
        let line = ledger_line(params, &decision, &self.policy);
        super::codex::append_ledger(&self.ctx.store, &self.ctx.session, &line)?;
        self.report.answers += 1;
        self.say(&format!("paddock: answered {method}: {decision}"));
        Ok(())
    }

    fn note(&mut self, method: &str, params: &Value) -> Result<()> {
        match method {
            wire::SESSION_UPDATE => {
                let Some(sid) = wire::session_id(params) else {
                    return Ok(());
                };
                if let Some(want) = self.loading.clone().filter(|w| *w != sid) {
                    bail!(
                        "grok agent sent the history of session {sid} for a load of {want}: custody ends, the store is unchanged"
                    );
                }
                if self.id.as_ref().is_some_and(|ours| *ours != sid) {
                    self.say_once(
                        &format!("other:{sid}"),
                        &format!("paddock: grok sent updates of session {sid}, which this custody does not hold; ignored"),
                    );
                }
                Ok(())
            }
            wire::XAI_NOTIFICATION => Ok(()),
            m => {
                self.report.unknown += 1;
                self.say_once(
                    &format!("note:{m}"),
                    &format!("paddock: grok sent {m}, which paddock does not read"),
                );
                Ok(())
            }
        }
    }

    /// Ingest the session folder by record identity, then refresh the
    /// guard and look for the rows of delivered entries. A pass that fails
    /// is logged and tried again after the next turn.
    fn ingest(&mut self) -> Result<()> {
        let Some(id) = self.id.clone() else {
            return Ok(());
        };
        let dir = codec::session_dir(&self.ctx.grok_home, &self.ctx.cwd, &id);
        if !dir.join(codec::SUMMARY_FILE).is_file() {
            self.say_once(
                "no-folder",
                &format!("paddock: no Grok session folder {} yet", dir.display()),
            );
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let r = st
            .lock()
            .and_then(|l| codec::ingest_locked(&l, &dir, VERSION, Some(&self.ctx.horse)));
        let i = match r {
            Ok(i) => i,
            Err(e) => {
                self.report.ingest_errors += 1;
                self.say(&format!(
                    "paddock: cannot ingest the Grok session {id}: {e:#}"
                ));
                return Ok(());
            }
        };
        if i.session != self.ctx.session {
            bail!(
                "the Grok session {id} went into session {}, not {}",
                i.session,
                self.ctx.session
            );
        }
        self.head = st.session(&self.ctx.session)?.head;
        st.refresh_guard(&self.ctx.session)?;
        self.land()
    }

    fn land(&mut self) -> Result<()> {
        if self.unlanded.is_empty() {
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let nodes = st.session_nodes(&st.session(&self.ctx.session)?)?;
        let mut keep = Vec::new();
        for u in std::mem::take(&mut self.unlanded) {
            let at = (u.from..nodes.len()).find(|&k| user_row_holds(&nodes[k].raw, &u.text));
            match at {
                Some(k) if st.delivery_landed(&self.ctx.session, &u.id, k as u64)? => {
                    self.report.landed += 1;
                }
                Some(_) => {}
                None => keep.push(u),
            }
        }
        self.unlanded = keep;
        Ok(())
    }

    fn deliver_entry(&mut self, e: &Entry) -> Result<()> {
        if self.state() != Wait::Idle {
            bail!("paddock delivers only at a safe point");
        }
        if e.session != self.ctx.session {
            bail!("session {} does not run in this custody", e.session);
        }
        let id = self.id.clone().context("no session is open")?;
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            // A refused entry waits a while before it goes out again; the
            // session is still at a safe point, so the run may end.
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let head = st.session(&e.session)?.head;
        let from = st.chain(head.as_ref())?.len();
        let text = e.wire_text();
        // The record comes first: a recorded entry never goes out twice.
        st.record_delivery(e, head.as_ref(), &e.id)?;
        if let Err(err) = self.send(
            wire::SESSION_PROMPT,
            wire::session_prompt(&id, &text),
            Pending::Prompt { entry: e.clone() },
        ) {
            st.retract_delivery(&e.session, &e.id)?;
            return Err(err);
        }
        self.report.deliveries += 1;
        self.retry_at = None;
        self.unlanded.push(Unlanded {
            id: e.id.clone(),
            text,
            from,
        });
        self.say(&format!(
            "paddock: delivered append {} ({}) to session {}",
            e.id, e.role, e.session
        ));
        Ok(())
    }
}

/// A user row of the session folder with `text`: an `updates.jsonl` user
/// chunk or a `chat_history.jsonl` user item.
pub(crate) fn user_row_holds(raw: &[u8], text: &str) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(raw) else {
        return false;
    };
    let want = text.trim();
    let is = |t: Option<&str>| t.is_some_and(|t| t.trim() == want);
    let update = &v["params"]["update"];
    if update["sessionUpdate"] == "user_message_chunk" {
        return is(update["content"]["text"].as_str());
    }
    v["type"] == "user"
        && v.get("synthetic_reason").is_none()
        && v["content"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| is(x["text"].as_str())))
}

impl<W: Write> Live for Grok<W> {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        Ok(match self.read(timeout)? {
            Wait::Closed => Pump::Closed,
            _ => Pump::Open,
        })
    }

    fn main_session(&self) -> Option<String> {
        Some(self.ctx.session.clone())
    }

    fn sessions(&self) -> Vec<LiveSession> {
        let idle = self.state() == Wait::Idle;
        self.id
            .iter()
            .map(|id| LiveSession::turns(&self.ctx.session, id, idle, self.head.clone()))
            .collect()
    }

    fn safe(&self, session: Option<&str>) -> bool {
        session.is_none_or(|s| s == self.ctx.session) && self.state() == Wait::Idle
    }

    fn deliver(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Entry(e) => self.deliver_entry(&e),
            // Typed lines go through the inbox (`typed`): one that comes
            // here still gets an entry, so that it has a delivery record.
            Input::Typed(t) => {
                let e = self.ctx.store.queue_append(&self.ctx.session, "user", &t)?;
                self.deliver_entry(&e)
            }
        }
    }

    fn interrupt(&mut self) -> Result<bool> {
        let running = self
            .waiting
            .values()
            .any(|w| matches!(w.pending, Pending::Prompt { .. }));
        let Some(id) = self.id.clone().filter(|_| running) else {
            return Ok(false);
        };
        self.client
            .notify(wire::SESSION_CANCEL, wire::session_cancel(&id))?;
        self.report.interrupts += 1;
        Ok(true)
    }

    fn close_input(&mut self) -> Result<()> {
        self.client.close();
        Ok(())
    }

    fn finish(&mut self, end: End) -> Result<()> {
        if end == End::TimedOut {
            self.say(&format!(
                "paddock: grok agent did not exit within {} s after its input closed",
                self.ctx.close_timeout.as_secs()
            ));
        }
        self.ingest()
    }

    fn typed(&self) -> Typed {
        Typed::Inbox
    }
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
