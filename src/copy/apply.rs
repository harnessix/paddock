//! Apply, keep and discard: the changes of a copy go to the original, or
//! go away.
//!
//! An apply is a host action. Its steps:
//!
//! 1. List the changes again, and stop when the list is not the one that
//!    the user saw (the token).
//! 2. Put the content of each file into `sync/` of the run. From here on
//!    the agent cannot change what paddock writes.
//! 3. Make a backup of the files of the original that the apply replaces
//!    or removes.
//! 4. Check each file of the original once more, then write it: a new
//!    file next to it and a rename.
//!
//! A conflict is never written unless the user chose how.

use super::backup::{self, Kind as BackupKind};
use super::changes::{self, base_file, base_of, link_hash, Change, Conflict, Status};
use super::run::{Run, Synced};
use super::tree::{self, Entry, Side};
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read as _, Write as _};

/// The largest file that paddock merges or shows as text.
pub const TEXT_MAX: u64 = 1_000_000;

/// What an apply does with a conflict.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnConflict {
    /// Leave it, and report it.
    #[default]
    Stop,
    /// Keep the original. The change stays in the copy.
    Original,
    /// Take the copy. The original's version goes into the backup.
    Copy,
    /// Keep both: the copy's version goes next to the original as
    /// `NAME.conflict-<run>-<time>`.
    Both,
    /// Take the merge of both, when it has no conflict marks.
    Merge,
}

impl OnConflict {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "stop" => Self::Stop,
            "original" => Self::Original,
            "copy" => Self::Copy,
            "both" => Self::Both,
            "merge" => Self::Merge,
            _ => bail!("{s} is not stop, original, copy, both or merge"),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// Files or folders. Empty: every change that is not kept.
    pub paths: Vec<String>,
    pub on_conflict: OnConflict,
    /// The token of the list that the user saw.
    pub token: Option<String>,
}

/// What an apply did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Applied {
    /// New and changed files that the original has now.
    pub written: Vec<String>,
    /// Files that left the original.
    pub deleted: Vec<String>,
    /// Conflicts that the merge of both sides solved.
    pub merged: Vec<String>,
    /// Conflicts with both versions kept: the path, and the name of the
    /// copy's version in the original.
    pub both: Vec<(String, String)>,
    /// Conflicts where the original stays.
    pub kept_original: Vec<String>,
    /// Conflicts that nobody decided. Nothing was written for them.
    pub conflicts: Vec<String>,
    /// Kept paths that an apply of everything left out.
    pub kept: Vec<String>,
    /// New symlinks that lead out of the project. paddock does not write
    /// them.
    pub unsafe_links: Vec<String>,
    /// The backup from before the apply.
    pub backup: Option<String>,
}

impl Applied {
    pub fn count(&self) -> usize {
        self.written.len() + self.deleted.len() + self.merged.len()
    }
}

/// What to do with one path of the original.
enum Act {
    /// Write the staged content with this hash and mode.
    Write {
        hash: String,
        mode: u32,
        merged: bool,
    },
    Link(String),
    Delete,
    /// Write the copy's version at another name.
    Aside {
        hash: String,
        mode: u32,
        name: String,
    },
}

struct Step {
    path: String,
    act: Act,
    /// The copy's file when paddock staged it.
    copy: Option<Entry>,
    /// The original's file when paddock listed the changes.
    original: Option<Entry>,
}

