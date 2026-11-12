//! The `sync` mode of the loft: `.paddock` of each work tree is a real
//! folder of the project, and paddock syncs it with the loft's notes
//! (`tree/.paddock/`) in both directions. Tools see the folder as part of
//! the project, so a write there needs no approval for a place outside it.
//!
//! The sync is a three-way merge per file. The base is the state of the
//! last sync of this work tree: `<loft>/sync/<work tree key>.json` maps
//! each path to the hash of its content. With L the work tree, S the loft
//! store and B the base:
//!
//! - L = S: nothing to do.
//! - L = B: only the store changed. Its version (or its delete) goes to L.
//! - S = B: only the work tree changed. Its version (or delete) goes to S.
//! - both changed: never a loss. Two versions: the loft version stays at
//!   the name, the work tree version goes next to it as
//!   `NAME.conflict-<host or run>-<time>`, on both sides. A delete against
//!   a change: the changed file stays, on both sides, and the report says
//!   so.
//!
//! Every write is a new file, synced to disk, then renamed into place in
//! one step. A replace swaps the new file with the old one
//! (RENAME_EXCHANGE) and reads what it took out: when that is not the
//! version that the sync planned to replace (an agent wrote in the
//! meantime), it stays as a conflict copy. A delete renames the file
//! aside first and checks it the same way. A crash leaves at most a
//! temporary file with `.paddock-tmp-<pid>-sync-` in its name. The next
//! sync removes it when its content is known, and keeps it as a conflict
//! copy when it is not. The base is written last, so a sync that stops
//! halfway repeats cleanly.
//!
//! The base belongs to one folder, not to a path: the folder has a marker
//! file (`.loft-sync`) with a random id, and the base has the same id and
//! the device and inode of the folder. A folder without it, with another
//! id or with another inode (made again after `git clean -fdx` or `rm -rf
//! .paddock`, a worktree that comes back at the same path, a copy of
//! another folder, a restored backup), starts from an empty base. After
//! its scan, a sync checks the folder again; when it changed during the
//! scan, the sync starts again from an empty base. With an
//! empty base a sync never deletes: it pulls what the loft has, pushes
//! what only the folder has, and keeps both versions of a file that
//! differs.
//!
//! A sync that finds a delete in the work tree writes no new marker (see
//! `found` in `sync_from`), and the next sync without a delete writes
//! it. This leaves a window for a backup restored in place. The backup
//! has marker X. A sync pushes v2 of a note and finds a delete, so the
//! marker stays X. If the user restores the backup into the same folder
//! before the next sync without a delete, the base is trusted, and v1
//! of the backup goes to the loft over v2. A user who deletes a file
//! before each sync keeps this window open.
//!
//! A second window stays open: a writer that keeps a file open writes to the
//! inode, not to the name. A sync reads the old version back after it
//! replaced or moved it, and unlinks it when the content is the expected
//! one. Bytes that such a writer adds through its open fd after that read
//! go to the unlinked inode and are lost. Tools that save a note write it
//! again through its name (or a new file and a rename), and the next sync
//! sees that. A writer that keeps appending to one open file (a log) can
//! lose the bytes of that short window.
//!
//! Nothing in the work tree is followed: symlinks, sockets and devices are
//! skipped and reported, and the same path on the other side stays. The
//! host request inbox (`inbox/`) and paddock's `.gitignore` at the top
//! stay with each work tree.

mod base;
mod files;
mod mode;
pub use base::*;
pub use files::*;
pub use mode::*;

use super::{Loft, NOTES};
use crate::env::HostEnv;
use crate::safefs::{Dir, Kind, Root};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// `[loft] mode`: how `.paddock` of the project reaches the loft.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `.paddock` is a symlink to the loft's notes.
    #[default]
    Link,
    /// `.paddock` is a real folder that paddock syncs with the loft.
    Sync,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Link => "link",
            Mode::Sync => "sync",
        }
    }
}

