//! Grok Build 1.0.70 session codec.
//!
//! A Grok session is a folder `$GROK_HOME/sessions/<encoded cwd>/<id>/`
//! with `summary.json`, `updates.jsonl` (the append-only log of session
//! updates, which drives a resume) and `chat_history.jsonl` (the items
//! that Grok sends to the model, rewritten by a rename on a compaction or
//! a rewind). Every complete row of the two files is a node, byte exact,
//! in one chain:
//! - an `updates.jsonl` row has the key `event:<eventId>` from
//!   `params._meta.eventId`, else `line:<n>` (its line in the log). Grok
//!   only appends to the log, so a known key with other bytes is refused;
//! - a `chat_history.jsonl` item has the key `chat:<prompt_index>:<hash>:<n>`:
//!   its content and its `prompt_index`, and `n` counts equal items before
//!   it. The file is rewritten, not appended, so the content is the
//!   identity, not the line;
//! - a rewrite of `chat_history.jsonl` (anything else than new items at
//!   the end) is an index node `rewrite:<hash>` that lists the keys of the
//!   new file. Items that the store has already get no new node.
//!
//! The chat items of a turn follow the updates rows of that turn, so the
//! last chat node of a turn is a fork point at a turn for both files.

#[cfg(test)]
pub(crate) mod fixture;
mod project;
pub mod replay;
pub mod request;
pub mod swap;

use super::store::{Hash, Lock, Origin};
use super::{Binding, Store};
use anyhow::{bail, Context as _, Result};
pub use project::{project, Projection};
use replay::{chat_keys, replay, Files};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const VERSION: &str = "1.0.70";
pub const HARNESS: &str = "grok";
/// The session folder of Grok: summary.json and the two JSONL files.
pub const CONTAINER: &str = "grok-session-dir";
pub const UPDATES: &str = "grok-updates-jsonl";
pub const CHAT: &str = "grok-chat-jsonl";
/// The only `chat_format_version` of `summary.json` that paddock reads.
pub const CHAT_FORMAT: u64 = 1;
const SUMMARY_MEDIA: &str = "grok-summary";
pub(crate) const UPDATES_FILE: &str = "updates.jsonl";
pub(crate) const CHAT_FILE: &str = "chat_history.jsonl";
pub(crate) const SUMMARY_FILE: &str = "summary.json";
const XAI_METHOD: &str = "_x.ai/session/update";
const ACP_METHOD: &str = "session/update";

