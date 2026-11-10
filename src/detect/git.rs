//! The git detector: the git directories of the project, with read-only
//! binds over every file that git on the host reads or runs.

use super::{Context, Detector};
use crate::spec::{Mount, SandboxSpec};
use anyhow::{bail, Result};
use std::path::Path;

pub struct Git;

impl Git {
    /// Make a file of a git directory exist, so a read-only bind can cover
    /// it. Without the file the sandbox can create it later and point git at
    /// its own content. The git directory is sandbox-writable, so the path is
    /// walked without following a symlink, and an existing file is never
    /// replaced. A symlink or a special file in its place is an error: a bind
    /// of it would follow it on the host.
    fn placeholder(git_dir: &Path, rel: &str, content: &str) -> Result<()> {
        use crate::safefs::Kind;
        let root = crate::safefs::Root::open(git_dir)?;
        let (parent, name) = match rel.rsplit_once('/') {
            Some((p, n)) => (Path::new(p), n),
            None => (Path::new(""), rel),
        };
        let dir = root.dir(parent, true)?;
        match dir.kind(name.as_ref())? {
            Some(Kind::File) => Ok(()),
            Some(other) => bail!(
                "{} is a {:?}, not a file. paddock does not bind it, because git on the host reads it.",
                git_dir.join(rel).display(),
                other
            ),
            None => dir.write(name.as_ref(), content.as_bytes(), 0o644),
        }
    }
}

impl Detector for Git {
    fn name(&self) -> &'static str {
        "git"
    }
    fn apply(&self, cx: &Context, spec: &mut SandboxSpec) -> Result<()> {
        let Some(layout) = crate::hostgit::Layout::discover(cx.project)? else {
            return Ok(());
        };
        if !crate::paths::within(&layout.common_dir, cx.project) {
            // A linked worktree: commits write to the main repository's git directory.
            spec.add_mount(Mount::same(
                &layout.common_dir,
                true,
                "detector git: common dir of a linked worktree",
            ));
        }
        // The git directories of all worktrees are read-only, also the ones
        // that other sandboxes create after this one started. A sandbox must
        // not change what host git reads for another work tree. Only the git
        // directory of this worktree stays writable.
        let worktrees = layout.common_dir.join("worktrees");
        crate::safefs::Root::open(&layout.common_dir)?.dir(Path::new("worktrees"), true)?;
        spec.add_mount(Mount::same(
            &worktrees,
            false,
            "detector git: other worktrees belong to other work trees",
        ));
        if layout.linked && layout.git_dir.starts_with(&worktrees) {
            spec.add_mount(Mount::same(
                &layout.git_dir,
                true,
                "detector git: the git directory of this worktree",
            ));
        }
        // `.git` gets a mount point of its own. A mount point cannot be
        // renamed or replaced in the sandbox, so the read-only files below it
        // cannot be moved aside with `mv .git .git2 && mkdir .git`.
        if layout.linked {
            spec.add_mount(Mount::same(
                &layout.dot_git,
                false,
                "detector git: the .git file names the git directory",
            ));
        } else {
            spec.add_mount(Mount::same(
                &layout.dot_git,
                true,
                "detector git: the git directory is its own mount point",
            ));
        }
        for dir in layout.protected_dirs() {
            if dir != layout.dot_git && crate::paths::within(&dir, cx.project) {
                // A worktree or submodule git directory inside the project
                // gets its own mount point as well.
                spec.add_mount(Mount::same(
                    &dir,
                    true,
                    "detector git: a git directory is its own mount point",
                ));
            }
            // The hooks folder: created without following a symlink, so a
            // read-only bind of it cannot land anywhere else.
            let hooks = dir.join("hooks");
            crate::safefs::Root::open(&dir)?.dir(Path::new("hooks"), true)?;
            spec.add_mount(Mount::same(
                &hooks,
                false,
                "detector git: hooks run on the host",
            ));
            // Submodule git directories: one that the agent creates here
            // would get no binds, and the host's `git status` reads its
            // config when it recurses. The ones that exist now are mounted
            // read-write above, below this read-only folder.
            crate::safefs::Root::open(&dir)?.dir(Path::new("modules"), true)?;
            spec.add_mount(Mount::same(
                dir.join("modules"),
                false,
                "detector git: new submodule git directories steer host git",
            ));
            let linked = dir.join("commondir").is_file();
            for f in crate::hostgit::PROTECTED_FILES {
                let p = dir.join(f);
                // `gitdir` exists only in a registered worktree. Never write
                // one: its content is a path paddock must not invent.
                if *f == "gitdir" {
                    if p.is_file() {
                        spec.add_mount(Mount::same(
                            &p,
                            false,
                            "detector git: gitdir names the work tree",
                        ));
                    }
                    continue;
                }
                // "." means "this directory", which is what a git directory
                // without a common directory of its own needs.
                let content = if *f == "commondir" && !linked {
                    ".\n"
                } else {
                    ""
                };
                Self::placeholder(&dir, f, content)?;
                spec.add_mount(Mount::same(
                    &p,
                    false,
                    "detector git: config and pointer files steer host git",
                ));
            }
        }
        Ok(())
    }
}
