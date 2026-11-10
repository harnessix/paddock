//! The Claude Code codec: session transcripts to store nodes and back.
//!
//! Pinned to Claude Code 2.1.317. The format is internal and closed: the
//! facts come from the docs at code.claude.com/docs (sessions, prompt
//! caching) and from reading the installed 2.1.317 binary without running
//! it. A transcript is `$CLAUDE_CONFIG_DIR/projects/<project>/<id>.jsonl`:
//! one JSON object per line, appended. Chain rows (`user`, `assistant`,
//! `attachment`, `system`, `progress`) have a `uuid` and a `parentUuid`.
//! Metadata rows (`last-prompt`, `custom-title`, `content-replacement`,
//! `api-request` and others) have no uuid. Every row of the file is a node,
//! metadata rows too; the message view skips the rows without a message. The last `last-prompt.leafUuid` is the active leaf.
//!
//! The store keeps the rows in the order paddock first saw them, byte for
//! byte, as a chain of nodes. A chain row is a node keyed `uuid:<uuid>`,
//! and its uuid is its identity: a row whose uuid the session has already
//! is not added again. Claude Code can rewrite a file of 5 MiB or more
//! that has a `compact_boundary`: it drops the rows before the boundary and
//! renames a new file over the old one. paddock keeps the dropped rows,
//! because it ingested them before. A cursor (the count and a digest of the
//! rows of the last file read) tells an append from a rewrite. Other
//! Claude Code versions are refused (fail closed).

pub mod json;
mod project;
pub mod request;
pub mod shape;

pub use project::{fork_rows, project, Projection};

use super::store::{Hash, Origin, Store};
use super::Binding;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const HARNESS: &str = "claude";
/// The Claude Code version that this codec follows: the installed binary
/// and the docs of 2026-11-04 show 2.1.317.
pub const VERSION: &str = "2.1.317";
pub const CONTAINER: &str = "claude-code-jsonl";

const ORIGIN: Origin = Origin {
    harness: HARNESS,
    container: CONTAINER,
    version: VERSION,
};

/// The row types that form the message chain.
pub const CHAIN_TYPES: [&str; 5] = ["user", "assistant", "attachment", "system", "progress"];

/// Refuse every Claude Code version but the pinned one.
pub fn check_version(version: &str) -> Result<()> {
    if version != VERSION {
        bail!(
            "Claude Code {version} is not supported: paddock reads and writes Claude Code sessions only for Claude Code {VERSION}"
        );
    }
    Ok(())
}

/// A Claude Code version whose push path into the interactive TUI was
/// checked in the strings of its binary: `--channels`, the
/// `notifications/claude/channel` push of an MCP server that declares
/// `capabilities.experimental["claude/channel"]`, and the `Stop` and
/// `UserPromptSubmit` hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelPin {
    pub version: &'static str,
    /// What the command line needs besides `--channels=server:<name>`
    /// for a server of `--mcp-config`, with `{}` for the server name.
    pub flags: &'static [&'static str],
}

/// The versions with a checked channel path. 2.1.318: a server that is
/// not a plugin of an approved marketplace loads only with
/// `--dangerously-load-development-channels`, which shows a confirmation
/// dialog at the start of the TUI. A version that is not here gets no
/// channel: paddock never guesses a protocol.
pub const CHANNEL_VERSIONS: [ChannelPin; 1] = [ChannelPin {
    version: "2.1.318",
    flags: &["--dangerously-load-development-channels=server:{}"],
}];

#[cfg(test)]
thread_local! {
    /// Tests: a channel pin for the codec version of this thread.
    static TEST_CHANNEL: std::cell::Cell<Option<ChannelPin>> = const { std::cell::Cell::new(None) };
}

/// Tests: give `version` the channel path of `like` in this thread.
#[cfg(test)]
pub(crate) fn test_channel_pin(version: &'static str, like: &ChannelPin) {
    TEST_CHANNEL.with(|c| {
        c.set(Some(ChannelPin {
            version,
            flags: like.flags,
        }))
    });
}

/// The channel pin of a Claude Code version, if it has one.
pub fn channel_pin(version: &str) -> Option<ChannelPin> {
    #[cfg(test)]
    if let Some(p) = TEST_CHANNEL
        .with(|c| c.get())
        .filter(|p| p.version == version)
    {
        return Some(p);
    }
    CHANNEL_VERSIONS
        .iter()
        .find(|p| p.version == version)
        .copied()
}

/// The fields of a row that the codec reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub ty: String,
    pub subtype: Option<String>,
    pub uuid: Option<String>,
    pub parent: Option<String>,
    pub session_id: Option<String>,
    pub version: Option<String>,
    pub cwd: Option<String>,
    pub sidechain: bool,
    pub agent_id: Option<String>,
    /// `leafUuid` of a `last-prompt` row.
    pub leaf: Option<String>,
}

