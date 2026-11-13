//! The commands of a run on a copy of the project: `paddock changes`,
//! `apply`, `keep`, `backups`, `restore` and `pointer`, and the copy part
//! of `paddock discard` and `paddock undo`.

use anyhow::{bail, Result};
use clap::Subcommand;
use paddock::copy::apply::{self, OnConflict, Opts};
use paddock::copy::changes::{self, Status};
use paddock::copy::run::Run;
use paddock::copy::{json, Config};
use paddock::env::HostEnv;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

mod backups;
mod pointer;
pub(crate) use backups::undo;
use backups::{backup_files_cmd, backups_cmd, restore_cmd, Go};
use pointer::{pointer_cmd, PointerCmd};

pub(crate) const HELP: &str = "A run with --copy works on a copy of the project. Your folder changes only when you apply.\n  paddock changes                 the runs that work on a copy\n  paddock changes 1a2b3c          what the run changed: new, changed, deleted, conflict\n  paddock changes 1a2b3c --diff   the changes as a diff\n  paddock apply 1a2b3c            write all changes to your folder, after a backup\n  paddock apply 1a2b3c src/       write only this folder\n  paddock keep 1a2b3c notes.txt   leave a file out of an apply of everything\n  paddock discard 1a2b3c src/a.rs drop a change from the copy\n  paddock undo                    take the last apply back\n  paddock backups                 the backups of this project\nA conflict is a file that you and the agent both changed. paddock never overwrites it: choose with --on-conflict original, copy, both or merge.";

#[derive(Subcommand)]
pub(crate) enum CopyCmd {
    /// List what a run changed in its copy of the project. Without a run:
    /// the runs that work on a copy.
    #[command(after_help = HELP)]
    Changes {
        /// A sandbox id from `paddock ps` or `paddock changes`, or its
        /// short form such as 1a2b3c.
        run: Option<String>,
        /// Print JSON (api_version paddock.changes/v1, or paddock.copies/v1
        /// without a run). A conflict that merges clean has
        /// `merge_preview`, the merged text (at most 64 KiB), and
        /// `merge_preview_cut`, true when the text was cut.
        #[arg(long)]
        json: bool,
        /// With --json: leave out what reads the files (the line counts,
        /// the text mark and the merge state), for a list that is asked
        /// for often.
        #[arg(long, requires = "json")]
        brief: bool,
        /// Show the changes as a diff: all, or only these files or folders.
        #[arg(long, value_name = "PATH", num_args = 0.., requires = "run")]
        diff: Option<Vec<String>>,
    },
    /// Write changes of a copy into your folder. paddock makes a backup
    /// first, and `paddock undo` takes the apply back.
    #[command(after_help = HELP)]
    Apply {
        /// A sandbox id, or its short form such as 1a2b3c.
        run: String,
        /// Files or folders. Without them: every change that is not kept.
        paths: Vec<String>,
        /// What to do with a file that both sides changed: stop (leave
        /// it), original (keep yours), copy (take the agent's), both (put
        /// the agent's next to yours), merge (when it has no conflict).
        #[arg(long, value_name = "CHOICE", default_value = "stop", value_parser = ["stop", "original", "copy", "both", "merge"])]
        on_conflict: String,
        /// Apply only when the list of changes is still the one with this
        /// token (`token` of `paddock changes --json`).
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
        /// Apply without a question.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print JSON (api_version paddock.apply/v1). Needs --yes.
        #[arg(long, requires = "yes")]
        json: bool,
    },
    /// Keep changes in the copy: an apply of everything leaves them out.
    Keep {
        /// A sandbox id, or its short form such as 1a2b3c.
        run: String,
        /// Files or folders with changes.
        #[arg(required = true)]
        paths: Vec<String>,
        /// Take the mark away again.
        #[arg(long)]
        off: bool,
        /// Print JSON (api_version paddock.keep/v1).
        #[arg(long)]
        json: bool,
    },
    /// List the backups of a project: the start of each run on a copy, and
    /// the state before each apply and restore.
    Backups {
        /// The project folder. Without it: the current one.
        project: Option<PathBuf>,
        /// List the files of this backup instead: each path with its
        /// size and kind (`file`, `link`, or `absent` when the backup has
        /// no file there). A backup id or its short form.
        #[arg(long, value_name = "BACKUP")]
        files: Option<String>,
        /// Print JSON (api_version paddock.backups/v1, or
        /// paddock.backup-files/v1 with --files).
        #[arg(long)]
        json: bool,
    },
    /// Bring files of the project back to a backup. paddock makes a backup
    /// of the replaced files first, so `paddock undo` takes a restore back.
    #[command(group = clap::ArgGroup::new("go").args(["yes", "plan"]).multiple(true))]
    Restore {
        /// A backup id from `paddock backups`, or its short form.
        backup: String,
        /// Files or folders. Without them: every file of the backup.
        paths: Vec<String>,
        /// The project folder. Without it: the current one.
        #[arg(long, value_name = "DIR")]
        project: Option<PathBuf>,
        /// Also remove the files that are newer than the backup.
        #[arg(long)]
        exact: bool,
        /// Restore without a question.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Show what the restore changes, and change nothing: the paths
        /// that it writes, removes and keeps.
        #[arg(long, conflicts_with = "yes")]
        plan: bool,
        /// Print JSON (api_version paddock.restore/v1, or
        /// paddock.restore-plan/v1 with --plan). Needs --yes or --plan.
        #[arg(long, requires = "go")]
        json: bool,
    },
    /// The pointer of a project: `.paddock/pointer.json` names the store
    /// with its copies and backups.
    Pointer {
        #[command(subcommand)]
        cmd: PointerCmd,
    },
}

