//! Live custody of the sessions of one `dsh --profile sdk` runtime.
//!
//! The custodian holds the request side of the SDK channel. Every
//! `session.event` goes into the store as it comes: the event envelope is
//! a node keyed by its `seq`, with the session head as its parent. The
//! custodian is a [`Live`]: the driver of harness.rs delivers at a safe
//! point, a `session.status` of `idle` (the reply is complete, dsh closed
//! the turn with `turn/end`) with no prompt on the way. A typed line goes
//! to the main session first, then the oldest inbox entry of each idle
//! session, each as `session/prompt`, which dsh queues as a new user
//! message (a followup, never a steer). The SDK has no cancel method
//! (`packages/sdk/protocol/README.md`, Known Limitations), so paddock
//! never cuts a running turn. The end of the user's input becomes
//! `shutdown` at the next safe point of the main session. dsh still takes
//! prompts until it answers `shutdown` or its input breaks, so the driver
//! still delivers inbox entries (a DNA hot-load, a graft) until then.
//!
//! File ingest is for recovery only: a gap in `seq` (a resumed session,
//! whose old rows dsh does not stream again), a row that the file stores
//! in another form (`sourceEventSeqs` runs are packed on disk), and the
//! native header, which the stream does not carry. The file in the dsh
//! home of the sandbox is the native original, so it wins.

use super::harness::{Control, Driver, End, Input, Live, LiveSession, Pump};
use super::rpc::{Client, Msg};
use crate::horse::dsh::{self, CONTAINER, HARNESS, VERSION};
use crate::horse::guard::{self, Op};
use crate::horse::inbox::Entry;
use crate::horse::store::{Hash, Origin, Store};
use crate::horse::Binding;
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::Duration;

const ORIGIN: Origin = Origin {
    harness: HARNESS,
    container: CONTAINER,
    version: VERSION,
};

/// What a custodian needs.
#[derive(Clone, Debug)]
pub struct Setup {
    pub store: Store,
    pub horse: String,
    /// The dsh id of the main session.
    pub main: String,
    /// Resume the main session (`session/resume`) before the first prompt.
    pub resume: bool,
    /// The dsh sessions folder as paddock sees it, for recovery.
    pub sessions_root: Option<PathBuf>,
    pub cwd: String,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    /// The sandbox id: the name of the live record.
    pub run: String,
    pub tick: Duration,
}

/// What a custody did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub events: usize,
    pub delivered: usize,
    pub reconciled: usize,
    pub sessions: Vec<String>,
}

enum Pending {
    Init,
    Resume,
    Prompt {
        native: String,
        entry: Option<Entry>,
        head: Option<Hash>,
    },
    Shutdown,
}

/// The sessions of one dsh runtime in custody.
pub struct Custodian<W: Write, O: Write> {
    set: Setup,
    client: Client<W>,
    rx: Receiver<Msg>,
    out: O,
    sessions: Vec<LiveSession>,
    pending: HashMap<u64, Pending>,
    ready: bool,
    /// What paddock did to end the runtime.
    ending: Ending,
    report: Report,
    refusals: refusal::Refusals,
}

/// The end of a dsh runtime, as paddock asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// paddock did not ask for the end.
    No,
    /// paddock sent `shutdown`. dsh still takes prompts until it answers.
    Asked,
    /// dsh answered `shutdown`, or its input is gone.
    Done,
}

/// Run the custody until the runtime closes its stdout.
pub fn run<W: Write, O: Write>(
    set: Setup,
    w: W,
    rx: Receiver<Msg>,
    control: &Receiver<Control>,
    out: O,
) -> Result<Report> {
    let mut d = Driver::new(set.store.clone(), &set.horse, &set.run, set.tick);
    let mut c = Custodian::start(set, w, rx, out)?;
    d.drive(&mut c, control)?;
    Ok(c.report())
}

impl<W: Write, O: Write> Custodian<W, O> {
    /// Send the SDK handshake. The main session joins the horse now.
    pub fn start(set: Setup, w: W, rx: Receiver<Msg>, out: O) -> Result<Self> {
        let mut c = Custodian {
            client: Client::new(w),
            rx,
            out,
            sessions: Vec::new(),
            pending: HashMap::new(),
            ready: false,
            ending: Ending::No,
            report: Report::default(),
            refusals: Default::default(),
            set,
        };
        let main = c.set.main.clone();
        c.live(&main)?;
        let mut p = json!({ "cwd": c.set.cwd, "provider": c.set.provider, "model": c.set.model });
        if let Some(e) = &c.set.effort {
            p["reasoningEffort"] = json!(e);
        }
        let id = c.client.request("initialize", Some(p))?;
        c.pending.insert(id, Pending::Init);
        Ok(c)
    }

