//! Custody of an interactive Codex run (the TUI) through the pty relay.
//!
//! Codex has no push path into its TUI: the TUI is its own process, not a
//! client of an app-server that paddock could hold. So paddock types the
//! entries into the pty of the run (pty.rs). Codex's `notify` program
//! (`paddock-host horse safe-point stop`, set in the sandbox copy of
//! `config.toml`, horse/codex/notify.rs) tells the host after each turn,
//! with the thread id. The first `stop` that names a thread with a
//! rollout file binds the session of the run to that thread; a `stop` of
//! another thread later is ignored.
//!
//! The loop tails the rollout file: a `user_message` row newer than the
//! last `stop` means busy again. A `task_complete` row with no `stop`
//! within 2 s counts as a missed `stop`. After paddock sent the interrupt
//! key for an urgent entry, the `turn_aborted` row is the safe point.
//! The ingest goes by record identity (codex.rs), after each `stop`, every
//! 5 s of a long turn, and at the end; a delivery has landed when its
//! user row is in the session after the head that it went out at.
//!
//! An entry whose row does not come while the TUI looks idle for 60 s
//! goes back into the inbox once (`pty.retry` in the ledger); after a
//! second miss it stays recorded and not landed.
//!
//! If no `stop` ever names a thread while the screen looks idle for 30 s
//! and entries wait, the notify program does not reach the host: the
//! custody says so once (`pty.no_stop` in the ledger), and the entries
//! stay queued.
//!
//! The loop is a [`Live`] under the one [`Driver`]: the driver delivers
//! the oldest entry when [`Live::safe`] says the TUI is idle, writes the
//! live record, and ends the custody when the run ends.

use super::{is_idle, PtyIdle, PtyTap};
use crate::horse::codex as codec;
use crate::horse::live::codex::rollout::find_rollout;
use crate::horse::live::harness::{Control, Driver, End, Input, Live, LiveSession, Pump};
use crate::horse::live::tui_state::{run_key, Runs, SafePoint, SignalRx};
use crate::horse::store::{Hash, Store};
use anyhow::{bail, Context as _, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The waits of the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// The longest wait of one loop pass.
    pub tick: Duration,
    /// How often a long turn is ingested.
    pub busy_ingest: Duration,
    /// How long the TUI may look idle before a typed entry counts as lost.
    pub land_wait: Duration,
    /// A turn end in the file with no `stop` this long is a missed `stop`.
    pub missed_stop: Duration,
    /// The screen looks idle this long, entries wait, and no `stop` ever
    /// named a thread: the notify program does not reach the host.
    pub no_stop: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            tick: Duration::from_millis(200),
            busy_ingest: Duration::from_secs(5),
            land_wait: Duration::from_secs(60),
            missed_stop: Duration::from_secs(2),
            no_stop: Duration::from_secs(30),
        }
    }
}

/// What the loop needs.
pub struct CustodyCtx {
    pub store: Store,
    pub horse: String,
    /// The store session of the run. It gets its thread id from the first
    /// `stop`.
    pub session: String,
    /// The `sessions` folder of Codex in the sandbox, as paddock sees it.
    pub sessions_root: PathBuf,
    pub version: String,
    /// The sandbox id: the name of the live record.
    pub run: String,
    /// The TUI states of this host process (`tui_state::runs()`).
    pub runs: &'static Runs,
    /// One line in the device ledger of the run.
    pub ledger: Box<dyn Fn(&str) + Send>,
    /// The end of the run: two `Close`s.
    pub control: Receiver<Control>,
    pub timing: Timing,
    pub idle: PtyIdle,
    pub tap: Arc<PtyTap>,
}

/// What a custody did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The thread of the run, once a `stop` named it.
    pub thread: Option<String>,
    /// `stop`s of the thread of the run.
    pub stops: usize,
    /// `stop`s of another thread, ignored.
    pub other_stops: usize,
    /// Turn ends in the file with no `stop`.
    pub missed_stops: usize,
    /// Entries typed into the TUI.
    pub typed: usize,
    pub landed: usize,
    pub retried: usize,
    /// Interrupt keys sent for urgent entries.
    pub interrupts: usize,
    /// Entries recorded and not landed at the end.
    pub not_landed: Vec<String>,
    pub ingests: usize,
    pub ingest_errors: usize,
}

/// A row of the rollout that the loop acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    /// A user message: a turn starts.
    User,
    /// The end of a turn, `aborted` after an interrupt.
    End { aborted: bool },
}

