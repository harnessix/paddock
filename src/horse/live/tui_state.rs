//! The safe points of an interactive TUI run in horse mode.
//!
//! An interactive Claude Code has no stream that says "the turn is done".
//! Its hooks tell the host instead (`paddock-host horse safe-point stop`
//! after a reply, `prompt` when a turn starts), and a channel server in
//! the sandbox (`paddock-host channel`) asks the host for the entries of
//! the inbox that it may push now. The host keeps one `TuiState` per run
//! in its own memory, never in the store: a new host process knows
//! nothing of an old TUI, so it starts busy and waits for the next stop.
//!
//! Both requests go through the broker (`horse.channel_wait` and
//! `horse.safe_point`), bound to the horse and the session of the calling
//! sandbox. A sandbox never names a session: it gets its own entries only.

use crate::broker::{Request, Response, Status, PROTOCOL};
use crate::horse::device::Access;
use crate::horse::inbox::Entry;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The broker op of the long poll of the channel server.
pub const CHANNEL_WAIT: &str = "channel_wait";
/// The broker op of a hook at a safe point.
pub const SAFE_POINT: &str = "safe_point";
/// The longest wait of one `channel_wait`. It stays below the default
/// read limit of the gateway device (100 s), so a wait never reads as
/// "still waiting" there.
pub const MAX_WAIT: Duration = Duration::from_secs(60);
/// How often a wait looks at the inbox: `paddock horse append` runs in
/// another process and does not wake this one.
const POLL: Duration = Duration::from_millis(250);

pub type EntryId = String;

/// What the host knows of one TUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuiState {
    /// A turn may be running: nothing is released.
    pub busy: bool,
    /// When the last `stop` came, in ms since the epoch. 0: never.
    pub last_stop_ms: u64,
    /// The entries that this run released, in order.
    pub released: Vec<EntryId>,
    /// paddock types the entries into the pty of this run (live/pty.rs):
    /// a channel server gets none.
    pub typed: bool,
}

impl Default for TuiState {
    /// Busy until the first `stop`: the host cannot see the TUI, so it
    /// never guesses that no turn runs.
    fn default() -> Self {
        Self {
            busy: true,
            last_stop_ms: 0,
            released: Vec::new(),
            typed: false,
        }
    }
}

/// A report as a custody loop hears it: the kind, and the thread of the
/// harness that it is about, when the harness names one (Codex's
/// `notify` gives the thread id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heard {
    pub kind: SafePoint,
    pub thread: Option<String>,
    /// The turn that the report is about, when the hook names one (Grok
    /// Build's `promptId`).
    pub turn: Option<String>,
}

/// A thread id as a hook may name it: 1 to 64 letters, digits, `-` or `_`.
pub fn check_thread(t: &str) -> Result<()> {
    if t.is_empty()
        || t.len() > 64
        || !t
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("the thread id of the safe point is not valid");
    }
    Ok(())
}

/// What a hook reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafePoint {
    /// The reply is complete: the TUI is idle.
    Stop,
    /// A turn starts: the TUI is busy.
    Prompt,
}

impl std::str::FromStr for SafePoint {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "stop" => Ok(Self::Stop),
            "prompt" => Ok(Self::Prompt),
            _ => bail!("the safe point {s:?} is not known: use stop or prompt"),
        }
    }
}

/// Apply a hook's report.
pub fn mark(state: &mut TuiState, kind: SafePoint, now_ms: u64) {
    match kind {
        SafePoint::Stop => {
            state.busy = false;
            state.last_stop_ms = now_ms;
        }
        SafePoint::Prompt => state.busy = true,
    }
}

/// The entries of `inbox` that may go out now, in inbox order. Only an
/// idle TUI releases, and never an entry twice. A release makes the TUI
/// busy: the pushed entries start a turn, and the next `stop` ends it.
pub fn release_pending(state: &mut TuiState, inbox: Vec<Entry>) -> Vec<Entry> {
    if state.busy || state.typed {
        return Vec::new();
    }
    let out: Vec<Entry> = inbox
        .into_iter()
        .filter(|e| !state.released.contains(&e.id))
        .collect();
    if !out.is_empty() {
        state.released.extend(out.iter().map(|e| e.id.clone()));
        state.busy = true;
    }
    out
}

