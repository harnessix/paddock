//! The seam between the custody loop and the harnesses that it holds.
//!
//! A custodian holds the stdio of one harness process and keeps the store
//! in step with it. Five custodians implement [`Live`]: dsh
//! (custody.rs, JSON-RPC to an SDK runtime that streams every event),
//! Claude Code (claude.rs, stream-json to `claude -p`, the transcript file
//! after each turn), Codex (codex.rs, `codex app-server`, the rollout
//! file after each turn), Grok Build (grok.rs, ACP to `grok agent
//! stdio`, the session folder after each turn) and agy (agy.rs,
//! stream-json, the conversation database after each turn).
//!
//! A [`Live`] knows its wire. It reads what the harness sends, ingests
//! after every turn, answers the requests of the harness, and fails closed
//! when the harness reports another session id. It says when a session is
//! at a safe point: no turn runs and no request of paddock is on the way.
//!
//! The [`Driver`] is the one loop over every [`Live`]. It owns what the
//! harnesses share: the controls of the run (typed lines, interrupts, the
//! end), the delivery of inbox entries only at a safe point, the end of the
//! custody at a safe point, and the live record that `paddock horse
//! status` shows. An idle main session is a safe point at once. The
//! custody does not close while an entry waits in the inbox of the main
//! session, or while a hitched child of the main session owes its result
//! (hold.rs): the driver delivers inbox entries until then. A second
//! `Close` ends it at once. When the
//! [`Live`] still takes input after it was asked to end, the driver still
//! delivers inbox entries at the safe points until the harness ends. A
//! headless run ([`Ends::WhenDone`]) ends when its work is done: the main
//! session is at a safe point and its inbox is empty. The loop state stays
//! in the [`Driver`], so a run can drive another [`Live`] after this one
//! with the same held lines.

use crate::horse::inbox::Entry;
use crate::horse::store::{Hash, Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// The harnesses that paddock can hold in custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    /// `dsh --profile sdk` (custody.rs).
    Dsh,
    /// `claude -p` over stream-json (claude.rs).
    Claude,
    /// `codex app-server` over stdio (codex.rs).
    Codex,
    /// `grok agent stdio` over ACP (grok.rs).
    Grok,
    /// `agy --input-format stream-json` (agy.rs).
    Agy,
}

impl Harness {
    /// The custody of a harness, by its profile name.
    pub fn of(name: &str) -> Option<Harness> {
        match name {
            crate::horse::dsh::HARNESS => Some(Harness::Dsh),
            crate::horse::claude::HARNESS => Some(Harness::Claude),
            crate::horse::codex::HARNESS => Some(Harness::Codex),
            crate::horse::grok::HARNESS => Some(Harness::Grok),
            crate::horse::agy::HARNESS => Some(Harness::Agy),
            _ => None,
        }
    }
}

/// What the run tells a custody while it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// A line that the user typed.
    User(String),
    /// Stop the running turn.
    Interrupt,
    /// End the custody at the next safe point. A second `Close` ends it
    /// at once.
    Close,
}

/// Read the user's lines into the control channel. The end of the input
/// is a `Close`.
pub fn read_user(r: impl Read + Send + 'static, tx: Sender<Control>) {
    std::thread::spawn(move || {
        for l in BufReader::new(r).lines() {
            let Ok(l) = l else { break };
            if tx.send(Control::User(l)).is_err() {
                return;
            }
        }
        let _ = tx.send(Control::Close);
    });
}

/// The pipes of a harness in custody, and the controls of the run.
pub struct ChannelIo<R, W> {
    /// The output of the harness.
    pub from: R,
    /// The input of the harness.
    pub to: W,
    pub control: Receiver<Control>,
}

/// What goes into a harness at a safe point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A typed line for the main session (with [`Typed::Direct`]). It is
    /// not an inbox entry, so it has no delivery record.
    Typed(String),
    /// The oldest inbox entry of `entry.session`. The custodian records
    /// the delivery when the harness took it.
    Entry(Entry),
}

/// Where the typed lines of the user go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    /// The driver holds them and delivers them before inbox entries, at
    /// the next safe point of the main session.
    Direct,
    /// They go into the inbox of the main session at once, like an
    /// append. The driver holds them only until the main session is known.
    Inbox,
}

