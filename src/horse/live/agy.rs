//! Custody of one agy (Antigravity CLI) conversation over stream-json.
//!
//! paddock holds a long-lived `agy --input-format stream-json
//! --output-format stream-json` process (wire.rs). A turn starts when
//! paddock writes a `user` line and ends at the next `result` line: the
//! safe point of the documentation. Between turns agy waits on stdin. The
//! custodian delivers inbox entries only between turns, one `user` line
//! each, and reads the conversation database after every turn with the
//! agy codec, which keys every row by its table and step index.
//!
//! stream-json takes text from the user only. So an entry of another role
//! (`system`, `developer`, `note`) goes out as a user line with its role
//! tag (`Entry::wire_text`). A `Stop` hook with `decision: "continue"`
//! could append a system message, but only from a hook process inside the
//! sandbox at the end of a reply, not on the line that paddock holds; and
//! agy would turn a `user` entry into a system message.
//!
//! paddock picks the conversation before the process starts: a session
//! with rows continues its database with `--conversation <id>` (projected
//! first when the sandbox does not have it). agy picks the id of a new
//! conversation, and the first line that names it binds the session.
//! After that, a line that names another conversation ends the custody
//! before the store takes anything of it (fail closed).
//!
//! The interrupt is SIGINT (signal.rs): agy then ends the turn with
//! `finished_reason: "interrupted"`, and that `result` is a safe point
//! like any other. Closing stdin ends agy after the running turn.
//!
//! The custodian is a [`Live`]: the driver of harness.rs runs the loop. A
//! typed line goes into the inbox like an append ([`Typed::Inbox`]).

pub mod signal;
pub mod wire;

use super::claude::{read_runtime, Msg};
use super::harness::{ChannelIo, Driver, End, Ends, Input, Live, LiveSession, Pump, Typed};
use crate::horse::agy::{self as codec, row::Row, HARNESS, VERSION};
use crate::horse::inbox::Entry;
use crate::horse::store::{Hash, Store};
use anyhow::{bail, Context as _, Result};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;
use wire::Line;

/// Stop the running turn of the process.
pub type Interrupt = Box<dyn FnMut() -> Result<()> + Send>;

/// The conversation that a custody starts on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Start {
    /// `--conversation <id>`: a conversation with a database. `None`: agy
    /// starts a new one.
    pub conversation: Option<String>,
}

impl Start {
    /// The conversation of a store session under the agy home `home` (the
    /// `.gemini/antigravity-cli` of the sandbox). A session without rows
    /// starts a new conversation. A session with rows continues its
    /// database, projected when it is not there.
    pub fn of(store: &Store, session: &str, home: &Path) -> Result<Start> {
        let s = store.session(session)?;
        if s.binding.harness != HARNESS {
            bail!("session {session} is not an agy session");
        }
        codec::check_version(&s.binding.version)?;
        let conversation = match (&s.binding.native_id, s.head.is_some()) {
            (_, false) => None,
            (Some(id), true) if db_path(home, id).is_file() => Some(id.clone()),
            (_, true) => Some(codec::project(store, session, home)?.native_id),
        };
        Ok(Start { conversation })
    }
}

/// The database of conversation `id` under the agy home `home`.
pub fn db_path(home: &Path, id: &str) -> PathBuf {
    home.join("conversations").join(format!("{id}.db"))
}

/// The arguments of `agy` for a custody, after the flags of the plain run
/// that stay (`--dangerously-skip-permissions`).
pub fn args(start: &Start) -> Vec<String> {
    let mut a: Vec<String> = [
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ]
    .map(String::from)
    .to_vec();
    if let Some(id) = &start.conversation {
        a.extend(["--conversation".to_string(), id.clone()]);
    }
    a
}

/// What a custodian needs besides the stdio.
pub struct CustodyCtx {
    pub store: Store,
    pub horse: String,
    /// The store session that this custody holds.
    pub session: String,
    pub start: Start,
    /// The agy home of the sandbox (`.gemini/antigravity-cli`), as paddock
    /// sees it.
    pub agy_home: PathBuf,
    /// The sandbox id: the name of the live record.
    pub run: String,
    pub tick: Duration,
    /// The longest wait for agy to exit after its input closed.
    pub close_timeout: Duration,
    pub out: Box<dyn Write + Send>,
    /// SIGINT to the process. `None`: paddock cannot stop a turn.
    pub interrupt: Option<Interrupt>,
}

