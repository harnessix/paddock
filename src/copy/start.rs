//! The start of a run on a copy: the decision, the backup and the working
//! copy.

use super::config::Default_;
use super::pointer::{self, State};
use super::run::{Run, RunRec};
use super::tree::{self, Method};
use super::{backup, Ask, Config};
use crate::env::HostEnv;
use crate::spec::Network;
use anyhow::{bail, Context as _, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Why a run works on a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// `--copy`.
    Flag,
    /// A run from a launcher entry.
    Launcher,
    /// The folder is no git work tree.
    NoGit,
    /// `[copy] default = "always"`.
    Config,
}

impl Why {
    pub fn text(self) -> &'static str {
        match self {
            Why::Flag => "--copy",
            Why::Launcher => "a launcher run",
            Why::NoGit => "the folder has no git history",
            Why::Config => "[copy] default = \"always\"",
        }
    }
}

/// Whether a run works on a copy. `blocked` names what makes a copy
/// impossible for this run: then `--copy` is an error, and every other
/// run works in the folder.
pub fn decide(cfg: &Config, ask: &Ask, git: bool, blocked: Option<&str>) -> Result<Option<Why>> {
    if !ask.allowed || ask.choice == Some(false) {
        return Ok(None);
    }
    if let Some(b) = blocked {
        if ask.choice == Some(true) {
            bail!("--copy does not work with {b}");
        }
        return Ok(None);
    }
    Ok(match (ask.choice, cfg.default) {
        (Some(true), _) => Some(Why::Flag),
        (_, Default_::Never) => None,
        (_, Default_::Always) => Some(Why::Config),
        (_, Default_::Auto) if ask.launcher => Some(Why::Launcher),
        (_, Default_::Auto) if !git => Some(Why::NoGit),
        _ => None,
    })
}

/// The backend shows the copy at the path of the project: it can mount.
/// Seatbelt and srt cannot, so the copy has its own path there.
pub fn in_place(backend: &str, network: Network, macos: bool) -> bool {
    match backend {
        "bwrap" | "docker" | "podman" => true,
        "auto" => !macos && network != Network::Allowlist,
        _ => false,
    }
}

/// The backend that was chosen in the end can show a copy in place.
pub fn mounts(backend: &str) -> bool {
    matches!(backend, "bwrap" | "docker" | "podman")
}

/// `project` is a git work tree or holds one: it has `.git`.
pub fn is_git(project: &Path) -> bool {
    std::fs::symlink_metadata(project.join(".git")).is_ok()
}

/// The rules of the copy: those of a snapshot, and never the pointer
/// folder.
pub fn rules(project: &Path) -> crate::snapshot::Skip {
    let mut skip = crate::snapshot::skip_rules(project, Duration::from_secs(20));
    skip.paths.insert(pointer::DIR.to_string());
    skip
}

/// What the start of a run made.
pub struct Started {
    pub run: Run,
    /// Lines for the user.
    pub notices: Vec<String>,
}

/// Make the copy of `project` for the run `id`: the pointer and the store
/// when they are missing, the backup, then the working copy. `ask` answers
/// a question about the pointer (pointer.rs).
pub fn create(
    env: &HostEnv,
    cfg: &Config,
    project: &Path,
    run: (&str, &str),
    in_place: bool,
    ask: &mut dyn FnMut(State, &str) -> bool,
) -> Result<Started> {
    let (store, mut notices) = pointer::ensure(env, cfg, project, ask)?;
    let mut started = fill(env, cfg, store, project, run, in_place)?;
    notices.append(&mut started.notices);
    started.notices = notices;
    Ok(started)
}

