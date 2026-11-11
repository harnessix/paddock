//! Write a session or a paddock fork back as a Grok session folder.

use super::replay::{jsonl, replay, Files};
use super::{
    check_version, encode_cwd, origin, stored_summary, update_key, url_encode, CHAT, CHAT_FILE,
    CONTAINER, HARNESS, SUMMARY_FILE, SUMMARY_MEDIA, UPDATES, UPDATES_FILE,
};
use crate::horse::claude::json;
use crate::horse::store::{sync_dir, Hash, Node, Store};
use anyhow::{bail, Context as _, Result};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Projection {
    /// The session folder.
    pub path: PathBuf,
    pub native_id: String,
    pub forked: bool,
    /// The folder with the old files after a swap.
    pub backup: Option<PathBuf>,
}

/// A new session id as Grok makes one: a UUID version 7.
pub(crate) fn uuid_v7() -> Result<String> {
    let mut id = crate::horse::uuid_v4()?;
    let ms = format!("{:012x}", crate::horse::now_ms());
    id.replace_range(0..8, &ms[..8]);
    id.replace_range(9..13, &ms[8..]);
    id.replace_range(14..15, "7");
    Ok(id)
}

/// Write session `session` as a Grok session folder under the Grok home
/// `home`. A session with a native id keeps it: its folder must be new.
/// A paddock fork gets a new id and the shape of a native fork. `swap`: a
/// paddock fork replaces the files of the session that it forks from,
/// under the same id.
pub fn project(store: &Store, session: &str, home: &Path, swap: bool) -> Result<Projection> {
    let s = store.session(session)?;
    if s.binding.harness != HARNESS || s.binding.container != CONTAINER {
        bail!("not a Grok session");
    }
    check_version(&s.binding.version)?;
    let nodes = store
        .native_chain(s.head.as_ref())
        .with_context(|| format!("session {} cannot be projected", s.id))?;
    let mut files = replay(&nodes)?;
    let summary = stored_summary(store, &s)?;
    let forked = s.binding.native_id.is_none();
    if swap && !forked {
        bail!("swap requires an unprojected fork");
    }
    let id = if forked && !swap {
        uuid_v7()?
    } else {
        s.binding
            .native_id
            .clone()
            .unwrap_or_else(|| summary.id.clone())
    };
    let dir = super::session_dir(home, &summary.cwd, &id);
    let summary_raw = if forked && !swap {
        files.updates = files
            .updates
            .iter()
            .map(|r| with_session_id(r, &id))
            .collect::<Result<_>>()?;
        fork_summary(&summary.raw, &id, &summary.id, &files)?
    } else if swap {
        counted_summary(&summary.raw, &files)?
    } else {
        summary.raw.clone()
    };
    let backup = publish(&dir, &summary.cwd, &files, &summary_raw, swap)?;
    if forked {
        let scoped = store.for_horse(&s.horse)?;
        let head = put_chain(&scoped, &nodes, &files)?;
        let header = scoped.put_blob(SUMMARY_MEDIA, &summary_raw)?;
        store.update_session(session, |s| {
            s.head = head;
            s.native_header = Some(header);
            s.binding.native_id = Some(id.clone());
        })?;
        if swap {
            store.write_ref(&format!("grok/{id}.json"), session.as_bytes())?;
        }
    }
    Ok(Projection {
        path: dir,
        native_id: id,
        forked,
        backup,
    })
}

/// The chain of a projected fork: the nodes in the same order, the
/// updates rows with their new bytes.
fn put_chain(store: &Store, nodes: &[Node], files: &Files) -> Result<Option<Hash>> {
    let mut head = None;
    let mut u = 0;
    for n in nodes {
        let container = n.meta.container.as_deref().unwrap_or_default();
        let (container, key, raw) = if container == UPDATES {
            let raw = &files.updates[u];
            u += 1;
            (UPDATES, update_key(raw, u - 1)?, raw.clone())
        } else {
            (CHAT, n.meta.key.clone().unwrap_or_default(), n.raw.clone())
        };
        head = Some(store.put_native(&origin(container), head.as_ref(), &key, &raw)?);
    }
    Ok(head)
}

/// An updates row under another session id: `params.sessionId` changes,
/// every other byte stays, as in a native fork.
pub(crate) fn with_session_id(raw: &[u8], id: &str) -> Result<Vec<u8>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(params) = json::raw_value(raw, "params")? {
        let p = json::edit(params, &[("sessionId", json::quote(id))], &[])?;
        return json::edit(raw, &[("params", String::from_utf8(p)?)], &[]);
    }
    json::edit(raw, &[("sessionId", json::quote(id))], &[])
}

fn now() -> String {
    crate::waiting::format_rfc3339(crate::horse::now_ms() / 1000)
}

