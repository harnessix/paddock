//! The JSON of the copy commands, for the launcher and the TUI.
//!
//! Each output is one object with an `api_version`:
//!
//! - `paddock.copies/v1`: `paddock changes --json`, the runs with a copy.
//! - `paddock.changes/v1`: `paddock changes RUN --json`.
//! - `paddock.backups/v1`: `paddock backups --json`.
//! - `paddock.pointer/v1`: `paddock pointer status --json`.
//! - `paddock.apply/v1`, `paddock.keep/v1`, `paddock.discard/v1`,
//!   `paddock.restore/v1`, `paddock.undo/v1`: the result of the action.
//! - `paddock.backup-files/v1`: `paddock backups --files ID --json`.
//! - `paddock.restore-plan/v1`: `paddock restore ID --plan --json`.
//!
//! Times are seconds since 1970. Sizes are bytes. A field that does not
//! apply is `null`, never missing.

use super::apply::TEXT_MAX;
use super::backup::{self, Backup};
use super::changes::{Change, Status};
use super::pointer::{self, State};
use super::run::Run;
use super::tree::{Entry, Side};
use super::Config;
use crate::env::HostEnv;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read as _;
use std::path::Path;

pub const COPIES: &str = "paddock.copies/v1";
pub const CHANGES: &str = "paddock.changes/v1";
pub const BACKUPS: &str = "paddock.backups/v1";
pub const POINTER: &str = "paddock.pointer/v1";
pub const APPLY: &str = "paddock.apply/v1";
pub const KEEP: &str = "paddock.keep/v1";
pub const DISCARD: &str = "paddock.discard/v1";
pub const RESTORE: &str = "paddock.restore/v1";
pub const UNDO: &str = "paddock.undo/v1";
pub const BACKUP_FILES: &str = "paddock.backup-files/v1";
pub const RESTORE_PLAN: &str = "paddock.restore-plan/v1";
/// The longest `merge_preview` of a change, in bytes.
pub const MERGE_PREVIEW_MAX: usize = 64 << 10;

/// `live`, `ended` or `removed`.
pub fn run_state(env: &HostEnv, run: &Run) -> &'static str {
    if !run.has_tree() {
        "removed"
    } else if run.running(env) {
        "live"
    } else {
        "ended"
    }
}

pub fn run_doc(env: &HostEnv, run: &Run) -> Value {
    let r = &run.rec;
    json!({
        "id": r.id,
        "short": crate::ids::short(&r.id),
        "harness": r.harness,
        "project": r.project,
        "copy": run.tree(),
        "state": run_state(env, run),
        "started": r.started,
        "ended": r.ended,
        "removed": r.removed,
        "method": r.method.word(),
        "shared": r.method.shared(),
        "in_place": r.in_place,
        "git": r.git,
        "files": r.files,
        "bytes": r.bytes,
        "store": r.store,
        "base": r.base,
        "carried": r.carried,
    })
}

fn entry_doc(e: &Option<Entry>) -> Value {
    match e {
        None => Value::Null,
        Some(e) => json!({
            "size": e.size,
            "mtime": e.link.is_none().then(|| e.secs()),
            "mode": format!("{:04o}", e.mode & 0o7777),
            "link": e.link,
        }),
    }
}

fn read_text(side: &Side, e: &Option<Entry>, path: &str) -> Option<Vec<u8>> {
    let Some(e) = e else {
        return Some(Vec::new());
    };
    if e.link.is_some() || e.size > TEXT_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    side.file(path)?
        .take(TEXT_MAX + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= TEXT_MAX && super::diff::is_text(&bytes)).then_some(bytes)
}

/// Both versions of a change as text: the original's, then the copy's.
/// None for a symlink, a binary file or a file above 1 MB.
pub fn texts(run: &Run, c: &Change) -> Option<(Vec<u8>, Vec<u8>)> {
    let copy = Side::open(&run.tree()).ok()?;
    let original = Side::open(&run.rec.project).ok()?;
    Some((
        read_text(&original, &c.original, &c.path)?,
        read_text(&copy, &c.copy, &c.path)?,
    ))
}

