//! The host paths of a build: the project, the folders a sandbox can
//! write, and the check that the harness does not come from one of them.

use crate::detect::git_toplevel;
use crate::env::HostEnv;
use crate::profile::Harness;
use crate::spec::SandboxSpec;
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

/// The host folders that a sandbox can write, as real paths: the project,
/// every read-write mount so far, and all sandbox homes and worktrees (a
/// sandbox of another project can write those).
pub fn sandbox_writable(env: &HostEnv, spec: &SandboxSpec) -> Vec<PathBuf> {
    let mut v = vec![
        spec.project.clone(),
        spec.home_src.clone(),
        env.paddock_data().join("homes"),
        env.paddock_data().join("worktrees"),
    ];
    v.extend(
        spec.mounts
            .iter()
            .filter(|m| m.kind == crate::spec::MountKind::Rw)
            .filter_map(|m| m.src.clone()),
    );
    v.into_iter()
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect()
}

/// The harness install mounts come from the executable and the script
/// headers it leads to (`profile::harness_closure`). A file in a folder the
/// sandbox can write could name any host path in its header, and paddock
/// would mount that path. So the harness must not come from such a folder,
/// for example through a host PATH entry inside the project.
pub fn check_harness_install(harness: &Harness, writable: &[PathBuf]) -> Result<()> {
    let all = std::iter::once(&harness.exe)
        .chain(&harness.install_mounts)
        .chain(&harness.path_dirs);
    for p in all {
        if let Some(w) = writable.iter().find(|w| p.starts_with(w)) {
            bail!(
                "the {} harness resolves to {}, inside {}, which the sandbox can write. \
                 paddock does not mount host paths that such a file names. \
                 Install the harness outside the project and the sandbox homes.",
                harness.profile.name,
                p.display(),
                w.display()
            );
        }
    }
    Ok(())
}

pub fn resolve_project(path: Option<&Path>) -> Result<PathBuf> {
    let raw = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let dir = std::fs::canonicalize(&raw)
        .with_context(|| format!("project directory {} does not exist", raw.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    Ok(git_toplevel(&dir).unwrap_or(dir))
}