fn mark(raw: &[u8]) -> Option<Mark> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    if v["type"] != "event_msg" {
        return None;
    }
    match v["payload"]["type"].as_str()? {
        "user_message" => Some(Mark::User),
        "task_complete" => Some(Mark::End { aborted: false }),
        "turn_aborted" => Some(Mark::End { aborted: true }),
        _ => None,
    }
}

/// The rollout file, read as it grows. A new inode or a shorter file is
/// read again from the top, and a torn last line waits for its newline.
#[derive(Default)]
struct Tail {
    path: Option<PathBuf>,
    /// (dev, inode, size, mtime ns) of the last look.
    seen: Option<(u64, u64, u64, i128)>,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    /// The marks of the rows that came since the last look.
    fn look(&mut self) -> Result<Vec<Mark>> {
        let Some(path) = self.path.clone() else {
            return Ok(Vec::new());
        };
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("cannot stat {}", path.display())),
        };
        let fp = (
            meta.dev(),
            meta.ino(),
            meta.len(),
            i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        );
        if self.seen == Some(fp) {
            return Ok(Vec::new());
        }
        let same = self.seen.is_some_and(|s| (s.0, s.1) == (fp.0, fp.1));
        if !same || fp.2 < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        self.seen = Some(fp);
        let mut f = std::fs::File::open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        let mut out = Vec::new();
        while let Some(at) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=at).collect();
            out.extend(mark(&line[..line.len() - 1]));
        }
        Ok(out)
    }
}

/// A typed entry whose row is not in the store yet.
struct Pending {
    id: String,
    text: String,
    at_head: Option<Hash>,
    /// Since when the TUI looked idle with no sign of a turn.
    quiet_since: Instant,
}

/// One Codex TUI run in custody.
pub struct Custodian {
    ctx: CustodyCtx,
    signals: SignalRx,
    key: String,
    tail: Tail,
    thread: Option<String>,
    head: Option<Hash>,
    /// A `stop` of the thread came, and no user row since.
    stopped: bool,
    /// The file showed a turn end that no `stop` covered yet.
    end_seen: Option<Instant>,
    /// The interrupt key went out, and no safe point came since.
    interrupted: bool,
    /// Urgent entries that got their interrupt key.
    interrupted_for: HashSet<String>,
    last_ingest: Instant,
    ingested: Option<(u64, u64, u64, i128)>,
    attempts: HashMap<String, u32>,
    pending: Vec<Pending>,
    /// Since when the screen looks idle while no `stop` named a thread.
    idle_unbound: Option<Instant>,
    /// The relay is gone: nothing more is typed.
    broken: bool,
    closed: bool,
    said: HashSet<String>,
    report: Report,
}

/// Hold the TUI run until the run ends (two `Close`s on the control
/// channel), then ingest a last time.
pub fn run(mut ctx: CustodyCtx, signals: SignalRx) -> Result<Report> {
    codec::check_version(&ctx.version)?;
    let (_keep, dummy) = std::sync::mpsc::channel();
    let control = std::mem::replace(&mut ctx.control, dummy);
    let mut d = Driver::new(ctx.store.clone(), &ctx.horse, &ctx.run, ctx.timing.tick);
    let mut c = Custodian::new(ctx, signals)?;
    let r = d.drive(&mut c, &control);
    c.ctx.runs.forget(&c.key);
    r?;
    Ok(std::mem::take(&mut c.report))
}