/// One change. `detail` adds the line counts and the merge state, which
/// read the files.
pub fn change_doc(env: &HostEnv, run: &Run, c: &Change, detail: bool) -> Value {
    let state = match (c.conflict, c.kept) {
        (Some(_), _) => "conflict",
        (None, true) => "kept",
        (None, false) => "open",
    };
    let mut v = json!({
        "path": c.path,
        "kind": c.kind.word(),
        "state": state,
        "conflict": c.conflict,
        "conflict_text": c.conflict.map(|k| k.text()),
        "kept": c.kept,
        "writing": c.writing,
        "copy": entry_doc(&c.copy),
        "original": entry_doc(&c.original),
        "text": Value::Null,
        "added": Value::Null,
        "removed": Value::Null,
        "merge": Value::Null,
    });
    if !detail {
        return v;
    }
    match texts(run, c) {
        Some((old, new)) => {
            let (added, removed) = super::diff::stat(&old, &new);
            v["text"] = true.into();
            v["added"] = added.into();
            v["removed"] = removed.into();
        }
        None => v["text"] = false.into(),
    }
    v["merge_preview"] = Value::Null;
    v["merge_preview_cut"] = Value::Null;
    if c.conflict.is_some() {
        v["merge"] = match super::apply::merge(env, run, &c.path) {
            Ok(Some((true, merged))) => {
                let (text, cut) = preview(&merged);
                v["merge_preview"] = text.map_or(Value::Null, Value::from);
                v["merge_preview_cut"] = text.map_or(Value::Null, |_| cut.into());
                "clean".into()
            }
            Ok(Some((false, _))) => "conflicts".into(),
            _ => Value::Null,
        };
    }
    v
}

/// The text of a clean merge for the screen: at most
/// [`MERGE_PREVIEW_MAX`] bytes, cut at the end of a character, and true
/// when it was cut. None when the bytes are not UTF-8.
pub fn preview(merged: &[u8]) -> (Option<&str>, bool) {
    let cut = merged.len() > MERGE_PREVIEW_MAX;
    let head = &merged[..merged.len().min(MERGE_PREVIEW_MAX)];
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        // A character that the cut splits: up to its start.
        Err(e) if cut && e.error_len().is_none() => {
            std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return (None, false),
    };
    (Some(text), cut)
}

pub fn counts_doc(st: &Status) -> Value {
    use super::changes::Kind;
    json!({
        "new": st.count(Kind::New),
        "changed": st.count(Kind::Changed),
        "deleted": st.count(Kind::Deleted),
        "conflicts": st.conflicts(),
        "kept": st.changes.iter().filter(|c| c.kept).count(),
        "ready": st.ready(),
        "applied": st.applied.len(),
        "behind": st.behind.len(),
    })
}

/// `paddock changes RUN --json`.
pub fn changes_doc(env: &HostEnv, cfg: &Config, run: &Run, st: &Status, detail: bool) -> Value {
    let changes: Vec<Value> = st
        .changes
        .iter()
        .map(|c| change_doc(env, run, c, detail))
        .collect();
    json!({
        "api_version": CHANGES,
        "run": run_doc(env, run),
        "pointer": pointer_part(env, cfg, &run.rec.project),
        "token": st.token,
        "counts": counts_doc(st),
        "changes": changes,
        "applied": st.applied,
        "behind": st.behind,
        "special": st.special,
        "auto_apply": cfg.apply == super::config::Apply::Auto,
    })
}

/// `paddock changes --json`: every run with a copy.
pub fn copies_doc(env: &HostEnv, runs: &[(Run, Option<Status>)]) -> Value {
    let rows: Vec<Value> = runs
        .iter()
        .map(|(run, st)| {
            let mut v = run_doc(env, run);
            v["counts"] = st.as_ref().map(counts_doc).unwrap_or(Value::Null);
            v
        })
        .collect();
    json!({ "api_version": COPIES, "runs": rows })
}

