//! The run mailbox: a two-way channel between a run and whoever started it.
//!
//! Each run gets one mailbox when it starts. It has two directions:
//!
//! - `down`: from the starter to the agent. The starter is a host caller (a
//!   terminal, a script, a Claude Code session on the host) or, for a child
//!   of `paddock-host spawn`, its parent sandbox (`Starter`).
//! - `up`: from the agent to the starter, plus events that paddock itself
//!   writes (events.rs): the sandbox ended, its process is gone, it is idle
//!   with unread mail, an approval card waits, a child started or ended.
//!
//! Storage, `$XDG_STATE_HOME/paddock/mail/<run>/`:
//!
//! - `down.jsonl`, `up.jsonl`: append-only logs, one JSON `Message` per line.
//! - `seq`: the last sequence number. One sequence counts both directions,
//!   so `paddock mail show RUN SEQ` names one message.
//! - `cursor-<dir>-<reader>`: the last sequence number that a reader got.
//!   The agent reads `down` as `agent`. `paddock mail watch` reads `up` as
//!   `starter` unless it names another reader. A cursor moves only after the
//!   reader got the messages, so a restart of either side loses nothing and
//!   shows nothing twice.
//! - `starter.json`, `run.json`: who started the run, and its paddock
//!   process (the liveness check of events.rs).
//! - `end`: the first end event (ended or gone). It makes that event unique.
//! - `lock`: every write takes it (flock), so writers of several processes
//!   get distinct sequence numbers and whole lines.
//!
//! The mailbox stays after the run, like the session record, and `paddock
//! gc --homes` removes it with the home. Mail is not paddock kv: kv is
//! shared state of many writers, mail is an ordered one-to-one channel.
//!
//! Mail text is data, never an instruction to paddock. It carries no
//! capability, so no approval card guards it. A sandbox writes only its own
//! `up` box and reads only its own `down` box (device.rs, serve.rs: the
//! sandbox id comes from the mount or the socket, never from the request).

pub mod deliver;
pub mod device;
pub mod events;
pub mod serve;
pub mod text;

use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read as _, Seek as _, Write as _};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The longest message text. Longer content goes to a file in the loft.
pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_REFS: usize = 16;
pub const MAX_REF: usize = 1024;
/// The reader name of the agent, for `down`.
pub const AGENT: &str = "agent";
/// The default reader name of `paddock mail watch`, for `up`.
pub const STARTER: &str = "starter";
/// Messages per minute that one sender may write into one box.
pub const RATE_PER_MINUTE: usize = 60;
/// How often a waiting reader looks at the log.
pub const POLL: Duration = Duration::from_millis(100);
/// The `from` of the events that paddock writes.
pub const PADDOCK: &str = "paddock";
/// The `from` of a host caller.
pub const HOST: &str = "host";
/// The `from` of the agent in its own sandbox.
pub const FROM_AGENT: &str = "agent";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Down,
    Up,
}

impl Dir {
    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Say,
    /// Expects an answer: a message with `reply_to` set to its seq.
    Ask,
    /// The agent's state: `working`, `blocked: REASON` or `done: SUMMARY`.
    Status,
    /// Written by paddock itself, never by an agent or a host caller.
    Event,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Say => "say",
            Kind::Ask => "ask",
            Kind::Status => "status",
            Kind::Event => "event",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "say" => Kind::Say,
            "ask" => Kind::Ask,
            "status" => Kind::Status,
            _ => return None,
        })
    }
}

/// One message. `seq` and `ts` come from the mailbox, not from the sender.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub seq: u64,
    pub ts: u64,
    pub from: String,
    pub kind: Kind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<u64>,
    /// The starter wants the agent to see this at once: a run with hook
    /// delivery shows it after the agent's next tool call (deliver.rs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupt: bool,
    /// The name of an event: `started`, `ended`, `gone`, `idle`, `card`,
    /// `child-started`, `child-ended`, `delivered`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// The exit code of `ended` and `child-ended`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    /// For a child's event: the child sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<String>,
    /// For `delivered`: the down messages that reached the agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seqs: Vec<u64>,
    /// For `delivered`: how they reached it (deliver.rs `Via`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

