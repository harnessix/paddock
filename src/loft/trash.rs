//! The trash of the loft. A delete in a work tree in sync mode never
//! removes a note: the sync moves the loft version to
//! `<loft>/trash/<time>/<path>`, one folder for each sync that deletes.
//! The other work trees then remove the file, as before.
//!
//! `paddock loft trash` lists the trash, `paddock loft restore PATH`
//! brings a file back to the loft and to the work trees. If the loft has
//! a newer file at the path, the restored one becomes a conflict copy
//! next to it. `paddock gc` removes the folders of the trash that are
//! older than `[loft] trash_days` (30). Nothing else removes them.

use super::sync::{self, Del};
use super::Loft;
use crate::env::HostEnv;
use crate::safefs::{Dir, Kind, Root};
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// The default of `[loft] trash_days`.
pub const DAYS: u64 = 30;
const DAY: u64 = 24 * 3600;

/// The trash folder of `loft`.
pub fn root(loft: &Loft) -> PathBuf {
    loft.root.join("trash")
}

/// The name of a folder of the trash: the time of the delete in UTC, for
/// example `20261112T143649Z`. A second folder in the same second gets
/// `-1`, `-2`...
fn stamp(secs: u64) -> String {
    crate::waiting::format_rfc3339(secs).replace(['-', ':'], "")
}

/// The time of the folder `name` of the trash, in seconds; None when it
/// is not a name that a sync made.
fn when(name: &str) -> Option<u64> {
    let b = name.as_bytes();
    if b.len() < 16 || !name.is_ascii() || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    if b.len() > 16 && (b[16] != b'-' || !name[17..].bytes().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let t = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &name[0..4],
        &name[4..6],
        &name[6..8],
        &name[9..11],
        &name[11..13],
        &name[13..15]
    );
    crate::waiting::parse_rfc3339(&t).ok()
}

/// The trash folder of one sync, made at its first delete.
pub(super) struct Bin<'a> {
    loft: &'a Loft,
    open: Option<(Dir, Dir, String)>,
}

impl<'a> Bin<'a> {
    pub(super) fn new(loft: &'a Loft) -> Self {
        Self { loft, open: None }
    }

    fn dir(&mut self) -> Result<&Dir> {
        if self.open.is_none() {
            let r = root(self.loft);
            crate::privfs::dir(&r)?;
            let top = Root::open(&r)?.dir(Path::new(""), false)?;
            let (d, name) = top.new_subdir(&stamp(sync::now_ms() / 1000))?;
            self.open = Some((top, d, name));
        }
        Ok(&self.open.as_ref().expect("just made").1)
    }

    /// Move `rel` of the store into this trash folder when it still has
    /// the content `expected`. A file that changed since the scan goes
    /// back to its name (or next to it as a conflict copy, when a new file
    /// took the name): Kept.
    pub(super) fn put(&mut self, store: &Dir, rel: &str, expected: &str, tag: &str) -> Result<Del> {
        let (parent, name) = sync::split(rel);
        let Ok(d) = sync::dir_at(store, parent, false) else {
            return Ok(Del::Done);
        };
        match d.kind(OsStr::new(name))? {
            None => return Ok(Del::Done),
            Some(Kind::File) => {}
            Some(_) => return Ok(Del::Kept),
        }
        let t = sync::dir_at(self.dir()?, parent, true)?;
        match d.rename_to(OsStr::new(name), &t, OsStr::new(name)) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return Ok(Del::Done),
            Err(e) => {
                return Err(
                    anyhow::Error::from(e).context(format!("cannot move {rel} to the loft trash"))
                )
            }
        }
        sync::step();
        let same = matches!(sync::read_hash(&t, name), Ok(Some((_, ref h))) if h == expected);
        if !same {
            // Another version than the planned one: it stays in the loft.
            if t.rename_to(OsStr::new(name), &d, OsStr::new(name)).is_err() {
                let c = sync::conflict_name(&d, rel, tag, &[]);
                t.rename_to(OsStr::new(name), &d, OsStr::new(sync::split(&c).1))
                    .with_context(|| {
                        format!(
                            "{rel} changed during the sync and cannot go back; it is in {}",
                            t.path().join(name).display()
                        )
                    })?;
            }
            return Ok(Del::Kept);
        }
        sync::prune_up(store, parent);
        Ok(Del::Done)
    }

    /// The end of the sync: a trash folder that holds nothing goes.
    pub(super) fn close(self) {
        if let Some((top, d, name)) = self.open {
            sync::prune(&d);
            drop(d);
            let _ = top.remove_dir(OsStr::new(&name));
        }
    }
}

/// One file in the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The folder of the trash: the time of the delete.
    pub at: String,
    /// That time, in seconds since the epoch.
    pub when: u64,
    /// The path in `.paddock`.
    pub path: String,
    pub bytes: u64,
}