/// The pointer state in short, for the lists: no size and no disk check,
/// so a screen can ask every second. `paddock pointer status --json` has
/// the rest.
fn pointer_part(env: &HostEnv, cfg: &Config, project: &Path) -> Value {
    match pointer::status(env, cfg, project, false) {
        Ok(st) => json!({
            "state": st.state.word(),
            "text": st.state.text(),
            "action": st.state.action(),
            "ok": st.state.ok(),
            "store": st.pointer.as_ref().map(|p| p.store.clone()),
        }),
        Err(e) => json!({
            "state": Value::Null, "text": format!("{e:#}"), "action": Value::Null,
            "ok": false, "store": Value::Null,
        }),
    }
}

fn pointer_fields(
    env: &HostEnv,
    cfg: &Config,
    project: &Path,
    st: &pointer::Status,
    state: State,
) -> Value {
    let store = st.store.as_ref().map(|s| {
        let runs = super::run::of_store(s);
        let (sizes, kind) = crate::storage::sizes(std::slice::from_ref(&s.dir));
        json!({
            "id": s.meta.id,
            "dir": s.dir,
            "original": s.meta.path,
            "created": s.meta.created,
            "runs": runs.len(),
            "copies": runs.iter().filter(|r| r.has_tree()).count(),
            "backups": backup::list(s).len(),
            "size": { "bytes": sizes.first().copied().unwrap_or(0), "kind": kind },
        })
    });
    let root = st
        .store
        .as_ref()
        .map(|s| s.dir.clone())
        .unwrap_or_else(|| cfg.root(env));
    json!({
        "project": project,
        "state": state.word(),
        "text": state.text(),
        "action": state.action(),
        "ok": state.ok(),
        "pointer": st.pointer.as_ref().map(|p| json!({
            "format": p.format, "store": p.store, "created": p.created,
        })),
        "store": store,
        "encryption": super::disk::of(&root),
    })
}

/// `paddock pointer status --json`.
pub fn pointer_doc(env: &HostEnv, cfg: &Config, project: &Path) -> Result<Value> {
    let st = pointer::status(env, cfg, project, true)?;
    let mut v = pointer_fields(env, cfg, project, &st, st.state);
    v["api_version"] = POINTER.into();
    Ok(v)
}

pub fn backup_doc(b: &Backup) -> Value {
    let m = &b.meta;
    json!({
        "id": m.id,
        "short": crate::ids::short(&m.id),
        "kind": m.kind.tag(),
        "words": m.kind.words(),
        "created": m.created,
        "run": m.run,
        "harness": m.harness,
        "files": b.count(),
        "paths": m.files.len(),
        "bytes": m.bytes,
        "shared": m.method.shared(),
        "method": m.method.word(),
        "full": m.full,
        "undone": m.undone,
        "by_undo": m.by_undo,
        "undoable": b.undoable(),
    })
}

/// `paddock backups --json`: the backups of the store, newest first, and
/// the snapshots of direct runs (`paddock undo` restores those).
pub fn backups_doc(env: &HostEnv, cfg: &Config, project: &Path) -> Result<Value> {
    let st = pointer::status(env, cfg, project, true)?;
    let backups: Vec<Value> = match (&st.store, st.state.ok()) {
        (Some(s), true) => backup::list(s).iter().map(backup_doc).collect(),
        _ => Vec::new(),
    };
    let base = crate::snapshot::project_dir(&crate::snapshot::root(env), project);
    let snapshots: Vec<Value> = crate::snapshot::list(&base)
        .iter()
        .map(|(_, m)| {
            json!({
                "id": m.id,
                "short": crate::ids::short(&m.id),
                "kind": "snapshot",
                "words": "start of run, in the folder",
                "created": m.started,
                "harness": m.harness,
                "files": m.files.len(),
                "bytes": m.bytes,
                "shared": m.reflink,
            })
        })
        .collect();
    Ok(json!({
        "api_version": BACKUPS,
        "project": project,
        "pointer": pointer_part(env, cfg, project),
        "backups": backups,
        "snapshots": snapshots,
    }))
}