pub(crate) const fn origin(container: &'static str) -> Origin<'static> {
    Origin {
        harness: HARNESS,
        container,
        version: VERSION,
    }
}

pub fn check_version(v: &str) -> Result<()> {
    if v != VERSION {
        bail!("Grok Build {v} is not supported; expected {VERSION}");
    }
    Ok(())
}

/// The folder name of a working directory under `sessions/`: the
/// URL-encoded path, or for a name over 255 bytes a slug of the last path
/// part, a dash and 16 hex digits of the BLAKE3 hash of the path.
pub fn encode_cwd(cwd: &str) -> String {
    let enc = url_encode(cwd);
    if enc.len() <= 255 {
        return enc;
    }
    let leaf = Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let mut slug = String::new();
    let mut dash = false;
    for c in leaf.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            dash = false;
        } else if !dash {
            slug.push('-');
            dash = true;
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(40).collect();
    let slug = if slug.is_empty() { "workspace" } else { &slug };
    let hash = blake3::hash(cwd.as_bytes()).to_hex();
    format!("{slug}-{}", &hash[..16])
}

/// The URL encoding of Grok: letters, digits and `-_.~` stay, every other
/// byte is `%XX`.
pub(crate) fn url_encode(s: &str) -> String {
    let mut enc = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    enc
}

/// The session folder of `id` in working directory `cwd` under `home`.
pub fn session_dir(home: &Path, cwd: &str, id: &str) -> PathBuf {
    home.join("sessions").join(encode_cwd(cwd)).join(id)
}

/// What paddock reads of an `updates.jsonl` row.
#[derive(Debug, Default, Clone)]
pub(crate) struct Update {
    pub key: Option<String>,
    /// A user message chunk of a prompt (not a host turn).
    pub user: bool,
    pub prompt_index: Option<u64>,
}

pub(crate) fn parse_update(raw: &[u8]) -> Result<Update> {
    if raw.is_empty() {
        return Ok(Update::default());
    }
    let v: Value = serde_json::from_slice(raw).context("invalid Grok update row")?;
    if !v.is_object() {
        bail!("a Grok update row is not a JSON object");
    }
    let method = v["method"].as_str().unwrap_or(ACP_METHOD);
    if method != ACP_METHOD && method != XAI_METHOD {
        bail!("unknown Grok update method {method}");
    }
    let p = if v.get("method").is_some() {
        &v["params"]
    } else {
        &v
    };
    let u = &p["update"];
    let tag = u["sessionUpdate"].as_str().unwrap_or_default();
    let host = u["_meta"]["hostTurn"].as_bool() == Some(true);
    Ok(Update {
        key: p["_meta"]["eventId"].as_str().map(|e| format!("event:{e}")),
        user: method == ACP_METHOD && !host && tag == "user_message_chunk",
        prompt_index: u["_meta"]["promptIndex"].as_u64(),
    })
}

/// The key of an updates row at line `index`.
pub(crate) fn update_key(raw: &[u8], index: usize) -> Result<String> {
    Ok(parse_update(raw)?
        .key
        .unwrap_or_else(|| format!("line:{index}")))
}

/// Complete lines of a JSONL file, and whether a torn last line was left
/// out. A cut line that a later write ended is left out too (see
/// `jsonl`). A missing file has no lines.
pub(crate) fn read_lines(path: &Path) -> Result<(Vec<Vec<u8>>, bool)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), false)),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    Ok(super::jsonl::lines(&bytes))
}

/// `summary.json` of a session folder, checked.
pub(crate) struct Summary {
    pub raw: Vec<u8>,
    pub id: String,
    pub cwd: String,
}

pub(crate) fn parse_summary(raw: &[u8]) -> Result<Summary> {
    let v: Value = serde_json::from_slice(raw).context("invalid Grok summary.json")?;
    let format = v["chat_format_version"].as_u64().unwrap_or(0);
    if format != CHAT_FORMAT {
        bail!("Grok chat_format_version {format} is not supported; expected {CHAT_FORMAT}");
    }
    let id = v["info"]["id"].as_str().context("summary.json has no id")?;
    super::claude::check_session_id(id)?;
    let cwd = v["info"]["cwd"]
        .as_str()
        .context("summary.json has no cwd")?;
    if !cwd.starts_with('/') {
        bail!("summary.json has a relative cwd");
    }
    Ok(Summary {
        raw: raw.to_vec(),
        id: id.into(),
        cwd: cwd.into(),
    })
}

/// A Grok session folder, read.
pub(crate) struct Folder {
    pub summary: Summary,
    pub updates: Vec<Vec<u8>>,
    pub chat: Vec<Vec<u8>>,
    pub torn: bool,
}

pub(crate) fn read_folder(dir: &Path) -> Result<Folder> {
    let raw = std::fs::read(dir.join(SUMMARY_FILE))
        .with_context(|| format!("no Grok summary.json in {}", dir.display()))?;
    let summary = parse_summary(&raw)?;
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if name != summary.id {
        bail!(
            "the session folder {name} does not match session {}",
            summary.id
        );
    }
    let group = dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if group != encode_cwd(&summary.cwd) {
        bail!(
            "the folder {group} is not the encoded working directory {} of the session",
            summary.cwd
        );
    }
    let (updates, torn_u) = read_lines(&dir.join(UPDATES_FILE))?;
    let (chat, torn_c) = read_lines(&dir.join(CHAT_FILE))?;
    for r in &updates {
        parse_update(r)?;
    }
    for r in &chat {
        replay::parse_chat(r)?;
    }
    Ok(Folder {
        summary,
        updates,
        chat,
        torn: torn_u || torn_c,
    })
}

