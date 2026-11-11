//! Custody of an interactive Claude Code run (the TUI) in horse mode.
//!
//! The TUI stays a pty run that the relay owns, as without horse mode.
//! paddock adds three things to its command line (see [`prepare`]): the
//! session id, a `Stop` and a `UserPromptSubmit` hook that tell the host
//! when a turn ends and starts (`paddock-host horse safe-point`), and the
//! MCP server `paddock` (`paddock-host channel`), loaded as a channel.
//!
//! The safe points come from the hooks, through the broker into the TUI
//! state of the host process (tui_state.rs). When the TUI is idle, the
//! channel server in the sandbox gets the pending entries of the inbox
//! (`horse.channel_wait`) and pushes them into the TUI. This loop only
//! watches: it hears the hooks, tails the transcript, ingests, and marks a
//! delivery landed when the user row that carries the entry is in the
//! store. A hook that never came is covered by the file: a last row that
//! is a complete assistant reply, unchanged for 2 s, is idle, and a last
//! row that is a user prompt is busy.
//!
//! An entry whose row does not come while the TUI sits idle for 60 s goes
//! back into the inbox once (`channel.retry` in the ledger). After a
//! second miss it stays recorded and not landed, and `paddock horse
//! status` shows it.
//!
//! The loop is a [`Live`] under the one [`Driver`]: the driver writes the
//! live record and ends the custody when the run ends. It never delivers
//! itself: [`Live::safe`] is always false, because the channel server
//! delivers.

use super::harness::{Control, Driver, End, Input, Live, LiveSession, Pump};
use super::tui_state::{run_key, Runs, SafePoint, SignalRx};
use crate::horse::claude as codec;
use crate::horse::store::{Hash, Store};
use anyhow::{bail, Context as _, Result};
use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

mod command;
pub use command::{
    apply, conflict, conflict_session, prepare, prepare_hooks, Setup, PROMPT_HOOK, SERVER,
    STOP_HOOK,
};

/// The waits of the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// The longest wait of one loop pass.
    pub tick: Duration,
    /// A complete reply unchanged this long is idle (a missed `Stop`).
    pub idle_after: Duration,
    /// How often a long turn is ingested.
    pub busy_ingest: Duration,
    /// How long an idle TUI may sit before a pushed entry counts as lost.
    pub land_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            tick: Duration::from_millis(200),
            idle_after: Duration::from_secs(2),
            busy_ingest: Duration::from_secs(5),
            land_wait: Duration::from_secs(60),
        }
    }
}

/// What the loop needs.
pub struct CustodyCtx {
    pub store: Store,
    pub horse: String,
    /// The store session of the run, bound to `native`.
    pub session: String,
    /// The Claude Code session id of `--session-id`.
    pub native: String,
    /// `$CLAUDE_CONFIG_DIR` of the sandbox, as paddock sees it.
    pub config_dir: PathBuf,
    /// The working folder of claude, as claude sees it.
    pub cwd: String,
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
}

/// What a custody did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// `Stop` hooks heard.
    pub stops: usize,
    /// `UserPromptSubmit` hooks heard.
    pub prompts: usize,
    /// Times the file said idle or busy where no hook did.
    pub missed_hooks: usize,
    /// Entries that the channel server got.
    pub released: usize,
    pub landed: usize,
    pub retried: usize,
    /// Entries recorded and not landed at the end.
    pub not_landed: Vec<String>,
    pub ingests: usize,
    pub ingest_errors: usize,
}

/// The last user or assistant row of the transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    /// A prompt: a turn runs.
    User,
    /// An assistant row in the middle of a turn (a tool call).
    Working,
    /// A complete reply, or the row of a native interrupt.
    Done,
}

/// The transcript file, read as it grows. Identity is the uuid, not the
/// offset: a new inode or a shorter file is read again from the top, and
/// a torn last line waits for its newline.
struct Tail {
    path: PathBuf,
    /// (dev, inode, size, mtime ns) of the last look.
    seen_fp: Option<(u64, u64, u64, i128)>,
    file: Option<(u64, u64)>,
    offset: u64,
    partial: Vec<u8>,
    uuids: HashSet<String>,
    last: Option<Last>,
    changed_at: Option<Instant>,
}

/// What one look at the file found.
#[derive(Default)]
struct Look {
    changed: bool,
    /// New user or assistant rows.
    rows: bool,
}

impl Tail {
    fn new(path: PathBuf) -> Tail {
        Tail {
            path,
            seen_fp: None,
            file: None,
            offset: 0,
            partial: Vec::new(),
            uuids: HashSet::new(),
            last: None,
            changed_at: None,
        }
    }

