//! The files of a sync: the scan, the writes and deletes in one step,
//! the conflict copies, and the recovery after a stop.

use super::*;

pub(super) fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub(super) fn is_tmp(name: &str) -> bool {
    name.contains(TMP)
}

pub(super) fn is_sync_tmp(name: &str) -> bool {
    name.find(TMP)
        .is_some_and(|i| name[i + TMP.len()..].contains(TMP_SYNC))
}

/// The name before the temporary suffix.
pub(super) fn tmp_owner(name: &str) -> &str {
    name.find(TMP).map(|i| &name[..i]).unwrap_or(name)
}

pub(super) fn tmp_name(name: &str) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("{name}{TMP}{}{TMP_SYNC}{n}", std::process::id())
}

/// One side: a tree of files, read without following anything.
#[derive(Default)]
pub(super) struct Side {
    pub(super) files: BTreeMap<String, String>,
    /// Paths that are there but cannot sync (a symlink, a special file, an
    /// unreadable file, a folder that cannot open). Nothing on the other
    /// side of these paths changes.
    pub(super) blocked: BTreeSet<String>,
    /// Temporary files of a sync that stopped: (path, hash). No hash: the
    /// file cannot be read (or is too large), so its content is unknown.
    pub(super) stale: Vec<(String, Option<String>)>,
    pub(super) skipped: Vec<String>,
}

impl Side {
    pub(super) fn blocks(&self, rel: &str) -> bool {
        self.blocked.iter().any(|b| {
            rel == b || rel.starts_with(&format!("{b}/")) || b.starts_with(&format!("{rel}/"))
        })
    }
}

pub(super) fn scan(dir: &Dir, prefix: &str, depth: usize, side: &mut Side) {
    let entries = match dir.stats() {
        Ok(e) => e,
        Err(e) => {
            let at = prefix.trim_end_matches('/').to_string();
            side.skipped.push(format!("{at}/ cannot be read: {e:#}"));
            side.blocked.insert(at);
            return;
        }
    };
    for (name, st) in entries {
        let Some(n) = name.to_str() else {
            side.skipped.push(format!(
                "{prefix}{} (the name is not UTF-8)",
                name.to_string_lossy()
            ));
            continue;
        };
        if prefix.is_empty()
            && (n == "inbox" || n == ".gitignore" || n == MARK || n == crate::copy::pointer::FILE)
        {
            continue;
        }
        let rel = format!("{prefix}{n}");
        if is_tmp(n) {
            if is_sync_tmp(n) && st.kind == Kind::File {
                let h = if st.size > MAX_FILE {
                    Err(anyhow::anyhow!("larger than {} MiB", MAX_FILE >> 20))
                } else {
                    dir.read(&name, MAX_FILE)
                };
                match h {
                    Ok(Some(b)) => side.stale.push((rel, Some(hash(&b)))),
                    Ok(None) => {}
                    Err(e) => {
                        side.skipped.push(format!(
                            "{rel}: a copy of an interrupted sync that cannot be read: {e:#}"
                        ));
                        side.stale.push((rel, None));
                    }
                }
            }
            continue;
        }
        if side.files.len() >= MAX_FILES {
            side.skipped
                .push(format!("{rel} (more than {MAX_FILES} files)"));
            side.blocked.insert(rel);
            continue;
        }
        match st.kind {
            Kind::File if st.size > MAX_FILE => {
                side.skipped
                    .push(format!("{rel} (larger than {} MiB)", MAX_FILE >> 20));
                side.blocked.insert(rel);
            }
            Kind::File => {
                #[cfg(test)]
                IN_SCAN.with(|f| {
                    if let Some(f) = f.borrow_mut().as_mut() {
                        f(dir.path())
                    }
                });
                match dir.read(&name, MAX_FILE) {
                    Ok(Some(b)) => {
                        side.files.insert(rel, hash(&b));
                    }
                    // Gone since the listing: absent.
                    Ok(None) => {}
                    Err(e) => {
                        side.skipped.push(format!("{rel} cannot be read: {e:#}"));
                        side.blocked.insert(rel);
                    }
                }
            }
            Kind::Dir if depth >= MAX_DEPTH => {
                side.skipped.push(format!("{rel}/ (too deep)"));
                side.blocked.insert(rel);
            }
            Kind::Dir => match dir.subdir(&name, false) {
                Ok(d) => scan(&d, &format!("{rel}/"), depth + 1, side),
                Err(e) => {
                    side.skipped.push(format!("{rel}/ cannot be opened: {e:#}"));
                    side.blocked.insert(rel);
                }
            },
            _ => {
                side.skipped
                    .push(format!("{rel} (a symlink or a special file, not followed)"));
                side.blocked.insert(rel);
            }
        }
    }
}