/// What a custody did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The conversation id.
    pub native_id: Option<String>,
    pub turns: usize,
    /// Turns that SIGINT ended.
    pub interrupted: usize,
    pub deliveries: usize,
    /// Deliveries whose user step came into the store.
    pub landed: usize,
    pub interrupts: usize,
    /// Lines of an event that paddock does not read.
    pub unknown: usize,
    /// Lines that paddock cannot read.
    pub bad: usize,
    /// Ingest passes that failed (each one is logged).
    pub ingest_errors: usize,
    /// agy closed its output before paddock closed its input.
    pub exited: bool,
}

/// A delivered entry whose user step is not in the store yet.
struct Unlanded {
    id: String,
    text: String,
    /// The length of the chain when it went out.
    from: usize,
}

/// One agy conversation in custody.
pub struct Agy<W: Write> {
    ctx: CustodyCtx,
    w: Option<W>,
    rx: Receiver<Msg>,
    /// The conversation id, once paddock knows it.
    id: Option<String>,
    head: Option<Hash>,
    /// A user line went out, and its `result` did not come yet.
    busy: bool,
    /// SIGINT went out during this turn.
    interrupting: bool,
    turn_started: bool,
    interrupt_pending: bool,
    unlanded: Vec<Unlanded>,
    logged: HashSet<String>,
    closed: bool,
    report: Report,
}

/// Run the custody until it ends (`ends`) or agy exits.
pub fn run<R, W>(io: ChannelIo<R, W>, ctx: CustodyCtx, ends: Ends) -> Result<Report>
where
    R: Read + Send + 'static,
    W: Write,
{
    let mut d = Driver::new(ctx.store.clone(), &ctx.horse, &ctx.run, ctx.tick)
        .close_wait(ctx.close_timeout)
        .ends(ends);
    let mut a = Agy::start(io.from, io.to, ctx)?;
    d.drive(&mut a, &io.control)?;
    Ok(a.report())
}

impl<W: Write> Agy<W> {
    /// Check the session and read the stdout of the process.
    pub fn start<R: Read + Send + 'static>(r: R, w: W, ctx: CustodyCtx) -> Result<Agy<W>> {
        let s = ctx.store.session(&ctx.session)?;
        if s.horse != ctx.horse || s.binding.harness != HARNESS {
            bail!(
                "session {} is not an agy session of horse {}",
                s.id,
                ctx.horse
            );
        }
        codec::check_version(&s.binding.version)?;
        if let Some(id) = &ctx.start.conversation {
            if s.binding.native_id.as_deref() != Some(id) {
                bail!("session {} is not agy conversation {id}", s.id);
            }
            if !db_path(&ctx.agy_home, id).is_file() {
                bail!(
                    "agy conversation {id} has no database {}: project it first",
                    db_path(&ctx.agy_home, id).display()
                );
            }
        } else if s.head.is_some() {
            bail!(
                "session {} has rows: agy must continue its conversation",
                s.id
            );
        }
        let (tx, rx) = std::sync::mpsc::channel();
        read_runtime(r, tx);
        Ok(Agy {
            id: ctx.start.conversation.clone(),
            head: s.head,
            ctx,
            w: Some(w),
            rx,
            busy: false,
            interrupting: false,
            turn_started: false,
            interrupt_pending: false,
            unlanded: Vec::new(),
            logged: HashSet::new(),
            closed: false,
            report: Report::default(),
        })
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

    /// No turn runs and the input is open: a safe point.
    fn idle(&self) -> bool {
        !self.busy && !self.closed && self.w.is_some()
    }

