//! The saved base of each work tree, and the lock of a loft.

use super::*;

/// The saved state of the last sync of one work tree.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(in crate::loft) struct Base {
    pub(super) worktree: PathBuf,
    pub(in crate::loft) files: BTreeMap<String, String>,
    /// The id in the marker of the folder that this base belongs to.
    #[serde(default)]
    pub(super) id: Option<String>,
    /// The device and inode of that folder. A copy of the folder (a
    /// restored backup) has the same marker but another inode, so it
    /// starts from an empty base too.
    #[serde(default)]
    pub(super) folder: Option<(u64, u64)>,
    /// Deletes in the work tree that a scan found and that wait for a
    /// second scan of the same folder.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(in crate::loft) pending: BTreeMap<String, Gone>,
    /// A sync that found a delete changed this base but wrote no new
    /// marker. So a backup with the marker of `id` is older than this
    /// base, and the next sync that finds no delete writes a new marker.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) stale: bool,
}

/// A file that a scan of the work tree missed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::loft) struct Gone {
    /// The hash of the base, which the store still has.
    pub(super) hash: String,
    /// When a scan missed it first, in milliseconds since the epoch.
    pub(super) since: u64,
    /// The same time on the clock of `mono_ms`. The wall clock can jump
    /// forward; this clock cannot. None in a base of an older version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mono: Option<u64>,
    /// The wait was over, but the move to the loft trash failed. The next
    /// sync tries the move again at once, and this delete does not stop
    /// its pulls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) failed: bool,
}

impl Gone {
    /// The wait of the delete is over at `now` (wall) and `mono`: both
    /// clocks agree that DELETE_WAIT passed. A monotonic time below the
    /// saved one means a reboot since, which takes longer than the wait.
    pub(super) fn waited(&self, now: u64, mono: u64) -> bool {
        let w = DELETE_WAIT.as_millis() as u64;
        now.saturating_sub(self.since) >= w && self.mono.is_none_or(|m| mono < m || mono - m >= w)
    }
}

pub(in crate::loft) fn now_ms() -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    #[cfg(test)]
    let t = t + LATER.with(|l| l.get()) + JUMP.with(|l| l.get());
    t
}

/// A clock in milliseconds that only goes forward, also across
/// processes of one boot: CLOCK_BOOTTIME on Linux (it counts a suspend
/// too), CLOCK_MONOTONIC elsewhere.
pub(super) fn mono_ms() -> u64 {
    #[cfg(target_os = "linux")]
    let id = libc::CLOCK_BOOTTIME;
    #[cfg(not(target_os = "linux"))]
    let id = libc::CLOCK_MONOTONIC;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime fills a timespec on the stack.
    let t = if unsafe { libc::clock_gettime(id, &mut ts) } == 0 {
        ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
    } else {
        0
    };
    #[cfg(test)]
    let t = t + LATER.with(|l| l.get());
    t
}

/// The file of the base of `worktree`.
pub fn base_file(loft: &Loft, worktree: &Path) -> PathBuf {
    let w = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    loft.root
        .join("sync")
        .join(format!("{}.json", crate::kv::shard::project_hash(&w)))
}