struct Run {
    state: Mutex<TuiState>,
    changed: Condvar,
    /// The custody loops that hear the hooks of this run.
    hear: Mutex<Vec<Sender<Heard>>>,
}

/// The hooks' reports of one run, as a custody loop hears them.
pub type SignalRx = Receiver<Heard>;

/// The key of the TUI state of the run of `session` of `horse`.
pub fn run_key(horse: &str, session: &str) -> String {
    format!("{horse}/{session}")
}

/// The TUI states of the runs of this host process.
#[derive(Default)]
pub struct Runs {
    runs: Mutex<HashMap<String, Arc<Run>>>,
}

impl Runs {
    fn run(&self, key: &str) -> Arc<Run> {
        self.runs
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_insert_with(|| {
                Arc::new(Run {
                    state: Mutex::new(TuiState::default()),
                    changed: Condvar::new(),
                    hear: Mutex::new(Vec::new()),
                })
            })
            .clone()
    }

    /// The state of one run, for status and tests.
    pub fn state(&self, key: &str) -> TuiState {
        self.run(key).state.lock().unwrap().clone()
    }

    pub fn mark(&self, key: &str, kind: SafePoint) {
        self.mark_thread(key, kind, None);
    }

    /// A report that names the thread of the harness.
    pub fn mark_thread(&self, key: &str, kind: SafePoint, thread: Option<&str>) {
        self.mark_turn(key, kind, thread, None);
    }

    /// A report that names the thread and the turn of the harness.
    pub fn mark_turn(&self, key: &str, kind: SafePoint, thread: Option<&str>, turn: Option<&str>) {
        let run = self.run(key);
        mark(&mut run.state.lock().unwrap(), kind, crate::horse::now_ms());
        run.changed.notify_all();
        let heard = Heard {
            kind,
            thread: thread.map(String::from),
            turn: turn.map(String::from),
        };
        run.hear
            .lock()
            .unwrap()
            .retain(|tx| tx.send(heard.clone()).is_ok());
    }

    /// paddock types the entries of this run into its pty: a channel
    /// server of the sandbox gets none of them.
    pub fn typed(&self, key: &str) {
        self.run(key).state.lock().unwrap().typed = true;
    }

    /// paddock typed an entry into the TUI: it is busy until the next
    /// `stop`. No custody loop hears this as a hook.
    pub fn hold(&self, key: &str) {
        self.run(key).state.lock().unwrap().busy = true;
    }

    /// Hear the reports of the hooks of one run from now on.
    pub fn subscribe(&self, key: &str) -> SignalRx {
        let (tx, rx) = std::sync::mpsc::channel();
        self.run(key).hear.lock().unwrap().push(tx);
        rx
    }

    /// The pushed entry `id` started no turn: the TUI is idle again, and
    /// with `again` the entry may go out once more (its delivery record
    /// is gone and it is back in the inbox).
    pub fn give_back(&self, key: &str, id: &str, again: bool) {
        let run = self.run(key);
        let mut st = run.state.lock().unwrap();
        if again {
            st.released.retain(|r| r != id);
        }
        st.busy = false;
        drop(st);
        run.changed.notify_all();
    }

    /// The run ended: drop its state.
    pub fn forget(&self, key: &str) {
        self.runs.lock().unwrap().remove(key);
    }