    fn look(&mut self) -> Result<Look> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Look::default()),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot stat {}", self.path.display()))
            }
        };
        let fp = (
            meta.dev(),
            meta.ino(),
            meta.len(),
            i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        );
        if self.seen_fp == Some(fp) {
            return Ok(Look::default());
        }
        self.seen_fp = Some(fp);
        self.changed_at = Some(Instant::now());
        if self.file != Some((fp.0, fp.1)) || fp.2 < self.offset {
            // A rewrite (Claude Code's compaction at 5 MiB): read it all
            // again; the uuids that are known stay known.
            self.file = Some((fp.0, fp.1));
            self.offset = 0;
            self.partial.clear();
        }
        let mut f = std::fs::File::open(&self.path)
            .with_context(|| format!("cannot open {}", self.path.display()))?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        let mut look = Look {
            changed: true,
            rows: false,
        };
        while let Some(at) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=at).collect();
            look.rows |= self.row(&line[..line.len() - 1]);
        }
        Ok(look)
    }

    /// Read one complete row. True for a new user or assistant row.
    fn row(&mut self, raw: &[u8]) -> bool {
        let Ok(n) = codec::info(raw) else {
            return false;
        };
        if !n.is_chain() || n.sidechain || !matches!(n.ty.as_str(), "user" | "assistant") {
            return false;
        }
        let Some(uuid) = n.uuid.clone() else {
            return false;
        };
        let fresh = self.uuids.insert(uuid);
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(raw) else {
            return fresh;
        };
        let last = if n.ty == "assistant" {
            let stop = v.pointer("/message/stop_reason").and_then(|s| s.as_str());
            match stop {
                Some("end_turn" | "stop_sequence" | "max_tokens" | "refusal") => Last::Done,
                _ => Last::Working,
            }
        } else if v["isMeta"] == true {
            return fresh;
        } else {
            let text = v
                .get("message")
                .and_then(super::claude::wire::text_of)
                .unwrap_or_default();
            if text.starts_with("[Request interrupted by user") {
                Last::Done
            } else if text.starts_with("<command-") || text.starts_with("<local-command-") {
                // A local slash command: no turn.
                return fresh;
            } else {
                Last::User
            }
        };
        self.last = Some(last);
        fresh
    }
}

/// A pushed entry whose row is not in the store yet.
struct Pending {
    id: String,
    text: String,
    /// The session head when the entry went out: its row comes after.
    at_head: Option<Hash>,
    /// Since when the TUI showed no sign of a turn.
    quiet_since: Instant,
}

/// One TUI run in custody.
pub struct Custodian {
    ctx: CustodyCtx,
    signals: SignalRx,
    key: String,
    tail: Tail,
    head: Option<Hash>,
    /// The TUI state as the loop saw it last: busy.
    busy: bool,
    /// A user or assistant row came since the TUI turned busy.
    turn_seen: bool,
    /// A `prompt` came after the last `stop`.
    turn_open: bool,
    last_ingest: Instant,
    ingested_fp: Option<(u64, u64, u64, i128)>,
    /// Entries that this loop follows, landed or lost included.
    known: HashSet<String>,
    attempts: HashMap<String, u32>,
    pending: Vec<Pending>,
    closed: bool,
    /// Marks of this loop that come back as signals: not hooks.
    echoes: Vec<SafePoint>,
    /// The last error of a pass, said once.
    error: Option<String>,
    /// The name in the ledger: `channel`, or `pty` when paddock types.
    label: &'static str,
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
        if s.horse != ctx.horse
            || s.binding.harness != codec::HARNESS
            || s.binding.native_id.as_deref() != Some(&ctx.native)
        {
            bail!(
                "session {} is not Claude Code session {} of horse {}",
                ctx.session,
                ctx.native,
                ctx.horse
            );
        }
        let path = codec::transcript_path(&ctx.config_dir, &ctx.cwd, &ctx.native);
        Ok(Custodian {
            key: run_key(&ctx.horse, &ctx.session),
            tail: Tail::new(path),
            head: s.head,
            busy: true,
            turn_seen: false,
            turn_open: false,
            last_ingest: Instant::now(),
            ingested_fp: None,
            known: HashSet::new(),
            attempts: HashMap::new(),
            pending: Vec::new(),
            closed: false,
            echoes: Vec::new(),
            error: None,
            label: "channel",
            report: Report::default(),
            signals,
            ctx,
        })
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    /// paddock types the entries of this run into its pty (pty/claude.rs).
    pub(crate) fn typed_by_pty(&mut self) {
        self.label = "pty";
    }

    /// One line in the device ledger of the run.
    pub(crate) fn note(&self, line: &str) {
        (self.ctx.ledger)(line)
    }

