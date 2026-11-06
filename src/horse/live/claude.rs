//! Live custody of one Claude Code session over stream-json.
//!
//! paddock holds a long-lived `claude -p --input-format stream-json
//! --output-format stream-json --verbose` process (wire.rs). A turn starts
//! when paddock writes a `user` line and ends at the next `result` line,
//! whatever its subtype. Between turns the process waits on stdin. The
//! custodian delivers inbox entries only between turns, one `user` line
//! each, and reads the transcript file after every turn: the stream does
//! not carry the rows that Claude Code writes, so the file is the source,
//! and `claude::ingest` keeps every row by its uuid.
//!
//! paddock picks the session id before the process starts: a new uuid with
//! `--session-id`, or the native id of a projected transcript with
//! `--resume`. Every `system/init` line must report that id. Another id
//! ends the custody with an error before anything goes into the store
//! (fail closed).
//!
//! An interrupt is a `control_request` with the `interrupt` subtype. The
//! custodian waits for its `control_response` and for the `result` of the
//! turn. That `result` is a safe point like any other.
//!
//! The custodian is a [`Live`]: the driver of harness.rs runs the loop,
//! holds the typed lines and writes the live record.

pub mod wire;

use super::harness::{ChannelIo, Driver, End, Ends, Input, Live, LiveSession, Pump};
use crate::horse::claude as codec;
use crate::horse::store::{Hash, Store};
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeSet;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};
use wire::Line;

/// How long the custodian waits for the answer to an interrupt after the
/// turn ended. Then it delivers again.
pub const INTERRUPT_WAIT: Duration = Duration::from_secs(30);

/// The session that a custody starts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Start {
    /// The Claude Code session id.
    pub native: String,
    /// The store session, when the store has one already.
    pub session: Option<String>,
    /// The transcript file exists: `--resume`. Else `--session-id`.
    pub resume: bool,
}

impl Start {
    /// A new session with a new id.
    pub fn fresh() -> Result<Start> {
        Ok(Start {
            native: crate::horse::uuid_v4()?,
            session: None,
            resume: false,
        })
    }

    /// Continue a session of the store. A session with rows is resumed: its
    /// transcript is projected under `config_dir` when the file is not
    /// there. A session without rows starts with its own id.
    pub fn resume(
        store: &Store,
        session: &str,
        config_dir: &std::path::Path,
        cwd: &str,
    ) -> Result<Start> {
        let s = store.session(session)?;
        if s.binding.harness != codec::HARNESS {
            bail!("session {session} is not a Claude Code session");
        }
        codec::check_version(&s.binding.version)?;
        let file = |id: &str| codec::transcript_path(config_dir, cwd, id);
        let native = match (&s.binding.native_id, s.head.is_some()) {
            (Some(id), false) => {
                return Ok(Start {
                    native: id.clone(),
                    session: Some(s.id),
                    resume: false,
                })
            }
            (Some(id), true) if file(id).is_file() => id.clone(),
            _ => {
                let p = codec::project(store, session, config_dir)?;
                if p.path != file(&p.native_id) {
                    bail!(
                        "session {session} was made in another folder: Claude Code resumes {} only from {}",
                        p.native_id,
                        file(&p.native_id).display()
                    );
                }
                p.native_id
            }
        };
        Ok(Start {
            native,
            session: Some(s.id),
            resume: true,
        })
    }
}

/// The arguments of `claude` for a custody. `--include-partial-messages`
/// is never set: the custodian reads whole messages only.
pub fn args(start: &Start, model: Option<&str>) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
    ]
    .map(String::from)
    .to_vec();
    let flag = if start.resume {
        "--resume"
    } else {
        "--session-id"
    };
    a.extend([flag.to_string(), start.native.clone()]);
    if let Some(m) = model {
        a.extend(["--model".to_string(), m.to_string()]);
    }
    a
}

/// What the reader of stdout sends to the custodian.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// A line of the process, without its newline.
    Line(Vec<u8>),
    /// The process closed its stdout.
    Closed,
}