impl Info {
    pub fn is_chain(&self) -> bool {
        CHAIN_TYPES.contains(&self.ty.as_str()) && self.uuid.is_some()
    }
}

/// Read the fields of a row. The row itself is not changed.
pub fn info(raw: &[u8]) -> Result<Info> {
    let v: Value = serde_json::from_slice(raw).context("a Claude Code row is not JSON")?;
    let o = v
        .as_object()
        .context("a Claude Code row is not an object")?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(Info {
        ty: s("type").context("a Claude Code row has no type")?,
        subtype: s("subtype"),
        uuid: s("uuid"),
        parent: s("parentUuid"),
        session_id: s("sessionId"),
        version: s("version"),
        cwd: s("cwd"),
        sidechain: o.get("isSidechain").and_then(Value::as_bool) == Some(true),
        agent_id: s("agentId"),
        leaf: s("leafUuid"),
    })
}

/// A session id must be a UUID, as Claude Code's adoption gate wants.
pub fn check_session_id(id: &str) -> Result<()> {
    let b = id.as_bytes();
    let ok = b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        });
    if !ok {
        bail!("not a Claude Code session id: {id}");
    }
    Ok(())
}

/// A transcript file, read.
#[derive(Debug)]
pub struct Log {
    pub session_id: String,
    /// The complete rows, byte for byte, without their newline.
    pub rows: Vec<Vec<u8>>,
    /// An incomplete last line was left out.
    pub torn: bool,
}

/// Read `<id>.jsonl`. A line without its newline is a write in progress
/// and is left out, and so is a cut line that a later write ended (see
/// `jsonl`). Every row must be a JSON object, and every row with a
/// `version` must have the pinned one.
pub fn read_file(path: &Path) -> Result<Log> {
    let id = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".jsonl"))
        .with_context(|| format!("{} is not a Claude Code transcript", path.display()))?;
    check_session_id(id)?;
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let (rows, torn) = super::jsonl::lines(&bytes);
    for (i, r) in rows.iter().enumerate() {
        let n = info(r).with_context(|| format!("{}: line {}", path.display(), i + 1))?;
        if let Some(v) = n.version.as_deref().filter(|v| *v != VERSION) {
            bail!(
                "{}: line {} was written by Claude Code {v}; paddock reads Claude Code {VERSION} only",
                path.display(),
                i + 1
            );
        }
        if n.is_chain() && !n.sidechain && n.session_id.as_deref() != Some(id) {
            bail!(
                "{}: line {} belongs to another session",
                path.display(),
                i + 1
            );
        }
    }
    Ok(Log {
        session_id: id.to_string(),
        rows,
        torn,
    })
}

/// `kM` of Claude Code: the project folder name of a cwd. Every UTF-16
/// unit that is not `[a-zA-Z0-9]` becomes `-`. A name longer than 200
/// units is cut to 200, then `-` and the base-36 of the absolute Java
/// `String.hashCode` of the cwd follow.
pub fn project_dir_name(cwd: &str) -> String {
    let name: String = cwd
        .encode_utf16()
        .map(|u| match char::from_u32(u32::from(u)) {
            Some(c) if c.is_ascii_alphanumeric() => c,
            _ => '-',
        })
        .collect();
    if name.len() <= 200 {
        return name;
    }
    format!("{}-{}", &name[..200], base36(java_hash(cwd).unsigned_abs()))
}

/// Java's `String.hashCode` over UTF-16 units, as Claude Code computes it.
pub fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(i32::from(u)))
}

fn base36(mut n: u32) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// The transcript path of a session under a Claude Code config folder.
pub fn transcript_path(config_dir: &Path, cwd: &str, id: &str) -> PathBuf {
    config_dir
        .join("projects")
        .join(project_dir_name(cwd))
        .join(format!("{id}.jsonl"))
}

/// Where the last ingest stopped: the number of complete rows of the file
/// and the sha256 of those rows, each with its newline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cursor {
    pub rows: usize,
    pub digest: String,
}

pub(crate) fn digest(rows: &[Vec<u8>]) -> String {
    let mut h = Sha256::new();
    for r in rows {
        h.update(r);
        h.update(b"\n");
    }
    super::hex(&h.finalize())
}

fn cursor_ref(session: &str) -> String {
    format!("claude/{session}.json")
}

pub(crate) fn read_cursor(store: &Store, session: &str) -> Result<Option<Cursor>> {
    match store.read_ref(&cursor_ref(session))? {
        Some(b) => Ok(Some(
            serde_json::from_slice(&b).context("a bad claude cursor")?,
        )),
        None => Ok(None),
    }
}