    /// No hook and no row of the file says that a turn runs.
    pub(crate) fn quiet(&self) -> bool {
        !self.busy && !self.turn_open
    }

    /// paddock typed entry `id` with `text` at `at_head`: follow it until
    /// its row lands. The TUI counts as busy until the `Stop` of its turn.
    pub(crate) fn track(&mut self, id: &str, text: &str, at_head: Option<Hash>) {
        self.ctx.runs.hold(&self.key);
        self.busy = true;
        self.turn_seen = false;
        self.known.insert(id.to_string());
        self.report.released += 1;
        self.pending.push(Pending {
            id: id.to_string(),
            text: text.to_string(),
            at_head,
            quiet_since: Instant::now(),
        });
    }

    fn activity(&mut self) {
        let now = Instant::now();
        for p in &mut self.pending {
            p.quiet_since = now;
        }
    }

    /// Mark the TUI state where the file says what no hook said.
    fn mark(&mut self, kind: SafePoint) {
        self.report.missed_hooks += 1;
        self.echoes.push(kind);
        self.ctx.runs.mark(&self.key, kind);
    }

    fn signal(&mut self, kind: SafePoint) -> Result<()> {
        if let Some(i) = self.echoes.iter().position(|k| *k == kind) {
            self.echoes.remove(i);
            return Ok(());
        }
        self.activity();
        match kind {
            SafePoint::Stop => {
                self.report.stops += 1;
                self.turn_open = false;
                // The transcript may lag the hook: look before ingesting.
                self.tail.look()?;
                self.ingest();
            }
            SafePoint::Prompt => {
                self.report.prompts += 1;
                self.turn_open = true;
            }
        }
        Ok(())
    }

    /// One pass after the signals: the state, the file, the landings.
    fn step(&mut self) -> Result<()> {
        let st = self.ctx.runs.state(&self.key);
        if st.busy && !self.busy {
            self.turn_seen = false;
        }
        self.busy = st.busy;
        self.follow(&st.released)?;
        let look = self.tail.look()?;
        if look.changed {
            self.activity();
        }
        if look.rows && self.busy {
            self.turn_seen = true;
        }
        let t = self.ctx.timing;
        match self.tail.last {
            Some(Last::User) if !self.busy && look.rows => {
                // A prompt without its hook.
                self.mark(SafePoint::Prompt);
                self.busy = true;
                self.turn_seen = true;
            }
            Some(Last::Done)
                if self.busy
                    && self.turn_seen
                    && self
                        .tail
                        .changed_at
                        .is_some_and(|c| c.elapsed() >= t.idle_after) =>
            {
                // A reply without its `Stop`.
                self.ingest();
                self.mark(SafePoint::Stop);
                self.busy = false;
                self.turn_open = false;
            }
            _ => {}
        }
        if self.busy && self.last_ingest.elapsed() >= t.busy_ingest {
            self.ingest();
        }
        self.retry()
    }

    /// Follow the entries that the channel server got since the last pass.
    fn follow(&mut self, released: &[String]) -> Result<()> {
        let new: Vec<&String> = released
            .iter()
            .filter(|id| !self.known.contains(*id))
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        let all = self.ctx.store.deliveries(&self.ctx.session)?;
        for id in new {
            // Released before its record is written: the next pass sees it.
            let Some(d) = all
                .iter()
                .find(|d| d.entry.id == *id && d.landed_seq.is_none())
            else {
                continue;
            };
            self.known.insert(id.clone());
            self.report.released += 1;
            self.pending.push(Pending {
                id: id.clone(),
                text: d.entry.wire_text(),
                at_head: d.at_head.clone(),
                quiet_since: Instant::now(),
            });
        }
        // Its row may be in the store already: the ingest of a `Stop`
        // can come before this pass saw the release.
        self.land_or_warn();
        Ok(())
    }

    /// Entries whose row did not come while the TUI sat idle.
    fn retry(&mut self) -> Result<()> {
        let quiet = !self.turn_open && !matches!(self.tail.last, Some(Last::User | Last::Working));
        if !quiet {
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
            if *n == 1 {
                match self.ctx.store.retract_delivery(&self.ctx.session, &p.id) {
                    Ok(Some(_)) => {
                        self.known.remove(&p.id);
                        self.report.retried += 1;
                        (self.ctx.ledger)(&format!("{}.retry {}", self.label, p.id));
                        log::warn!(
                            "horse channel: entry {} did not arrive; it goes out once more",
                            p.id
                        );
                        self.ctx.runs.give_back(&self.key, &p.id, true);
                    }
                    other => {
                        if let Err(e) = other {
                            log::warn!("horse channel: cannot queue entry {} again: {e:#}", p.id);
                        }
                        self.report.not_landed.push(p.id.clone());
                        self.ctx.runs.give_back(&self.key, &p.id, false);
                    }
                }
            } else {
                self.report.not_landed.push(p.id.clone());
                (self.ctx.ledger)(&format!("{}.lost {}", self.label, p.id));
                log::warn!("horse channel: entry {} did not arrive twice; it stays recorded and not landed", p.id);
                self.ctx.runs.give_back(&self.key, &p.id, false);
            }
        }
        Ok(())
    }