pub(in crate::loft) fn split(rel: &str) -> (&str, &str) {
    match rel.rsplit_once('/') {
        Some((p, n)) => (p, n),
        None => ("", rel),
    }
}

/// The folder `rel` below `root`, one step at a time, never through a
/// symlink.
pub(in crate::loft) fn dir_at(root: &Dir, rel: &str, create: bool) -> Result<Dir> {
    let mut d = root.subdir(OsStr::new("."), false)?;
    for c in rel.split('/').filter(|c| !c.is_empty()) {
        d = d.subdir(OsStr::new(c), create)?;
    }
    Ok(d)
}

pub(in crate::loft) fn read_hash(d: &Dir, name: &str) -> Result<Option<(Vec<u8>, String)>> {
    Ok(d.read(OsStr::new(name), MAX_FILE)?.map(|b| {
        let h = hash(&b);
        (b, h)
    }))
}

/// A free name for a conflict copy of `rel` in the folder `d`, also free
/// in `taken`.
pub(in crate::loft) fn conflict_name(
    d: &Dir,
    rel: &str,
    tag: &str,
    taken: &[&BTreeMap<String, String>],
) -> String {
    let base = format!("{rel}.conflict-{tag}");
    for i in 0..10_000 {
        let cand = if i == 0 {
            base.clone()
        } else {
            format!("{base}-{i}")
        };
        let free_here = d
            .kind(OsStr::new(split(&cand).1))
            .is_ok_and(|k| k.is_none());
        if free_here && !taken.iter().any(|m| m.contains_key(&cand)) {
            return cand;
        }
    }
    format!("{base}-{}", crate::state::now())
}

pub(super) enum Put {
    Done,
    /// Someone changed the target during the sync. Its content, when the
    /// sync took it out, is now the conflict copy (the path).
    Raced(Option<String>),
}

/// Write `bytes` at `rel` below `root`. `expected`: the hash of the file
/// that this replaces, or None for a new file.
pub(super) fn put(
    root: &Dir,
    rel: &str,
    bytes: &[u8],
    expected: Option<&str>,
    tag: &str,
) -> Result<Put> {
    let (parent, name) = split(rel);
    let d = dir_at(root, parent, true)?;
    let tmp = tmp_name(name);
    let mut f = d.create_new(OsStr::new(&tmp), 0o600)?;
    let w = f.write_all(bytes).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = w {
        let _ = d.remove(OsStr::new(&tmp));
        return Err(e.into());
    }
    step();
    let Some(expected) = expected else {
        return match d.rename_noreplace(OsStr::new(&tmp), OsStr::new(name)) {
            Ok(()) => Ok(Put::Done),
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {
                // A new file at the name since the scan: it stays, and the
                // next sync compares again. The content of `bytes` is
                // still on the other side.
                let _ = d.remove(OsStr::new(&tmp));
                Ok(Put::Raced(None))
            }
            Err(e) => {
                let _ = d.remove(OsStr::new(&tmp));
                Err(anyhow::Error::from(e).context(format!("cannot write {rel}")))
            }
        };
    };
    if d.kind(OsStr::new(name))? != Some(Kind::File) {
        let _ = d.remove(OsStr::new(&tmp));
        return Ok(Put::Raced(None));
    }
    d.exchange(OsStr::new(&tmp), OsStr::new(name))?;
    step();
    // `tmp` holds the version that was at the name. Bytes that a writer
    // adds through an open fd after this read are lost when it goes (see
    // the top of this file).
    let got = match read_hash(&d, &tmp) {
        Ok(g) => g,
        // Now larger than MAX_FILE, or unreadable: an unknown version. It
        // must not stay under the temporary name.
        Err(e) => return keep_unknown(&d, rel, &tmp, tag, e).map(|c| Put::Raced(Some(c))),
    };
    match got {
        Some((_, h)) if h == expected => {
            d.remove(OsStr::new(&tmp))?;
            Ok(Put::Done)
        }
        Some(_) => {
            let c = conflict_name(&d, rel, tag, &[]);
            d.rename_noreplace(OsStr::new(&tmp), OsStr::new(split(&c).1))?;
            Ok(Put::Raced(Some(c)))
        }
        None => Ok(Put::Raced(None)),
    }
}