/// The merge of a conflict: the text, and whether it has no conflict
/// marks. None: paddock cannot merge this path (no git, a binary or large
/// file, a symlink, or one side has no file).
pub fn merge(env: &HostEnv, run: &Run, path: &str) -> Result<Option<(bool, Vec<u8>)>> {
    let Some(git) = env.which("git") else {
        return Ok(None);
    };
    let base = run.base()?;
    let manifest = base.manifest();
    let (_, _, r) = base_of(run, &base, &manifest, path);
    let copy = Side::open(&run.tree())?;
    let original = Side::open(&run.rec.project)?;
    let (Some(a), Some(b), Some(o)) = (
        copy.file(path),
        base_file(run, &r, path),
        original.file(path),
    ) else {
        return Ok(None);
    };
    let tmp = tempfile::tempdir()?;
    for (name, mut f) in [("copy", a), ("base", b), ("original", o)] {
        if f.metadata()?.len() > TEXT_MAX {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        if !super::diff::is_text(&bytes) {
            return Ok(None);
        }
        File::create(tmp.path().join(name))?.write_all(&bytes)?;
    }
    // No configuration of the user or the system, and no repository: the
    // three files are all that git reads.
    let out = std::process::Command::new(git)
        .current_dir(tmp.path())
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("HOME", tmp.path())
        .args([
            "merge-file",
            "-p",
            "-L",
            "your folder",
            "-L",
            "start of the run",
            "-L",
            "the copy",
            "original",
            "base",
            "copy",
        ])
        .output()
        .context("cannot run git merge-file")?;
    match out.status.code() {
        Some(0) => Ok(Some((true, out.stdout))),
        Some(n) if (1..=127).contains(&n) => Ok(Some((false, out.stdout))),
        _ => Ok(None),
    }
}

fn entry_now(side: &Side, path: &str) -> Result<Option<Entry>> {
    let Some(mut e) = side.entry(path) else {
        return Ok(None);
    };
    e.hash = match (&e.link, side.file(path)) {
        (Some(t), _) => Some(link_hash(t)),
        (None, Some(mut f)) => Some(tree::hash(&mut f)?),
        (None, None) => None,
    };
    Ok(Some(e))
}

fn same(a: &Option<Entry>, b: &Option<Entry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_fp(b),
        _ => false,
    }
}

/// A free name next to `path` for the copy's version of a conflict.
fn aside_name(original: &Side, run: &Run, path: &str) -> String {
    let stem = format!(
        "{path}.conflict-{}-{}",
        crate::ids::short(&run.rec.id),
        crate::state::now()
    );
    let mut name = stem.clone();
    let mut n = 1;
    while original.exists(&name) {
        name = format!("{stem}-{n}");
        n += 1;
    }
    name
}

/// Put the copy's file at `path` into `sync/`. None: the file is gone or
/// no regular file by now.
fn stage(run: &Run, copy: &Side, path: &str) -> Result<Option<(String, Entry)>> {
    let Some(e) = copy.entry(path) else {
        return Ok(None);
    };
    let Some(mut f) = copy.file(path) else {
        return Ok(None);
    };
    Ok(Some((run.stash(&mut f)?, e)))
}

/// Work out the steps of an apply: what to write where.
fn plan(
    env: &HostEnv,
    run: &mut Run,
    st: &Status,
    o: &Opts,
    out: &mut Applied,
) -> Result<Vec<Step>> {
    let chosen: Vec<&Change> = if o.paths.is_empty() {
        st.changes
            .iter()
            .filter(|c| {
                if c.kept {
                    out.kept.push(c.path.clone());
                }
                !c.kept
            })
            .collect()
    } else {
        changes::select(st, &o.paths)?
    };
    let copy = Side::open(&run.tree())?;
    let original = Side::open(&run.rec.project)?;
    let mut steps = Vec::new();
    for c in chosen {
        let p = &c.path;
        let step = |act: Act, copy_entry: Option<Entry>| Step {
            path: p.clone(),
            act,
            copy: copy_entry,
            original: c.original.clone(),
        };
        // What the user chose for a conflict, as a plain action.
        let take_copy = match (c.conflict, o.on_conflict) {
            (None, _) | (Some(_), OnConflict::Copy) => true,
            (Some(_), OnConflict::Stop) => {
                out.conflicts.push(p.clone());
                continue;
            }
            (Some(_), OnConflict::Original) => {
                run.rec.kept.insert(p.clone());
                out.kept_original.push(p.clone());
                continue;
            }
            (Some(Conflict::OriginalDeleted), OnConflict::Both) => true,
            (Some(Conflict::CopyDeleted), OnConflict::Both) => {
                run.rec.kept.insert(p.clone());
                out.kept_original.push(p.clone());
                continue;
            }
            (Some(_), OnConflict::Both) => false,
            (Some(_), OnConflict::Merge) => {
                match merge(env, run, p)? {
                    Some((true, text)) => {
                        let tmp = tempfile::NamedTempFile::new()?;
                        tmp.as_file().write_all(&text)?;
                        let hash = run.stash(&mut File::open(tmp.path())?)?;
                        let mode = c.original.as_ref().map(|e| e.mode).unwrap_or(0o644);
                        steps.push(step(
                            Act::Write {
                                hash,
                                mode,
                                merged: true,
                            },
                            None,
                        ));
                    }
                    _ => out.conflicts.push(p.clone()),
                }
                continue;
            }
        };
        let Some(e) = &c.copy else {
            steps.push(step(Act::Delete, None));
            continue;
        };
        if let Some(t) = &e.link {
            if tree::link_leaves(p, t) {
                out.unsafe_links.push(p.clone());
                continue;
            }
            if take_copy {
                steps.push(step(Act::Link(t.clone()), Some(e.clone())));
            } else {
                // A symlink has no second version to put aside.
                out.conflicts.push(p.clone());
            }
            continue;
        }
        let Some((hash, staged)) = stage(run, &copy, p)? else {
            // The agent removed or replaced it since the list.
            out.conflicts.push(p.clone());
            continue;
        };
        let mode = staged.mode;
        let act = if take_copy {
            Act::Write {
                hash,
                mode,
                merged: false,
            }
        } else {
            Act::Aside {
                hash,
                mode,
                name: aside_name(&original, run, p),
            }
        };
        steps.push(step(act, Some(staged)));
    }
    Ok(steps)
}

