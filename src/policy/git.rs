//! The git part of the policy: a writable mount that holds a git directory
//! must keep every file that git on the host reads or runs read-only.

use crate::spec::{MountKind, SandboxSpec};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Every writable mount that contains a git directory must also carry the
/// read-only overrides for its hooks, config and pointer files, and the git
/// directory must be a mount point of its own: a mount point cannot be
/// renamed, so the read-only entries below it cannot be moved aside. The
/// `worktrees` folder of a repository must be read-only too: other work
/// trees, also ones that start after this sandbox, keep their git
/// directories there.
pub(super) fn check(spec: &SandboxSpec) -> Result<()> {
    for m in spec.mounts.iter().filter(|m| m.kind == MountKind::Rw) {
        let src = m.src.as_ref().unwrap();
        // Where the sandbox sees a path of this mount. A copy of a project
        // is mounted at the path of the project (copy.rs), so the source
        // and the mount point differ.
        let seen = |p: &Path| match p.strip_prefix(src) {
            Ok(rel) => m.dst.join(rel),
            Err(_) => p.to_path_buf(),
        };
        for git_dir in git_dirs_under(src) {
            let own_mount = spec
                .mounts
                .iter()
                .any(|o| o.dst == seen(&git_dir) && o.kind == MountKind::Rw);
            if git_dir.starts_with(src) && git_dir != *src && !own_mount {
                bail!(
                    "policy: {} is not a mount point of its own inside {} (from {}). The sandbox could rename it and plant a new one.",
                    git_dir.display(),
                    src.display(),
                    m.origin
                );
            }
            let mut dirs = vec![git_dir.clone()];
            let mut needed = Vec::new();
            if git_dir.join("objects").is_dir() {
                needed.push(git_dir.join("worktrees"));
            }
            if let Ok(Some(l)) = crate::hostgit::Layout::discover(src) {
                needed.push(l.common_dir.join("worktrees"));
                dirs.extend(l.protected_dirs());
            }
            for dir in &dirs {
                needed.push(dir.join("hooks"));
                needed.extend(crate::hostgit::PROTECTED_FILES.iter().map(|f| dir.join(f)));
            }
            for p in needed {
                if !p.exists() || !p.starts_with(src) {
                    continue;
                }
                if writable(spec, &seen(&p)) {
                    bail!(
                        "policy: {} is writable inside {} (from {}). Git reads and runs it on the host.",
                        p.display(),
                        src.display(),
                        m.origin
                    );
                }
            }
        }
    }
    Ok(())
}

/// Whether the sandbox can write `p`: the deepest mount that holds it
/// decides.
fn writable(spec: &SandboxSpec, p: &Path) -> bool {
    spec.mounts
        .iter()
        .filter(|m| p.starts_with(&m.dst))
        .max_by_key(|m| m.dst.components().count())
        .is_none_or(|m| {
            !matches!(
                m.kind,
                MountKind::Ro | MountKind::MaskFile | MountKind::MaskDir
            )
        })
}

/// The git directories at the top of `root`: `.git`, `root` itself when it
/// is a repository, or `root` itself when it is the git directory of a
/// linked worktree.
fn git_dirs_under(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let top = root.join(".git");
    if top.is_dir() {
        v.push(top);
    }
    let head = root.join("HEAD").is_file();
    if head && (root.join("objects").is_dir() || root.join("commondir").is_file()) {
        v.push(root.to_path_buf());
    }
    v
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