impl Message {
    /// A new message: the mailbox sets `seq` and `ts`.
    pub fn new(from: &str, kind: Kind, text: impl Into<String>) -> Message {
        Message {
            seq: 0,
            ts: 0,
            from: from.to_string(),
            kind,
            text: text.into(),
            refs: Vec::new(),
            reply_to: None,
            interrupt: false,
            event: None,
            code: None,
            child: None,
            seqs: Vec::new(),
            via: None,
        }
    }

    /// An event of paddock.
    pub fn event(name: &str, text: impl Into<String>) -> Message {
        let mut m = Message::new(PADDOCK, Kind::Event, text);
        m.event = Some(name.to_string());
        m
    }

    /// The end of the run: `ended` or `gone`.
    pub fn is_end(&self) -> bool {
        self.kind == Kind::Event && matches!(self.event.as_deref(), Some("ended" | "gone"))
    }
}

/// Who started a run, recorded when the mailbox is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Starter {
    /// `host` or `sandbox`.
    pub kind: String,
    /// The caller key of a host caller: `$PADDOCK_CALLER`, or `user`. `paddock
    /// mail watch --mine` follows the runs of one key.
    pub caller: String,
    /// For a child of `paddock-host spawn`: the parent sandbox. The child's
    /// `up` box is forwarded there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub created: u64,
}

impl Starter {
    /// The host caller of this process.
    pub fn host() -> Starter {
        Starter {
            kind: "host".into(),
            caller: caller_key(),
            parent: None,
            created: crate::state::now(),
        }
    }

    pub fn sandbox(parent: &str) -> Starter {
        Starter {
            kind: "sandbox".into(),
            caller: format!("sandbox:{parent}"),
            parent: Some(parent.to_string()),
            created: crate::state::now(),
        }
    }
}

/// `$PADDOCK_CALLER`, when it is a short plain name, or `user`.
pub fn caller_key() -> String {
    std::env::var("PADDOCK_CALLER")
        .ok()
        .filter(|c| valid_name(c))
        .unwrap_or_else(|| "user".into())
}

/// The paddock process of a run, for the liveness check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInfo {
    pub id: String,
    pub harness: String,
    pub project: PathBuf,
    pub pid: u32,
    /// The start time of `pid` in clock ticks after boot (Linux), so a
    /// reused pid does not look alive.
    #[serde(default)]
    pub pid_start: Option<u64>,
    pub started: u64,
    #[serde(default)]
    pub detached: bool,
    /// How down mail reaches the agent without its help: `hooks` when the
    /// harness runs paddock's mail hooks (deliver.rs), else none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horse_session: Option<String>,
}

/// How a send failed. The text says what to do instead.
#[derive(Debug)]
pub enum SendError {
    TooLarge(usize),
    Invalid(String),
    /// The sender wrote too many messages in the last minute.
    Rate,
    /// The sender may not write into this box.
    Denied(String),
    /// The run ended: nobody reads this box any more.
    Ended,
    NoMailbox(String),
    Io(anyhow::Error),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::TooLarge(n) => write!(f, "{}", too_large(*n)),
            SendError::Invalid(m) | SendError::Denied(m) => write!(f, "{m}"),
            SendError::Rate => write!(
                f,
                "too many messages: at most {RATE_PER_MINUTE} per minute into one mailbox. Wait a minute, or put more into one message"
            ),
            SendError::Ended => write!(f, "the run ended, so nobody reads this mailbox"),
            SendError::NoMailbox(id) => write!(
                f,
                "sandbox {id} has no mailbox: `paddock mail ls` lists the runs that have one"
            ),
            SendError::Io(e) => write!(f, "{e:#}"),
        }
    }
}

/// The line for a message that is too long: it names the loft route.
pub fn too_large(n: usize) -> String {
    format!(
        "the message has {n} bytes, more than the limit of {MAX_TEXT} bytes (64 KiB). Write the long text to a file in the loft (.paddock/ of the project, for example .paddock/mail/report.md) and send a short message that names it with --ref .paddock/mail/report.md"
    )
}

/// A reader or caller name: 1 to 64 of `A-Z a-z 0-9 - _ . :`.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// A ref is a path in the project or its loft: relative, without `..`.
/// Host paths outside the run's view go through `--attach` and `give`.
pub fn check_ref(r: &str) -> Result<(), SendError> {
    let p = Path::new(r);
    let ok = !r.is_empty()
        && r.len() <= MAX_REF
        && !r.chars().any(char::is_control)
        && p.is_relative()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(SendError::Invalid(format!(
            "ref {r:?} is not a path in the project: use a relative path such as .paddock/mail/report.md, without .. (at most {MAX_REF} bytes)"
        )))
    }
}