/// The harness after a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pump {
    Open,
    /// The harness closed its output.
    Closed,
}

/// How a custody ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The harness closed its output before paddock closed its input.
    Exited,
    /// paddock closed the input, and the harness closed its output.
    Closed,
    /// paddock closed the input, but the harness did not close its output
    /// within the close wait.
    TimedOut,
}

/// One harness process in custody. Every method returns at once except
/// [`Live::pump`], which waits for up to its timeout.
pub trait Live {
    /// Read what the harness sent for up to `timeout`, then what is there
    /// already, and act on it. An error ends the custody (fail closed).
    fn pump(&mut self, timeout: Duration) -> Result<Pump>;
    /// The store session of the main session, once it has one.
    fn main_session(&self) -> Option<String>;
    /// The sessions in custody, the main session first. Empty until the
    /// harness confirmed the id of the main session.
    fn sessions(&self) -> Vec<LiveSession>;
    /// A safe point of a store session, or of the main session for
    /// `None` (also before the main session has a store session).
    fn safe(&self, session: Option<&str>) -> bool;
    /// Send one input. The driver calls it only at a safe point of its
    /// session. A write that fails closes the input, and the input waits:
    /// an entry stays in the inbox.
    fn deliver(&mut self, input: Input) -> Result<()>;
    /// Stop the running turn. False when no turn runs or the harness has
    /// no way to stop one.
    fn interrupt(&mut self) -> Result<bool>;
    /// Close the input of the harness, or ask it to end. The harness then
    /// closes its output.
    fn close_input(&mut self) -> Result<()>;
    /// True while the harness still takes inputs after
    /// [`Live::close_input`]: it was only asked to end, and it did not
    /// end yet. The driver then still delivers inbox entries at the safe
    /// points, so an entry queued at the end (a DNA hot-load, a graft) is
    /// not held back. False when `close_input` closed the pipe.
    fn takes_input(&self) -> bool {
        false
    }
    /// The custody ends: ingest the session files a last time.
    fn finish(&mut self, end: End) -> Result<()>;
    /// Where the typed lines of the user go.
    fn typed(&self) -> Typed {
        Typed::Direct
    }
    /// True when [`Live::reload`] can load a session again while the
    /// harness runs. A swap of a session in a custody without it is
    /// refused, and the custody goes on.
    fn reloads(&self) -> bool {
        false
    }
    /// Drop `session` from the memory of the harness, call `write` with
    /// the folder of the native session files, and load the session again
    /// from its file. Only at a safe point of `session`. When `write`
    /// fails, the harness loads the unchanged file, and the error comes
    /// back in [`Reloaded::written`]. An error ends the custody.
    fn reload(
        &mut self,
        session: &str,
        write: &mut dyn FnMut(&std::path::Path) -> Result<()>,
    ) -> Result<Reloaded> {
        let _ = write;
        anyhow::bail!("this harness cannot load session {session} again while it runs")
    }
}

/// A session after [`Live::reload`].
#[derive(Debug)]
pub struct Reloaded {
    /// The native id that the harness holds now.
    pub native: String,
    /// What `write` gave.
    pub written: Result<()>,
}

/// One session in custody.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveSession {
    pub session: String,
    pub native: String,
    pub idle: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Hash>,
    #[serde(skip)]
    pub(super) next_seq: u64,
    #[serde(skip)]
    pub(super) reconcile: bool,
    #[serde(skip)]
    pub(super) prompting: bool,
}

impl LiveSession {
    /// A session as a turn-based custodian reports it.
    pub fn turns(session: &str, native: &str, idle: bool, head: Option<Hash>) -> Self {
        LiveSession {
            session: session.into(),
            native: native.into(),
            idle,
            head,
            next_seq: 0,
            reconcile: false,
            prompting: false,
        }
    }
}

/// The live record of a run: `live/<run>.json` in the store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub run: String,
    pub pid: u32,
    pub horse: String,
    pub main: String,
    pub started_ms: u64,
    pub updated_ms: u64,
    pub sessions: Vec<LiveSession>,
}