    /// Wait at most `timeout` for entries that may go out, and record each
    /// one with `record` before it is returned. When a record fails, that
    /// entry and the ones after it are not released, so the next wait
    /// tries them again; the ones before it are returned.
    pub fn wait(
        &self,
        key: &str,
        timeout: Duration,
        mut inbox: impl FnMut() -> Result<Vec<Entry>>,
        mut record: impl FnMut(&Entry) -> Result<()>,
    ) -> Result<Vec<Entry>> {
        let run = self.run(key);
        let deadline = Instant::now() + timeout;
        let mut st = run.state.lock().unwrap();
        loop {
            if !st.busy {
                let out = release_pending(&mut st, inbox()?);
                for (i, e) in out.iter().enumerate() {
                    if let Err(err) = record(e) {
                        let rest: Vec<&EntryId> = out[i..].iter().map(|e| &e.id).collect();
                        st.released.retain(|id| !rest.contains(&id));
                        if i == 0 {
                            st.busy = false;
                            return Err(err);
                        }
                        log::warn!(
                            "horse channel: entry {} is not recorded, it waits for the next safe point: {err:#}",
                            e.id
                        );
                        return Ok(out[..i].to_vec());
                    }
                }
                if !out.is_empty() {
                    return Ok(out);
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Vec::new());
            }
            st = run.changed.wait_timeout(st, left.min(POLL)).unwrap().0;
        }
    }
}

/// The runs of this host process.
pub fn runs() -> &'static Runs {
    static RUNS: OnceLock<Runs> = OnceLock::new();
    RUNS.get_or_init(Runs::default)
}

/// Whether a broker request is one of the two of this module.
pub fn is_live_op(req: &Request) -> bool {
    req.cap == "horse"
        && req
            .args
            .get("op")
            .and_then(|v| v.as_str())
            .is_some_and(|op| op == CHANNEL_WAIT || op == SAFE_POINT)
}

fn key(access: &Access) -> String {
    run_key(&access.horse, &access.session)
}

/// A `horse.channel_wait` or `horse.safe_point` request of the sandbox of
/// `access`. `ledger` gets one line per released entry. Without a horse
/// both answer the error of the other horse requests.
pub fn handle(access: Option<&Access>, req: &Request, ledger: &dyn Fn(&str)) -> Response {
    handle_in(runs(), access, req, ledger)
}

pub(crate) fn handle_in(
    runs: &Runs,
    access: Option<&Access>,
    req: &Request,
    ledger: &dyn Fn(&str),
) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        if req.v != PROTOCOL {
            bail!("unsupported protocol version");
        }
        let access = access.ok_or_else(|| anyhow!("this sandbox has no horse"))?;
        let op = req.args.get("op").and_then(|v| v.as_str()).unwrap_or("");
        if op == SAFE_POINT {
            let kind: SafePoint = req
                .args
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("missing safe point kind"))?
                .parse()?;
            let thread = req.args.get("thread").and_then(|v| v.as_str());
            let turn = req.args.get("turn").and_then(|v| v.as_str());
            for t in thread.iter().chain(turn.iter()) {
                check_thread(t)?;
            }
            runs.mark_turn(&key(access), kind, thread, turn);
            return Ok(serde_json::json!({"bytes": []}));
        }
        let since = req.args.get("since").and_then(|v| v.as_u64()).unwrap_or(0);
        let timeout = req
            .args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .map_or(MAX_WAIT, Duration::from_millis)
            .min(MAX_WAIT);
        let k = key(access);
        let had = runs.state(&k).released.len() as u64;
        if since < had {
            // A response got lost on its way to the channel server. Its
            // entries stay recorded and not landed: the ingest sees them.
            log::warn!("horse channel: the channel server got {since} of {had} released entries");
        }
        let store = &access.store;
        let entries = runs.wait(
            &k,
            timeout,
            || {
                // An entry whose delivery is recorded but whose inbox file
                // stayed (a failed remove) never goes out twice.
                let done = store.deliveries(&access.session)?;
                let mut pending = store.inbox(&access.session)?;
                pending.retain(|e| !done.iter().any(|d| d.entry.id == e.id));
                Ok(pending)
            },
            |e| {
                let head = store.session(&access.session)?.head;
                store.record_delivery(e, head.as_ref(), &e.id)?;
                ledger(&format!("channel.release {}", e.id));
                Ok(())
            },
        )?;
        let next = since + entries.len() as u64;
        Ok(serde_json::json!({"entries": entries, "next": next}))
    })();
    match result {
        Ok(v) => Response::done(v),
        Err(e) => Response::new(Status::Failed, format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests;