/// Write the changes of `run` into the original.
pub fn apply(env: &HostEnv, run: &mut Run, o: &Opts) -> Result<Applied> {
    let _lock = run.store.lock()?;
    let st = changes::status(run)?;
    if !st.tree {
        bail!("the copy of {} is gone", run.rec.id);
    }
    if o.token.as_ref().is_some_and(|t| *t != st.token) {
        bail!("the changes are not the ones that you saw: a file changed since then. List them again, then apply");
    }
    let mut out = Applied::default();
    let steps = plan(env, run, &st, o, &mut out)?;
    if steps.is_empty() {
        run.save()?;
        return Ok(out);
    }
    let original = Side::open(&run.rec.project)?;
    // Nothing is written when the original is not as the list saw it.
    for s in &steps {
        if !same(&original.entry(&s.path), &s.original) {
            bail!(
                "{} changed in your folder after paddock listed the changes. Nothing was written. List the changes again",
                s.path
            );
        }
    }
    let touched: BTreeSet<String> = steps
        .iter()
        .map(|s| match &s.act {
            Act::Aside { name, .. } => name.clone(),
            _ => s.path.clone(),
        })
        .collect();
    let mut backup = backup::before(
        &run.store,
        BackupKind::Apply,
        &run.rec.project,
        Some((&run.rec.id, &run.rec.harness)),
        &touched,
    )?;
    out.backup = Some(backup.meta.id.clone());
    let copy = Side::open(&run.tree())?;
    let done = write_steps(run, &steps, &original, &copy, &mut backup, &mut out);
    // Also after a failure: the record says what was written.
    backup.save()?;
    run.save()?;
    done?;
    Ok(out)
}