impl Custodian {
    pub fn new(ctx: CustodyCtx, signals: SignalRx) -> Result<Self> {
        let s = ctx.store.session(&ctx.session)?;
        if s.horse != ctx.horse || s.binding.harness != codec::HARNESS {
            bail!(
                "session {} is not a Codex session of horse {}",
                ctx.session,
                ctx.horse
            );
        }
        let key = run_key(&ctx.horse, &ctx.session);
        // The channel server of the sandbox gets none of these entries.
        ctx.runs.typed(&key);
        let mut c = Custodian {
            key,
            tail: Tail::default(),
            thread: None,
            head: s.head,
            stopped: false,
            end_seen: None,
            interrupted: false,
            interrupted_for: HashSet::new(),
            last_ingest: Instant::now(),
            ingested: None,
            attempts: HashMap::new(),
            pending: Vec::new(),
            idle_unbound: None,
            broken: false,
            closed: false,
            said: HashSet::new(),
            report: Report::default(),
            signals,
            ctx,
        };
        // A session that has its thread already (a resumed session).
        if let Some(t) = s.binding.native_id {
            c.tail.path = find_rollout(&c.ctx.sessions_root, &t);
            c.report.thread = Some(t.clone());
            c.thread = Some(t);
        }
        Ok(c)
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    fn say_once(&mut self, key: &str, line: &str) {
        if self.said.insert(key.to_string()) {
            log::warn!("horse pty: {line}");
        }
    }

    /// The TUI showed a sign of a turn: no typed entry is lost yet.
    fn activity(&mut self) {
        let now = Instant::now();
        for p in &mut self.pending {
            p.quiet_since = now;
        }
    }

    /// Bind the session of the run to `thread`, the first thread that a
    /// `stop` names and that has a rollout file of the TUI.
    fn bind(&mut self, thread: &str) -> Result<bool> {
        let Some(path) = find_rollout(&self.ctx.sessions_root, thread) else {
            self.say_once(
                &format!("no-rollout:{thread}"),
                &format!(
                    "a stop names thread {thread}, which has no rollout under {}",
                    self.ctx.sessions_root.display()
                ),
            );
            return Ok(false);
        };
        if sub_agent(&path) {
            return Ok(false);
        }
        self.ctx
            .store
            .lock()?
            .bind_native(&self.ctx.session, thread)?;
        (self.ctx.ledger)(&format!("pty.thread {thread}"));
        self.thread = Some(thread.to_string());
        self.report.thread = Some(thread.to_string());
        self.tail.path = Some(path);
        Ok(true)
    }

    fn signal(&mut self, kind: SafePoint, thread: Option<String>) -> Result<()> {
        if kind == SafePoint::Prompt {
            self.stopped = false;
            self.activity();
            return Ok(());
        }
        match (&self.thread, &thread) {
            (None, Some(t)) => {
                if !self.bind(t)? {
                    return Ok(());
                }
            }
            (None, None) => {
                self.say_once(
                    "no-thread",
                    "a stop names no thread, and the run has none yet",
                );
                return Ok(());
            }
            (Some(ours), Some(t)) if ours != t => {
                self.report.other_stops += 1;
                self.say_once(
                    &format!("other:{t}"),
                    &format!("a stop of thread {t} is not the thread {ours} of this run; ignored"),
                );
                return Ok(());
            }
            _ => {}
        }
        self.report.stops += 1;
        // The rows of the turn that ended come before the stop.
        self.marks()?;
        self.stopped = true;
        self.end_seen = None;
        self.interrupted = false;
        self.activity();
        self.ingest();
        Ok(())
    }

    /// Read the new rows of the rollout and act on them, in order.
    fn marks(&mut self) -> Result<()> {
        let marks = self.tail.look()?;
        if !marks.is_empty() {
            self.activity();
        }
        for m in marks {
            match m {
                Mark::User => {
                    self.stopped = false;
                    self.end_seen = None;
                }
                Mark::End { aborted: true } if self.interrupted => {
                    // The native interrupt ended the turn: a safe point.
                    self.stopped = true;
                    self.interrupted = false;
                    self.end_seen = None;
                    self.ingest();
                }
                Mark::End { .. } if !self.stopped => self.end_seen = Some(Instant::now()),
                Mark::End { .. } => {}
            }
        }
        Ok(())
    }

    /// The screen part of the idle test.
    fn screen_idle(&self) -> bool {
        self.ctx
            .tap
            .screen()
            .is_some_and(|(s, unchanged)| is_idle(&s, &self.ctx.idle, unchanged))
    }

    /// Idle by all three parts: the thread is known, its `stop` came with
    /// no newer user row, and the screen is idle.
    fn idle(&self) -> bool {
        self.thread.is_some() && self.stopped && !self.broken && self.screen_idle()
    }

    /// One pass after the signals.
    fn step(&mut self) -> Result<()> {
        self.marks()?;
        if let Some(at) = self.end_seen {
            if !self.stopped && at.elapsed() >= self.ctx.timing.missed_stop {
                // A turn end with no `stop`: the notify program did not
                // reach the host.
                self.report.missed_stops += 1;
                self.say_once(
                    "missed",
                    "a turn ended in the rollout with no stop from Codex's notify program",
                );
                self.stopped = true;
                self.end_seen = None;
                self.ingest();
            }
        }
        if !self.stopped && self.last_ingest.elapsed() >= self.ctx.timing.busy_ingest {
            self.ingest();
        }
        self.unbound()?;
        self.urgent()?;
        self.retry()
    }

    /// The screen looks idle and entries wait, but no `stop` ever named
    /// the thread of the run: say once that the notify program does not
    /// reach the host. The entries stay queued: a screen alone is never
    /// a safe point.
    fn unbound(&mut self) -> Result<()> {
        if self.thread.is_some() || self.broken || !self.screen_idle() {
            self.idle_unbound = None;
            return Ok(());
        }
        let since = *self.idle_unbound.get_or_insert_with(Instant::now);
        if since.elapsed() < self.ctx.timing.no_stop
            || self.said.contains("no-stop")
            || self.ctx.store.inbox(&self.ctx.session)?.is_empty()
        {
            return Ok(());
        }
        (self.ctx.ledger)("pty.no_stop");
        self.say_once(
            "no-stop",
            &format!(
                "the TUI looks idle, but Codex's notify program never told the host that a turn ended; the entries stay queued (look at notify in {} of the sandbox)",
                codec::notify::CONFIG
            ),
        );
        Ok(())
    }

    /// An urgent entry waits while a turn runs: send the interrupt key
    /// once for it.
    fn urgent(&mut self) -> Result<()> {
        if self.stopped || self.interrupted || self.broken || self.thread.is_none() {
            return Ok(());
        }
        let Some(e) =
            super::urgent_entry(&self.ctx.store, &self.ctx.session, &self.interrupted_for)?
        else {
            return Ok(());
        };
        self.interrupted_for.insert(e.id.clone());
        match self.ctx.tap.interrupt(&self.ctx.idle) {
            Ok(()) => {
                self.interrupted = true;
                self.report.interrupts += 1;
                (self.ctx.ledger)(&format!("pty.interrupt {}", e.id));
            }
            Err(err) => {
                self.broken = true;
                log::warn!("horse pty: cannot send the interrupt key: {err}");
            }
        }
        Ok(())
    }

    /// Entries whose row did not come while the TUI looked idle.
    fn retry(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        // The TUI looks idle but no turn ended: what was typed started
        // nothing.
        if self.stopped || !self.screen_idle() {
            self.activity();
            return Ok(());
        }
        let wait = self.ctx.timing.land_wait;
        if !self.pending.iter().any(|p| p.quiet_since.elapsed() >= wait) {
            return Ok(());
        }
        // The row may be in the file but not in the store yet.
        self.ingest();
        let (lost, keep): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.quiet_since.elapsed() >= wait);
        self.pending = keep;
        for p in lost {
            let n = self.attempts.entry(p.id.clone()).or_insert(0);
            *n += 1;
            let again = *n == 1
                && match self.ctx.store.retract_delivery(&self.ctx.session, &p.id) {
                    Ok(e) => e.is_some(),
                    Err(e) => {
                        log::warn!("horse pty: cannot queue entry {} again: {e:#}", p.id);
                        false
                    }
                };
            if again {
                self.report.retried += 1;
                (self.ctx.ledger)(&format!("pty.retry {}", p.id));
                log::warn!(
                    "horse pty: entry {} did not arrive; it goes out once more",
                    p.id
                );
                // The TUI is idle: the entry may go out at once.
                self.stopped = true;
            } else {
                self.report.not_landed.push(p.id.clone());
                (self.ctx.ledger)(&format!("pty.lost {}", p.id));
                log::warn!(
                    "horse pty: entry {} did not arrive; it stays recorded and not landed",
                    p.id
                );
            }
        }
        Ok(())
    }