pub(crate) fn write_cursor(store: &Store, session: &str, rows: &[Vec<u8>]) -> Result<()> {
    let c = Cursor {
        rows: rows.len(),
        digest: digest(rows),
    };
    store.write_ref(&cursor_ref(session), &serde_json::to_vec(&c)?)
}

/// The key of a row node: `uuid:<uuid>` for a chain row, `type:<type>`
/// for a metadata row.
pub fn key(n: &Info) -> String {
    match (&n.uuid, n.is_chain()) {
        (Some(u), true) => format!("uuid:{u}"),
        _ => format!("type:{}", n.ty),
    }
}

/// What an ingest did.
#[derive(Debug, PartialEq, Eq)]
pub struct Ingest {
    pub session: String,
    /// Rows of the file that the store had already.
    pub kept: usize,
    /// New rows.
    pub added: usize,
    /// Claude Code rewrote the file since the last ingest. The rows that it
    /// dropped stay in the store and in the session.
    pub rewritten: bool,
    pub torn: bool,
}

/// Ingest a transcript into the store. A session that the store does not
/// know yet goes into `horse`, or into a new horse with the default DNA.
pub fn ingest(store: &Store, path: &Path, version: &str, horse: Option<&str>) -> Result<Ingest> {
    ingest_locked(&store.lock()?, path, version, horse)
}

/// Ingest while holding the store writer lock.
pub fn ingest_locked(
    locked: &super::store::Lock<'_>,
    path: &Path,
    version: &str,
    horse: Option<&str>,
) -> Result<Ingest> {
    ingest_impl(locked, path, version, horse, false)
}

pub(crate) fn ingest_run_locked(
    locked: &super::store::Lock<'_>,
    path: &Path,
    version: &str,
    horse: &str,
) -> Result<Ingest> {
    ingest_impl(locked, path, version, Some(horse), true)
}

fn ingest_impl(
    locked: &super::store::Lock<'_>,
    path: &Path,
    version: &str,
    horse: Option<&str>,
    scoped: bool,
) -> Result<Ingest> {
    let store = locked.store;
    check_version(version)?;
    let log = read_file(path)?;
    let existing = if scoped {
        store.sessions()?.into_iter().find(|s| {
            s.horse == horse.unwrap_or_default()
                && s.binding.harness == HARNESS
                && s.binding.native_id.as_deref() == Some(&log.session_id)
        })
    } else {
        store.session_by_native(HARNESS, &log.session_id)?
    };
    let session = match existing {
        Some(s) => s,
        None => {
            let h = match horse {
                Some(h) => store.horse(h)?,
                None => super::dna::new_default_horse_locked(locked)?,
            };
            locked.new_session(&h.id, binding(Some(log.session_id.clone())))?
        }
    };
    let old = store.session_nodes(&session)?;
    let cursor = read_cursor(store, &session.id)?;
    // An append keeps the rows of the last read as a prefix. Anything else
    // with rows in the store is a rewrite.
    let start = match &cursor {
        Some(c) if c.rows <= log.rows.len() && digest(&log.rows[..c.rows]) == c.digest => c.rows,
        _ => 0,
    };
    let rewritten = start == 0 && !old.is_empty();
    let mut known: HashSet<String> = old
        .iter()
        .filter_map(|n| n.meta.key.as_deref()?.strip_prefix("uuid:"))
        .map(str::to_string)
        .collect();
    let (mut kept, mut added) = (start, 0);
    let mut head: Option<Hash> = session.head.clone();
    let scoped = store.for_horse(&session.horse)?;
    for raw in &log.rows[start..] {
        let n = info(raw)?;
        if n.is_chain() {
            let u = n.uuid.clone().expect("a chain row has a uuid");
            if !known.insert(u) {
                kept += 1;
                continue;
            }
        }
        head = Some(scoped.put_native(&ORIGIN, head.as_ref(), &key(&n), raw)?);
        added += 1;
    }
    locked.update_session(&session.id, |s| s.head = head)?;
    write_cursor(store, &session.id, &log.rows)?;
    Ok(Ingest {
        session: session.id,
        kept,
        added,
        rewritten,
        torn: log.torn,
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

/// The message view of a row: the role and a short text of a chain row
/// with a message, or `None` for other rows.
pub fn view(raw: &[u8]) -> Option<super::dsh::View> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    let m = v.get("message")?;
    let role = m.get("role").and_then(Value::as_str)?.to_string();
    let text = match m.get("content")? {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b.get("text").and_then(Value::as_str).unwrap_or("").into(),
                Some("tool_use") => format!(
                    "[call {}]",
                    b.get("name").and_then(Value::as_str).unwrap_or("")
                ),
                Some(t) => format!("[{t}]"),
                None => String::new(),
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    Some(super::dsh::View { role, text })
}

#[cfg(test)]
mod rewrite_tests;
#[cfg(test)]
mod tests;