pub(super) fn load_base(loft: &Loft, worktree: &Path) -> Result<Base> {
    let f = base_file(loft, worktree);
    match std::fs::read(&f) {
        Ok(b) => serde_json::from_slice(&b)
            .with_context(|| format!("{} is damaged; move it aside to start again", f.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Base {
            worktree: worktree.to_path_buf(),
            ..Base::default()
        }),
        Err(e) => Err(e.into()),
    }
}

/// The id in the marker of the folder `local`, if it has a good one.
pub(super) fn mark_of(local: &Dir) -> Option<String> {
    if local.kind(OsStr::new(MARK)).ok()?? != Kind::File {
        return None;
    }
    let b = local.read(OsStr::new(MARK), 256).ok()??;
    let id = String::from_utf8(b).ok()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// The folder `local` is the one of `base`: the same marker id, and the
/// same inode. The device is not compared: on btrfs and FUSE, st_dev of
/// the same folder can change after a reboot.
pub(super) fn is_folder_of(base: &Base, local: &Dir) -> bool {
    base.id.is_some()
        && base.id == mark_of(local)
        && base
            .folder
            .is_some_and(|(_, ino)| local.id().is_ok_and(|(_, i)| i == ino))
}

/// The base of the folder `local`: the saved one when it belongs to this
/// folder (`is_folder_of`), else an empty one (the bool is then true). An
/// empty base never deletes anything, so a stale one cannot.
pub(super) fn base_of(loft: &Loft, worktree: &Path, local: &Dir) -> Result<(Base, bool)> {
    let base = load_base(loft, worktree)?;
    match is_folder_of(&base, local) {
        true => Ok((base, false)),
        false => Ok((
            Base {
                worktree: worktree.to_path_buf(),
                ..Base::default()
            },
            true,
        )),
    }
}

/// A new marker in the folder `local`; its id goes into the base.
pub(super) fn new_mark(local: &Dir) -> Result<String> {
    let id = crate::horse::new_id("loft")?;
    local
        .write(OsStr::new(MARK), format!("{id}\n").as_bytes(), 0o600)
        .context("cannot write the marker of the sync folder")?;
    Ok(id)
}

pub(super) fn save_base(loft: &Loft, worktree: &Path, base: &Base) -> Result<()> {
    let f = base_file(loft, worktree);
    if let Some(p) = f.parent() {
        crate::privfs::dir(p)?;
    }
    crate::privfs::write_atomic(&f, serde_json::to_vec_pretty(base)?)?;
    Ok(())
}

/// No base of `loft` keeps `rel` any more, so each work tree treats the
/// file in the loft as new: it gets the file, or keeps both versions.
/// One exception: a base that has `rel` with the restored content `hash`,
/// no delete of it that waits, and a work tree that still has a file at
/// `rel`. That work tree did not sync since the delete, so its base is
/// right: a newer edit there goes to the loft, not to a conflict copy.
/// For a restore from the trash; the caller holds the lock.
pub(in crate::loft) fn forget(loft: &Loft, rel: &str, hash: &str) -> Result<()> {
    let Ok(rd) = std::fs::read_dir(loft.root.join("sync")) else {
        return Ok(());
    };
    for e in rd.flatten() {
        let f = e.path();
        if f.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Ok(mut b) = std::fs::read(&f)
            .map_err(anyhow::Error::from)
            .and_then(|b| Ok(serde_json::from_slice::<Base>(&b)?))
        else {
            continue;
        };
        let keep = b.files.get(rel).is_some_and(|h| h == hash)
            && !b.pending.contains_key(rel)
            && has_file(&b.worktree, rel);
        if keep {
            continue;
        }
        if b.files.remove(rel).is_some() | b.pending.remove(rel).is_some() {
            crate::privfs::write_atomic(&f, serde_json::to_vec_pretty(&b)?)?;
        }
    }
    Ok(())
}

/// `.paddock` of `worktree` has a file at `rel`.
pub(super) fn has_file(worktree: &Path, rel: &str) -> bool {
    let (parent, name) = split(rel);
    open_local(worktree)
        .and_then(|d| dir_at(&d, parent, false))
        .is_ok_and(|d| matches!(d.kind(OsStr::new(name)), Ok(Some(Kind::File))))
}

/// The work tree is in sync mode: `.paddock` is a real folder and the
/// work tree has a base.
pub fn synced(loft: &Loft, worktree: &Path) -> bool {
    std::fs::symlink_metadata(worktree.join(NOTES)).is_ok_and(|m| m.is_dir())
        && base_file(loft, worktree).is_file()
}

/// The exclusive lock of all syncs with one loft. It goes with the file.
pub(in crate::loft) struct Lock(#[allow(dead_code)] std::fs::File);

pub(in crate::loft) fn lock(loft: &Loft) -> Result<Lock> {
    lock_within(loft, LOCK_WAIT)
}

/// The lock, or an error after `wait`. A sync that hangs (a slow or stuck
/// file system) must not stop every run start of the repository: the
/// caller warns and goes on without a sync.
pub(in crate::loft) fn lock_within(loft: &Loft, wait: Duration) -> Result<Lock> {
    crate::privfs::dir(&loft.root)?;
    let path = loft.root.join("sync.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    let start = std::time::Instant::now();
    let mut warned = false;
    loop {
        // SAFETY: flock on an open file; the lock goes with the file.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Lock(f));
        }
        let e = std::io::Error::last_os_error();
        if !matches!(
            e.raw_os_error(),
            Some(libc::EWOULDBLOCK) | Some(libc::EINTR)
        ) {
            return Err(anyhow::Error::from(e).context(format!("cannot lock {}", path.display())));
        }
        let waited = start.elapsed();
        if waited >= wait {
            bail!(
                "another sync of this loft holds {} for {}s; no sync now",
                path.display(),
                waited.as_secs()
            );
        }
        if !warned && waited >= LOCK_WARN.min(wait / 2) {
            log::warn!(
                "waiting for {}: another sync of this loft holds it",
                path.display()
            );
            warned = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