    /// What the custody did.
    pub fn report(&self) -> Report {
        let mut r = self.report.clone();
        r.sessions = self.sessions.iter().map(|s| s.session.clone()).collect();
        r
    }

    fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    fn handle(&mut self, m: Msg) -> Result<()> {
        match m {
            Msg::Response { id, result } => self.response(id, result)?,
            Msg::Event {
                session,
                ty,
                seq,
                raw,
            } => self.ingest(&session, &ty, seq, &raw)?,
            Msg::Status { session, idle } => {
                let i = self.live(&session)?;
                self.sessions[i].idle = idle;
                if idle {
                    if self.sessions[i].reconcile || self.header_missing(i)? {
                        self.reconcile(i)?;
                    }
                    let sid = self.sessions[i].session.clone();
                    self.set.store.refresh_guard(&sid)?;
                }
            }
            Msg::Other(_) | Msg::Closed => {}
        }
        Ok(())
    }

    fn response(&mut self, id: u64, result: std::result::Result<Value, String>) -> Result<()> {
        let Some(p) = self.pending.remove(&id) else {
            return Ok(());
        };
        match (p, result) {
            (Pending::Init, Ok(_)) if self.set.resume => {
                let id = self.client.request(
                    "session/resume",
                    Some(json!({ "sessionId": self.set.main })),
                )?;
                self.pending.insert(id, Pending::Resume);
            }
            (Pending::Init | Pending::Resume, Ok(_)) => self.ready = true,
            (Pending::Init, Err(e)) => bail!("dsh refused the SDK handshake: {e}"),
            (Pending::Resume, Err(e)) => {
                bail!("dsh cannot resume session {}: {e}", self.set.main)
            }
            (
                Pending::Prompt {
                    native,
                    entry,
                    head,
                },
                Ok(v),
            ) => {
                let i = self.live(&native)?;
                self.sessions[i].prompting = false;
                let mid = v
                    .get("messageId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(e) = entry {
                    self.refusals.taken(&e);
                    self.set.store.record_delivery(&e, head.as_ref(), mid)?;
                    self.report.delivered += 1;
                    self.say(&format!(
                        "paddock: delivered append {} ({}) to session {}",
                        e.id, e.role, e.session
                    ));
                }
            }
            (Pending::Prompt { native, entry, .. }, Err(e)) => {
                // A refused prompt starts no turn, and dsh sends no status.
                let i = self.live(&native)?;
                self.sessions[i].prompting = false;
                self.sessions[i].idle = true;
                match entry {
                    Some(x) => self.refused(x, &e)?,
                    None => self.say(&format!("paddock: dsh did not take the prompt: {e}")),
                }
            }
            (Pending::Shutdown, _) => self.ending = Ending::Done,
        }
        Ok(())
    }

    fn prompt(&mut self, native: &str, text: &str, entry: Option<Entry>) -> Result<()> {
        let i = self.live(native)?;
        let p = json!({ "sessionId": native, "contentBlocks": [{ "type": "text", "text": text }] });
        let id = self.client.request("session/prompt", Some(p))?;
        let s = &mut self.sessions[i];
        s.idle = false;
        s.prompting = true;
        self.pending.insert(
            id,
            Pending::Prompt {
                native: native.into(),
                entry,
                head: s.head.clone(),
            },
        );
        Ok(())
    }

    /// An inbox entry goes out: the guard judges the append first.
    fn deliver_entry(&mut self, e: Entry) -> Result<()> {
        let Some(native) = self
            .sessions
            .iter()
            .find(|s| s.session == e.session)
            .map(|s| s.native.clone())
        else {
            bail!("session {} is not in this custody", e.session);
        };
        if self.refusals.waits(&e.session) {
            // The session is still at a safe point, so the run can end.
            return Ok(());
        }
        let v = guard::check(
            &self.set.store,
            &e.session,
            &Op::Append { after: None },
            crate::horse::now_ms(),
        )?;
        self.set.store.save_verdict(&v)?;
        if !v.keeps || !v.reasons.is_empty() {
            self.say(&format!("paddock: {}", v.message()));
        }
        let text = e.wire_text();
        self.prompt(&native, &text, Some(e))
    }