    fn line(&mut self, raw: &[u8]) -> Result<()> {
        match wire::parse(raw) {
            Line::Init { conversation } => self.conversation("init", conversation),
            Line::Step { conversation } => {
                self.conversation("step_update", conversation)?;
                if self.busy {
                    self.turn_started = true;
                    if self.interrupt_pending {
                        self.interrupt_pending = false;
                        self.interrupt()?;
                    }
                }
                Ok(())
            }
            Line::Result(r) => {
                self.conversation("result", r.conversation.clone())?;
                if self.id.is_none() {
                    bail!("agy ended a turn before it named its conversation; custody stops");
                }
                self.busy = false;
                self.turn_started = false;
                self.interrupt_pending = false;
                self.interrupting = false;
                self.report.turns += 1;
                if r.interrupted() {
                    self.report.interrupted += 1;
                    self.say("paddock: the agy turn ended on the interrupt");
                } else if !r.ok() {
                    self.say(&format!(
                        "paddock: the agy turn ended with {} ({})",
                        r.status,
                        r.finished_reason.as_deref().unwrap_or("no reason")
                    ));
                }
                if let Some(t) = r.response.filter(|t| !t.is_empty()) {
                    self.say(&format!("agy: {t}"));
                }
                self.ingest()
            }
            Line::Unknown { event } => {
                self.report.unknown += 1;
                self.say_once(
                    &format!("event:{event}"),
                    &format!(
                        "paddock: agy sent a line of event {event}; paddock ignores such lines"
                    ),
                );
                Ok(())
            }
            Line::Bad { why } => {
                self.report.bad += 1;
                self.say_once(
                    "bad",
                    &format!("paddock: agy sent a line that paddock cannot read ({why}); paddock ignores such lines"),
                );
                Ok(())
            }
        }
    }

    /// A line named conversation `got`: the one of this custody, or the
    /// first one of a new conversation.
    fn conversation(&mut self, what: &str, got: Option<String>) -> Result<()> {
        let Some(got) = got else {
            return Ok(());
        };
        match &self.id {
            Some(id) if *id == got => Ok(()),
            Some(id) => bail!(
                "agy reports conversation {got} in a {what} line, but this custody holds conversation {id}; custody stops before the store takes anything of it"
            ),
            None => self.bind(&got),
        }
    }

    /// Bind the session to the new conversation `id`.
    fn bind(&mut self, id: &str) -> Result<()> {
        crate::horse::claude::check_session_id(id)
            .with_context(|| format!("agy started conversation {id}"))?;
        let st = self.ctx.store.clone();
        let locked = st.lock()?;
        if let Some(other) = st
            .session_by_native(HARNESS, id)?
            .filter(|o| o.id != self.ctx.session)
        {
            bail!(
                "agy conversation {id} is bound to store session {} already",
                other.id
            );
        }
        locked.bind_native(&self.ctx.session, id)?;
        drop(locked);
        self.id = Some(id.to_string());
        self.say(&format!(
            "paddock: agy conversation {id} in custody for session {}",
            self.ctx.session
        ));
        Ok(())
    }