fn out(v: &serde_json::Value) {
    println!("{v}");
}

/// Ask on the terminal. Without one, the command needs `--yes`.
fn confirm(question: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        bail!("no terminal to ask; run the command again with --yes");
    }
    use paddock::setup::Ask as _;
    Ok(paddock::setup::Terminal.yes_no(question, false))
}

fn project_of(path: Option<&Path>) -> Result<PathBuf> {
    paddock::build::resolve_project(path)
}

/// A direct sandbox writes the project: a restore would fight it.
fn no_direct_run(env: &HostEnv, project: &Path) -> Result<()> {
    let direct = paddock::state::Running::list(env)
        .into_iter()
        .find(|r| r.project == project && paddock::copy::run::of_id(env, &r.id).is_none());
    if let Some(r) = direct {
        bail!(
            "the sandbox {} works in this folder itself; stop it first (`paddock stop {}`)",
            r.id,
            paddock::ids::short(&r.id)
        );
    }
    Ok(())
}

pub(crate) fn run(env: &HostEnv, cmd: CopyCmd) -> Result<i32> {
    let cfg = Config::load(env)?;
    match cmd {
        CopyCmd::Changes {
            run: None, json, ..
        } => list_runs(env, &cfg, json),
        CopyCmd::Changes {
            run: Some(id),
            json,
            brief,
            diff,
        } => changes_cmd(env, &cfg, &id, json.then_some(!brief), diff),
        CopyCmd::Apply {
            run,
            paths,
            on_conflict,
            token,
            yes,
            json,
        } => {
            let o = Opts {
                paths,
                on_conflict: OnConflict::parse(&on_conflict)?,
                token,
            };
            apply_cmd(env, &cfg, &run, o, yes, json)
        }
        CopyCmd::Keep {
            run,
            paths,
            off,
            json,
        } => {
            let mut r = paddock::copy::run::find(env, &cfg, &run)?;
            let done = apply::keep(&mut r, &paths, !off)?;
            if json {
                out(&json::result(
                    json::KEEP,
                    Some(&r),
                    serde_json::json!({ "kept": !off, "paths": done }),
                ));
            } else if off {
                println!(
                    "{} file(s) are part of an apply of everything again.",
                    done.len()
                );
            } else {
                println!(
                    "Kept {} file(s) in the copy. `paddock apply {}` leaves them out.",
                    done.len(),
                    paddock::ids::short(&r.rec.id)
                );
            }
            Ok(0)
        }
        CopyCmd::Backups {
            project,
            files: Some(id),
            json,
        } => backup_files_cmd(env, &cfg, project.as_deref(), &id, json),
        CopyCmd::Backups {
            project,
            files: None,
            json,
        } => backups_cmd(env, &cfg, project.as_deref(), json),
        CopyCmd::Restore {
            backup,
            paths,
            project,
            exact,
            yes,
            plan,
            json,
        } => restore_cmd(
            env,
            &cfg,
            project.as_deref(),
            &backup,
            &paths,
            exact,
            if plan { Go::Plan } else { Go::Restore { yes } },
            json,
        ),
        CopyCmd::Pointer { cmd } => pointer_cmd(env, &cfg, cmd),
    }
}

fn list_runs(env: &HostEnv, cfg: &Config, as_json: bool) -> Result<i32> {
    let runs: Vec<(Run, Option<Status>)> = paddock::copy::run::all(env, cfg)
        .into_iter()
        .filter(|r| r.has_tree())
        .take(64)
        .map(|r| {
            let st = changes::status(&r).ok();
            (r, st)
        })
        .collect();
    if as_json {
        out(&json::copies_doc(env, &runs));
        return Ok(0);
    }
    if runs.is_empty() {
        println!("No run works on a copy. `paddock run --copy` starts one, and `paddock backups` lists the backups of this project.");
        return Ok(0);
    }
    for (r, st) in &runs {
        println!(
            "{:<8} {:<8} {:<6} {:<40} {}",
            paddock::ids::short(&r.rec.id),
            r.rec.harness,
            json::run_state(env, r),
            match st {
                Some(st) if st.changes.is_empty() => "no open change".to_string(),
                Some(st) => st.counts_line(),
                None => "cannot read the changes".to_string(),
            },
            r.rec.project.display()
        );
    }
    Ok(0)
}

