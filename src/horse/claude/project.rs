//! Project a session head back into a Claude Code transcript.
//!
//! A session with a native id gets its rows back, byte for byte, in the
//! order paddock first saw them. After a native rewrite this is the file
//! without the rewrite: the dropped rows are back, and the rows of the
//! rewrite follow them. A fork that has no native id yet gets what Claude
//! Code's `/branch` writes (2.1.317): the chain rows on the active path to
//! the fork point with their own uuids, a new `sessionId`, `parentUuid` set
//! to the previous copied row that is not `progress`, `isSidechain: false`,
//! no `sessionKind`, and the fork marker `forkedFrom: {sessionId,
//! messageUuid}`. The state rows follow (`content-replacement`, then the
//! last `memory-mode`, `relocated`, `atis-latch` and `tools-used`), then a
//! `last-prompt` row that names the fork point as the leaf. The `message`
//! of every row stays byte for byte. The file goes into the project folder
//! of its cwd, through a synced temporary file and a link that never
//! replaces a file.

use super::request::{active_leaf, path_to, ACCUMULATE, LAST_WINS};
use super::{info, json, transcript_path, write_cursor, Info, CONTAINER, HARNESS, ORIGIN};
use crate::horse::store::{sync_dir, Hash, Store};
use anyhow::{bail, Context as _, Result};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// A projected transcript.
#[derive(Debug)]
pub struct Projection {
    pub path: PathBuf,
    pub native_id: String,
    /// The projection made a new native session for a fork.
    pub forked: bool,
}

/// The rows of a fork of session `parent_id` whose fork point is the last
/// of `rows`: a chain row is the leaf itself, any other row stands for the
/// active leaf at that point. Returns the rows and the cwd of the leaf.
pub fn fork_rows(
    rows: &[Vec<u8>],
    parent_id: &str,
    new_id: &str,
) -> Result<(Vec<Vec<u8>>, String)> {
    let parsed: Vec<(Info, &[u8])> = rows
        .iter()
        .map(|r| Ok((info(r)?, r.as_slice())))
        .collect::<Result<_>>()?;
    let (last, _) = parsed
        .last()
        .context("a Claude Code fork needs at least one row")?;
    let leaf = if last.is_chain() && !last.sidechain {
        last.uuid.clone().expect("chain row")
    } else {
        active_leaf(&parsed).context("the fork point has no chain row before it")?
    };
    let path = path_to(&parsed, &leaf)?;
    let sid = json::quote(new_id);
    let mut out = Vec::new();
    // A history suppression goes first, as /branch writes it.
    if let Some((_, raw)) = parsed
        .iter()
        .rev()
        .find(|(n, _)| n.ty == "history-suppression" && n.agent_id.is_none())
    {
        out.push(with_session(raw, &sid)?);
    }
    let mut prev: Option<String> = None;
    for i in &path {
        let (n, raw) = &parsed[*i];
        let uuid = n.uuid.as_deref().expect("chain row");
        let parent = prev.as_deref().map_or("null".to_string(), json::quote);
        let from = format!(
            "{{\"sessionId\":{},\"messageUuid\":{}}}",
            json::quote(parent_id),
            json::quote(uuid)
        );
        out.push(json::edit(
            raw,
            &[
                ("parentUuid", parent),
                ("isSidechain", "false".into()),
                ("sessionId", sid.clone()),
                ("forkedFrom", from),
            ],
            &["sessionKind"],
        )?);
        if n.ty != "progress" {
            prev = Some(uuid.to_string());
        }
    }
    for (n, raw) in &parsed {
        if n.agent_id.is_none() && ACCUMULATE.contains(&n.ty.as_str()) {
            out.push(with_session(raw, &sid)?);
        }
    }
    for t in LAST_WINS {
        if let Some((_, raw)) = parsed
            .iter()
            .rev()
            .find(|(n, _)| n.ty == t && n.agent_id.is_none())
        {
            out.push(with_session(raw, &sid)?);
        }
    }
    out.push(
        format!(
            "{{\"type\":\"last-prompt\",\"leafUuid\":{},\"explicit\":true,\"sessionId\":{sid}}}",
            json::quote(&leaf)
        )
        .into_bytes(),
    );
    let cwd = parsed[*path.last().expect("a path has its leaf")]
        .0
        .cwd
        .clone()
        .context("the fork point has no cwd")?;
    Ok((out, cwd))
}

