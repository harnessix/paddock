//! Custody of one Codex thread over `codex app-server` on stdio.
//!
//! paddock initializes the server, resumes the thread of the session (a
//! session with a projected rollout) or starts a new one (a session
//! without rows), and then holds the turns. A turn is busy from
//! `turn/start` (or a `turn/started` of a turn that paddock did not
//! start) until its `turn/completed`, whatever its status. Only then does
//! an inbox entry go out: a `user` entry as `turn/start`, a `system`,
//! `developer` or `note` entry as one developer message through
//! `thread/inject_items`. A `turn/start` during a running turn steers that
//! turn, so paddock never sends one then, and never uses `turn/steer`.
//! After every turn paddock ingests the rollout file by record identity.
//! The cache of a Codex thread is keyed by its root thread id, which is
//! the native id of the session and so the native id of its guard tuple.
//!
//! A warm side branch (side.rs) is an ephemeral `thread/fork` of the
//! thread in the same server; while it is open, its turns run and the
//! thread of the session waits.
//!
//! The custodian is a [`Live`]: the driver of harness.rs runs the loop. A
//! typed line goes into the inbox like an append ([`Typed::Inbox`]), so it
//! has a delivery record like any entry.

mod approve;
mod reload;
pub(crate) mod rollout;
pub mod side;
pub mod wire;

pub use approve::{args, ledger, policy, Answer, Policy};
pub use rollout::find_rollout;
use rollout::holds;

use super::harness::{ChannelIo, Driver, End, Ends, Input, Live, LiveSession, Pump, Typed};
use crate::horse::codex::{self as codec, check_version, HARNESS, VERSION};
use crate::horse::inbox::Entry;
use crate::horse::store::Hash;
use anyhow::{bail, Context as _, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use wire::{Client, In, Incoming, Note};

/// The wait before a refused entry is offered again, when no turn ends
/// before it.
const RETRY: Duration = Duration::from_secs(30);

/// What a custody needs from the run.
pub struct CustodyCtx {
    pub store: crate::horse::store::Store,
    pub horse: String,
    /// The store session that this custody holds.
    pub session: String,
    /// The working directory of the harness, as the harness sees it.
    pub cwd: String,
    /// The folder of the native session files, as paddock sees it.
    pub sessions_root: PathBuf,
    /// The sandbox id: the name of the live record.
    pub run: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The longest wait of one loop pass.
    pub tick: Duration,
    /// The longest wait for the answer to a request of paddock.
    pub reply_timeout: Duration,
    /// The longest wait for the harness to exit after its input closed.
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
    pub deliveries: usize,
    /// Deliveries whose row came into the store.
    pub landed: usize,
    pub interrupts: usize,
    /// Requests of the harness that paddock answered (approvals and more).
    pub answers: usize,
    /// Notifications that paddock does not know.
    pub unknown: usize,
    /// Lines that were not JSON-RPC.
    pub malformed: usize,
    /// Ingest passes that failed (each one is logged).
    pub ingest_errors: usize,
    /// The total tokens of the thread after its last turn.
    pub tokens: Option<u64>,
    /// The harness closed its output before the custody ended.
    pub exited: bool,
    /// Turns that ended on a warm side branch (side.rs).
    pub side_turns: usize,
}

/// The state of the thread after a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// A safe point: no turn runs and no request is on the way.
    Idle,
    Busy,
    /// The server closed its output.
    Closed,
}

/// Run the custody until it ends (`ends`) or the server exits.
pub fn run<R, W>(io: ChannelIo<R, W>, ctx: CustodyCtx, policy: Policy, ends: Ends) -> Result<Report>
where
    R: Read + Send + 'static,
    W: Write,
{
    let (tx, rx) = std::sync::mpsc::channel();
    wire::read_server(io.from, tx);
    let mut d = Driver::new(ctx.store.clone(), &ctx.horse, &ctx.run, ctx.tick)
        .close_wait(ctx.close_timeout)
        .ends(ends);
    let mut c = Codex::start(io.to, rx, ctx, policy)?;
    d.drive(&mut c, &io.control)?;
    Ok(c.report())
}