#[derive(Debug)]
pub struct Ingest {
    pub session: String,
    pub added: usize,
    pub kept: usize,
    /// Rows of the store that the files do not have any more. They stay.
    pub missing: usize,
    /// `chat_history.jsonl` was rewritten since the last ingest.
    pub rewritten: bool,
    pub torn: bool,
}

/// Read the session folder `dir` into the store.
pub fn ingest(store: &Store, dir: &Path, version: &str, horse: Option<&str>) -> Result<Ingest> {
    ingest_locked(&store.lock()?, dir, version, horse)
}

pub fn ingest_locked(
    locked: &Lock<'_>,
    dir: &Path,
    version: &str,
    horse: Option<&str>,
) -> Result<Ingest> {
    ingest_impl(locked, dir, version, super::target::Target::Any(horse))
}

pub(crate) fn ingest_run_locked(
    locked: &Lock<'_>,
    dir: &Path,
    version: &str,
    horse: &str,
) -> Result<Ingest> {
    ingest_impl(locked, dir, version, super::target::Target::Run(horse))
}

/// Ingest a private copy of a host session file into its host session,
/// or into a new host session in `horse`.
pub(crate) fn ingest_host_locked(
    locked: &Lock<'_>,
    dir: &Path,
    version: &str,
    horse: &str,
    origin: &super::HostOrigin,
) -> Result<Ingest> {
    ingest_impl(
        locked,
        dir,
        version,
        super::target::Target::Host { horse, origin },
    )
}