    /// The index of a session in custody; a new one joins the horse.
    fn live(&mut self, native: &str) -> Result<usize> {
        if let Some(i) = self.sessions.iter().position(|s| s.native == native) {
            return Ok(i);
        }
        let st = &self.set.store;
        let locked = st.lock()?;
        let s = match st.sessions()?.into_iter().find(|s| {
            s.horse == self.set.horse
                && s.binding.harness == HARNESS
                && s.binding.native_id.as_deref() == Some(native)
        }) {
            Some(s) => s,
            None => locked.new_session(
                &self.set.horse,
                Binding {
                    harness: HARNESS.into(),
                    native_id: Some(native.into()),
                    version: VERSION.into(),
                    container: CONTAINER.into(),
                },
            )?,
        };
        let next_seq = st.session_nodes(&s)?.len() as u64;
        self.sessions.push(LiveSession {
            session: s.id,
            native: native.into(),
            idle: true,
            head: s.head,
            next_seq,
            reconcile: false,
            prompting: false,
        });
        Ok(self.sessions.len() - 1)
    }

    fn ingest(&mut self, native: &str, ty: &str, seq: u64, raw: &[u8]) -> Result<()> {
        let i = self.live(native)?;
        self.report.events += 1;
        if seq > self.sessions[i].next_seq {
            self.reconcile(i)?;
        }
        let s = &self.sessions[i];
        if seq < s.next_seq {
            return Ok(());
        }
        if seq > s.next_seq {
            // The file does not have the rows before it yet.
            self.sessions[i].reconcile = true;
            return Ok(());
        }
        let (sid, head) = (s.session.clone(), s.head.clone());
        if ty == "request/header" {
            self.check_header(&sid, raw)?;
        }
        let st = &self.set.store;
        let locked = st.lock()?;
        let h =
            st.for_session(&sid)?
                .put_native(&ORIGIN, head.as_ref(), &format!("seq:{seq}"), raw)?;
        locked.update_session(&sid, |x| x.head = Some(h.clone()))?;
        drop(locked);
        let s = &mut self.sessions[i];
        s.head = Some(h);
        s.next_seq += 1;
        if raw.windows(17).any(|w| w == b"\"sourceEventSeqs\"") {
            s.reconcile = true;
        }
        let v: Value = serde_json::from_slice(raw).unwrap_or_default();
        if ty == "user/message" {
            if let Some(mid) = v.pointer("/data/id").and_then(Value::as_str) {
                st.delivery_landed(&sid, mid, seq)?;
            }
        }
        if ty == "assistant/message" {
            if let Some(view) = dsh::view(raw).filter(|x| !x.text.is_empty()) {
                self.say(&format!("dsh: {}", view.text));
            }
        }
        Ok(())
    }

    /// A new request header with another model or tool list: the guard
    /// judges the change on the rows before it.
    fn check_header(&mut self, sid: &str, raw: &[u8]) -> Result<()> {
        let v: Value = serde_json::from_slice(raw).unwrap_or_default();
        let st = self.set.store.clone();
        let old = guard::tuple(&st, &st.session(sid)?)?;
        let cfg = v.pointer("/data/header/config");
        let model = cfg.and_then(|c| c.get("model")).and_then(Value::as_str);
        let mut ops = Vec::new();
        if let (Some(a), Some(b)) = (old.model.as_deref(), model) {
            if a != b {
                ops.push(Op::Model(b.into()));
            }
        }
        let tools = crate::horse::dsh::request::J::parse(raw)
            .ok()
            .and_then(|j| Some(j.get("data")?.get("header")?.get("tools")?.text()));
        if let (Some(old_hash), Some(t)) = (&old.tools_hash, &tools) {
            if Hash::of(t.as_bytes()) != *old_hash {
                ops.push(Op::Tools { added_only: false });
            }
        }
        for op in ops {
            let v = guard::check(&st, sid, &op, crate::horse::now_ms())?;
            st.save_verdict(&v)?;
            self.say(&format!("paddock: {}", v.message()));
        }
        Ok(())
    }

    fn header_missing(&self, i: usize) -> Result<bool> {
        let s = self.set.store.session(&self.sessions[i].session)?;
        Ok(s.native_header.is_none() && self.set.sessions_root.is_some())
    }