/// summary.json of a native fork: the new id, the parent, the kind, new
/// times and counts. The fields that describe the last turn of the parent
/// go away, as in a native fork at a turn.
pub(crate) fn fork_summary(raw: &[u8], id: &str, parent: &str, files: &Files) -> Result<Vec<u8>> {
    let info = json::raw_value(raw, "info")?.context("summary.json has no info")?;
    let info = json::edit(info, &[("id", json::quote(id))], &[])?;
    let t = json::quote(&now());
    json::edit(
        raw,
        &[
            ("info", String::from_utf8(info)?),
            ("created_at", t.clone()),
            ("updated_at", t.clone()),
            ("num_messages", files.updates.len().to_string()),
            ("num_chat_messages", files.chat.len().to_string()),
            ("parent_session_id", json::quote(parent)),
            ("forked_at", t),
            ("next_trace_turn", "0".into()),
            ("session_kind", json::quote("fork")),
        ],
        &[
            "agent_id",
            "attempt_id",
            "collection_id",
            "request_id",
            "hidden",
            "git_root_dir",
            "git_remotes",
            "pending_cwd_switch_reminder",
            "fork_context_source",
            "fork_parent_prompt_id",
            "inherited_prefix_len",
            "last_turn_summary",
            "last_turn_summary_prompt_id",
            "last_recap",
        ],
    )
}

/// summary.json after a swap: the counts of the branch.
pub(crate) fn counted_summary(raw: &[u8], files: &Files) -> Result<Vec<u8>> {
    json::edit(
        raw,
        &[
            ("updated_at", json::quote(&now())),
            ("num_messages", files.updates.len().to_string()),
            ("num_chat_messages", files.chat.len().to_string()),
        ],
        &[
            "last_turn_summary",
            "last_turn_summary_prompt_id",
            "last_recap",
        ],
    )
}

fn mkdir(dir: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot make {}", dir.display()))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Write the three files into the session folder `dir`. A new folder is
/// written as a temporary folder and renamed. `replace`: the files of the
/// folder are replaced one by one by a rename while paddock holds Grok's
/// summary lock, and the old files stay as hard links in a folder
/// `.paddock-old-<uuid>` in the session folder, whose path comes back.
pub(crate) fn publish(
    dir: &Path,
    cwd: &str,
    files: &Files,
    summary: &[u8],
    replace: bool,
) -> Result<Option<PathBuf>> {
    let group = dir.parent().context("the session folder has no parent")?;
    mkdir(group)?;
    if encode_cwd(cwd) != url_encode(cwd) {
        // The long form: Grok finds the path in `.cwd`.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(group.join(".cwd"))
        {
            Ok(mut f) => {
                f.write_all(cwd.as_bytes())?;
                f.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).context("cannot write the .cwd file"),
        }
    }
    let parts: [(&str, Vec<u8>); 3] = [
        (UPDATES_FILE, jsonl(&files.updates)),
        (CHAT_FILE, jsonl(&files.chat)),
        (SUMMARY_FILE, summary.to_vec()),
    ];
    if !replace {
        if dir.exists() {
            bail!("the Grok session folder {} exists already", dir.display());
        }
        let temp = group.join(format!(".paddock-{}", crate::horse::uuid_v4()?));
        mkdir(&temp)?;
        let written = (|| -> Result<()> {
            for (name, bytes) in &parts {
                write_new(&temp.join(name), bytes)?;
            }
            sync_dir(&temp)?;
            std::fs::rename(&temp, dir).context("cannot publish the session folder")
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&temp);
            return Err(e);
        }
        sync_dir(group)?;
        return Ok(None);
    }
    if !dir.join(SUMMARY_FILE).is_file() {
        bail!(
            "no Grok session at {}: project the session first",
            dir.display()
        );
    }
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join(format!("{SUMMARY_FILE}.lock")))?;
    // SAFETY: flock on a descriptor that this function owns.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("Grok holds the summary lock of the session");
    }
    let backup = dir.join(format!(".paddock-old-{}", crate::horse::uuid_v4()?));
    mkdir(&backup)?;
    for (name, _) in &parts {
        match std::fs::hard_link(dir.join(name), backup.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("cannot keep the old session files"),
        }
    }
    sync_dir(&backup)?;
    for (name, bytes) in &parts {
        let temp = dir.join(format!(".paddock-{}", crate::horse::uuid_v4()?));
        let r = write_new(&temp, bytes)
            .and_then(|()| std::fs::rename(&temp, dir.join(name)).map_err(Into::into));
        if let Err(e) = r {
            let _ = std::fs::remove_file(&temp);
            return Err(e.context(format!("cannot replace {name}")));
        }
    }
    sync_dir(dir)?;
    drop(lock);
    Ok(Some(backup))
}