fn walk(d: &Dir, prefix: &str, depth: usize, out: &mut Vec<(String, u64)>) {
    let Ok(entries) = d.stats() else {
        return;
    };
    for (n, st) in entries {
        let Some(name) = n.to_str() else {
            continue;
        };
        let rel = format!("{prefix}{name}");
        match st.kind {
            Kind::File => out.push((rel, st.size)),
            Kind::Dir if depth < 64 => {
                if let Ok(sub) = d.subdir(&n, false) {
                    walk(&sub, &format!("{rel}/"), depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

/// The folders of the trash with their time, oldest first.
fn folders(loft: &Loft) -> Vec<(String, u64)> {
    let Ok(top) = Root::open(&root(loft)).and_then(|r| r.dir(Path::new(""), false)) else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64)> = top
        .entries()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, k)| *k == Kind::Dir)
        .filter_map(|(n, _)| {
            let n = n.to_str()?.to_string();
            when(&n).map(|w| (n, w))
        })
        .collect();
    out.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    out
}

/// Every file in the trash of `loft`, the newest delete first.
pub fn list(loft: &Loft) -> Vec<Entry> {
    let mut out = Vec::new();
    for (at, when) in folders(loft).into_iter().rev() {
        let Ok(d) = Root::open(&root(loft)).and_then(|r| r.dir(Path::new(&at), false)) else {
            continue;
        };
        let mut files = Vec::new();
        walk(&d, "", 0, &mut files);
        files.sort();
        out.extend(files.into_iter().map(|(path, bytes)| Entry {
            at: at.clone(),
            when,
            path,
            bytes,
        }));
    }
    out
}

/// The lines of `paddock loft trash`.
pub fn render(env: &HostEnv, loft: &Loft) -> String {
    let all = list(loft);
    let days = super::Cfg::load(env).trash_days;
    if all.is_empty() {
        return "The loft trash is empty.\n".into();
    }
    let mut t = String::new();
    for e in &all {
        t.push_str(&format!(
            "{}  {}  ({} bytes)\n",
            crate::waiting::format_rfc3339(e.when),
            e.path,
            e.bytes
        ));
    }
    t.push_str(&format!(
        "\n{} deleted notes in {}. `paddock loft restore PATH` brings the newest one of a path back. `paddock gc --yes` removes those older than {days} days.\n",
        all.len(),
        root(loft).display()
    ));
    t
}

/// A path in `.paddock` as the user names it: `plan.md`, `.paddock/plan.md`
/// or a folder `notes/`.
fn clean(path: &str) -> Result<String> {
    let mut p = path.trim();
    for pre in ["./", ".paddock/"] {
        p = p.strip_prefix(pre).unwrap_or(p);
    }
    let p = p.trim_end_matches('/');
    let bad = p.is_empty()
        || Path::new(p)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)));
    if bad {
        bail!("{path:?} is not a path in .paddock, such as plan.md or notes/");
    }
    Ok(p.to_string())
}

/// `paddock loft restore PATH`: the newest version of PATH in the trash
/// (of each file below PATH, for a folder) goes back to the loft. If the
/// loft has another file at the path, the restored one becomes a conflict
/// copy next to it. No base keeps the path, so each work tree gets the
/// file at its next sync; `worktree`, when it is in sync mode, syncs now.
/// Returns the lines for the user.
pub fn restore(loft: &Loft, worktree: Option<&Path>, path: &str, who: &str) -> Result<Vec<String>> {
    let p = clean(path)?;
    let _lock = sync::lock(loft)?;
    let wt = worktree.filter(|w| sync::synced(loft, w));
    let mut lines = Vec::new();
    if let Some(w) = wt {
        // First the changes of the work tree, so a newer file there is in
        // the loft before the restore compares.
        match sync::sync_locked(loft, w, who) {
            Ok(r) => lines.extend(r.lines()),
            Err(e) => lines.push(format!("cannot sync {}/.paddock first: {e:#}", w.display())),
        }
    }
    let mut seen = BTreeSet::new();
    let pick: Vec<Entry> = list(loft)
        .into_iter()
        .filter(|e| e.path == p || e.path.starts_with(&format!("{p}/")))
        .filter(|e| seen.insert(e.path.clone()))
        .collect();
    if pick.is_empty() {
        bail!("no {p} in the loft trash: `paddock loft trash` lists what it has");
    }
    let store = sync::open_store(loft)?;
    let tag = sync::tag(who);
    for e in &pick {
        let (line, at, hash) = back(loft, &store, e, &tag)?;
        lines.push(line);
        sync::forget(loft, &at, &hash)?;
    }
    if let Some(w) = wt {
        match sync::sync_locked(loft, w, who) {
            Ok(r) => lines.extend(r.lines()),
            Err(e) => lines.push(format!("cannot sync {}/.paddock: {e:#}", w.display())),
        }
    }
    Ok(lines)
}