enum Pending {
    Init,
    Thread {
        resume: Option<String>,
    },
    TurnStart {
        entry: Option<Entry>,
        head: Option<Hash>,
    },
    Inject {
        entry: Entry,
        head: Option<Hash>,
    },
    Interrupt {
        turn: String,
    },
    /// `thread/unsubscribe` before a reload.
    Unsubscribe,
    /// `thread/fork` of a warm side branch.
    Fork(Box<side::Opening>),
    /// `thread/unsubscribe` of the side branch at its close.
    SideClose(Box<side::request::Request>),
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

/// One Codex thread in custody.
pub struct Codex<W: Write> {
    ctx: CustodyCtx,
    policy: Policy,
    client: Client<W>,
    rx: Receiver<In>,
    thread: Option<String>,
    /// The head of the session after the last ingest.
    head: Option<Hash>,
    waiting: HashMap<i64, Waiting>,
    /// The running turn.
    busy: Option<String>,
    /// Turns that ended, newest last, for a `turn/completed` that comes
    /// before the answer to its `turn/start`.
    done: VecDeque<String>,
    interrupt_wanted: bool,
    unlanded: Vec<Unlanded>,
    /// A refused entry waits until a turn ends, or until this time.
    retry_at: Option<Instant>,
    rollout: Option<PathBuf>,
    logged: HashSet<String>,
    closed: bool,
    /// A reload runs: the thread is closed or not open yet.
    reloading: Option<reload::Step>,
    /// The warm side branch, while it is open, and the side request in
    /// work.
    side: Option<side::Side>,
    side_asked: Option<String>,
    report: Report,
}

impl<W: Write> Codex<W> {
    /// Initialize the server and open the thread of the session.
    pub fn start(w: W, rx: Receiver<In>, ctx: CustodyCtx, policy: Policy) -> Result<Codex<W>> {
        let s = ctx.store.session(&ctx.session)?;
        if s.horse != ctx.horse {
            bail!("session {} is not of horse {}", s.id, ctx.horse);
        }
        if s.binding.harness != HARNESS {
            bail!(
                "session {} is a {} session, not a Codex session",
                s.id,
                s.binding.harness
            );
        }
        check_version(&s.binding.version)?;
        let resume = match &s.binding.native_id {
            Some(id) => {
                if find_rollout(&ctx.sessions_root, id).is_none() {
                    bail!(
                        "session {} has no rollout file of thread {id} under {}: project it first",
                        s.id,
                        ctx.sessions_root.display()
                    );
                }
                Some(id.clone())
            }
            None if s.head.is_some() => bail!(
                "session {} has rows but no Codex thread: project it first",
                s.id
            ),
            None => None,
        };
        let mut c = Codex {
            ctx,
            policy,
            client: Client::new(w),
            rx,
            thread: None,
            head: s.head.clone(),
            waiting: HashMap::new(),
            busy: None,
            done: VecDeque::new(),
            interrupt_wanted: false,
            unlanded: Vec::new(),
            retry_at: None,
            rollout: None,
            logged: HashSet::new(),
            closed: false,
            reloading: None,
            side: None,
            side_asked: None,
            report: Report::default(),
        };
        c.send(wire::INITIALIZE, wire::initialize(), Pending::Init)?;
        c.settle()?;
        c.client.notify(wire::INITIALIZED)?;
        let model = c.ctx.model.clone();
        let p = wire::thread_params(&c.ctx.cwd, &c.policy, model.as_deref(), resume.as_deref());
        let method = if resume.is_some() {
            wire::THREAD_RESUME
        } else {
            wire::THREAD_START
        };
        c.send(method, p, Pending::Thread { resume })?;
        c.settle()?;
        Ok(c)
    }

    /// What the custody did so far.
    pub fn report(&self) -> Report {
        let mut r = self.report.clone();
        r.native_id = self.thread.clone();
        r
    }

    fn say(&mut self, line: &str) {
        let _ = writeln!(self.ctx.out, "{line}");
        let _ = self.ctx.out.flush();
    }

    /// Say a line once per key.
    fn say_once(&mut self, key: &str, line: &str) {
        if self.logged.insert(key.to_string()) {
            self.say(line);
        }
    }

    fn send(&mut self, method: &'static str, params: Value, pending: Pending) -> Result<()> {
        let id = self.client.request(method, params)?;
        self.waiting.insert(
            id,
            Waiting {
                pending,
                method,
                sent: Instant::now(),
            },
        );
        Ok(())
    }