/// The largest file that the sync copies. A larger one stays where it is,
/// and the report names it.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_DEPTH: usize = 64;
/// The marker of the sync's own temporary files.
const TMP: &str = ".paddock-tmp-";
const TMP_SYNC: &str = "-sync-";
/// The marker of the folder that the base belongs to, at the top of
/// `.paddock` in the work tree. It never syncs.
pub const MARK: &str = ".loft-sync";
/// How long a sync waits for the lock of the loft, and when it says so.
const LOCK_WAIT: Duration = Duration::from_secs(60);
const LOCK_WARN: Duration = Duration::from_secs(5);
/// How often the watcher looks for changes.
pub const WATCH_EVERY: Duration = Duration::from_secs(2);

/// A delete in the work tree waits this long, at least, for a second scan
/// that still misses the file. An `rm -rf` or a `git clean` in progress
/// removes the marker last, so one scan cannot tell it from a delete.
pub const DELETE_WAIT: Duration = Duration::from_secs(5);

/// When at least this many files of the folder go between two syncs, and
/// they are most of the folder, no delete reaches the loft.
const MASS_MIN: usize = 4;

/// The mode of `project`: `[loft] mode` of the project `paddock.toml`,
/// else of the user configuration.
pub fn mode(env: &HostEnv, project: &Path) -> Mode {
    project_mode(project).unwrap_or_else(|| super::Cfg::load(env).mode)
}

/// `[loft] mode` of the project `paddock.toml`, read without following a
/// symlink. A bad value is a warning and no setting.
pub fn project_mode(project: &Path) -> Option<Mode> {
    let root = Root::open(project).ok()?;
    let bytes = root
        .read(Path::new(crate::manifest::PROJECT_FILE), 1024 * 1024)
        .ok()??;
    let v: toml::Value = toml::from_str(std::str::from_utf8(&bytes).ok()?).ok()?;
    let m = v.get("loft")?.get("mode")?;
    match m.clone().try_into::<Mode>() {
        Ok(m) => Some(m),
        Err(_) => {
            log::warn!(
                "[loft] mode in {}/paddock.toml must be \"link\" or \"sync\"",
                project.display()
            );
            None
        }
    }
}

/// The work tree of `dir`: the top of its git work tree (a linked
/// worktree counts as itself), else `dir`.
pub fn worktree_of(dir: &Path) -> PathBuf {
    crate::hostgit::Layout::discover(dir)
        .ok()
        .flatten()
        .map(|l| l.work_tree)
        .unwrap_or_else(|| dir.to_path_buf())
}

/// What one sync did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// From the loft to the work tree.
    pub pulled: Vec<String>,
    /// From the work tree to the loft.
    pub pushed: Vec<String>,
    /// Deletes that reached the other side.
    pub deleted: Vec<String>,
    /// (path, conflict copy): both sides changed the file.
    pub conflicts: Vec<(String, String)>,
    /// Deletes that did not sync, because the other side changed the file.
    pub kept: Vec<String>,
    /// Paths that the sync left alone, with the reason.
    pub skipped: Vec<String>,
    /// Deletes in the work tree that wait for a later sync.
    pub waiting: Vec<String>,
    /// Deletes that reached the loft: the loft versions are in its trash.
    pub trashed: Vec<String>,
    /// Writes to the work tree that wait for the next sync, because this
    /// one found deletes in it.
    pub deferred: Vec<String>,
}

impl Report {
    pub fn changed(&self) -> bool {
        !(self.pulled.is_empty()
            && self.pushed.is_empty()
            && self.deleted.is_empty()
            && self.conflicts.is_empty()
            && self.kept.is_empty())
    }

    /// The lines for the user; empty when nothing happened.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let n = |v: &[String]| super::push::names(&v.iter().cloned().collect());
        if !self.pulled.is_empty() {
            out.push(format!("from the loft: {}", n(&self.pulled)));
        }
        if !self.pushed.is_empty() {
            out.push(format!("to the loft: {}", n(&self.pushed)));
        }
        if !self.deleted.is_empty() {
            out.push(format!("deleted: {}", n(&self.deleted)));
        }
        if !self.trashed.is_empty() {
            out.push(format!(
                "in the loft trash: {}. `paddock loft restore PATH` brings one back",
                n(&self.trashed)
            ));
        }
        for (p, c) in &self.conflicts {
            out.push(format!(
                "conflict: both sides changed {p}. The loft version stays, the other one is {c}"
            ));
        }
        for p in &self.kept {
            out.push(format!(
                "kept: {p} was deleted on one side and changed on the other, so it stays"
            ));
        }
        for s in &self.skipped {
            out.push(format!("skipped: {s}"));
        }
        if !self.deferred.is_empty() {
            out.push(format!(
                "next sync: {} from the loft, because this sync found deletes in the work tree",
                n(&self.deferred)
            ));
        }
        if !self.waiting.is_empty() {
            out.push(format!(
                "waits: {} deleted in the work tree; the loft version goes to the loft trash at a sync {}s or more from now",
                n(&self.waiting),
                DELETE_WAIT.as_secs()
            ));
        }
        out
    }
}