    /// Read the session file and make the store agree with it.
    fn reconcile(&mut self, i: usize) -> Result<()> {
        let Some(root) = self.set.sessions_root.clone() else {
            return Ok(());
        };
        let (sid, native) = (
            self.sessions[i].session.clone(),
            self.sessions[i].native.clone(),
        );
        let Some(dir) = find_session_dir(&root, &native) else {
            return Ok(());
        };
        let log = dsh::read_file(&dsh::current_file(&dir)?)
            .with_context(|| format!("cannot read the dsh file of {native}"))?;
        if log.header.id != native {
            bail!("the dsh file in {} has another id", dir.display());
        }
        let st = &self.set.store;
        let locked = st.lock()?;
        let old = st.session_nodes(&st.session(&sid)?)?;
        let scoped = st.for_session(&sid)?;
        let (mut parent, mut same) = (None::<Hash>, true);
        for (k, r) in log.rows.iter().enumerate() {
            let h = scoped.put_native(&ORIGIN, parent.as_ref(), &format!("seq:{k}"), r)?;
            same &= old.get(k).is_some_and(|n| n.hash == h);
            parent = Some(h);
        }
        let header = scoped.put_blob("dsh-header", &log.header_raw)?;
        // A file that lags behind the stream keeps the live head.
        let take = !same || log.rows.len() >= old.len();
        let s = locked.update_session(&sid, |s| {
            if take {
                s.head = parent.clone();
            }
            s.native_header = Some(header);
        })?;
        drop(locked);
        let l = &mut self.sessions[i];
        if take {
            l.head = s.head;
            l.next_seq = log.rows.len() as u64;
        }
        l.reconcile = !take;
        self.report.reconciled += 1;
        if !same {
            self.say(&format!(
                "paddock: session {sid} took {} rows from the dsh file",
                log.rows.len()
            ));
        }
        Ok(())
    }
}

impl<W: Write, O: Write> Live for Custodian<W, O> {
    fn pump(&mut self, timeout: Duration) -> Result<Pump> {
        let mut m = match self.rx.recv_timeout(timeout) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(Pump::Closed),
        };
        while let Some(x) = m.take() {
            if x == Msg::Closed {
                return Ok(Pump::Closed);
            }
            self.handle(x)?;
            m = match self.rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return Ok(Pump::Closed),
            };
        }
        Ok(Pump::Open)
    }

    fn main_session(&self) -> Option<String> {
        self.sessions.first().map(|s| s.session.clone())
    }

    fn sessions(&self) -> Vec<LiveSession> {
        self.sessions.clone()
    }

    fn safe(&self, session: Option<&str>) -> bool {
        let s = match session {
            None => self.sessions.first(),
            Some(id) => self.sessions.iter().find(|s| s.session == id),
        };
        self.ready && s.is_some_and(|s| s.idle && !s.prompting)
    }

    fn deliver(&mut self, input: Input) -> Result<()> {
        let r = match input {
            Input::Typed(t) => {
                let main = self.set.main.clone();
                self.prompt(&main, &t, None)
            }
            Input::Entry(e) => self.deliver_entry(e),
        };
        match r {
            // dsh can end after `shutdown` at any time, also during this
            // write. The entry stays in the inbox for the next run. Any
            // other error still ends the custody.
            Err(e) if self.ending != Ending::No && is_io(&e) => {
                self.ending = Ending::Done;
                self.say(&format!(
                    "paddock: dsh ended and did not take the input: {e:#}"
                ));
                Ok(())
            }
            r => r,
        }
    }

    fn interrupt(&mut self) -> Result<bool> {
        self.say("paddock: the dsh SDK cannot stop a turn; the turn runs to its end");
        Ok(false)
    }

    fn close_input(&mut self) -> Result<()> {
        self.ending = Ending::Asked;
        match self.client.request("shutdown", None) {
            Ok(id) => {
                self.pending.insert(id, Pending::Shutdown);
            }
            // The runtime closed its input already: it ends.
            Err(_) => self.ending = Ending::Done,
        }
        Ok(())
    }

    fn takes_input(&self) -> bool {
        self.ending == Ending::Asked
    }

    fn finish(&mut self, _end: End) -> Result<()> {
        // The runtime is gone: the files are complete now.
        for i in 0..self.sessions.len() {
            if self.sessions[i].reconcile || self.header_missing(i)? {
                self.reconcile(i)?;
            }
        }
        Ok(())
    }
}

/// A write to the runtime failed.
fn is_io(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<std::io::Error>())
}

/// The folder of a dsh session under a sessions root, in any project.
pub fn find_session_dir(root: &Path, native: &str) -> Option<PathBuf> {
    let seg = dsh::encode_segment(native);
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&seg))
        .find(|p| p.is_dir())
}

mod refusal;

#[cfg(test)]
mod fixture_tests;

#[cfg(test)]
mod refusal_tests;