/// The file `tmp` in `d` (a version of `rel` that the sync took out)
/// cannot be read: it becomes a conflict copy of `rel`. Returns its path.
pub(super) fn keep_unknown(
    d: &Dir,
    rel: &str,
    tmp: &str,
    tag: &str,
    why: anyhow::Error,
) -> Result<String> {
    let c = conflict_name(d, rel, tag, &[]);
    match d.rename_noreplace(OsStr::new(tmp), OsStr::new(split(&c).1)) {
        Ok(()) => Ok(c),
        Err(e) => Err(anyhow::Error::from(e).context(format!(
            "{rel}: the version that the sync took out cannot be read ({why:#}), and it cannot become a conflict copy; it is {}",
            d.path().join(tmp).display()
        ))),
    }
}

pub(in crate::loft) enum Del {
    Done,
    /// The file changed since the scan: it stays.
    Kept,
}

/// Delete `rel` below `root` when it still has the content `expected`.
pub(super) fn del(root: &Dir, rel: &str, expected: &str, tag: &str) -> Result<Del> {
    let (parent, name) = split(rel);
    let Ok(d) = dir_at(root, parent, false) else {
        return Ok(Del::Done);
    };
    if d.kind(OsStr::new(name))? != Some(Kind::File) {
        return Ok(if d.kind(OsStr::new(name))?.is_none() {
            Del::Done
        } else {
            Del::Kept
        });
    }
    let tmp = tmp_name(name);
    match d.rename_noreplace(OsStr::new(name), OsStr::new(&tmp)) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok(Del::Done),
        Err(e) => return Err(anyhow::Error::from(e).context(format!("cannot delete {rel}"))),
    }
    step();
    // As in `put`: a write through an open fd after this read is lost.
    let got = match read_hash(&d, &tmp) {
        Ok(g) => g,
        Err(e) => {
            // Unknown content: back to its name, else a conflict copy.
            if d.rename_noreplace(OsStr::new(&tmp), OsStr::new(name))
                .is_err()
            {
                keep_unknown(&d, rel, &tmp, tag, e)?;
            }
            return Ok(Del::Kept);
        }
    };
    match got {
        Some((_, h)) if h == expected => {
            d.remove(OsStr::new(&tmp))?;
        }
        Some(_) => {
            // Changed in the meantime: back to its name, or a conflict copy
            // when a new file took the name.
            if d.rename_noreplace(OsStr::new(&tmp), OsStr::new(name))
                .is_err()
            {
                let c = conflict_name(&d, rel, tag, &[]);
                d.rename_noreplace(OsStr::new(&tmp), OsStr::new(split(&c).1))?;
            }
            return Ok(Del::Kept);
        }
        None => {}
    }
    prune_up(root, parent);
    Ok(Del::Done)
}

/// The folder `parent` below `root`, and the folders above it, go when
/// they are empty.
pub(in crate::loft) fn prune_up(root: &Dir, parent: &str) {
    let parts: Vec<&str> = parent.split('/').filter(|c| !c.is_empty()).collect();
    for i in (0..parts.len()).rev() {
        let Ok(up) = dir_at(root, &parts[..i].join("/"), false) else {
            break;
        };
        if up.remove_dir(OsStr::new(parts[i])).is_err() {
            break;
        }
    }
}

/// The folder `.paddock` of the work tree. It must be a real folder.
pub(super) fn open_local(worktree: &Path) -> Result<Dir> {
    Root::open(worktree)?
        .dir(Path::new(NOTES), false)
        .with_context(|| format!("{}/{NOTES} is not a folder", worktree.display()))
}

pub(in crate::loft) fn open_store(loft: &Loft) -> Result<Dir> {
    crate::privfs::dir(&loft.tree())?;
    crate::privfs::dir(&loft.notes())?;
    Root::open(&loft.tree())?.dir(Path::new(NOTES), true)
}

/// The tag of conflict copies: who synced (a run id or the host name),
/// and the time in UTC.
pub fn tag(who: &str) -> String {
    let who: String = who
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let who = if who.is_empty() { "host".into() } else { who };
    let t = crate::waiting::format_rfc3339(crate::state::now()).replace(['-', ':'], "");
    format!("{who}-{t}")
}

/// Who syncs on the host, outside a run.
pub fn host_name() -> String {
    let h = crate::report::hostname();
    if h.is_empty() {
        "host".into()
    } else {
        h
    }
}