// For the tests: runs in a sync between base_of and the scan of the
// folder (an `rm -rf .paddock` or a `git clean` in that window).
#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_SCAN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

// For the tests: runs in the scan before each file is read, with the
// path of its folder (an `rm -rf` that goes on during the scan).
#[cfg(test)]
type ScanHook = Box<dyn FnMut(&Path)>;
#[cfg(test)]
thread_local! {
    pub(super) static IN_SCAN: std::cell::RefCell<Option<ScanHook>> = const { std::cell::RefCell::new(None) };
}

// For the tests: milliseconds added to the clock of the pending deletes.
#[cfg(test)]
thread_local! {
    pub(super) static LATER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// For the tests: milliseconds added to the wall clock only (a jump of
// the system clock).
#[cfg(test)]
thread_local! {
    pub(super) static JUMP: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

// Crash injection for the tests: the sync stops (a panic) at step N.
#[cfg(test)]
thread_local! {
    pub(super) static CRASH_AT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

// For the tests: runs in to_link between the scan of the folder aside
// and the removal of its files (a write of an agent in that window).
#[cfg(test)]
thread_local! {
    pub(super) static AFTER_SCAN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn step() {
    #[cfg(test)]
    CRASH_AT.with(|c| match c.get() {
        Some(0) => {
            c.set(None);
            panic!("injected crash");
        }
        Some(n) => c.set(Some(n - 1)),
        None => {}
    });
}

/// One planned step for a path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Act {
    /// Both sides have the same content, or both have none.
    Same(Option<String>),
    /// The store version goes to the work tree: (expected local, store).
    Pull(Option<String>, String),
    Push(Option<String>, String),
    DeleteLocal(String),
    DeleteStore(String),
    /// A delete against a change: the change goes to the side that
    /// deleted.
    RestoreLocal(String),
    RestoreStore(String),
    /// Both changed: (local, store).
    Conflict(String, String),
    /// One side cannot sync this path.
    Blocked,
}

fn plan(local: &Side, store: &Side, base: &Base) -> BTreeMap<String, Act> {
    let all: BTreeSet<&String> = local
        .files
        .keys()
        .chain(store.files.keys())
        .chain(base.files.keys())
        .collect();
    let mut out = BTreeMap::new();
    for p in all {
        let (l, s, b) = (local.files.get(p), store.files.get(p), base.files.get(p));
        let act = if local.blocks(p) || store.blocks(p) {
            Act::Blocked
        } else if l == s {
            Act::Same(l.cloned())
        } else if l == b {
            match s {
                Some(s) => Act::Pull(l.cloned(), s.clone()),
                None => Act::DeleteLocal(l.cloned().unwrap_or_default()),
            }
        } else if s == b {
            match l {
                Some(l) => Act::Push(s.cloned(), l.clone()),
                None => Act::DeleteStore(s.cloned().unwrap_or_default()),
            }
        } else {
            match (l, s) {
                (None, Some(s)) => Act::RestoreLocal(s.clone()),
                (Some(l), None) => Act::RestoreStore(l.clone()),
                (Some(l), Some(s)) => Act::Conflict(l.clone(), s.clone()),
                (None, None) => Act::Same(None),
            }
        };
        out.insert(p.clone(), act);
    }
    out
}

/// Copy `rel` from `from` to `to`. The source must still have `want`.
/// Returns false when it changed since the scan (the next sync sees it).
fn copy(
    from: &Dir,
    to: &Dir,
    rel: &str,
    want: &str,
    expected: Option<&str>,
    tag: &str,
    r: &mut Report,
) -> Result<bool> {
    let (parent, name) = split(rel);
    let Some((bytes, h)) = dir_at(from, parent, false)
        .ok()
        .map(|d| read_hash(&d, name))
        .transpose()?
        .flatten()
    else {
        return Ok(false);
    };
    if h != want {
        return Ok(false);
    }
    Ok(match put(to, rel, &bytes, expected, tag)? {
        Put::Done => true,
        Put::Raced(Some(c)) => {
            r.conflicts.push((rel.to_string(), c));
            false
        }
        Put::Raced(None) => false,
    })
}

/// Sync the `.paddock` folder of `worktree` with the loft's notes, under
/// the lock of the loft. `who` names the run or the host in conflict
/// copies.
pub fn sync(loft: &Loft, worktree: &Path, who: &str) -> Result<Report> {
    let _lock = lock(loft)?;
    sync_locked(loft, worktree, who)
}

pub(super) fn sync_locked(loft: &Loft, worktree: &Path, who: &str) -> Result<Report> {
    sync_from(loft, worktree, who, false)
}

/// The paths of `v`, for a line of the report.
fn names_of<'a>(v: impl IntoIterator<Item = &'a String>) -> String {
    super::push::names(&v.into_iter().cloned().collect())
}

/// The sync. With `retry`, the folder changed during a first try, and it
/// can be on its way out (an `rm -rf` that is not done yet). Then the
/// sync starts from an empty base, and it only pushes: it writes nothing
/// to the folder (no pull, no marker) and saves no base. So the next sync
/// starts from an empty base too.
fn sync_from(loft: &Loft, worktree: &Path, who: &str, retry: bool) -> Result<Report> {
    let local_dir = open_local(worktree)?;
    let store_dir = open_store(loft)?;
    let tag = tag(who);
    let mut r = Report::default();
    let saved = load_base(loft, worktree)?;
    let fresh = retry || !is_folder_of(&saved, &local_dir);
    let mut base = if fresh {
        if !retry && !saved.pending.is_empty() {
            r.skipped.push(dropped(&saved, worktree));
        }
        Base {
            worktree: worktree.to_path_buf(),
            ..Base::default()
        }
    } else {
        saved
    };
    if fresh && !retry {
        // A folder that this base does not know: it starts empty, and the
        // id of the new marker and the folder go into the base when it is
        // saved. A stop before that leaves another id, so the next sync
        // starts empty too.
        base.id = Some(new_mark(&local_dir)?);
    }
    // Also for a known folder: its device can change (see is_folder_of).
    base.folder = Some(local_dir.id()?);
    #[cfg(test)]
    BEFORE_SCAN.with(|f| {
        if let Some(f) = f.borrow_mut().take() {
            f()
        }
    });
    let (mut local, mut store) = (Side::default(), Side::default());
    scan(&local_dir, "", 0, &mut local);
    scan(&store_dir, "", 0, &mut store);
    // The folder can go during the scan (`rm -rf .paddock`, `git clean
    // -fdx`): the scan then misses files. So the path must still lead to
    // the same folder, with the same marker. If not, nothing of this scan
    // is used, and the sync tries again, only to push. An empty base needs
    // no check. The check misses an rm that has not reached the marker
    // yet; for that, a delete in the work tree waits for a second scan.
    if !fresh && !open_local(worktree).is_ok_and(|d| is_folder_of(&base, &d)) {
        log::warn!(
            "{}/{NOTES} changed during the sync; it syncs again from an empty base, only to the loft",
            worktree.display()
        );
        drop(local_dir);
        let mut again = sync_from(loft, worktree, who, true)?;
        let mut first = vec![format!(
            "{}/{NOTES} changed during the sync: this sync wrote nothing to it, and the next sync starts from an empty base",
            worktree.display()
        )];
        if !base.pending.is_empty() {
            first.push(dropped(&base, worktree));
        }
        again.skipped.splice(0..0, first);
        return Ok(again);
    }
    if !retry {
        recover(&local_dir, &mut local, &store, &base, &tag, &mut r);
    }
    recover(&store_dir, &mut store, &local, &base, &tag, &mut r);
    r.skipped.extend(
        local
            .skipped
            .iter()
            .map(|s| format!("{s} in the work tree")),
    );
    r.skipped
        .extend(store.skipped.iter().map(|s| format!("{s} in the loft")));
    let mut plan = plan(&local, &store, &base);
    if retry {
        plan.retain(|_, a| matches!(a, Act::Push(..)));
    }
    let mut next = base.files.clone();
    // A delete in the work tree waits: it reaches the loft only when a
    // scan at least DELETE_WAIT after the first one still misses the
    // file, in the same folder with the same marker (else this base is
    // not used, and its pending deletes go with it). Then the loft
    // version goes to the trash of the loft, never away (trash.rs).
    let gone: Vec<(String, String)> = plan
        .iter()
        .filter_map(|(p, a)| match a {
            Act::DeleteStore(h) => Some((p.clone(), h.clone())),
            _ => None,
        })
        .collect();
    // A file with the same content at a new path is a move (`mv` of a
    // folder), not part of an rm.
    let new_here: BTreeSet<&String> = plan
        .values()
        .filter_map(|a| match a {
            Act::Push(None, l) => Some(l),
            _ => None,
        })
        .collect();
    // A delete whose move to the trash failed waited already: it is not
    // an rm on its way.
    let failed = |p: &String, h: &String| {
        base.pending
            .get(p)
            .is_some_and(|g| g.failed && g.hash == *h)
    };
    let lost: Vec<&String> = gone
        .iter()
        .filter(|(p, h)| !new_here.contains(h) && !failed(p, h))
        .map(|(p, _)| p)
        .collect();
    let mut pending = BTreeMap::new();
    // The deletes whose wait is over.
    let mut ripe = BTreeMap::new();
    // A sync that finds a delete writes nothing new into the folder (no
    // pull, no marker): an rm can be on its way, and a new file after
    // its listing keeps the folder alive. The next sync does it.
    let found = gone.iter().any(|(p, h)| !failed(p, h));
    if found {
        plan.retain(|p, a| {
            let write = matches!(a, Act::Pull(..) | Act::RestoreLocal(_) | Act::Conflict(..));
            if write {
                r.deferred.push(p.clone());
            }
            !write
        });
    }
    if lost.len() >= MASS_MIN && lost.len() * 2 > base.files.len() {
        // Most of the folder at once: an rm in progress, more likely than
        // a cleanup. No delete; the base forgets these files, so the next
        // sync brings them back from the loft.
        r.skipped.push(format!(
            "{} of the {} files of {}/{NOTES} are gone at once ({}), so no delete reaches the loft, and the next sync brings them back. To delete many notes, delete them in the loft, or a few at a time",
            lost.len(),
            base.files.len(),
            worktree.display(),
            names_of(lost.iter().copied())
        ));
        for p in lost.iter().copied() {
            plan.remove(p);
            next.remove(p);
        }
    }
    let (now, mono) = (now_ms(), mono_ms());
    for (p, h) in gone {
        if !plan.contains_key(&p) {
            continue;
        }
        let g = base
            .pending
            .get(&p)
            .filter(|g| g.hash == h)
            .cloned()
            .unwrap_or(Gone {
                hash: h,
                since: now,
                mono: Some(mono),
                failed: false,
            });
        if !g.waited(now, mono) {
            plan.remove(&p);
            r.waiting.push(p.clone());
            pending.insert(p, g);
        } else {
            ripe.insert(p, g);
        }
    }
    let mut bin = super::trash::Bin::new(loft);
    for (p, act) in plan {
        let on_local = matches!(act, Act::DeleteLocal(_));
        let to_trash = matches!(act, Act::DeleteStore(_));
        let res: Result<()> = (|| {
            match act {
                Act::Blocked => {}
                Act::Same(h) => {
                    match h {
                        Some(h) => next.insert(p.clone(), h),
                        None => next.remove(&p),
                    };
                }
                Act::Pull(exp, s) => {
                    if copy(&store_dir, &local_dir, &p, &s, exp.as_deref(), &tag, &mut r)? {
                        next.insert(p.clone(), s);
                        r.pulled.push(p.clone());
                    }
                }
                Act::Push(exp, l) => {
                    if copy(&local_dir, &store_dir, &p, &l, exp.as_deref(), &tag, &mut r)? {
                        next.insert(p.clone(), l);
                        r.pushed.push(p.clone());
                    }
                }
                Act::DeleteLocal(exp) | Act::DeleteStore(exp) => {
                    let side = if on_local {
                        "the work tree"
                    } else {
                        "the loft"
                    };
                    let done = if on_local {
                        del(&local_dir, &p, &exp, &tag)?
                    } else {
                        bin.put(&store_dir, &p, &exp, &tag)?
                    };
                    match done {
                        Del::Done => {
                            next.remove(&p);
                            r.deleted.push(p.clone());
                            if !on_local {
                                r.trashed.push(p.clone());
                            }
                        }
                        Del::Kept => r.kept.push(format!("{p} (changed in {side})")),
                    }
                }
                Act::RestoreLocal(s) => {
                    r.kept.push(p.clone());
                    if copy(&store_dir, &local_dir, &p, &s, None, &tag, &mut r)? {
                        next.insert(p.clone(), s);
                    }
                }
                Act::RestoreStore(l) => {
                    r.kept.push(p.clone());
                    if copy(&local_dir, &store_dir, &p, &l, None, &tag, &mut r)? {
                        next.insert(p.clone(), l);
                    }
                }
                Act::Conflict(l, s) => {
                    conflict(
                        &local_dir, &store_dir, &local, &store, &p, &l, &s, &tag, &mut next, &mut r,
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            if let Some(mut g) = ripe.remove(&p).filter(|_| to_trash) {
                // The move to the trash failed (no space, no access, another
                // file system). The note stays in the loft and in the base,
                // and the delete stays pending, marked: the next sync tries
                // the move again at once, and its pulls go on.
                g.failed = true;
                pending.insert(p.clone(), g);
                r.skipped.push(format!(
                    "{p}: {e:#}; the note stays in the loft, and the next sync tries the delete again"
                ));
            } else {
                r.skipped.push(format!("{p}: {e:#}"));
            }
        }
        step();
    }
    bin.close();
    if retry {
        return Ok(r);
    }
    let changed = next != base.files;
    if !fresh && !found && (changed || base.stale) {
        // A new id for each new base: a backup of the folder from before
        // this sync has the old id, so its restore starts from an empty
        // base, also when the restored folder got the old inode again. A
        // stop before the base is saved leaves another id: the next sync
        // starts empty, which is safe.
        base.id = Some(new_mark(&local_dir)?);
        base.stale = false;
    } else if found && changed {
        // No marker now (see `found`): the next sync without a delete
        // writes it.
        base.stale = true;
    }
    base.files = next;
    base.pending = pending;
    save_base(loft, worktree, &base)?;
    Ok(r)
}

/// The line for the pending deletes of `base` that a changed folder drops.
fn dropped(base: &Base, worktree: &Path) -> String {
    format!(
        "{} deleted in the work tree, but {}/{NOTES} changed since, so these deletes do not reach the loft",
        names_of(base.pending.keys()),
        worktree.display()
    )
}

/// What a sync would do now, without the lock and without writing: the
/// paths that differ, and why. For `paddock loft status`.
pub fn pending(loft: &Loft, worktree: &Path) -> Result<Vec<String>> {
    let local_dir = open_local(worktree)?;
    let store_dir = open_store(loft)?;
    let (base, _) = base_of(loft, worktree, &local_dir)?;
    let (mut local, mut store) = (Side::default(), Side::default());
    scan(&local_dir, "", 0, &mut local);
    scan(&store_dir, "", 0, &mut store);
    let mut out = Vec::new();
    for (p, a) in plan(&local, &store, &base) {
        let why = match a {
            Act::Same(_) | Act::Blocked => continue,
            Act::Pull(..) => "changed in the loft",
            Act::Push(..) => "changed in the work tree",
            Act::DeleteLocal(_) => "deleted in the loft",
            Act::DeleteStore(_) if base.pending.get(&p).is_some_and(|g| g.failed) => {
                "deleted in the work tree, the move to the loft trash failed"
            }
            Act::DeleteStore(_) if base.pending.contains_key(&p) => {
                "deleted in the work tree, waits for a second sync"
            }
            Act::DeleteStore(_) => "deleted in the work tree",
            Act::RestoreLocal(_) | Act::RestoreStore(_) => {
                "deleted on one side, changed on the other"
            }
            Act::Conflict(..) => "changed on both sides",
        };
        out.push(format!("{p} ({why})"));
    }
    Ok(out)
}

#[cfg(test)]
mod folder_tests;
#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
#[cfg(test)]
mod trash_tests;