fn ingest_impl(
    locked: &Lock<'_>,
    dir: &Path,
    version: &str,
    target: super::target::Target<'_>,
) -> Result<Ingest> {
    let store = locked.store;
    check_version(version)?;
    let f = read_folder(dir)?;
    let id = f.summary.id.clone();
    let active = store
        .read_ref(&format!("grok/{id}.json"))?
        .map(String::from_utf8)
        .transpose()?;
    let active = if target.scoped() { None } else { active };
    let session = match active {
        Some(s) => store.session(&s)?,
        None => target.session(locked, HARNESS, &id, || binding(Some(id.clone())))?,
    };
    check_version(&session.binding.version)?;
    if session.binding.harness != HARNESS || session.binding.container != CONTAINER {
        bail!("invalid Grok binding");
    }
    let nodes = store.session_nodes(&session)?;
    let known_updates: HashMap<String, Vec<u8>> = nodes
        .iter()
        .filter(|n| n.meta.container.as_deref() == Some(UPDATES))
        .map(|n| (n.meta.key.clone().unwrap_or_default(), n.raw.clone()))
        .collect();
    let known_chat: HashSet<String> = nodes
        .iter()
        .filter(|n| n.meta.container.as_deref() == Some(CHAT))
        .filter_map(|n| n.meta.key.clone())
        .collect();
    let state: Files = replay(&nodes)?;

    // updates.jsonl: new rows by key.
    let mut new_updates = Vec::new();
    let mut seen = 0;
    for (i, raw) in f.updates.iter().enumerate() {
        let key = update_key(raw, i)?;
        match known_updates.get(&key) {
            Some(old) if old == raw => seen += 1,
            Some(_) => bail!("Grok updates.jsonl changed: row {key} has other bytes"),
            None => new_updates.push(i),
        }
    }
    // chat_history.jsonl: an append of new items, or a rewrite.
    let keys = chat_keys(&f.chat)?;
    let appended = keys.len() >= state.chat_keys.len()
        && keys[..state.chat_keys.len()] == state.chat_keys[..]
        && keys[state.chat_keys.len()..]
            .iter()
            .all(|k| !known_chat.contains(k));
    let rewritten = !appended;
    let mut new_chat: Vec<usize> = Vec::new();
    let mut queued = HashSet::new();
    for (i, k) in keys.iter().enumerate() {
        if !known_chat.contains(k) && queued.insert(k.clone()) {
            new_chat.push(i);
        }
    }

    let mut head = session.head.clone();
    let mut added = 0;
    let scoped = store.for_horse(&session.horse)?;
    let mut put = |container: &'static str, key: &str, raw: &[u8]| -> Result<()> {
        head = Some(scoped.put_native(&origin(container), head.as_ref(), key, raw)?);
        added += 1;
        Ok(())
    };
    let turns = replay::chat_turns(&f.chat)?;
    let mut next_chat = 0;
    if rewritten {
        for &i in &new_updates {
            put(UPDATES, &update_key(&f.updates[i], i)?, &f.updates[i])?;
        }
        let list = serde_json::to_vec(&keys)?;
        let key = format!("rewrite:{}", &Hash::of(&list).as_str()[..16]);
        put(CHAT, &key, &list)?;
    } else {
        let opens = replay::update_opens(&f.updates)?;
        for &i in &new_updates {
            if let Some(p) = opens[i] {
                while next_chat < new_chat.len() && turns[new_chat[next_chat]] < p as i64 {
                    let c = new_chat[next_chat];
                    put(CHAT, &keys[c], &f.chat[c])?;
                    next_chat += 1;
                }
            }
            put(UPDATES, &update_key(&f.updates[i], i)?, &f.updates[i])?;
        }
    }
    for &c in &new_chat[next_chat..] {
        put(CHAT, &keys[c], &f.chat[c])?;
    }
    let header = scoped.put_blob(SUMMARY_MEDIA, &f.summary.raw)?;
    locked.update_session(&session.id, |s| {
        s.head = head;
        s.native_header = Some(header);
    })?;
    let missing = known_updates.len().saturating_sub(seen)
        + state.chat_keys.iter().filter(|k| !keys.contains(k)).count();
    Ok(Ingest {
        session: session.id,
        added,
        kept: seen + (keys.len() - new_chat.len()),
        missing,
        rewritten: rewritten && !state.chat_keys.is_empty(),
        torn: f.torn,
    })
}

pub(crate) fn binding(native_id: Option<String>) -> Binding {
    Binding {
        harness: HARNESS.into(),
        native_id,
        version: VERSION.into(),
        container: CONTAINER.into(),
    }
}

/// The session id in the summary.json of the session folder `dir`.
pub fn read_summary_id(dir: &Path) -> Result<String> {
    let raw = std::fs::read(dir.join(SUMMARY_FILE))
        .with_context(|| format!("no Grok summary.json in {}", dir.display()))?;
    Ok(parse_summary(&raw)?.id)
}

/// The summary.json of a session, from its header blob.
pub(crate) fn stored_summary(store: &Store, s: &super::Session) -> Result<Summary> {
    let h = s
        .native_header
        .as_ref()
        .context("the session has no Grok summary.json")?;
    parse_summary(&store.get(h)?.raw)
}

/// The last chat node of turn `turn` of a session: a fork there keeps the
/// updates and the chat items up to the end of that turn.
pub fn turn_end(store: &Store, session: &str, turn: u64) -> Result<Hash> {
    let s = store.session(session)?;
    let nodes = store.session_nodes(&s)?;
    let mut last = None;
    let mut cur: i64 = -1;
    for n in &nodes {
        if n.meta.container.as_deref() != Some(CHAT) || replay::is_rewrite(n) {
            continue;
        }
        if let Some(p) = replay::parse_chat(&n.raw)?.prompt_index {
            cur = p as i64;
        }
        if cur == turn as i64 {
            last = Some(n.hash.clone());
        }
    }
    last.with_context(|| format!("session {session} has no chat items of turn {turn}"))
}

#[cfg(test)]
mod rewrite_tests;
#[cfg(test)]
mod tests;
