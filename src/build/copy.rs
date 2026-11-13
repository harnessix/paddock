//! The build step of a run on a copy of the project (copy.rs).

use super::{Built, Options};
use crate::copy::start::{self, Why};
use crate::copy::{run::Run, Config, CopyOf};
use crate::env::HostEnv;
use crate::manifest::Manifest;
use crate::profile::Profile;
use crate::spec::{Mount, SandboxSpec};
use anyhow::Result;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

/// What the build decided about a copy.
#[derive(Debug, Clone, Default)]
pub struct Copy {
    /// The copy of this run.
    pub run: Option<Run>,
    /// Why the run works on a copy.
    pub why: Option<String>,
    /// `paddock explain`: what a run would do. Nothing was made.
    pub plan: Option<String>,
    /// Lines for the user about the pointer and the copy.
    pub notices: Vec<String>,
    /// The run comes from a launcher entry.
    pub launcher: bool,
}

/// The copy of a run while the build can still fail.
pub(super) struct PendingCopy {
    out: Copy,
    /// A copy at the path of the project is made at the end of the build
    /// (`finish`): the detectors put mount points into the project first,
    /// and the copy takes them along.
    late: Option<Late>,
}

struct Late {
    cfg: Config,
    store: crate::copy::store::Store,
    original: PathBuf,
}

impl Drop for PendingCopy {
    fn drop(&mut self) {
        if let Some(run) = self.out.run.take() {
            start::discard_failed(&run);
        }
    }
}

/// What makes a copy impossible for this run.
fn blocked(
    env: &HostEnv,
    opts: &Options,
    profile: &Profile,
    backend: &str,
    project: &Path,
) -> Option<&'static str> {
    use crate::loft::sync::Mode;
    if opts.child.is_some() {
        Some("a run that another sandbox started")
    } else if opts.worktree {
        Some("--worktree, which has its own copy on a branch")
    } else if opts.transparent.is_some() {
        Some("paddock wrap")
    } else if profile.app.is_some() {
        Some("a desktop app")
    } else if backend == "vm" {
        Some("the vm backend")
    } else if std::fs::symlink_metadata(project.join(".git")).is_ok_and(|m| !m.is_dir()) {
        // The git directory is in another folder: git in a copy would
        // write the index and the branch of the real work tree.
        Some("a linked git worktree or a submodule")
    } else if crate::loft::sync::mode(env, project) == Mode::Sync {
        Some("[loft] mode = \"sync\", which keeps the notes in the project folder")
    } else {
        None
    }
}

impl PendingCopy {
    fn none() -> Self {
        PendingCopy {
            out: Copy::default(),
            late: None,
        }
    }

    /// Decide. A copy at its own path is made now: the build reads the
    /// project there. A copy at the path of the project is made by `finish`.
    pub(super) fn start(
        env: &HostEnv,
        opts: &Options,
        profile: &Profile,
        m: &Manifest,
        project: &Path,
        id: &str,
    ) -> Result<Self> {
        if !opts.copy.allowed {
            return Ok(Self::none());
        }
        let cfg = Config::load(env)?;
        let backend = opts
            .backend
            .clone()
            .or(m.backend.clone())
            .unwrap_or_else(|| "auto".into());
        let why = start::decide(
            &cfg,
            &opts.copy,
            start::is_git(project),
            blocked(env, opts, profile, &backend, project),
        )?;
        let Some(why) = why else {
            return Ok(Self::none());
        };
        let network = opts.network.or(m.network.mode).unwrap_or_default();
        let in_place = start::in_place(&backend, network, cfg!(target_os = "macos"));
        let mut out = Copy {
            why: Some(why.text().to_string()),
            launcher: opts.copy.launcher,
            ..Default::default()
        };
        if opts.copy.plan_only {
            out.plan = Some(start::explain_line(why, &cfg, env, in_place));
            return Ok(PendingCopy { out, late: None });
        }
        // A detached run has a terminal that nobody reads: it never asks.
        let attended = opts.detach.is_none();
        let mut ask = |_: crate::copy::pointer::State, q: &str| {
            use crate::setup::Ask as _;
            attended
                && std::io::stdin().is_terminal()
                && std::io::stderr().is_terminal()
                && crate::setup::Terminal.yes_no(q, true)
        };
        // The user did not ask for a copy: an error names the way out.
        let hint = |e: anyhow::Error| {
            match why {
            Why::Flag => e,
            _ => e.context(format!(
                "paddock starts this run on a copy of the project, because of {}, and cannot make the copy. `--direct` works in the folder itself, and [copy] default = \"never\" in the user configuration makes that the default",
                why.text()
            )),
        }
        };
        let (store, notices) =
            crate::copy::pointer::ensure(env, &cfg, project, &mut ask).map_err(hint)?;
        out.notices = notices;
        if in_place {
            let late = Some(Late {
                cfg,
                store,
                original: project.to_path_buf(),
            });
            return Ok(PendingCopy { out, late });
        }
        let s = start::fill(env, &cfg, store, project, (id, &profile.name), false).map_err(hint)?;
        out.notices.extend(s.notices);
        out.run = Some(s.run);
        Ok(PendingCopy { out, late: None })
    }