/// Write the live record of a run.
pub fn write_record(store: &Store, r: &Record) -> Result<()> {
    store.write_ref(
        &format!("live/{}.json", r.run),
        &serde_json::to_vec_pretty(r)?,
    )
}

/// Remove the live record of a run that ended. A record that is not there
/// is not an error.
pub fn remove_record(store: &Store, run: &str) {
    let _ = store.remove_ref(&format!("live/{run}.json"));
}

/// The part of a session that the live record shows.
type Shown = (String, String, bool, Option<Hash>);

/// Who ends a custody.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ends {
    /// The run: a [`Control::Close`], or the end of the control channel.
    #[default]
    ByRun,
    /// The work, for a headless run with no user: the driver delivers what
    /// waits in the inbox of the main session (the task first), and
    /// closes when the main session is at a safe point and its inbox is
    /// empty.
    WhenDone,
}

/// The custody loop.
pub struct Driver {
    store: Store,
    horse: String,
    /// The sandbox id: the name of the live record.
    run: String,
    /// The longest wait of one loop pass.
    tick: Duration,
    /// The longest wait for the harness to close its output after paddock
    /// closed its input. `None`: wait until it does.
    close_wait: Option<Duration>,
    held: VecDeque<String>,
    closing: bool,
    /// [`Ends::WhenDone`]: while closing, deliver the inbox entries first.
    drain: bool,
    started_ms: u64,
    shown: Option<Vec<Shown>>,
    /// Hitched children of the main session that still owe their result.
    owed: usize,
    /// The custody does not close yet (hold.rs), and it delivers inbox
    /// entries meanwhile.
    holding: bool,
    /// The run said that it holds its end for the queued entries.
    said_queued: bool,
    /// The inbox entries that went to the harness once. An entry that the
    /// harness refused does not hold the end again.
    tried: std::collections::HashSet<String>,
}

impl Driver {
    pub fn new(store: Store, horse: &str, run: &str, tick: Duration) -> Driver {
        Driver {
            store,
            horse: horse.into(),
            run: run.into(),
            tick,
            close_wait: None,
            held: VecDeque::new(),
            closing: false,
            drain: false,
            started_ms: crate::horse::now_ms(),
            shown: None,
            owed: 0,
            holding: false,
            said_queued: false,
            tried: Default::default(),
        }
    }

    /// Stop waiting for the harness this long after its input closed.
    pub fn close_wait(mut self, d: Duration) -> Driver {
        self.close_wait = Some(d);
        self
    }

    /// Who ends the custody. With [`Ends::WhenDone`], the custody is
    /// closing from the start.
    pub fn ends(mut self, ends: Ends) -> Driver {
        self.drain = ends == Ends::WhenDone;
        self.closing |= self.drain;
        self
    }

    /// Typed lines that wait for a safe point.
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// Hold the custody of `live` until the harness closes its output, or
    /// until the close wait ends after paddock closed the input. The live
    /// record is gone when this returns, also after an error.
    pub fn drive(&mut self, live: &mut dyn Live, control: &Receiver<Control>) -> Result<End> {
        let r = self.pass(live, control);
        remove_record(&self.store, &self.run);
        self.shown = None;
        r
    }