/// Read the stdout of the process into the channel, one line per message.
pub fn read_runtime(r: impl Read + Send + 'static, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut br = BufReader::new(r);
        loop {
            let mut buf = Vec::new();
            match br.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if buf.last() == Some(&b'\n') {
                        buf.pop();
                    }
                    if !buf.is_empty() && tx.send(Msg::Line(buf)).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Msg::Closed);
    });
}

/// What a line meant.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `system/init` confirmed the session id; `first` for the first one.
    Init {
        first: bool,
    },
    /// Text of the assistant.
    Text(String),
    /// The turn is over.
    TurnEnd(wire::TurnResult),
    /// The answer to the interrupt that paddock sent.
    Answered {
        error: Option<String>,
    },
    /// A line of a type that paddock does not read; `first` for the first
    /// line of that type.
    Unknown {
        ty: String,
        first: bool,
    },
    Bad {
        why: String,
    },
    /// A line with nothing for paddock.
    Quiet,
}

/// The stream-json side of a Claude Code process: the turn state.
pub struct Claude<W: Write> {
    w: Option<W>,
    native: String,
    confirmed: bool,
    busy: bool,
    interrupting: Option<(String, Instant)>,
    requests: u64,
    unknown: BTreeSet<String>,
}

impl<W: Write> Claude<W> {
    pub fn new(w: W, native: &str) -> Self {
        Claude {
            w: Some(w),
            native: native.into(),
            confirmed: false,
            busy: false,
            interrupting: None,
            requests: 0,
            unknown: BTreeSet::new(),
        }
    }

    /// A `system/init` line confirmed the session id.
    pub fn confirmed(&self) -> bool {
        self.confirmed
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    fn check_id(&self, what: &str, id: Option<&str>) -> Result<()> {
        match id {
            Some(id) if id != self.native => bail!(
                "claude reports session {id} in a {what} line, but paddock started session {}; custody stops",
                self.native
            ),
            _ => Ok(()),
        }
    }

    /// Read one line of the process.
    pub fn read(&mut self, raw: &[u8]) -> Result<Event> {
        Ok(match wire::parse(raw) {
            Line::System(s) if s.subtype.as_deref() == Some("init") => {
                let id = s
                    .session_id
                    .as_deref()
                    .context("claude sent a system/init line without a session id")?;
                self.check_id("system/init", Some(id))?;
                if let Some(v) = &s.claude_code_version {
                    codec::check_version(v)?;
                }
                let first = !self.confirmed;
                self.confirmed = true;
                Event::Init { first }
            }
            Line::System(_) | Line::User(_) => Event::Quiet,
            Line::Assistant(m) => {
                self.check_id("assistant", m.session_id.as_deref())?;
                let raw = serde_json::to_vec(&serde_json::json!({ "message": m.message }))?;
                match codec::view(&raw).map(|v| v.text) {
                    Some(t) if !t.is_empty() => Event::Text(t),
                    _ => Event::Quiet,
                }
            }
            Line::Result(r) => {
                self.check_id("result", r.session_id.as_deref())?;
                if !self.confirmed {
                    bail!(
                        "claude ended a turn before a system/init line confirmed session {}; custody stops",
                        self.native
                    );
                }
                self.busy = false;
                Event::TurnEnd(r)
            }
            Line::Control(c) => match &self.interrupting {
                Some((id, _)) if *id == c.response.request_id => {
                    self.interrupting = None;
                    Event::Answered {
                        error: (c.response.subtype != "success").then(|| {
                            c.response
                                .error
                                .unwrap_or_else(|| c.response.subtype.clone())
                        }),
                    }
                }
                _ => Event::Quiet,
            },
            Line::Unknown { ty, .. } => Event::Unknown {
                first: self.unknown.insert(ty.clone()),
                ty,
            },
            Line::Bad { why, .. } => Event::Bad { why },
        })
    }

    /// The turn is over, but the interrupt got no answer for `after`: stop
    /// waiting for it. True when the custodian stopped waiting now.
    pub fn give_up_interrupt(&mut self, after: Duration) -> bool {
        match &self.interrupting {
            Some((_, at)) if !self.busy && at.elapsed() >= after => {
                self.interrupting = None;
                true
            }
            _ => false,
        }
    }

    fn write(&mut self, line: &[u8]) -> Result<()> {
        let w = self.w.as_mut().context("the input of claude is closed")?;
        w.write_all(line)
            .and_then(|()| w.flush())
            .context("cannot write to claude")
    }

    /// The native session id that the process works on.
    pub fn native_id(&self) -> &str {
        &self.native
    }

    /// No turn runs, no interrupt waits for its answer, and the input is
    /// still open: a safe point for a delivery.
    pub fn is_idle(&self) -> bool {
        !self.busy && self.interrupting.is_none() && self.w.is_some()
    }

    /// Start a turn with one user message.
    pub fn deliver(&mut self, wire_text: &str) -> Result<()> {
        if self.busy {
            bail!("a turn of claude runs: paddock delivers only between turns");
        }
        let line = wire::user_line(&self.native, wire_text);
        self.write(&line)?;
        self.busy = true;
        Ok(())
    }

    /// Ask claude to stop the running turn. False when no turn runs.
    pub fn interrupt(&mut self) -> Result<bool> {
        if !self.busy || self.interrupting.is_some() || self.w.is_none() {
            return Ok(false);
        }
        self.requests += 1;
        let id = format!("paddock-interrupt-{}", self.requests);
        self.write(&wire::interrupt_line(&id))?;
        self.interrupting = Some((id, Instant::now()));
        Ok(true)
    }

    /// Close the input of the process. The process then ends.
    pub fn close(&mut self) {
        self.w = None;
    }
}

/// Wait for the process after it closed its stdout: its exit code.
pub type Wait = Box<dyn FnOnce() -> Option<i32> + Send>;

/// What a custodian needs besides the stdio.
pub struct CustodyCtx {
    pub store: Store,
    pub horse: String,
    pub start: Start,
    /// `$CLAUDE_CONFIG_DIR` of the sandbox, as paddock sees it.
    pub config_dir: PathBuf,
    /// The working folder of claude, as claude sees it.
    pub cwd: String,
    /// The version of the installed Claude Code.
    pub version: String,
    /// The sandbox id: the name of the live record.
    pub run: String,
    pub tick: Duration,
    pub out: Box<dyn Write + Send>,
    pub wait: Option<Wait>,
}

/// What a custody did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub native_id: String,
    pub session: Option<String>,
    pub turns: usize,
    pub deliveries: usize,
    pub landed: usize,
    pub interrupts: usize,
    pub unknown: usize,
    pub bad: usize,
    pub exit: Option<i32>,
}

