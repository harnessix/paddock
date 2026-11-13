//! Restore a backup, and `paddock undo` for the last apply.
//!
//! Every restore first makes a `before restore` backup of the files that
//! it replaces or removes, so a restore has an undo too.
//!
//! An undo takes back the newest apply or restore of the project. It
//! touches a file only when the file is still as that action left it. A
//! file that the user changed since then stays, unless the user forces it.

use super::backup::{self, Backup, Kind};
use super::changes::{clean_path, covers, link_hash};
use super::run::Run;
use super::store::Store;
use super::tree::{self, Entry, Side};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// What a restore changes in the project.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Plan {
    /// Files that come back with the content of the backup.
    pub restore: Vec<String>,
    /// Files that leave the project: the backup has no file there.
    pub remove: Vec<String>,
    /// Files that changed after the action that an undo takes back. They
    /// stay without `force`.
    pub changed_since: Vec<String>,
    /// Files that are newer than a full backup. They stay unless the
    /// restore is exact.
    pub newer: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.restore.is_empty() && self.remove.is_empty()
    }
}

/// What a restore did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcome {
    /// The backup that was restored.
    pub of: String,
    pub restored: Vec<String>,
    pub removed: Vec<String>,
    /// Files that stayed because they changed since.
    pub skipped: Vec<String>,
    /// The backup from before this restore.
    pub backup: Option<String>,
}