    /// Read the new rows of the transcript into the store. An error is a
    /// warning: the TUI goes on, and the final ingest of the run reads the
    /// file again.
    fn ingest(&mut self) {
        self.last_ingest = Instant::now();
        if !self.tail.path.is_file() || self.ingested_fp == self.tail.seen_fp {
            self.land_or_warn();
            return;
        }
        match self.ingest_now() {
            Ok(()) => {
                self.ingested_fp = self.tail.seen_fp;
                self.report.ingests += 1;
            }
            Err(e) => {
                self.report.ingest_errors += 1;
                log::warn!(
                    "horse channel: cannot ingest {}: {e:#}",
                    self.tail.path.display()
                );
            }
        }
        self.land_or_warn();
    }

    fn ingest_now(&mut self) -> Result<()> {
        let st = self.ctx.store.clone();
        let got = codec::ingest_run_locked(
            &st.lock()?,
            &self.tail.path,
            &self.ctx.version,
            &self.ctx.horse,
        )?;
        if got.session != self.ctx.session {
            bail!(
                "the transcript went into session {}, not {}",
                got.session,
                self.ctx.session
            );
        }
        if got.rewritten {
            log::info!("horse channel: claude rewrote the transcript of session {}; the store keeps the old rows", self.ctx.session);
        }
        self.head = st.session(&self.ctx.session)?.head;
        st.refresh_guard(&self.ctx.session)?;
        Ok(())
    }

    fn land_or_warn(&mut self) {
        if let Err(e) = self.land() {
            log::warn!("horse channel: cannot note a landing: {e:#}");
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
            let tag = format!("entry_id=\"{}\"", p.id);
            let from = p
                .at_head
                .as_ref()
                .and_then(|h| nodes.iter().position(|n| n.hash == *h))
                .map_or(0, |i| i + 1);
            let at = (from..nodes.len()).find(|k| {
                !claimed.contains(k)
                    && user_text(&nodes[*k].raw)
                        .is_some_and(|t| t.contains(&tag) || t.contains(&p.text))
            });
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
}

/// The text of a main-chain user row, or `None`.
fn user_text(raw: &[u8]) -> Option<String> {
    let n = codec::info(raw).ok()?;
    if n.ty != "user" || !n.is_chain() || n.sidechain {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(raw).ok()?;
    super::claude::wire::text_of(v.get("message")?)
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
                // No hook reaches this loop any more: the file still does.
                std::thread::sleep(timeout);
                None
            }
        };
        while let Some(k) = next.take() {
            if let Err(e) = self.signal(k.kind) {
                log::warn!("horse channel: {e:#}");
            }
            next = self.signals.try_recv().ok();
        }
        // The TUI goes on after an error of the store or the file: say it
        // once, and try again in the next pass. The final ingest of the
        // run reads the file again.
        match self.step() {
            Ok(()) => self.error = None,
            Err(e) => {
                let e = format!("{e:#}");
                if self.error.as_ref() != Some(&e) {
                    log::warn!("horse channel: {e}");
                    self.error = Some(e);
                }
            }
        }
        Ok(Pump::Open)
    }

    fn main_session(&self) -> Option<String> {
        Some(self.ctx.session.clone())
    }

    fn sessions(&self) -> Vec<LiveSession> {
        vec![LiveSession::turns(
            &self.ctx.session,
            &self.ctx.native,
            !self.busy,
            self.head.clone(),
        )]
    }

    /// The channel server delivers, never the driver.
    fn safe(&self, _session: Option<&str>) -> bool {
        false
    }

    fn deliver(&mut self, _input: Input) -> Result<()> {
        bail!("a TUI run gets its entries through the channel server")
    }

    fn interrupt(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn close_input(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }

    fn finish(&mut self, _end: End) -> Result<()> {
        // Process hook reports already received before closing the custody.
        while let Ok(report) = self.signals.try_recv() {
            if let Err(error) = self.signal(report.kind) {
                log::warn!("horse channel: {error:#}");
            }
        }
        // The TUI is gone: its transcript is complete now.
        if let Err(e) = self.tail.look() {
            log::warn!("horse channel: {e:#}");
        }
        self.ingest();
        self.report
            .not_landed
            .extend(self.pending.drain(..).map(|p| p.id));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