/// A delivery whose user row is not in the store yet.
struct Landing {
    message_id: String,
    text: String,
    /// The number of session rows when paddock wrote the line.
    from: usize,
}

/// One Claude Code session in custody.
pub struct Custodian<W: Write> {
    ctx: CustodyCtx,
    claude: Claude<W>,
    rx: Receiver<Msg>,
    sid: Option<String>,
    head: Option<Hash>,
    landings: Vec<Landing>,
    report: Report,
}

/// Run the custody until the process closes its stdout. `ends`: who ends
/// the custody.
pub fn run<R, W>(io: ChannelIo<R, W>, mut ctx: CustodyCtx, ends: Ends) -> Result<Report>
where
    R: Read + Send + 'static,
    W: Write,
{
    let wait = ctx.wait.take();
    let mut d = Driver::new(ctx.store.clone(), &ctx.horse, &ctx.run, ctx.tick).ends(ends);
    let mut c = Custodian::start(io.from, io.to, ctx)?;
    d.drive(&mut c, &io.control)?;
    c.claude.close();
    let mut report = std::mem::take(&mut c.report);
    report.session = c.sid.clone();
    report.exit = wait.and_then(|w| w());
    Ok(report)
}

impl<W: Write> Custodian<W> {
    /// Check the session and read the stdout of the process.
    pub fn start<R: Read + Send + 'static>(r: R, w: W, ctx: CustodyCtx) -> Result<Self> {
        codec::check_version(&ctx.version)?;
        let head = Self::head(&ctx)?;
        let (tx, rx) = std::sync::mpsc::channel();
        read_runtime(r, tx);
        let native = ctx.start.native.clone();
        Ok(Custodian {
            claude: Claude::new(w, &native),
            rx,
            sid: ctx.start.session.clone(),
            head,
            landings: Vec::new(),
            report: Report {
                native_id: native,
                ..Report::default()
            },
            ctx,
        })
    }

    /// What the custody did so far.
    pub fn report(&self) -> &Report {
        &self.report
    }

    fn head(ctx: &CustodyCtx) -> Result<Option<Hash>> {
        Ok(match &ctx.start.session {
            Some(sid) => {
                let s = ctx.store.session(sid)?;
                if s.binding.harness != codec::HARNESS
                    || s.binding.native_id.as_deref() != Some(&ctx.start.native)
                    || s.horse != ctx.horse
                {
                    bail!(
                        "session {sid} is not Claude Code session {} of horse {}",
                        ctx.start.native,
                        ctx.horse
                    );
                }
                s.head
            }
            None => None,
        })
    }

    fn say(&mut self, line: &str) {
        let _ = writeln!(self.ctx.out, "{line}");
        let _ = self.ctx.out.flush();
    }

    fn line(&mut self, raw: &[u8]) -> Result<()> {
        match self.claude.read(raw)? {
            Event::Init { first: true } => self.bind()?,
            Event::Text(t) => self.say(&format!("claude: {t}")),
            Event::TurnEnd(r) => {
                self.report.turns += 1;
                if r.is_error || r.subtype != "success" {
                    self.say(&format!(
                        "paddock: the claude turn ended with {}",
                        r.subtype
                    ));
                }
                self.ingest()?;
            }
            Event::Answered { error: Some(e) } => {
                self.say(&format!("paddock: claude refused the interrupt: {e}"))
            }
            Event::Unknown { ty, first } => {
                self.report.unknown += 1;
                if first {
                    self.say(&format!(
                        "paddock: claude sent a line of type {ty}; paddock ignores lines of this type"
                    ));
                }
            }
            Event::Bad { why } => {
                self.report.bad += 1;
                if self.report.bad == 1 {
                    self.say(&format!(
                        "paddock: claude sent a line that paddock cannot read ({why}); paddock ignores such lines"
                    ));
                }
            }
            Event::Init { first: false } | Event::Answered { error: None } | Event::Quiet => {}
        }
        Ok(())
    }

    /// The first `system/init` confirmed the id: a new session joins the
    /// horse now, before its first row.
    fn bind(&mut self) -> Result<()> {
        if self.sid.is_some() {
            return Ok(());
        }
        let st = &self.ctx.store;
        let locked = st.lock()?;
        let native = self.claude.native_id().to_string();
        let s = match st.sessions()?.into_iter().find(|s| {
            s.horse == self.ctx.horse
                && s.binding.harness == codec::HARNESS
                && s.binding.native_id.as_deref() == Some(&native)
        }) {
            Some(s) => s,
            None => locked.new_session(&self.ctx.horse, codec::binding(Some(native)))?,
        };
        drop(locked);
        self.head = s.head;
        self.sid = Some(s.id);
        Ok(())
    }

    fn ask_interrupt(&mut self) -> bool {
        match self.claude.interrupt() {
            Ok(true) => {
                self.report.interrupts += 1;
                self.say("paddock: asked claude to stop the turn");
                true
            }
            Ok(false) => {
                self.say("paddock: no claude turn runs; nothing to interrupt");
                false
            }
            Err(e) => {
                self.broken(&e);
                false
            }
        }
    }

    /// A write failed: the process does not read its input any more.
    fn broken(&mut self, e: &anyhow::Error) {
        self.say(&format!(
            "paddock: {e:#}; paddock closes the input of claude"
        ));
        self.claude.close();
    }

    /// An inbox entry goes out at the safe point.
    fn deliver_entry(&mut self, e: crate::horse::inbox::Entry) -> Result<()> {
        let Some(sid) = self.sid.clone().filter(|s| *s == e.session) else {
            bail!("session {} is not in this custody", e.session);
        };
        let st = self.ctx.store.clone();
        let text = e.wire_text();
        let from = st.session_nodes(&st.session(&sid)?)?.len();
        if let Err(err) = self.claude.deliver(&text) {
            self.say(&format!("paddock: append {} stays in the inbox", e.id));
            self.broken(&err);
            return Ok(());
        }
        let message_id = format!("entry:{}", e.id);
        st.record_delivery(&e, self.head.as_ref(), &message_id)?;
        self.report.deliveries += 1;
        self.landings.push(Landing {
            message_id,
            text,
            from,
        });
        self.say(&format!(
            "paddock: delivered append {} ({}) to session {}",
            e.id, e.role, e.session
        ));
        Ok(())
    }

    /// Read the new rows of the transcript into the store.
    fn ingest(&mut self) -> Result<()> {
        let sid = self
            .sid
            .clone()
            .context("claude ended a turn before its session joined the horse")?;
        let path =
            codec::transcript_path(&self.ctx.config_dir, &self.ctx.cwd, self.claude.native_id());
        if !path.is_file() {
            self.say(&format!(
                "paddock: claude wrote no transcript at {} yet",
                path.display()
            ));
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let got = codec::ingest_run_locked(&st.lock()?, &path, &self.ctx.version, &self.ctx.horse)
            .with_context(|| format!("cannot ingest {}", path.display()))?;
        if got.session != sid {
            bail!(
                "the transcript {} went into session {}, not {sid}",
                path.display(),
                got.session
            );
        }
        if got.rewritten {
            self.say(&format!(
                "paddock: claude rewrote the transcript of session {sid}; the store keeps the old rows"
            ));
        }
        self.head = st.session(&sid)?.head;
        st.refresh_guard(&sid)?;
        self.land(&sid)
    }

    /// Note the deliveries whose user row is in the session now.
    fn land(&mut self, sid: &str) -> Result<()> {
        if self.landings.is_empty() {
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let nodes = st.session_nodes(&st.session(sid)?)?;
        let mut claimed = BTreeSet::new();
        let mut waiting = Vec::new();
        for l in std::mem::take(&mut self.landings) {
            let at = (l.from..nodes.len()).find(|k| {
                !claimed.contains(k) && user_text(&nodes[*k].raw).as_deref() == Some(&l.text)
            });
            match at {
                Some(k) => {
                    claimed.insert(k);
                    if st.delivery_landed(sid, &l.message_id, k as u64)? {
                        self.report.landed += 1;
                    }
                }
                None => waiting.push(l),
            }
        }
        self.landings = waiting;
        Ok(())
    }
}

impl<W: Write> Live for Custodian<W> {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        let mut m = match self.rx.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(Pump::Closed),
        };
        while let Some(x) = m.take() {
            match x {
                Msg::Closed => return Ok(Pump::Closed),
                Msg::Line(l) => self.line(&l)?,
            }
            m = match self.rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return Ok(Pump::Closed),
            };
        }
        if self.claude.give_up_interrupt(INTERRUPT_WAIT) {
            self.say("paddock: claude did not answer the interrupt; paddock delivers again");
        }
        Ok(Pump::Open)
    }

    fn main_session(&self) -> Option<String> {
        self.sid.clone()
    }

    fn sessions(&self) -> Vec<LiveSession> {
        let native = self.claude.native_id();
        self.sid
            .iter()
            .map(|sid| LiveSession::turns(sid, native, self.claude.is_idle(), self.head.clone()))
            .collect()
    }

    fn safe(&self, session: Option<&str>) -> bool {
        self.claude.is_idle() && session.is_none_or(|s| self.sid.as_deref() == Some(s))
    }

    fn deliver(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Typed(t) => {
                if let Err(e) = self.claude.deliver(&t) {
                    self.broken(&e);
                }
                Ok(())
            }
            Input::Entry(e) => self.deliver_entry(e),
        }
    }

    fn interrupt(&mut self) -> Result<bool> {
        Ok(self.ask_interrupt())
    }

    fn close_input(&mut self) -> Result<()> {
        self.claude.close();
        Ok(())
    }

    fn finish(&mut self, _end: End) -> Result<()> {
        // The process is gone: its transcript is complete now.
        if self.sid.is_some() {
            self.ingest()?;
        }
        Ok(())
    }
}

/// The text of a main-chain user row of a transcript, or `None`.
fn user_text(raw: &[u8]) -> Option<String> {
    let n = codec::info(raw).ok()?;
    if n.ty != "user" || !n.is_chain() || n.sidechain {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(raw).ok()?;
    wire::text_of(v.get("message")?)
}

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;