fn with_hash(side: &Side, path: &str) -> Result<Option<Entry>> {
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

/// The file at `path` is `want`, by its fingerprint or by its content.
fn is(side: &Side, path: &str, backup: &Backup, want: &Option<Entry>) -> Result<bool> {
    let now = side.entry(path);
    let (now, want) = match (now, want) {
        (None, None) => return Ok(true),
        (Some(n), Some(w)) => (n, w),
        _ => return Ok(false),
    };
    if now.same_fp(want) {
        return Ok(true);
    }
    if now.link.is_some() || want.link.is_some() || now.size != want.size || now.mode != want.mode {
        return Ok(false);
    }
    let Some(mut f) = side.file(path) else {
        return Ok(false);
    };
    match (&want.hash, backup.file(path)) {
        (Some(h), _) => Ok(tree::hash(&mut f)? == *h),
        (None, Some(mut g)) => tree::same_bytes(&mut f, &mut g),
        (None, None) => Ok(false),
    }
}

fn selected(paths: &[String]) -> Result<Vec<String>> {
    paths.iter().map(|p| clean_path(p)).collect()
}

fn picks(sel: &[String], path: &str) -> bool {
    sel.is_empty() || sel.iter().any(|s| covers(s, path))
}

/// What a restore of `backup` changes in `project`. `paths` narrows it to
/// files or folders. `exact`: files that a full backup does not have leave
/// the project.
pub fn plan(backup: &Backup, project: &Path, paths: &[String], exact: bool) -> Result<Plan> {
    let sel = selected(paths)?;
    let side = Side::open(project)?;
    let mut plan = Plan::default();
    for (p, want) in &backup.meta.files {
        if !picks(&sel, p) || is(&side, p, backup, want)? {
            continue;
        }
        match want {
            Some(_) => plan.restore.push(p.clone()),
            None => plan.remove.push(p.clone()),
        }
    }
    if backup.meta.full {
        let mut skip = backup.meta.skip.union(&super::start::rules(project));
        skip.paths.insert(super::pointer::DIR.to_string());
        for p in tree::scan(project, &skip)?.files.into_keys() {
            if backup.meta.files.contains_key(&p) || !picks(&sel, &p) {
                continue;
            }
            if exact {
                plan.remove.push(p);
            } else {
                plan.newer.push(p);
            }
        }
    }
    if !sel.is_empty() && plan.is_empty() && plan.newer.is_empty() {
        let known = backup.meta.files.keys().any(|p| picks(&sel, p));
        if !known {
            bail!(
                "the backup {} has no file at {}",
                backup.meta.id,
                paths.join(", ")
            );
        }
    }
    Ok(plan)
}

/// What an undo of the action behind `backup` changes: each path goes back
/// to its state from before, when it is still as the action left it.
pub fn undo_plan(backup: &Backup, project: &Path, force: bool) -> Result<Plan> {
    let side = Side::open(project)?;
    let mut plan = Plan::default();
    for (p, want) in &backup.meta.files {
        if is(&side, p, backup, want)? {
            continue;
        }
        let as_left = match backup.meta.after.get(p) {
            Some(after) => is(&side, p, backup, after)?,
            // The action stopped before this path.
            None => continue,
        };
        if !as_left && !force {
            plan.changed_since.push(p.clone());
            continue;
        }
        match want {
            Some(_) => plan.restore.push(p.clone()),
            None => plan.remove.push(p.clone()),
        }
    }
    Ok(plan)
}

/// Carry out `plan` with the content of `backup`. `by_undo` marks the
/// safety backup as the work of an undo.
pub fn restore(
    store: &Store,
    backup: &Backup,
    project: &Path,
    plan: &Plan,
    by_undo: bool,
) -> Result<Outcome> {
    let _lock = store.lock()?;
    let mut out = Outcome {
        of: backup.meta.id.clone(),
        skipped: plan.changed_since.clone(),
        ..Default::default()
    };
    if plan.is_empty() {
        return Ok(out);
    }
    let touched: BTreeSet<String> = plan.restore.iter().chain(&plan.remove).cloned().collect();
    let mut safety = backup::before(store, Kind::Restore, project, None, &touched)?;
    safety.meta.by_undo = by_undo;
    out.backup = Some(safety.meta.id.clone());
    let side = Side::open(project)?;
    let mut after: BTreeMap<String, Option<Entry>> = BTreeMap::new();
    let done = (|| -> Result<()> {
        for p in &plan.remove {
            // A folder that the backup has files for stays.
            side.remove(p, &|d| {
                backup
                    .meta
                    .files
                    .iter()
                    .any(|(k, e)| e.is_some() && covers(d, k))
            })?;
            after.insert(p.clone(), None);
            out.removed.push(p.clone());
        }
        for p in &plan.restore {
            let Some(Some(e)) = backup.meta.files.get(p) else {
                continue;
            };
            let mut f = backup.file(p);
            if e.link.is_none() && f.is_none() {
                bail!("the backup {} has no content for {p}", backup.meta.id);
            }
            side.put(p, e, f.as_mut(), true)?;
            after.insert(p.clone(), with_hash(&side, p)?);
            out.restored.push(p.clone());
        }
        Ok(())
    })();
    safety.meta.after = after;
    safety.save()?;
    done?;
    Ok(out)
}

/// The newest apply or restore of the store that an undo can take back.
pub fn last_undoable(store: &Store) -> Option<Backup> {
    backup::list(store).into_iter().find(|b| b.undoable())
}

/// Take back the action behind `backup`.
pub fn undo(store: &Store, mut backup: Backup, project: &Path, plan: &Plan) -> Result<Outcome> {
    let out = restore(store, &backup, project, plan, true)?;
    {
        let _lock = store.lock()?;
        backup.meta.undone = Some(crate::state::now());
        backup.save()?;
        // The run forgets the apply: its changes are open again.
        if let (Kind::Apply, Some(id)) = (backup.meta.kind, &backup.meta.run) {
            if let Ok(mut run) = Run::load(store, id) {
                for p in out.restored.iter().chain(&out.removed) {
                    match backup.meta.synced.get(p) {
                        Some(Some(s)) => {
                            run.rec.synced.insert(p.clone(), s.clone());
                        }
                        Some(None) => {
                            run.rec.synced.remove(p);
                        }
                        // Not a path of the copy: the other version of a
                        // conflict with both kept.
                        None => {}
                    }
                }
                run.save()?;
            }
        }
    }
    Ok(out)
}
