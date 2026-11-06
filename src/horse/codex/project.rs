use super::{check_version, key, parse, CONTAINER, HARNESS, ORIGIN};
use crate::horse::{
    claude::json,
    store::{sync_dir, Store},
};
use anyhow::{bail, Context as _, Result};
use std::{
    io::Write,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Projection {
    pub path: PathBuf,
    pub native_id: String,
    pub forked: bool,
    pub backup: Option<PathBuf>,
}

pub fn project(store: &Store, session: &str, config: &Path, swap: bool) -> Result<Projection> {
    let s = store.session(session)?;
    if s.binding.harness != HARNESS || s.binding.container != CONTAINER {
        bail!("not a Codex session");
    }
    check_version(&s.binding.version)?;
    let mut rows: Vec<Vec<u8>> = store
        .native_chain(s.head.as_ref())
        .with_context(|| format!("session {} cannot be projected", s.id))?
        .into_iter()
        .map(|n| n.raw)
        .collect();
    let meta = parse(rows.first().context("empty session")?)?;
    let parent_id = meta["payload"]["id"]
        .as_str()
        .context("missing thread id")?
        .to_owned();
    super::super::claude::check_session_id(&parent_id)?;
    let rel = String::from_utf8(
        store
            .get(s.native_header.as_ref().context("missing rollout path")?)?
            .raw,
    )?;
    let relpath = Path::new(&rel);
    if relpath
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("invalid rollout path");
    }
    let forked = s.binding.native_id.is_none();
    if swap && !forked {
        bail!("swap requires an unprojected fork");
    }
    let id = if forked && !swap {
        let mut id = crate::horse::uuid_v4()?;
        let ms = format!("{:012x}", crate::horse::now_ms());
        id.replace_range(0..8, &ms[..8]);
        id.replace_range(9..13, &ms[8..]);
        id.replace_range(14..15, "7");
        id
    } else {
        s.binding
            .native_id
            .clone()
            .unwrap_or_else(|| parent_id.clone())
    };
    let mut path = config.join("sessions").join(relpath);
    if forked && !swap {
        let payload = json::raw_value(&rows[0], "payload")?.context("missing payload")?;
        let changed = json::edit(
            payload,
            &[
                ("id", json::quote(&id)),
                ("session_id", json::quote(&id)),
                ("forked_from_id", json::quote(&parent_id)),
            ],
            &[],
        )?;
        let new_meta = json::edit(&rows[0], &[("payload", String::from_utf8(changed)?)], &[])?;
        rows.insert(0, new_meta);
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(&parent_id, &id);
        path.set_file_name(name);
    }
    let backup = publish(config, &path, &id, &rows, swap)?;
    if forked {
        let mut head = None;
        for (i, r) in rows.iter().enumerate() {
            head = Some(store.put_native(&ORIGIN, head.as_ref(), &key(&parse(r)?, i), r)?);
        }
        let rel = path
            .strip_prefix(config.join("sessions"))?
            .to_string_lossy();
        let header = store.put_blob("codex-path", rel.as_bytes())?;
        store.update_session(session, |s| {
            s.head = head;
            s.native_header = Some(header);
            s.binding.native_id = Some(id.clone());
        })?;
        if swap {
            store.write_ref(&format!("codex/{id}.json"), session.as_bytes())?;
        }
    }
    Ok(Projection {
        path,
        native_id: id,
        forked,
        backup,
    })
}

/// Write `rows` as the rollout `path` of thread `id` under the Codex home
/// `config`, while paddock holds the writer lock of the thread. The file
/// is a synced temporary file first. `replace`: it replaces the rollout
/// by a rename, and the old file stays as a hard link next to it, whose
/// path comes back. Else it must be a new file.
pub(super) fn publish(
    config: &Path,
    path: &Path,
    id: &str,
    rows: &[Vec<u8>],
    replace: bool,
) -> Result<Option<PathBuf>> {
    let lock_dir = config.join("thread-writer-locks");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&lock_dir)?;
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_dir.join(format!("{id}.lock")))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("Codex thread is locked by a writer");
    }
    let dir = path.parent().context("rollout path has no folder")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let temp = dir.join(format!(".paddock-{}", crate::horse::uuid_v4()?));
    let written = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        for r in rows {
            f.write_all(r)?;
            f.write_all(b"\n")?;
        }
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(e.context("cannot write the rollout"));
    }
    let backup = if replace {
        let backup = path.with_extension(format!("jsonl.paddock-{}", crate::horse::uuid_v4()?));
        if let Err(e) = std::fs::hard_link(path, &backup) {
            let _ = std::fs::remove_file(&temp);
            return Err(e).context("cannot preserve old rollout");
        }
        if let Err(e) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            let _ = std::fs::remove_file(&backup);
            return Err(e).context("cannot replace the rollout");
        }
        Some(backup)
    } else {
        let result = std::fs::hard_link(&temp, path);
        let _ = std::fs::remove_file(&temp);
        result.context("cannot publish rollout")?;
        None
    };
    sync_dir(dir)?;
    drop(lock);
    Ok(backup)
}