    /// The path that the sandbox works at.
    pub(super) fn work(&self, project: PathBuf) -> PathBuf {
        match &self.out.run {
            Some(r) => start::work_path(r),
            None => project,
        }
    }

    fn record(&self, spec: &mut SandboxSpec) {
        let Some(r) = &self.out.run else {
            return;
        };
        spec.notices.extend(self.out.notices.iter().cloned());
        spec.copy = Some(CopyOf {
            original: r.rec.project.clone(),
            tree: r.tree(),
            in_place: r.rec.in_place,
            store: r.rec.store.clone(),
            why: self.out.why.clone().unwrap_or_default(),
        });
    }

    /// The project mount. A copy at its own path is the project of the
    /// sandbox. For a copy at the path of the project, `finish` moves the
    /// mount to the copy.
    pub(super) fn mount(&self, spec: &mut SandboxSpec, project: &Path) {
        spec.add_mount(Mount::same(project, true, "project"));
        self.record(spec);
    }

    /// The end of the build, for a copy at the path of the project: make
    /// the copy, and take every mount below the project from the copy. So
    /// nothing of the sandbox writes the folder of the user, also not git.
    pub(super) fn finish(&mut self, env: &HostEnv, spec: &mut SandboxSpec) -> Result<()> {
        let Some(l) = self.late.take() else {
            return Ok(());
        };
        let run = (spec.id.as_str(), spec.harness.as_str());
        let s = start::fill(env, &l.cfg, l.store, &l.original, run, true)?;
        let tree = s.run.tree();
        self.out.notices.extend(s.notices);
        self.out.run = Some(s.run);
        for m in spec.mounts.iter_mut() {
            let Some(rel) = m
                .src
                .as_ref()
                .and_then(|p| p.strip_prefix(&l.original).ok())
            else {
                continue;
            };
            let to = tree.join(rel);
            source_in_copy(&l.original.join(rel), &to)?;
            if rel.as_os_str().is_empty() {
                m.origin = "project: a copy; your folder changes only at `paddock apply`".into();
            }
            m.src = Some(to);
        }
        self.record(spec);
        Ok(())
    }

    pub(super) fn keep(&mut self) -> Copy {
        std::mem::take(&mut self.out)
    }
}

/// A mount source below the project must exist in the copy too. Most do:
/// the copy was made after the mount points. One in a part that the copy
/// leaves out (build output without shared data) is made empty here.
fn source_in_copy(original: &Path, to: &Path) -> Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Ok(());
    }
    let Ok(m) = std::fs::symlink_metadata(original) else {
        return Ok(());
    };
    if m.is_dir() {
        std::fs::create_dir_all(to)?;
    } else if m.is_file() {
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::copy(original, to)?;
    }
    Ok(())
}

impl Built {
    /// The run gets a snapshot of the project for `paddock undo`. A
    /// worktree run has its own branch, a spawned child shares the project
    /// of its parent, and a run on a copy has the backup of its start.
    pub fn takes_snapshot(&self) -> bool {
        self.worktree.is_none()
            && self.spawn.is_none()
            && self.parent.is_none()
            && self.copy.run.is_none()
    }
}

#[cfg(test)]
#[path = "copy_tests.rs"]
mod tests;