/// One line per change: the mark, the word, the path, and why.
fn render(env: &HostEnv, run: &Run, st: &Status) -> String {
    let short = paddock::ids::short(&run.rec.id);
    let mut t = format!(
        "{} · {} · {} · copy: {}\n",
        run.rec.harness,
        run.rec.project.display(),
        json::run_state(env, run),
        if st.changes.is_empty() {
            "no open change".to_string()
        } else {
            st.counts_line()
        }
    );
    if !st.tree {
        t.push_str("The copy is gone. The backups are still here: `paddock backups`.\n");
        return t;
    }
    for c in &st.changes {
        let (glyph, word) = match (c.conflict, c.kept) {
            (Some(_), _) => ('!', "conflict"),
            (None, true) => ('=', "kept"),
            (None, false) => (c.kind.glyph(), c.kind.word()),
        };
        let note = match (c.conflict, c.writing) {
            (Some(k), _) => format!("  {}", k.text()),
            (None, true) => "  writing".to_string(),
            _ => String::new(),
        };
        t.push_str(&format!("  {glyph} {word:<9} {}{note}\n", c.path));
    }
    for p in &st.applied {
        t.push_str(&format!("  ✓ applied   {p}\n"));
    }
    if !st.behind.is_empty() {
        let names: Vec<&str> = st.behind.iter().take(5).map(|b| b.path.as_str()).collect();
        t.push_str(&format!(
            "Your folder has {} change(s) of its own that the copy does not have: {}{}\n",
            st.behind.len(),
            names.join(", "),
            if st.behind.len() > 5 { ", ..." } else { "" }
        ));
    }
    if !st.special.is_empty() {
        t.push_str(&format!(
            "Not listed (no file, folder or symlink): {}\n",
            st.special.join(", ")
        ));
    }
    match (st.changes.is_empty(), st.applied.is_empty()) {
        (true, true) => {
            t.push_str("No changes yet. The agent works in its copy. Your folder stays as it is.\n")
        }
        (true, false) => t.push_str(
            "All changes are in your folder. `paddock undo` takes the last apply back.\n",
        ),
        _ => {
            t.push_str(&format!(
                "`paddock apply {short}` writes {} change(s) to your folder, after a backup. `paddock changes {short} --diff` shows them.\n",
                st.ready()
            ));
            if st.conflicts() > 0 {
                t.push_str(&format!(
                    "A conflict is never overwritten. Choose per file: `paddock apply {short} PATH --on-conflict original`, `copy`, `both` or `merge`.\n"
                ));
            }
        }
    }
    t
}

/// `as_json`: print JSON, with the details that read the files or without.
fn changes_cmd(
    env: &HostEnv,
    cfg: &Config,
    id: &str,
    as_json: Option<bool>,
    diff: Option<Vec<String>>,
) -> Result<i32> {
    let run = paddock::copy::run::find(env, cfg, id)?;
    let st = changes::status(&run)?;
    if let Some(detail) = as_json {
        out(&json::changes_doc(env, cfg, &run, &st, detail));
        return Ok(0);
    }
    let Some(paths) = diff else {
        print!("{}", render(env, &run, &st));
        return Ok(0);
    };
    let chosen: Vec<&changes::Change> = if paths.is_empty() {
        st.changes.iter().collect()
    } else {
        changes::select(&st, &paths)?
    };
    for c in chosen {
        match json::texts(&run, c) {
            Some((old, new)) => {
                print!("{}", paddock::copy::diff::unified(&c.path, &old, &new, 3))
            }
            None => println!(
                "{}: {}, no text to show (a binary file, a symlink, or above 1 MB). Your folder: {}. The copy: {}.",
                c.path,
                c.kind.word(),
                size_of(&c.original),
                size_of(&c.copy)
            ),
        }
    }
    Ok(0)
}

fn size_of(e: &Option<paddock::copy::Entry>) -> String {
    match e {
        None => "no file".into(),
        Some(e) => match &e.link {
            Some(t) => format!("a link to {t}"),
            None => paddock::storage::human(e.size),
        },
    }
}