/// Move the file of `e` from the trash back into the store. Returns the
/// line for the user, the path that the file has now (a conflict copy
/// when the loft has another file at its path) and its hash.
fn back(loft: &Loft, store: &Dir, e: &Entry, tag: &str) -> Result<(String, String, String)> {
    let top = Root::open(&root(loft))?.dir(Path::new(""), false)?;
    let folder = top.subdir(OsStr::new(&e.at), false)?;
    let (parent, name) = sync::split(&e.path);
    let src = sync::dir_at(&folder, parent, false)?;
    let mine = sync::read_hash(&src, name)?
        .map(|(_, h)| h)
        .with_context(|| format!("{} is no longer in the trash", e.path))?;
    let d = sync::dir_at(store, parent, true)?;
    let line = match d.kind(OsStr::new(name))? {
        None => match src.rename_to(OsStr::new(name), &d, OsStr::new(name)) {
            Ok(()) => Some(format!("restored: {}", e.path)),
            Err(x) if x.raw_os_error() == Some(libc::EEXIST) => None,
            Err(x) => {
                return Err(anyhow::Error::from(x).context(format!("cannot restore {}", e.path)))
            }
        },
        Some(Kind::File) if sync::read_hash(&d, name)?.is_some_and(|(_, h)| h == mine) => {
            src.remove(OsStr::new(name))?;
            Some(format!(
                "restored: {} (the loft has the same content already)",
                e.path
            ))
        }
        Some(_) => None,
    };
    let (line, at) = match line {
        Some(l) => (l, e.path.clone()),
        None => {
            let c = sync::conflict_name(&d, &e.path, tag, &[]);
            src.rename_to(OsStr::new(name), &d, OsStr::new(sync::split(&c).1))
                .with_context(|| format!("cannot restore {}", e.path))?;
            let l = format!(
                "restored: {} as {c}, because the loft has a newer {}. Keep what you want in {} and delete the copy",
                e.path, e.path, e.path
            );
            (l, c)
        }
    };
    sync::prune_up(&folder, parent);
    drop((src, folder));
    let _ = top.remove_dir(OsStr::new(&e.at));
    Ok((line, at, mine))
}

/// The lofts of this host.
fn lofts(env: &HostEnv) -> Vec<Loft> {
    let base = env.paddock_data().join("loft");
    let Ok(rd) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut out: Vec<Loft> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| Loft { root: e.path() })
        .collect();
    out.sort_by(|a, b| a.root.cmp(&b.root));
    out
}

/// The folders of the loft trashes that `paddock gc` removes: older than
/// `[loft] trash_days` at `now` (seconds).
pub fn gc_items(env: &HostEnv, now: u64) -> Vec<crate::storage::Item> {
    let days = super::Cfg::load(env).trash_days;
    let mut out = Vec::new();
    for loft in lofts(env) {
        for (at, when) in folders(&loft) {
            if now.saturating_sub(when) <= days * DAY {
                continue;
            }
            let dir = root(&loft).join(&at);
            let mut files = Vec::new();
            if let Ok(d) = Root::open(&dir).and_then(|r| r.dir(Path::new(""), false)) {
                walk(&d, "", 0, &mut files);
            }
            out.push(crate::storage::Item {
                what: format!(
                    "loft trash {} ({} deleted notes)",
                    dir.display(),
                    files.len()
                ),
                tier: crate::storage::Tier::Auto,
                bytes: files.iter().map(|(_, b)| b).sum(),
                reason: format!("deleted more than {days} days ago ([loft] trash_days)"),
                warning: None,
                needs: vec![],
                action: crate::storage::Action::LoftTrash { dir },
            });
        }
    }
    out
}

/// Remove the folder `dir` of a loft trash, for `paddock gc`. It must be
/// `<data>/loft/<key>/trash/<time>`.
pub fn remove(env: &HostEnv, dir: &Path) -> Result<()> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| when(n).is_some());
    let trash = dir.parent();
    let loft_root = trash.and_then(|t| t.parent());
    let lofts = std::fs::canonicalize(env.paddock_data().join("loft")).ok();
    let ok = name.is_some()
        && trash.and_then(|t| t.file_name()) == Some(OsStr::new("trash"))
        && loft_root
            .and_then(|l| l.parent())
            .and_then(|p| std::fs::canonicalize(p).ok())
            .is_some_and(|p| Some(p) == lofts);
    let (Some(name), Some(trash), Some(loft_root), true) = (name, trash, loft_root, ok) else {
        bail!("{} is not a folder of a loft trash", dir.display());
    };
    let loft = Loft {
        root: loft_root.to_path_buf(),
    };
    let _lock = sync::lock_within(&loft, std::time::Duration::from_secs(10))?;
    Root::open(trash)?
        .remove_tree(Path::new(name))
        .with_context(|| format!("remove {}", dir.display()))
}

#[cfg(test)]
pub(super) fn stamp_for_tests(secs: u64) -> String {
    stamp(secs)
}