    /// Read until no request of the start is open.
    fn settle(&mut self) -> Result<()> {
        while self
            .waiting
            .values()
            .any(|w| matches!(w.pending, Pending::Init | Pending::Thread { .. }))
        {
            if self.read(self.ctx.tick)? == Wait::Closed {
                bail!("codex app-server exited before the thread was open");
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
        if let Some(w) = self
            .waiting
            .values()
            .find(|w| w.sent.elapsed() > self.ctx.reply_timeout)
        {
            bail!(
                "codex app-server did not answer {} within {} s",
                w.method,
                self.ctx.reply_timeout.as_secs()
            );
        }
        Ok(self.state())
    }

    fn state(&self) -> Wait {
        let sending = self
            .waiting
            .values()
            .any(|w| !matches!(w.pending, Pending::Interrupt { .. }));
        let held = self.retry_at.is_some_and(|t| Instant::now() < t);
        if self.thread.is_none()
            || self.busy.is_some()
            || sending
            || held
            || self.reloading.is_some()
        {
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
            Incoming::Note(n) => self.note(n),
            Incoming::Malformed => {
                self.report.malformed += 1;
                self.say_once(
                    "malformed",
                    "paddock: codex wrote a line that is not JSON-RPC; such lines are skipped",
                );
                Ok(())
            }
        }
    }

    fn response(&mut self, w: Waiting, r: std::result::Result<Value, String>) -> Result<()> {
        match (w.pending, r) {
            (Pending::Init, Ok(_)) => Ok(()),
            (Pending::Init, Err(e)) => bail!("codex app-server refused initialize: {e}"),
            (Pending::Thread { resume }, Ok(v)) => self.opened(resume, &v),
            (Pending::Thread { resume: Some(id) }, Err(e)) => {
                bail!("codex app-server cannot resume thread {id}: {e}")
            }
            (Pending::Thread { resume: None }, Err(e)) => {
                bail!("codex app-server cannot start a thread: {e}")
            }
            (Pending::TurnStart { entry, head }, Ok(v)) => {
                let turn = v.get("turn").and_then(wire::Turn::from);
                if let Some(t) = turn.filter(|t| !self.done.contains(&t.id)) {
                    self.busy = Some(t.id);
                }
                if let Some(e) = entry {
                    self.delivered(&e, head.as_ref())?;
                }
                if self.interrupt_wanted {
                    self.interrupt_wanted = false;
                    self.interrupt_now()?;
                }
                Ok(())
            }
            (Pending::Inject { entry, head }, Ok(_)) => {
                self.delivered(&entry, head.as_ref())?;
                // The row can be in the rollout already.
                self.ingest()
            }
            (Pending::TurnStart { entry, .. }, Err(e)) => {
                self.interrupt_wanted = false;
                self.refused(entry.as_ref(), &e);
                Ok(())
            }
            (Pending::Inject { entry, .. }, Err(e)) => {
                self.refused(Some(&entry), &e);
                Ok(())
            }
            (Pending::Interrupt { turn }, Err(e)) => {
                self.say(&format!(
                    "paddock: codex did not interrupt turn {turn}: {e}"
                ));
                Ok(())
            }
            (Pending::Interrupt { .. }, Ok(_)) => Ok(()),
            (Pending::Unsubscribe, r) => self.unsubscribed(r),
            (Pending::Fork(o), r) => self.forked(o, r),
            (Pending::SideClose(q), r) => self.side_closed(q, r),
        }
    }

    /// The answer to `thread/start` or `thread/resume`. A resumed thread
    /// must have the id that paddock asked for; a new one is bound to the
    /// session.
    fn opened(&mut self, resume: Option<String>, v: &Value) -> Result<()> {
        let (id, cli) = wire::thread_of(v).context("codex app-server answered without a thread")?;
        match cli {
            Some(c) => check_version(&c)?,
            None => bail!("codex app-server did not say its version"),
        }
        match resume {
            Some(want) if want != id => bail!(
                "codex app-server resumed thread {id} instead of {want}: custody ends, the store is unchanged"
            ),
            Some(_) => {}
            None => {
                crate::horse::claude::check_session_id(&id)
                    .with_context(|| format!("codex app-server started thread {id}"))?;
                let st = &self.ctx.store;
                if let Some(other) = st.session_by_native(HARNESS, &id)? {
                    bail!("thread {id} is bound to session {} already", other.id);
                }
                st.update_session(&self.ctx.session, |s| s.binding.native_id = Some(id.clone()))?;
            }
        }
        self.say(&format!(
            "paddock: codex thread {id} in custody for session {}",
            self.ctx.session
        ));
        self.thread = Some(id);
        Ok(())
    }

    fn delivered(&mut self, e: &Entry, head: Option<&Hash>) -> Result<()> {
        let st = &self.ctx.store;
        let from = st.chain(head)?.len();
        st.record_delivery(e, head, &e.id)?;
        self.report.deliveries += 1;
        self.retry_at = None;
        if self.side.as_ref().is_some_and(|s| s.session == e.session) {
            self.side_input(e)?;
        } else {
            self.unlanded.push(Unlanded {
                id: e.id.clone(),
                text: e.wire_text(),
                from,
            });
        }
        self.say(&format!(
            "paddock: delivered append {} ({}) to session {}",
            e.id, e.role, e.session
        ));
        Ok(())
    }

    fn refused(&mut self, e: Option<&Entry>, why: &str) {
        if let Some(x) = e {
            self.side_refused(&x.id);
        }
        let what = e.map_or("a turn".to_string(), |x| format!("append {}", x.id));
        self.retry_at = Some(Instant::now() + RETRY);
        self.say(&format!(
            "paddock: codex did not take {what}: {why}; it stays in the inbox until the next turn ends"
        ));
    }

    fn answer(&mut self, id: &Value, method: &str, params: &Value) -> Result<()> {
        let a = self.policy.answer(method);
        match &a {
            Answer::Result(v, _) => self.client.respond(id, v.clone())?,
            Answer::Error(code, m) => self.client.respond_error(id, *code, m)?,
        }
        let line = approve::ledger_line(method, params, &a, &self.policy);
        approve::append_ledger(&self.ctx.store, &self.ctx.session, &line)?;
        self.report.answers += 1;
        self.say(&format!("paddock: answered {method}: {}", a.decision()));
        Ok(())
    }

    /// The thread whose turns run now: the side branch while it is open.
    fn ours(&self, thread: &str) -> bool {
        self.active().1 == Some(thread)
    }

    fn note(&mut self, n: Note) -> Result<()> {
        match n {
            Note::TurnStarted { thread, turn } if self.ours(&thread) => {
                if self.busy.is_none() && !self.done.contains(&turn.id) {
                    self.busy = Some(turn.id);
                }
            }
            Note::TurnCompleted { thread, turn } if self.ours(&thread) => {
                if self.busy.as_ref().is_some_and(|b| *b != turn.id) {
                    let b = self.busy.clone().unwrap_or_default();
                    self.say(&format!(
                        "paddock: turn {} ended while turn {b} runs",
                        turn.id
                    ));
                    return Ok(());
                }
                self.busy = None;
                self.done.push_back(turn.id.clone());
                if self.done.len() > 32 {
                    self.done.pop_front();
                }
                self.report.turns += 1;
                self.retry_at = None;
                if turn.status != "completed" {
                    let why = turn.error.map(|e| format!(": {e}")).unwrap_or_default();
                    self.say(&format!("paddock: turn {} {}{why}", turn.id, turn.status));
                }
                match self.side.is_some() {
                    true => self.side_turn_done(&turn.id),
                    false => self.ingest()?,
                }
            }
            Note::RawItem { thread, item } if self.is_side(&thread) => self.side_item(item)?,
            Note::TokenUsage { thread, total } if self.thread.as_deref() == Some(&thread) => {
                self.report.tokens = total.or(self.report.tokens);
            }
            Note::Error { message } => self.say(&format!("codex: {message}")),
            Note::Other(m) => {
                self.report.unknown += 1;
                self.say_once(
                    &format!("note:{m}"),
                    &format!("paddock: codex sent {m}, which paddock does not read"),
                );
            }
            Note::ThreadClosed { thread } if self.is_side(&thread) => self.side_thread_closed(),
            Note::ThreadClosed { thread } if self.thread.as_deref() == Some(&thread) => {
                self.thread_closed()
            }
            Note::TurnStarted { .. }
            | Note::RawItem { .. }
            | Note::TurnCompleted { .. }
            | Note::TokenUsage { .. }
            | Note::ThreadStarted { .. }
            | Note::ThreadClosed { .. } => {}
        }
        Ok(())
    }

    /// Ingest the rollout file by record identity, then refresh the guard
    /// and look for the rows of delivered entries. A pass that fails is
    /// logged and tried again after the next turn.
    fn ingest(&mut self) -> Result<()> {
        let Some(thread) = self.thread.clone() else {
            return Ok(());
        };
        let path = match self.rollout.clone() {
            Some(p) if p.is_file() => p,
            _ => match find_rollout(&self.ctx.sessions_root, &thread) {
                Some(p) => p,
                None => {
                    self.say_once(
                        "no-rollout",
                        &format!(
                            "paddock: no rollout file of thread {thread} under {} yet",
                            self.ctx.sessions_root.display()
                        ),
                    );
                    return Ok(());
                }
            },
        };
        self.rollout = Some(path.clone());
        let st = self.ctx.store.clone();
        let r = st
            .lock()
            .and_then(|l| codec::ingest_run_locked(&l, &path, VERSION, &self.ctx.horse));
        let i = match r {
            Ok(i) => i,
            Err(e) => {
                self.report.ingest_errors += 1;
                self.say(&format!(
                    "paddock: cannot ingest the rollout of thread {thread}: {e:#}"
                ));
                return Ok(());
            }
        };
        if i.session != self.ctx.session {
            bail!(
                "the rollout of thread {thread} went into session {}, not {}",
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
            let at = nodes
                .iter()
                .enumerate()
                .skip(u.from)
                .find(|(_, n)| holds(&n.raw, &u.text))
                .map(|(k, _)| k);
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

    fn interrupt_now(&mut self) -> Result<bool> {
        let thread = self.active().1.map(String::from);
        let (Some(thread), Some(turn)) = (thread, self.busy.clone()) else {
            return Ok(false);
        };
        let p = wire::turn_interrupt(&thread, &turn);
        self.send(wire::TURN_INTERRUPT, p, Pending::Interrupt { turn })?;
        self.report.interrupts += 1;
        Ok(true)
    }
}

impl<W: Write> Codex<W> {
    fn deliver_entry(&mut self, e: &Entry) -> Result<()> {
        if self.state() != Wait::Idle {
            bail!("paddock delivers only at a safe point");
        }
        let (session, thread) = self.active();
        if e.session != session {
            bail!("session {} does not run in this custody now", e.session);
        }
        let thread = thread.context("no thread is open")?.to_string();
        let head = self.ctx.store.session(&e.session)?.head;
        self.side_sent(e);
        let text = e.wire_text();
        if e.role == "user" {
            let effort = self.ctx.effort.clone();
            let p = wire::turn_start(&thread, &text, &e.id, effort.as_deref());
            let entry = Some(e.clone());
            self.send(wire::TURN_START, p, Pending::TurnStart { entry, head })
        } else {
            let p = wire::inject_items(&thread, "developer", &text);
            let entry = e.clone();
            self.send(wire::INJECT_ITEMS, p, Pending::Inject { entry, head })
        }
    }
}

impl<W: Write> Live for Codex<W> {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        let w = self.read(timeout)?;
        if w == Wait::Idle {
            self.serve_side()?;
        }
        Ok(if w == Wait::Closed {
            Pump::Closed
        } else {
            Pump::Open
        })
    }

    fn main_session(&self) -> Option<String> {
        Some(self.ctx.session.clone())
    }

    fn sessions(&self) -> Vec<LiveSession> {
        let idle = self.state() == Wait::Idle;
        let main = self.thread.iter().map(|t| {
            let idle = idle || self.side.is_some();
            LiveSession::turns(&self.ctx.session, t, idle, self.head.clone())
        });
        let side = self
            .side
            .iter()
            .map(|s| LiveSession::turns(&s.session, &s.thread, idle, s.head.clone()));
        main.chain(side).collect()
    }

    fn safe(&self, session: Option<&str>) -> bool {
        let ok = session.is_none_or(|s| s == self.active().0);
        !self.closed && self.state() == Wait::Idle && ok
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
        if self.busy.is_some() {
            return self.interrupt_now();
        }
        let starting = self
            .waiting
            .values()
            .any(|w| matches!(w.pending, Pending::TurnStart { .. }));
        self.interrupt_wanted = starting;
        Ok(starting)
    }

    fn close_input(&mut self) -> Result<()> {
        self.client.close();
        Ok(())
    }

    fn finish(&mut self, end: End) -> Result<()> {
        if end == End::TimedOut {
            self.say(&format!(
                "paddock: codex app-server did not exit within {} s after its input closed",
                self.ctx.close_timeout.as_secs()
            ));
        }
        self.end_side("closed with the run");
        self.ingest()
    }

    fn typed(&self) -> Typed {
        Typed::Inbox
    }

    fn reloads(&self) -> bool {
        true
    }

    fn reload(
        &mut self,
        session: &str,
        write: &mut dyn FnMut(&Path) -> Result<()>,
    ) -> Result<super::harness::Reloaded> {
        self.reload_thread(session, write)
    }
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