/// One file of a backup: `file` or `link` with its size, or `absent`
/// (the path had no file then, and a restore removes it).
fn backup_file_doc(path: &str, size: Option<u64>, link: bool) -> Value {
    json!({
        "path": path,
        "size": size,
        "kind": match (size, link) {
            (None, _) => "absent",
            (Some(_), true) => "link",
            (Some(_), false) => "file",
        },
    })
}

/// `paddock backups --files ID --json`: the files of one backup of the
/// store, or of one snapshot of a direct run, in path order.
pub fn backup_files_doc(env: &HostEnv, cfg: &Config, project: &Path, id: &str) -> Result<Value> {
    let st = pointer::status(env, cfg, project, true)?;
    let found = match (&st.store, st.state.ok()) {
        (Some(s), true) => backup::find(s, id).ok(),
        _ => None,
    };
    let (doc, files): (Value, Vec<Value>) = match found {
        Some(b) => (
            backup_doc(&b),
            b.meta
                .files
                .iter()
                .map(|(p, e)| {
                    let e = e.as_ref();
                    backup_file_doc(p, e.map(|e| e.size), e.is_some_and(|e| e.link.is_some()))
                })
                .collect(),
        ),
        None => {
            let base = crate::snapshot::project_dir(&crate::snapshot::root(env), project);
            let snaps = crate::snapshot::list(&base);
            let m = match crate::ids::find(id, &snaps, |(_, m)| &m.id) {
                Ok(Some((_, m))) => m.clone(),
                Ok(None) => anyhow::bail!("no backup {id}. `paddock backups` lists them"),
                Err(e) => anyhow::bail!("{}", format!("{e:#}").replace("sandbox id", "backup id")),
            };
            (
                json!({
                    "id": m.id,
                    "short": crate::ids::short(&m.id),
                    "kind": "snapshot",
                    "created": m.started,
                    "harness": m.harness,
                }),
                m.files
                    .iter()
                    .map(|(p, f)| backup_file_doc(p, Some(f.size), f.link.is_some()))
                    .collect(),
            )
        }
    };
    Ok(json!({
        "api_version": BACKUP_FILES,
        "project": project,
        "backup": doc,
        "files": files,
    }))
}

/// `paddock restore ID --plan --json`: what the restore would change,
/// and nothing changed. `write`: paths that come back with the content of
/// the backup. `remove`: paths that leave the project. `keep`: paths that
/// stay as they are, newer than a full backup (`--exact` removes them).
pub fn restore_plan_doc(
    project: &Path,
    b: &Backup,
    plan: &super::restore::Plan,
    exact: bool,
) -> Value {
    let mut keep = plan.newer.clone();
    keep.extend(plan.changed_since.iter().cloned());
    keep.sort();
    json!({
        "api_version": RESTORE_PLAN,
        "project": project,
        "of": b.meta.id,
        "kind": b.meta.kind.tag(),
        "exact": exact,
        "write": plan.restore,
        "remove": plan.remove,
        "keep": keep,
    })
}

/// The result of an action, with its `api_version`.
pub fn result(api: &str, run: Option<&Run>, body: impl serde::Serialize) -> Value {
    let mut v = serde_json::to_value(body).unwrap_or(Value::Null);
    if !v.is_object() {
        v = json!({ "paths": v });
    }
    v["api_version"] = api.into();
    if let Some(r) = run {
        v["run"] = r.rec.id.clone().into();
        v["project"] = json!(r.rec.project);
    }
    v
}