    /// Read the new rows of the rollout into the store. An error is a
    /// warning: the TUI goes on, and the final ingest of the run reads the
    /// file again.
    fn ingest(&mut self) {
        self.last_ingest = Instant::now();
        let Some(path) = self.tail.path.clone().filter(|p| p.is_file()) else {
            return;
        };
        if self.ingested.is_some() && self.ingested == self.tail.seen {
            self.land_or_warn();
            return;
        }
        let seen = self.tail.seen;
        match self.ingest_now(&path) {
            Ok(()) => {
                self.ingested = seen;
                self.report.ingests += 1;
            }
            Err(e) => {
                self.report.ingest_errors += 1;
                log::warn!("horse pty: cannot ingest {}: {e:#}", path.display());
            }
        }
        self.land_or_warn();
    }

    fn ingest_now(&mut self, path: &Path) -> Result<()> {
        let st = self.ctx.store.clone();
        let got = codec::ingest_run_locked(&st.lock()?, path, &self.ctx.version, &self.ctx.horse)?;
        if got.session != self.ctx.session {
            bail!(
                "the rollout went into session {}, not {}",
                got.session,
                self.ctx.session
            );
        }
        self.head = st.session(&self.ctx.session)?.head;
        st.refresh_guard(&self.ctx.session)?;
        Ok(())
    }

    fn land_or_warn(&mut self) {
        if let Err(e) = self.land() {
            log::warn!("horse pty: cannot note a landing: {e:#}");
        }
    }