fn apply_cmd(
    env: &HostEnv,
    cfg: &Config,
    id: &str,
    o: Opts,
    yes: bool,
    as_json: bool,
) -> Result<i32> {
    let mut run = paddock::copy::run::find(env, cfg, id)?;
    if !yes {
        let st = changes::status(&run)?;
        let chosen: Vec<&changes::Change> = if o.paths.is_empty() {
            st.changes.iter().filter(|c| !c.kept).collect()
        } else {
            changes::select(&st, &o.paths)?
        };
        if chosen.is_empty() {
            println!("Nothing to apply.");
            return Ok(0);
        }
        for c in chosen.iter().take(20) {
            let word = match c.conflict {
                Some(_) => "conflict",
                None => c.kind.word(),
            };
            println!("  {} {word:<9} {}", c.kind.glyph(), c.path);
        }
        if chosen.len() > 20 {
            println!("  and {} more", chosen.len() - 20);
        }
        let deletes = chosen
            .iter()
            .filter(|c| c.kind == changes::Kind::Deleted && c.conflict.is_none())
            .count();
        if deletes > 0 {
            println!("{deletes} file(s) leave your folder.");
        }
        println!("paddock makes a backup first. `paddock undo` takes this apply back.");
        let q = format!("Write these changes to {}?", run.rec.project.display());
        if !confirm(&q, false)? {
            return Ok(1);
        }
    }
    let done = apply::apply(env, &mut run, &o)?;
    let left = done.conflicts.len() + done.unsafe_links.len();
    if as_json {
        out(&json::result(json::APPLY, Some(&run), &done));
        return Ok(if left > 0 { 1 } else { 0 });
    }
    println!(
        "Applied {} file(s) to {}: {} written, {} deleted, {} merged.{}",
        done.count(),
        run.rec.project.display(),
        done.written.len(),
        done.deleted.len(),
        done.merged.len(),
        if done.backup.is_some() {
            " `paddock undo` takes it back."
        } else {
            ""
        }
    );
    for (p, aside) in &done.both {
        println!("Both versions of {p}: yours stays, the agent's is {aside}.");
    }
    if !done.kept_original.is_empty() {
        println!(
            "Your version stays, and the change stays in the copy: {}",
            done.kept_original.join(", ")
        );
    }
    if !done.kept.is_empty() {
        println!("Kept in the copy, not applied: {}", done.kept.join(", "));
    }
    if !done.unsafe_links.is_empty() {
        println!(
            "Not written: symlinks that lead out of the project: {}",
            done.unsafe_links.join(", ")
        );
    }
    if !done.conflicts.is_empty() {
        println!(
            "Not written: {} conflict(s): {}\nChoose with --on-conflict original, copy, both or merge.",
            done.conflicts.len(),
            done.conflicts.join(", ")
        );
    }
    Ok(if left > 0 { 1 } else { 0 })
}

/// `paddock discard` for a run on a copy. None: the run has no copy, and
/// the command means the files of the sandbox home.
pub(crate) fn discard(
    env: &HostEnv,
    id: &str,
    paths: &[String],
    yes: bool,
    as_json: bool,
) -> Result<Option<i32>> {
    let cfg = Config::load(env)?;
    let Ok(mut run) = paddock::copy::run::find(env, &cfg, id) else {
        return Ok(None);
    };
    if paths.is_empty() {
        let st = changes::status(&run)?;
        let q = format!(
            "Remove the whole copy of {} with {} open change(s)? Your folder and the backups stay.",
            run.rec.id,
            st.changes.len()
        );
        if !confirm(&q, yes)? {
            return Ok(Some(1));
        }
        apply::discard_all(env, &mut run)?;
        if as_json {
            out(&json::result(
                json::DISCARD,
                Some(&run),
                serde_json::json!({ "copy_removed": true, "paths": [] }),
            ));
        } else {
            println!(
                "Removed the copy of {}. Your folder did not change.",
                run.rec.id
            );
        }
        return Ok(Some(0));
    }
    // No path names a change of the copy: the paths are files of the
    // sandbox home, the older meaning of the command.
    let st = changes::status(&run)?;
    let known = |p: &String| {
        changes::clean_path(p)
            .is_ok_and(|s| st.changes.iter().any(|c| changes::covers(&s, &c.path)))
    };
    if !paths.iter().any(known) {
        return Ok(None);
    }
    let q = format!(
        "Drop the changes of {} from the copy? This has no undo.",
        paths.join(", ")
    );
    if !confirm(&q, yes)? {
        return Ok(Some(1));
    }
    let done = apply::discard(&mut run, paths)?;
    if as_json {
        out(&json::result(
            json::DISCARD,
            Some(&run),
            serde_json::json!({ "copy_removed": false, "paths": done }),
        ));
    } else {
        println!(
            "Dropped {} change(s) from the copy. Your folder did not change.",
            done.len()
        );
    }
    Ok(Some(0))
}