/// Check what a sender gave, before it goes into a log.
pub fn check(m: &Message) -> Result<(), SendError> {
    if m.text.len() > MAX_TEXT {
        return Err(SendError::TooLarge(m.text.len()));
    }
    if m.text.trim().is_empty() && m.kind != Kind::Event {
        return Err(SendError::Invalid("the message is empty".into()));
    }
    if m.refs.len() > MAX_REFS {
        return Err(SendError::Invalid(format!(
            "at most {MAX_REFS} refs per message"
        )));
    }
    m.refs.iter().try_for_each(|r| check_ref(r))?;
    if m.kind == Kind::Status {
        text::check_status(&m.text).map_err(SendError::Invalid)?;
    }
    Ok(())
}

pub fn root(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("mail")
}

/// The folder of one run's mailbox. The id must be a sandbox id.
pub fn dir(env: &HostEnv, id: &str) -> Result<PathBuf> {
    if !crate::view::valid_id(id) {
        bail!("{id:?} is not a sandbox id");
    }
    Ok(root(env).join(id))
}

pub fn exists(env: &HostEnv, id: &str) -> bool {
    dir(env, id).is_ok_and(|d| d.join("starter.json").is_file())
}

/// The runs that have a mailbox, oldest first.
pub fn list(env: &HostEnv) -> Vec<String> {
    let mut out: Vec<(u64, String)> = std::fs::read_dir(root(env))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_str()?.to_string();
            let s = starter(env, &id)?;
            Some((s.created, id))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, id)| id).collect()
}

/// Make the mailbox of a run, once. A second call keeps the first starter.
pub fn create(env: &HostEnv, id: &str, starter: &Starter) -> Result<()> {
    let d = dir(env, id)?;
    crate::privfs::dir(&d)?;
    match crate::privfs::write_new(&d.join("starter.json"), serde_json::to_vec(starter)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot make the mailbox of {id}")),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn starter(env: &HostEnv, id: &str) -> Option<Starter> {
    read_json(&dir(env, id).ok()?.join("starter.json"))
}

pub fn run_info(env: &HostEnv, id: &str) -> Option<RunInfo> {
    read_json(&dir(env, id).ok()?.join("run.json"))
}

pub fn set_run_info(env: &HostEnv, info: &RunInfo) -> Result<()> {
    let d = dir(env, &info.id)?;
    crate::privfs::write_atomic(&d.join("run.json"), serde_json::to_vec(info)?)?;
    Ok(())
}

/// The end event of the run, once one is written.
pub fn end(env: &HostEnv, id: &str) -> Option<Message> {
    read_json(&dir(env, id).ok()?.join("end"))
}

/// Hold the write lock of a mailbox until the guard drops.
pub struct Lock {
    _file: std::fs::File,
}

pub fn lock(d: &Path) -> Result<Lock> {
    let f = crate::privfs::append(&d.join("lock"))?;
    // SAFETY: flock on an open file; the lock goes with the file.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        bail!("cannot lock {}", d.display());
    }
    Ok(Lock { _file: f })
}

fn log_file(d: &Path, dir: Dir) -> PathBuf {
    d.join(format!("{}.jsonl", dir.name()))
}

/// The largest seq in the logs, for a missing `seq` file.
fn scan_seq(d: &Path) -> u64 {
    [Dir::Down, Dir::Up]
        .iter()
        .flat_map(|x| parse_log(&std::fs::read(log_file(d, *x)).unwrap_or_default()))
        .map(|m| m.seq)
        .max()
        .unwrap_or(0)
}

/// The last sequence number of the mailbox.
pub fn last_seq(env: &HostEnv, id: &str) -> u64 {
    let Ok(d) = dir(env, id) else { return 0 };
    match std::fs::read_to_string(d.join("seq")) {
        Ok(t) => t.trim().parse().unwrap_or_else(|_| scan_seq(&d)),
        Err(_) => scan_seq(&d),
    }
}

/// Whole lines only: a line that a writer is still writing waits.
fn parse_log(bytes: &[u8]) -> Vec<Message> {
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    bytes[..end]
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice(l).ok())
        .collect()
}

/// Every message of one direction, in order.
pub fn read(env: &HostEnv, id: &str, dir: Dir) -> Vec<Message> {
    let Ok(d) = self::dir(env, id) else {
        return Vec::new();
    };
    parse_log(&std::fs::read(log_file(&d, dir)).unwrap_or_default())
}