    /// Read the conversation database into the store, then refresh the
    /// guard and look for the steps of delivered entries. A pass that
    /// fails is logged and tried again after the next turn.
    fn ingest(&mut self) -> Result<()> {
        let Some(id) = self.id.clone() else {
            return Ok(());
        };
        let path = db_path(&self.ctx.agy_home, &id);
        if !path.is_file() {
            self.say_once(
                "no-db",
                &format!(
                    "paddock: agy wrote no conversation database {} yet",
                    path.display()
                ),
            );
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let r = st
            .lock()
            .and_then(|l| codec::ingest_locked(&l, &path, VERSION, Some(&self.ctx.horse)));
        let i = match r {
            Ok(i) => i,
            Err(e) => {
                self.report.ingest_errors += 1;
                self.say(&format!(
                    "paddock: cannot ingest the agy conversation {id}: {e:#}"
                ));
                return Ok(());
            }
        };
        if i.session != self.ctx.session {
            bail!(
                "the agy conversation {id} went into session {}, not {}",
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
        let mut claimed = HashSet::new();
        let mut keep = Vec::new();
        for u in std::mem::take(&mut self.unlanded) {
            let want = u.text.trim();
            let at = (u.from..nodes.len()).find(|k| {
                !claimed.contains(k) && user_text(&nodes[*k].raw).is_some_and(|t| t.trim() == want)
            });
            match at {
                Some(k) => {
                    claimed.insert(k);
                    if st.delivery_landed(&self.ctx.session, &u.id, k as u64)? {
                        self.report.landed += 1;
                    }
                }
                None => keep.push(u),
            }
        }
        self.unlanded = keep;
        Ok(())
    }

    /// A write failed: agy does not read its input any more.
    fn broken(&mut self, e: &anyhow::Error) {
        self.say(&format!("paddock: {e:#}; paddock closes the input of agy"));
        self.w = None;
    }

    fn write(&mut self, line: &[u8]) -> Result<()> {
        let w = self.w.as_mut().context("the input of agy is closed")?;
        w.write_all(line)
            .and_then(|()| w.flush())
            .context("cannot write to agy")
    }

    fn deliver_entry(&mut self, e: &Entry) -> Result<()> {
        if !self.idle() {
            bail!("paddock delivers only at a safe point");
        }
        if e.session != self.ctx.session {
            bail!("session {} does not run in this custody", e.session);
        }
        let st = self.ctx.store.clone();
        let head = st.session(&e.session)?.head;
        let from = st.chain(head.as_ref())?.len();
        let text = e.wire_text();
        // The record comes first: a recorded entry never goes out twice.
        st.record_delivery(e, head.as_ref(), &e.id)?;
        if let Err(err) = self.write(&wire::user_line(&text)) {
            st.retract_delivery(&e.session, &e.id)?;
            self.say(&format!("paddock: append {} stays in the inbox", e.id));
            self.broken(&err);
            return Ok(());
        }
        self.busy = true;
        self.report.deliveries += 1;
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

/// The text of a user input step of the agy codec (type 14, the text in
/// field 1 of field 19 of its payload), or `None`.
pub fn user_text(raw: &[u8]) -> Option<String> {
    use crate::horse::agy::{proto, row::Cell};
    let r = Row::decode(raw).ok()?;
    if r.get("step_type").and_then(Cell::int) != Some(codec::USER_INPUT) {
        return None;
    }
    let payload = r.get("step_payload").and_then(Cell::bytes)?;
    let input = proto::bytes(payload, 19).ok()??;
    let text = proto::bytes(input, 1).ok()??;
    String::from_utf8(text.to_vec()).ok()
}

impl<W: Write> Live for Agy<W> {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        if self.closed {
            return Ok(Pump::Closed);
        }
        let mut m = match self.rx.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Msg::Closed),
        };
        while let Some(x) = m.take() {
            match x {
                Msg::Closed => {
                    self.closed = true;
                    if self.w.is_some() {
                        self.report.exited = true;
                    }
                    return Ok(Pump::Closed);
                }
                Msg::Line(l) => self.line(&l)?,
            }
            m = match self.rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Msg::Closed),
            };
        }
        Ok(Pump::Open)
    }

    fn main_session(&self) -> Option<String> {
        Some(self.ctx.session.clone())
    }

    /// The session is in custody from the start: a new conversation gets
    /// its id from agy only after its first user line.
    fn sessions(&self) -> Vec<LiveSession> {
        let native = self.id.clone().unwrap_or_default();
        vec![LiveSession::turns(
            &self.ctx.session,
            &native,
            self.idle(),
            self.head.clone(),
        )]
    }

    fn safe(&self, session: Option<&str>) -> bool {
        session.is_none_or(|s| s == self.ctx.session) && self.idle()
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
        if !self.busy || self.interrupting || self.closed {
            return Ok(false);
        }
        if !self.turn_started {
            self.interrupt_pending = true;
            return Ok(false);
        }
        let Some(f) = self.ctx.interrupt.as_mut() else {
            self.say_once(
                "no-interrupt",
                "paddock: this agy run has no way to stop a turn",
            );
            return Ok(false);
        };
        if let Err(e) = f() {
            self.say(&format!("paddock: cannot stop the agy turn: {e:#}"));
            return Ok(false);
        }
        self.interrupting = true;
        self.report.interrupts += 1;
        self.say("paddock: sent SIGINT to agy to stop the turn");
        Ok(true)
    }

    fn close_input(&mut self) -> Result<()> {
        self.w = None;
        Ok(())
    }

    fn finish(&mut self, end: End) -> Result<()> {
        if end == End::TimedOut {
            self.say(&format!(
                "paddock: agy did not exit within {} s after its input closed",
                self.ctx.close_timeout.as_secs()
            ));
        }
        // The process is gone: its database is complete now.
        self.ingest()
    }

    fn typed(&self) -> Typed {
        Typed::Inbox
    }
}

#[cfg(test)]
pub(crate) mod tests;