fn write_steps(
    run: &mut Run,
    steps: &[Step],
    original: &Side,
    copy: &Side,
    backup: &mut backup::Backup,
    out: &mut Applied,
) -> Result<()> {
    let has_dir = |d: &str| copy.is_dir(d);
    for s in steps {
        let p = &s.path;
        let target = match &s.act {
            Act::Aside { name, .. } => name,
            _ => p,
        };
        // The backup holds what is there now. A file that changed since
        // the backup is not in it: stop before the write.
        let before = backup.meta.files.get(target).cloned().flatten();
        if !same(&original.entry(target), &before) {
            bail!(
                "{target} changed in your folder during the apply. paddock stopped there. `paddock undo` takes back what it wrote"
            );
        }
        let content = |hash: &str| {
            run.stashed(hash)
                .with_context(|| format!("the staged content of {p} is gone"))
        };
        match &s.act {
            Act::Write { hash, mode, .. } | Act::Aside { hash, mode, .. } => {
                original.put_file(target, &mut content(hash)?, *mode, None)?
            }
            Act::Link(t) => original.put_link(target, t)?,
            Act::Delete => original.remove(target, &has_dir)?,
        }
        let after = entry_now(original, target)?;
        backup.meta.after.insert(target.clone(), after.clone());
        match &s.act {
            Act::Aside { name, .. } => {
                run.rec.kept.insert(p.clone());
                out.both.push((p.clone(), name.clone()));
                continue;
            }
            Act::Write {
                hash, mode, merged, ..
            } if *merged => {
                // The merge goes to the copy too, so both sides agree.
                copy.put_file(p, &mut content(hash)?, *mode, None)?;
                out.merged.push(p.clone());
            }
            Act::Delete => out.deleted.push(p.clone()),
            _ => out.written.push(p.clone()),
        }
        backup
            .meta
            .synced
            .insert(p.clone(), run.rec.synced.get(p).cloned());
        let copy_entry = match &s.act {
            Act::Write { merged: true, .. } => copy.entry(p),
            _ => s.copy.clone(),
        };
        run.rec.synced.insert(
            p.clone(),
            Synced {
                hash: after.as_ref().and_then(|e| e.hash.clone()),
                copy: copy_entry,
                original: after.map(|mut e| {
                    e.hash = None;
                    e
                }),
            },
        );
        run.rec.kept.remove(p);
    }
    Ok(())
}

/// Mark `paths` as kept in the copy (`on`), or take the mark away.
pub fn keep(run: &mut Run, paths: &[String], on: bool) -> Result<Vec<String>> {
    let _lock = run.store.lock()?;
    let st = changes::status(run)?;
    if !st.tree {
        bail!("the copy of {} is gone", run.rec.id);
    }
    let mut done = Vec::new();
    for c in changes::select(&st, paths)? {
        let changed = if on {
            run.rec.kept.insert(c.path.clone())
        } else {
            run.rec.kept.remove(&c.path)
        };
        if changed {
            done.push(c.path.clone());
        }
    }
    run.save()?;
    Ok(done)
}

/// Drop the changes of `paths` from the copy: each file goes back to its
/// base. The original is not touched.
pub fn discard(run: &mut Run, paths: &[String]) -> Result<Vec<String>> {
    let _lock = run.store.lock()?;
    let st = changes::status(run)?;
    if !st.tree {
        bail!("the copy of {} is gone", run.rec.id);
    }
    let base = run.base()?;
    let manifest = base.manifest();
    let copy = Side::open(&run.tree())?;
    let mut done = Vec::new();
    let mut synced: BTreeMap<String, Synced> = BTreeMap::new();
    for c in changes::select(&st, paths)? {
        let p = &c.path;
        let (bc, _, r) = base_of(run, &base, &manifest, p);
        match bc {
            None => copy.remove(p, &|d| manifest.keys().any(|k| changes::covers(d, k)))?,
            Some(e) => {
                let mut f = base_file(run, &r, p);
                if e.link.is_none() && f.is_none() {
                    bail!("the base content of {p} is gone, so paddock cannot bring it back");
                }
                copy.put(p, e, f.as_mut(), true)?;
            }
        }
        // The copy's file is a new one: the record gets its new time.
        if let Some(s) = run.rec.synced.get(p) {
            let mut s = s.clone();
            s.copy = copy.entry(p);
            synced.insert(p.clone(), s);
        }
        done.push(p.clone());
    }
    for (p, s) in synced {
        run.rec.synced.insert(p, s);
    }
    for p in &done {
        run.rec.kept.remove(p);
    }
    run.save()?;
    Ok(done)
}

/// Remove the whole working copy. The backups stay.
pub fn discard_all(env: &HostEnv, run: &mut Run) -> Result<()> {
    if run.running(env) {
        bail!(
            "the sandbox of {} still runs. Stop it first (`paddock stop {}`)",
            run.rec.id,
            crate::ids::short(&run.rec.id)
        );
    }
    let _lock = run.store.lock()?;
    run.remove_tree()
}