/// The messages of one direction after `after`.
pub fn read_after(env: &HostEnv, id: &str, dir: Dir, after: u64) -> Vec<Message> {
    let mut v = read(env, id, dir);
    v.retain(|m| m.seq > after);
    v
}

/// One message by its seq, in either direction.
pub fn find(env: &HostEnv, id: &str, seq: u64) -> Option<(Dir, Message)> {
    [Dir::Down, Dir::Up].into_iter().find_map(|d| {
        read(env, id, d)
            .into_iter()
            .find(|m| m.seq == seq)
            .map(|m| (d, m))
    })
}

fn rate_ok(d: &Path, dir: Dir, from: &str, now: u64) -> bool {
    let f = d.join(format!("rate-{}", dir.name()));
    let mut times: Vec<(String, u64)> = read_json(&f).unwrap_or_default();
    times.retain(|(_, t)| now.saturating_sub(*t) < 60);
    if times.iter().filter(|(w, _)| w == from).count() >= RATE_PER_MINUTE {
        return false;
    }
    times.push((from.to_string(), now));
    let _ = crate::privfs::write_atomic(&f, serde_json::to_vec(&times).unwrap_or_default());
    true
}

/// Append one message. Sets its seq and time and returns it. Checks the
/// size, the refs and the rate of senders; events of paddock skip the rate.
pub fn append(env: &HostEnv, id: &str, dir: Dir, mut m: Message) -> Result<Message, SendError> {
    check(&m)?;
    let d = self::dir(env, id).map_err(SendError::Io)?;
    if !exists(env, id) {
        return Err(SendError::NoMailbox(id.to_string()));
    }
    let _g = lock(&d).map_err(SendError::Io)?;
    let now = crate::state::now();
    if m.kind != Kind::Event && !rate_ok(&d, dir, &m.from, now) {
        return Err(SendError::Rate);
    }
    m.seq = last_seq(env, id) + 1;
    m.ts = now;
    // The seq goes first: a crash between the two leaves a gap, never two
    // messages with one seq.
    let io = |e: std::io::Error| SendError::Io(e.into());
    crate::privfs::write_atomic(&d.join("seq"), format!("{}\n", m.seq)).map_err(io)?;
    let mut line = serde_json::to_vec(&m).map_err(|e| SendError::Io(e.into()))?;
    line.push(b'\n');
    let mut f = crate::privfs::append(&log_file(&d, dir)).map_err(io)?;
    // A line that a crashed writer left half-written ends here.
    let len = f.metadata().map_err(io)?.len();
    if len > 0 {
        let mut last = [0u8];
        let mut r = std::fs::File::open(log_file(&d, dir)).map_err(io)?;
        r.seek(std::io::SeekFrom::Start(len - 1)).map_err(io)?;
        r.read_exact(&mut last).map_err(io)?;
        if last[0] != b'\n' {
            line.insert(0, b'\n');
        }
    }
    f.write_all(&line).map_err(io)?;
    Ok(m)
}

/// Write the end event of a run, once: `ended` or `gone`, whichever comes
/// first. Returns the message when this call wrote it.
pub fn append_end(env: &HostEnv, id: &str, m: Message) -> Option<Message> {
    let d = dir(env, id).ok()?;
    if !exists(env, id) {
        return None;
    }
    let _g = lock(&d).ok()?;
    if d.join("end").exists() {
        return None;
    }
    // The lock is not reentrant: `append` takes it again after this one goes.
    crate::privfs::write(&d.join("end"), serde_json::to_vec(&m).ok()?).ok()?;
    drop(_g);
    let written = append(env, id, Dir::Up, m).ok()?;
    let _ = crate::privfs::write_atomic(&d.join("end"), serde_json::to_vec(&written).ok()?);
    Some(written)
}

fn cursor_file(d: &Path, dir: Dir, reader: &str) -> PathBuf {
    d.join(format!("cursor-{}-{reader}", dir.name()))
}

/// The last seq that `reader` got from `dir`. 0 before the first read.
pub fn cursor(env: &HostEnv, id: &str, dir: Dir, reader: &str) -> u64 {
    let Ok(d) = self::dir(env, id) else { return 0 };
    if !valid_name(reader) {
        return 0;
    }
    std::fs::read_to_string(cursor_file(&d, dir, reader))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0)
}