/// A state row with the new session id, when it has one.
fn with_session(raw: &[u8], sid: &str) -> Result<Vec<u8>> {
    if json::raw_value(raw, "sessionId")?.is_some() {
        json::edit(raw, &[("sessionId", sid.to_string())], &[])
    } else {
        Ok(raw.to_vec())
    }
}

/// The cwd of the last main-thread chain row.
fn last_cwd(rows: &[Vec<u8>]) -> Result<String> {
    for r in rows.iter().rev() {
        let n = info(r)?;
        if n.is_chain() && !n.sidechain {
            if let Some(c) = n.cwd {
                return Ok(c);
            }
        }
    }
    bail!("the session has no chain row with a cwd")
}

/// Write the transcript of `session` under the Claude Code config folder
/// `config_dir` (`$CLAUDE_CONFIG_DIR`, by default `~/.claude` of the
/// sandbox home).
pub fn project(store: &Store, session: &str, config_dir: &Path) -> Result<Projection> {
    let s = store.session(session)?;
    if s.binding.harness != HARNESS || s.binding.container != CONTAINER {
        bail!("session {session} is not a Claude Code session");
    }
    super::check_version(&s.binding.version)?;
    let rows: Vec<Vec<u8>> = store
        .native_chain(s.head.as_ref())
        .with_context(|| format!("session {} cannot be projected", s.id))?
        .into_iter()
        .map(|n| n.raw)
        .collect();

    if let Some(id) = &s.binding.native_id {
        let path = transcript_path(config_dir, &last_cwd(&rows)?, id);
        publish(&path, &rows)?;
        return Ok(Projection {
            path,
            native_id: id.clone(),
            forked: false,
        });
    }

    let fork = s
        .fork_of
        .as_ref()
        .context("the session has no native id and is no fork")?;
    let parent_id = store
        .session(&fork.session)?
        .binding
        .native_id
        .with_context(|| format!("project the source session {} first", fork.session))?;
    let new_id = crate::horse::uuid_v4()?;
    let (rows, cwd) = fork_rows(&rows, &parent_id, &new_id)?;
    let path = transcript_path(config_dir, &cwd, &new_id);
    publish(&path, &rows)?;
    // The store follows the native file: its rows differ from the parent's
    // in the session fields, so they are a chain of their own. `fork_of`
    // and the shared uuids keep the link to the parent.
    let mut head: Option<Hash> = None;
    let scoped = store.for_horse(&s.horse)?;
    for r in &rows {
        head = Some(scoped.put_native(&ORIGIN, head.as_ref(), &super::key(&info(r)?), r)?);
    }
    store.update_session(session, |s| {
        s.head = head;
        s.binding.native_id = Some(new_id.clone());
    })?;
    write_cursor(store, session, &rows)?;
    Ok(Projection {
        path,
        native_id: new_id,
        forked: true,
    })
}

/// Publish a transcript without replacing anything. Folders are 0700 and
/// the file is 0600, as Claude Code makes them.
fn publish(path: &Path, rows: &[Vec<u8>]) -> Result<()> {
    let dir = path.parent().expect("a transcript has a folder");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    let mut bytes = Vec::new();
    for r in rows {
        bytes.extend_from_slice(r);
        bytes.push(b'\n');
    }
    let name = path.file_name().expect("a transcript has a name");
    let tmp = dir.join(format!(
        ".{}.paddock-tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("cannot create {}", tmp.display()))?;
    let written = f.write_all(&bytes).and_then(|_| f.sync_all());
    drop(f);
    let linked = written.and_then(|_| std::fs::hard_link(&tmp, path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!(
            "{} exists; paddock never replaces a Claude Code transcript",
            path.display()
        ),
        Err(e) => return Err(e).with_context(|| format!("cannot write {}", path.display())),
    }
    sync_dir(dir)
}