    fn pass(&mut self, live: &mut dyn Live, control: &Receiver<Control>) -> Result<End> {
        let mut deadline: Option<Option<Instant>> = None;
        let mut forced = false;
        let end = loop {
            let mut now = false;
            loop {
                match control.try_recv() {
                    Ok(Control::User(l)) if l.trim().is_empty() => {}
                    Ok(Control::User(l)) => self.held.push_back(l),
                    Ok(Control::Interrupt) => {
                        live.interrupt()?;
                    }
                    Ok(Control::Close) if self.closing => now = true,
                    Ok(Control::Close) => self.closing = true,
                    Err(TryRecvError::Disconnected) => {
                        self.closing = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            // A `ctl disconnect` closes like the first Close of the run.
            if super::disconnect::take(&self.store, live)? {
                self.closing = true;
            }
            self.queue_typed(live)?;
            if now && deadline.is_none() {
                // A second Close: the end comes at once, with no more input.
                forced = true;
                deadline = Some(self.close(live)?);
            }
            if live.pump(self.tick)? == Pump::Closed {
                break if deadline.is_some() {
                    End::Closed
                } else {
                    End::Exited
                };
            }
            if deadline.is_none() {
                self.flush_turn_end(live);
                crate::horse::codex::swap::request::serve(&self.store, live, self.tick)?;
                if self.safe_point(live)? {
                    deadline = Some(self.close(live)?);
                }
            } else if !forced && live.takes_input() {
                self.deliver_entries(live)?;
            }
            if let Some(Some(d)) = deadline {
                if Instant::now() >= d {
                    break End::TimedOut;
                }
            }
            self.show(live)?;
        };
        live.finish(end)?;
        Ok(end)
    }

    fn close(&mut self, live: &mut dyn Live) -> Result<Option<Instant>> {
        live.close_input()?;
        Ok(self.close_wait.map(|d| Instant::now() + d))
    }

    /// With [`Typed::Inbox`], the held lines go into the inbox of the main
    /// session as soon as it is known.
    fn queue_typed(&mut self, live: &dyn Live) -> Result<()> {
        if live.typed() != Typed::Inbox || self.held.is_empty() {
            return Ok(());
        }
        let Some(main) = live.main_session() else {
            return Ok(());
        };
        while let Some(l) = self.held.pop_front() {
            self.store.queue_append(&main, "user", &l)?;
        }
        Ok(())
    }

    /// Deliver what waits at the safe points. True when the custody
    /// closes now: the user asked for the end, and the main session is at
    /// a safe point with no typed line left.
    fn safe_point(&mut self, live: &mut dyn Live) -> Result<bool> {
        if live.safe(None) {
            if live.typed() == Typed::Direct {
                if let Some(t) = self.held.pop_front() {
                    live.deliver(Input::Typed(t))?;
                    return Ok(false);
                }
            }
            if self.closing && !self.waiting(live)? {
                return Ok(true);
            }
        }
        if !self.closing || self.drain || self.holding {
            self.deliver_entries(live)?;
        }
        Ok(false)
    }

    /// Deliver one inbox entry of each session at a safe point: the oldest
    /// urgent entry (`paddock horse append --interrupt`), else the oldest.
    fn deliver_entries(&mut self, live: &mut dyn Live) -> Result<()> {
        for s in live.sessions() {
            if !live.safe(Some(&s.session)) {
                continue;
            }
            let mut inbox = self.store.inbox(&s.session)?;
            if let Some(i) = inbox
                .iter()
                .position(|e| e.interrupt)
                .or(inbox.first().map(|_| 0))
            {
                let e = inbox.swap_remove(i);
                self.tried.insert(e.id.clone());
                live.deliver(Input::Entry(e))?;
            }
        }
        Ok(())
    }

    /// Release normal trainer advice only when the main session is idle.
    fn flush_turn_end(&self, live: &dyn Live) {
        let sessions = live.sessions();
        // An already-idle session is also between turns. Looking at the
        // current state covers a short turn whose start and end arrived in
        // one pump, without releasing anything during a running turn.
        if sessions.first().is_some_and(|current| current.idle) {
            if let Some(env) = self.store.host_env() {
                if let Err(error) =
                    crate::trainer::advice::deliver::flush_if_pending(env, &self.run)
                {
                    log::warn!("cannot deliver trainer advice at turn end: {error:#}");
                }
            }
        }
    }

    /// Write the live record when what it shows changed.
    fn show(&mut self, live: &dyn Live) -> Result<()> {
        let sessions = live.sessions();
        let Some(main) = sessions.first().map(|s| s.native.clone()) else {
            return Ok(());
        };
        let now: Vec<Shown> = sessions
            .iter()
            .map(|s| (s.session.clone(), s.native.clone(), s.idle, s.head.clone()))
            .collect();
        if self.shown.as_ref() == Some(&now) {
            return Ok(());
        }
        write_record(
            &self.store,
            &Record {
                run: self.run.clone(),
                pid: std::process::id(),
                horse: self.horse.clone(),
                main,
                started_ms: self.started_ms,
                updated_ms: crate::horse::now_ms(),
                sessions,
            },
        )?;
        self.shown = Some(now);
        Ok(())
    }
}

mod hold;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod seam_tests;

#[cfg(test)]
mod hitch_tests;

#[cfg(test)]
mod window_tests;
