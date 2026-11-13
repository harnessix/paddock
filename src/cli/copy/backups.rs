//! `paddock backups`, `paddock restore`, and the copy part of `paddock
//! undo`.

use super::{confirm, no_direct_run, out, project_of};
use anyhow::{bail, Result};
use paddock::copy::pointer;
use paddock::copy::{backup, json, restore, Config};
use paddock::env::HostEnv;
use std::path::Path;

pub(super) fn backups_cmd(
    env: &HostEnv,
    cfg: &Config,
    project: Option<&Path>,
    as_json: bool,
) -> Result<i32> {
    let project = project_of(project)?;
    let doc = json::backups_doc(env, cfg, &project)?;
    if as_json {
        out(&doc);
        return Ok(0);
    }
    let rows = doc["backups"].as_array().cloned().unwrap_or_default();
    let snaps = doc["snapshots"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() && snaps.is_empty() {
        println!("No backups yet. paddock makes one at the start of each run on a copy and before each apply.");
        return Ok(0);
    }
    for b in rows.iter().chain(&snaps) {
        let size = if b["shared"] == true {
            "shared".to_string()
        } else {
            paddock::storage::human(b["bytes"].as_u64().unwrap_or(0))
        };
        println!(
            "{}  {:<27} {:>6} files  {:>9}  {:<8} {}{}",
            paddock::storage::local_time(b["created"].as_u64().unwrap_or(0)),
            b["words"].as_str().unwrap_or(""),
            b["files"].as_u64().unwrap_or(0),
            size,
            b["harness"].as_str().unwrap_or(""),
            b["short"].as_str().unwrap_or(""),
            if b["undone"].is_u64() { "  undone" } else { "" }
        );
    }
    println!("`paddock restore ID` brings the files of a backup back. `paddock undo` takes the last apply back.");
    if !snaps.is_empty() {
        println!("A backup \"in the folder\" is the snapshot of a run without a copy: `paddock undo ID` restores it.");
    }
    Ok(0)
}

/// `paddock backups --files ID`: the files of one backup.
pub(super) fn backup_files_cmd(
    env: &HostEnv,
    cfg: &Config,
    project: Option<&Path>,
    id: &str,
    as_json: bool,
) -> Result<i32> {
    let project = project_of(project)?;
    let doc = json::backup_files_doc(env, cfg, &project, id)?;
    if as_json {
        out(&doc);
        return Ok(0);
    }
    let files = doc["files"].as_array().cloned().unwrap_or_default();
    println!(
        "The backup {} has {} path(s):",
        doc["backup"]["id"].as_str().unwrap_or(id),
        files.len()
    );
    for f in &files {
        let size = match f["size"].as_u64() {
            Some(n) => paddock::storage::human(n),
            None => "-".to_string(),
        };
        println!(
            "  {:<6} {:>9}  {}",
            f["kind"].as_str().unwrap_or(""),
            size,
            f["path"].as_str().unwrap_or("")
        );
    }
    Ok(0)
}

/// What `paddock restore` does after the plan.
pub(crate) enum Go {
    /// `--plan`: print the plan and change nothing.
    Plan,
    /// Restore, after a question unless `yes`.
    Restore { yes: bool },
}

fn usable_store(
    env: &HostEnv,
    cfg: &Config,
    project: &Path,
) -> Result<paddock::copy::store::Store> {
    let st = pointer::status(env, cfg, project, true)?;
    match (st.state.ok(), st.store) {
        (true, Some(s)) => Ok(s),
        _ => bail!(
            "{} {}",
            st.state.text(),
            match st.state.action() {
                Some(a) => format!("Run `{a}` in the folder first."),
                None => "This folder has no backups.".to_string(),
            }
        ),
    }
}

fn print_plan(plan: &restore::Plan) {
    let show = |label: &str, v: &[String]| {
        if !v.is_empty() {
            let names: Vec<&str> = v.iter().take(10).map(String::as_str).collect();
            println!(
                "  {} {label}: {}{}",
                v.len(),
                names.join(", "),
                if v.len() > 10 { ", ..." } else { "" }
            );
        }
    };
    show("come back", &plan.restore);
    show("leave the folder", &plan.remove);
    show(
        "changed since, and stay (--force restores them)",
        &plan.changed_since,
    );
    show(
        "are newer than the backup, and stay (--exact removes them)",
        &plan.newer,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn restore_cmd(
    env: &HostEnv,
    cfg: &Config,
    project: Option<&Path>,
    id: &str,
    paths: &[String],
    exact: bool,
    go: Go,
    as_json: bool,
) -> Result<i32> {
    let project = project_of(project)?;
    let store = usable_store(env, cfg, &project)?;
    let b = backup::find(&store, id)?;
    let yes = match go {
        Go::Plan => {
            let plan = restore::plan(&b, &project, paths, exact)?;
            if as_json {
                out(&json::restore_plan_doc(&project, &b, &plan, exact));
            } else {
                println!(
                    "A restore of the backup {} ({}, {}) changes this, and nothing changed now:",
                    b.meta.id,
                    b.meta.kind.words(),
                    paddock::storage::local_time(b.meta.created)
                );
                print_plan(&plan);
            }
            return Ok(0);
        }
        Go::Restore { yes } => yes,
    };
    no_direct_run(env, &project)?;
    let plan = restore::plan(&b, &project, paths, exact)?;
    if plan.is_empty() {
        if as_json {
            out(&json::result(
                json::RESTORE,
                None,
                restore::Outcome::default(),
            ));
        } else {
            println!("The folder is the same as the backup {}.", b.meta.id);
            print_plan(&plan);
        }
        return Ok(0);
    }
    if !as_json {
        println!(
            "Restore the backup {} ({}, {}):",
            b.meta.id,
            b.meta.kind.words(),
            paddock::storage::local_time(b.meta.created)
        );
        print_plan(&plan);
        println!(
            "paddock makes a backup of these files first. `paddock undo` takes the restore back."
        );
    }
    if !confirm(
        &format!("Restore these files in {}?", project.display()),
        yes,
    )? {
        return Ok(1);
    }
    let done = restore::restore(&store, &b, &project, &plan, false)?;
    if as_json {
        let mut v = json::result(json::RESTORE, None, &done);
        v["project"] = serde_json::json!(project);
        v["newer"] = serde_json::json!(plan.newer);
        out(&v);
    } else {
        println!(
            "Restored {} file(s), removed {}.",
            done.restored.len(),
            done.removed.len()
        );
    }
    Ok(0)
}

/// `paddock undo` for a project with applies. None: the project has no
/// apply to take back, or the snapshot of a direct run is newer: the
/// snapshot undo of `snapshot.rs` runs.
pub(crate) fn undo(
    env: &HostEnv,
    list: bool,
    yes: bool,
    force: bool,
    as_json: bool,
) -> Result<Option<i32>> {
    let cfg = Config::load(env)?;
    let project = project_of(None)?;
    let Ok(st) = pointer::status(env, &cfg, &project, true) else {
        return Ok(None);
    };
    let (true, Some(store)) = (st.state.ok(), st.store) else {
        return Ok(None);
    };
    if list {
        // The applies and restores first, then the snapshots (the caller).
        for b in backup::list(&store).iter().filter(|b| b.undoable()) {
            println!(
                "{}  {:<14} {:>6} files  {}",
                paddock::storage::local_time(b.meta.created),
                b.meta.kind.words(),
                b.meta.files.len(),
                paddock::ids::short(&b.meta.id)
            );
        }
        return Ok(None);
    }
    let Some(b) = restore::last_undoable(&store) else {
        return Ok(None);
    };
    let snaps = paddock::snapshot::list(&paddock::snapshot::project_dir(
        &paddock::snapshot::root(env),
        &project,
    ));
    if snaps
        .first()
        .is_some_and(|(_, m)| m.started > b.meta.created)
    {
        return Ok(None);
    }
    no_direct_run(env, &project)?;
    let plan = restore::undo_plan(&b, &project, force)?;
    if !as_json {
        println!(
            "Take back: {} of {} ({}).",
            match b.meta.kind {
                backup::Kind::Apply => "the apply",
                _ => "the restore",
            },
            paddock::storage::local_time(b.meta.created),
            b.meta.run.as_deref().unwrap_or("no run")
        );
        print_plan(&plan);
    }
    if !plan.is_empty() && !confirm(&format!("Undo it in {}?", project.display()), yes)? {
        return Ok(Some(1));
    }
    let done = restore::undo(&store, b, &project, &plan)?;
    if as_json {
        let mut v = json::result(json::UNDO, None, &done);
        v["project"] = serde_json::json!(project);
        out(&v);
    } else {
        println!(
            "Restored {} file(s), removed {}.{}",
            done.restored.len(),
            done.removed.len(),
            if done.skipped.is_empty() {
                String::new()
            } else {
                format!(
                    " {} file(s) changed after it and stay: `paddock undo --force` restores them too.",
                    done.skipped.len()
                )
            }
        );
    }
    Ok(Some(0))
}