    /// Note the entries whose user row is in the session now.
    fn land(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let st = self.ctx.store.clone();
        let nodes = st.session_nodes(&st.session(&self.ctx.session)?)?;
        let mut claimed = HashSet::new();
        let mut waiting = Vec::new();
        for p in std::mem::take(&mut self.pending) {
            let from = p
                .at_head
                .as_ref()
                .and_then(|h| nodes.iter().position(|n| n.hash == *h))
                .map_or(0, |i| i + 1);
            let at = (from..nodes.len())
                .find(|k| !claimed.contains(k) && user_row_holds(&nodes[*k].raw, &p.text));
            match at {
                Some(k) => {
                    claimed.insert(k);
                    if st.delivery_landed(&self.ctx.session, &p.id, k as u64)? {
                        self.report.landed += 1;
                    }
                }
                None => waiting.push(p),
            }
        }
        self.pending = waiting;
        Ok(())
    }

    fn deliver_entry(&mut self, e: &crate::horse::inbox::Entry) -> Result<()> {
        let typed = super::type_entry(&self.ctx.store, &self.ctx.tap, &self.ctx.idle, e)?;
        let Some((text, head)) = typed else {
            self.broken = true;
            return Ok(());
        };
        (self.ctx.ledger)(&format!("pty.deliver {}", e.id));
        self.report.typed += 1;
        // The entry starts a turn: busy until its `stop`.
        self.stopped = false;
        self.pending.push(Pending {
            id: e.id.clone(),
            text,
            at_head: head,
            quiet_since: Instant::now(),
        });
        Ok(())
    }
}

/// The rollout is the one of a sub-agent thread, not of the TUI.
fn sub_agent(path: &Path) -> bool {
    use std::io::BufRead as _;
    let mut line = String::new();
    let read = std::fs::File::open(path)
        .map(std::io::BufReader::new)
        .and_then(|mut r| r.read_line(&mut line));
    let first = read
        .ok()
        .and_then(|_| serde_json::from_str::<Value>(&line).ok());
    let src = first.as_ref().map(|v| &v["payload"]["source"]);
    src.is_some_and(|s| s.get("subagent").is_some() || s == "subagent")
}

/// A user message row of the rollout with `text`.
fn user_row_holds(raw: &[u8], text: &str) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(raw) else {
        return false;
    };
    let want = text.trim();
    v["type"] == "response_item"
        && v["payload"]["type"] == "message"
        && v["payload"]["role"] == "user"
        && v["payload"]["content"].as_array().is_some_and(|c| {
            c.iter().any(|x| {
                x["text"]
                    .as_str()
                    .is_some_and(|t| t.trim() == want || (!want.is_empty() && t.contains(want)))
            })
        })
}

impl Live for Custodian {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        if self.closed {
            return Ok(Pump::Closed);
        }
        let mut next = match self.signals.recv_timeout(timeout) {
            Ok(k) => Some(k),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                // No stop reaches this loop any more: the file still does.
                std::thread::sleep(timeout);
                None
            }
        };
        while let Some(h) = next.take() {
            if let Err(e) = self.signal(h.kind, h.thread) {
                let e = format!("{e:#}");
                self.say_once(&e, &e);
            }
            next = self.signals.try_recv().ok();
        }
        // The TUI goes on after an error of the store or the file: say it
        // once, and try again in the next pass.
        if let Err(e) = self.step() {
            let e = format!("{e:#}");
            self.say_once(&e, &e);
        }
        Ok(Pump::Open)
    }

    fn main_session(&self) -> Option<String> {
        Some(self.ctx.session.clone())
    }

    /// Empty until a `stop` named the thread of the run.
    fn sessions(&self) -> Vec<LiveSession> {
        match &self.thread {
            Some(t) => vec![LiveSession::turns(
                &self.ctx.session,
                t,
                self.idle(),
                self.head.clone(),
            )],
            None => Vec::new(),
        }
    }

    fn safe(&self, session: Option<&str>) -> bool {
        session.is_none_or(|s| s == self.ctx.session) && self.idle()
    }

    fn deliver(&mut self, input: Input) -> Result<()> {
        match input {
            Input::Entry(e) => self.deliver_entry(&e),
            Input::Typed(_) => bail!("the user types into the TUI directly"),
        }
    }

    fn interrupt(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn close_input(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }

    fn finish(&mut self, _end: End) -> Result<()> {
        // The TUI is gone: its rollout is complete now.
        if let Err(e) = self.marks() {
            log::warn!("horse pty: {e:#}");
        }
        self.ingested = None;
        self.ingest();
        self.report
            .not_landed
            .extend(self.pending.drain(..).map(|p| p.id));
        Ok(())
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