/// Move a reader's cursor forward. It never moves back.
pub fn set_cursor(env: &HostEnv, id: &str, dir: Dir, reader: &str, seq: u64) -> Result<()> {
    if !valid_name(reader) {
        bail!("{reader:?} is not a reader name: 1 to 64 letters, digits, - _ . :");
    }
    let d = self::dir(env, id)?;
    if seq <= cursor(env, id, dir, reader) {
        return Ok(());
    }
    crate::privfs::write_atomic(&cursor_file(&d, dir, reader), format!("{seq}\n"))?;
    Ok(())
}

/// Down messages that the agent has not read yet.
pub fn unread_down(env: &HostEnv, id: &str) -> Vec<Message> {
    read_after(env, id, Dir::Down, cursor(env, id, Dir::Down, AGENT))
}

/// Wait until `dir` has messages after `after`, until `end` or until
/// `stop` says so. None when the time passed first.
pub fn wait(
    env: &HostEnv,
    id: &str,
    dir: Dir,
    after: u64,
    end: Instant,
    stop: &dyn Fn() -> bool,
) -> Option<Vec<Message>> {
    let file = self::dir(env, id).ok().map(|d| log_file(&d, dir));
    let mut seen = None;
    loop {
        let size = file
            .as_ref()
            .and_then(|f| std::fs::metadata(f).ok())
            .map(|m| m.len());
        if size != seen {
            seen = size;
            let v = read_after(env, id, dir, after);
            if !v.is_empty() {
                return Some(v);
            }
        }
        if Instant::now() >= end || stop() {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// Send a message down to the agent and hand it to the delivery of the
/// run. The log is the record; delivery only shortens the way.
pub fn send_down(env: &HostEnv, id: &str, m: Message) -> Result<Message, SendError> {
    if end(env, id).is_some() {
        return Err(SendError::Ended);
    }
    let m = append(env, id, Dir::Down, m)?;
    delivery(env, id).deliver(env, id, &m);
    Ok(m)
}

/// How a down message reaches an agent that does not read its mail.
pub trait Delivery: Send + Sync {
    /// Called after the message is in the log.
    fn deliver(&self, env: &HostEnv, id: &str, m: &Message);
    /// When the agent gets a message, for the starter: the end of
    /// `paddock mail send`'s answer.
    fn when(&self, m: &Message) -> &'static str;
}

/// The agent reads its mail itself (`paddock-host mail wait`).
pub struct SelfRead;

impl Delivery for SelfRead {
    fn deliver(&self, _: &HostEnv, _: &str, _: &Message) {}

    fn when(&self, _: &Message) -> &'static str {
        "the agent gets it when it reads its mail"
    }
}

/// The delivery of one run: the hooks of deliver.rs when the run has them.
pub fn delivery(env: &HostEnv, id: &str) -> Box<dyn Delivery> {
    match run_info(env, id).and_then(|r| r.delivery).as_deref() {
        Some(deliver::RELAY) => Box::new(deliver::Relay),
        Some(deliver::HOOKS) => Box::new(deliver::Hooks),
        _ => Box::new(SelfRead),
    }
}

/// A mailbox on disk, for `paddock gc`.
#[derive(Debug, Clone, Serialize)]
pub struct Stored {
    pub id: String,
    pub path: PathBuf,
    /// When the run ended: its end event, or the last write of a mailbox
    /// whose run never recorded one and no longer runs.
    pub ended: Option<u64>,
    pub bytes: u64,
}

/// Every mailbox, with its size and end.
pub fn stored(env: &HostEnv) -> Vec<Stored> {
    list(env)
        .into_iter()
        .filter_map(|id| {
            let path = dir(env, &id).ok()?;
            let files: Vec<std::fs::Metadata> = std::fs::read_dir(&path)
                .ok()?
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .collect();
            let bytes = files.iter().map(|m| m.len()).sum();
            let last = files
                .iter()
                .filter_map(|m| {
                    m.modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                })
                .map(|d| d.as_secs())
                .max();
            events::check_gone(env, &id);
            let ended = end(env, &id).map(|m| m.ts).or_else(|| {
                let alive = run_info(env, &id).is_some_and(|r| events::alive(r.pid, r.pid_start));
                (!alive).then_some(last).flatten()
            });
            Some(Stored {
                id,
                path,
                ended,
                bytes,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