/// Stale temporary files of a sync that stopped. A known content (the
/// file at its name on either side, or its base) goes. Anything else
/// becomes a conflict copy, so no version is lost.
pub(super) fn recover(
    root: &Dir,
    me: &mut Side,
    other: &Side,
    base: &Base,
    tag: &str,
    r: &mut Report,
) {
    for (rel, h) in std::mem::take(&mut me.stale) {
        let (parent, name) = split(&rel);
        let owner = format!(
            "{}{}",
            if parent.is_empty() {
                String::new()
            } else {
                format!("{parent}/")
            },
            tmp_owner(name)
        );
        let Ok(d) = dir_at(root, parent, false) else {
            continue;
        };
        let known = h.as_ref().is_some_and(|h| {
            [
                me.files.get(&owner),
                other.files.get(&owner),
                base.files.get(&owner),
            ]
            .iter()
            .any(|k| k.is_some_and(|k| k == h))
        });
        if known {
            let _ = d.remove(OsStr::new(name));
            continue;
        }
        let c = conflict_name(&d, &owner, tag, &[&me.files, &other.files]);
        match d.rename_noreplace(OsStr::new(name), OsStr::new(split(&c).1)) {
            Ok(()) => {
                r.conflicts.push((owner, c.clone()));
                match h {
                    Some(h) => {
                        me.files.insert(c, h);
                    }
                    // It cannot sync; the next scan reports it, and the
                    // status lists it with the conflicts.
                    None => {
                        me.blocked.insert(c);
                    }
                }
            }
            Err(e) => r
                .skipped
                .push(format!("{rel}: cannot keep an interrupted copy: {e}")),
        }
    }
}

/// Both sides changed `p`. The work tree version goes next to it as a
/// conflict copy, on both sides; the loft version stays at the name. A
/// copy that an interrupted sync made already is used again.
#[allow(clippy::too_many_arguments)]
pub(super) fn conflict(
    local_dir: &Dir,
    store_dir: &Dir,
    local: &Side,
    store: &Side,
    p: &str,
    l: &str,
    s: &str,
    tag: &str,
    next: &mut BTreeMap<String, String>,
    r: &mut Report,
) -> Result<()> {
    let pre = format!("{p}.conflict-");
    let again = store
        .files
        .iter()
        .chain(local.files.iter())
        .find(|(k, h)| k.starts_with(&pre) && *h == l)
        .map(|(k, _)| k.clone());
    let c = match again {
        Some(c) => c,
        None => {
            let d = dir_at(store_dir, split(p).0, true)?;
            conflict_name(&d, p, tag, &[&local.files, &store.files])
        }
    };
    let (parent, name) = split(p);
    let src = dir_at(local_dir, parent, false)?;
    let Some((bytes, h)) = read_hash(&src, name)? else {
        return Ok(());
    };
    if h != l {
        // Changed again since the scan: the next sync decides.
        return Ok(());
    }
    // 1. The work tree version, as the conflict copy in the loft, then
    // 2. the same copy in the work tree. A race leaves it to the next sync.
    for side in [store_dir, local_dir] {
        if file_hash(side, &c)?.as_deref() != Some(l)
            && !matches!(put(side, &c, &bytes, None, tag)?, Put::Done)
        {
            return Ok(());
        }
    }
    next.insert(c.clone(), l.to_string());
    // 3. The loft version at the name in the work tree.
    if copy(store_dir, local_dir, p, s, Some(l), tag, r)? {
        next.insert(p.to_string(), s.to_string());
    }
    r.conflicts.push((p.to_string(), c));
    Ok(())
}

pub(super) fn file_hash(root: &Dir, rel: &str) -> Result<Option<String>> {
    let (parent, name) = split(rel);
    let Ok(d) = dir_at(root, parent, false) else {
        return Ok(None);
    };
    Ok(read_hash(&d, name)?.map(|(_, h)| h))
}

/// The open conflict copies in the loft's notes, also those that cannot
/// sync (too large, unreadable), and copies of an interrupted sync that
/// cannot be read.
pub fn conflicts(loft: &Loft) -> Vec<String> {
    let Ok(d) = Root::open(&loft.tree()).and_then(|r| r.dir(Path::new(NOTES), false)) else {
        return Vec::new();
    };
    open_copies(&d)
}

/// The same for the `.paddock` folder of a work tree in sync mode: what
/// only it has.
pub fn local_conflicts(worktree: &Path) -> Vec<String> {
    open_local(worktree)
        .map(|d| open_copies(&d))
        .unwrap_or_default()
}

pub(super) fn open_copies(d: &Dir) -> Vec<String> {
    let mut side = Side::default();
    scan(d, "", 0, &mut side);
    let mut out: Vec<String> = side
        .files
        .into_keys()
        .chain(side.blocked)
        .filter(|k| split(k).1.contains(".conflict-"))
        .collect();
    out.sort();
    out.dedup();
    out.extend(
        side.stale
            .into_iter()
            .filter(|(_, h)| h.is_none())
            .map(|(k, _)| format!("{k} (a copy of an interrupted sync that cannot be read)")),
    );
    out
}