/// The second step of [`create`], with the store of the project: the
/// backup of the start, then the working copy.
pub fn fill(
    env: &HostEnv,
    cfg: &Config,
    store: super::store::Store,
    project: &Path,
    run: (&str, &str),
    in_place: bool,
) -> Result<Started> {
    let (id, harness) = run;
    if !super::run::valid_id(id) {
        bail!("invalid sandbox id {id}");
    }
    let mut notices = Vec::new();
    let _lock = store.lock()?;
    let skip = rules(project);
    let found =
        tree::scan(project, &skip).with_context(|| format!("cannot read {}", project.display()))?;
    let base = backup::start(&store, project, &found, run, &skip, cfg.max)?;
    let dir = store.runs().join(id);
    if dir.exists() {
        crate::shadow::remove_dir(&dir)?;
    }
    let tree_dir = super::run::tree_of(&dir);
    let made = (|| -> Result<RunRec> {
        crate::privfs::dir(&tree_dir)?;
        let manifest = base.manifest();
        // From the backup, not from the project: the copy starts as the
        // state that paddock compares with.
        let shared = base.meta.method.shared() && tree::shares(&base.files_dir(), &dir, &manifest);
        tree::clone_files(&base.files_dir(), &tree_dir, &manifest, shared)?;
        let carried = carry(project, &tree_dir, &dir, &found, cfg.max, base.meta.bytes)?;
        notes_folder(env, project, &tree_dir, in_place)?;
        Ok(RunRec {
            format: super::FORMAT,
            id: id.to_string(),
            harness: harness.to_string(),
            project: project.to_path_buf(),
            store: store.meta.id.clone(),
            base: base.meta.id.clone(),
            started: crate::state::now(),
            ended: None,
            removed: None,
            method: Method::of(shared),
            files: manifest.len(),
            bytes: base.meta.bytes,
            skip: skip.clone(),
            git: is_git(project),
            in_place,
            carried,
            kept: BTreeSet::new(),
            synced: BTreeMap::new(),
        })
    })();
    let rec = match made {
        Ok(r) => r,
        Err(e) => {
            let _ = crate::shadow::remove_dir(&dir);
            let _ = crate::shadow::remove_dir(&base.dir);
            return Err(e);
        }
    };
    let run = Run { dir, store, rec };
    run.save()?;
    super::end::sweep(env, &run.store, crate::state::now());
    backup::prune(&run.store, &super::run::needed_bases(&run.store));
    notices.push(start_line(&run));
    Ok(Started { run, notices })
}

/// The line that tells the user where the agent works.
pub fn start_line(run: &Run) -> String {
    let how = match run.rec.method {
        Method::Reflink | Method::Clone => "it shares its data with the project".to_string(),
        Method::Copy => format!("a plain copy of {}", crate::storage::human(run.rec.bytes)),
    };
    format!(
        "the agent works on a copy of {} ({} files, {how}). Your folder changes only when you apply: `paddock changes {id}` lists the changes, `paddock apply {id}` writes them",
        run.rec.project.display(),
        run.rec.files,
        id = crate::ids::short(&run.rec.id)
    )
}

/// Bring the parts outside the change list into the working copy: `.git`
/// always, build output only when it costs no bytes.
fn carry(
    project: &Path,
    tree_dir: &Path,
    run_dir: &Path,
    found: &tree::Scan,
    max: u64,
    used: u64,
) -> Result<Vec<String>> {
    let shared = tree::shares(project, run_dir, &found.files);
    let mut budget = max.saturating_sub(if shared { 0 } else { used });
    let mut carried = Vec::new();
    for rel in &found.left_out {
        if rel == pointer::DIR {
            continue;
        }
        let git = rel.rsplit('/').next() == Some(".git");
        if !git && !shared {
            continue;
        }
        match tree::carry(project, tree_dir, rel, shared, &mut budget) {
            Ok(()) => carried.push(rel.clone()),
            Err(e) if git => {
                return Err(e.context(format!(
                    "cannot copy {rel}. Start with --direct to work in the folder itself"
                )))
            }
            // Build output is a help, not a need.
            Err(e) => log::warn!("the copy starts without {rel}: {e:#}"),
        }
    }
    Ok(carried)
}

/// `.paddock` of the working copy leads to the notes of the project, not
/// to the pointer. In place, paddock mounts the notes on the empty folder
/// (loft.rs). At its own path, the folder is a link to the notes, as
/// `paddock loft init` makes it in a project.
fn notes_folder(env: &HostEnv, project: &Path, tree_dir: &Path, in_place: bool) -> Result<()> {
    let at = tree_dir.join(pointer::DIR);
    if in_place {
        return Ok(crate::privfs::dir(&at)?);
    }
    let notes = crate::loft::Loft::of(env, project).notes();
    crate::privfs::dir(&notes)?;
    std::os::unix::fs::symlink(&notes, &at)
        .with_context(|| format!("cannot make {}", at.display()))?;
    Ok(())
}

/// The plan of a run on a copy, before anything is made (`paddock
/// explain`).
pub fn explain_line(why: Why, cfg: &Config, env: &HostEnv, in_place: bool) -> String {
    format!(
        "a copy, because of {}. The store is in {}. The sandbox sees the copy {}. `--direct` works in the folder itself",
        why.text(),
        cfg.root(env).display(),
        if in_place {
            "at the path of the project"
        } else {
            "at its own path"
        }
    )
}

/// The working copy of the run `id` in the store of `project`, when the
/// folder is there. For the cleanup after a failed start.
pub fn discard_failed(run: &Run) {
    let _ = crate::shadow::remove_dir(&run.dir);
    let base = run.store.backups().join(&run.rec.base);
    let _ = crate::shadow::remove_dir(&base);
}

/// The path that the sandbox works at.
pub fn work_path(run: &Run) -> PathBuf {
    if run.rec.in_place {
        run.rec.project.clone()
    } else {
        run.tree()
    }
}
