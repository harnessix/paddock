//! The path rules of the locate step: shims, real paths and system paths.

use crate::env::is_executable;
use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

/// A version-manager shim re-enters the manager, which is not mounted in the sandbox.
pub(super) fn is_shim(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(s) if s == "shims" || s == "command-wrappers"))
}

pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    let real = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve {}", path.display()))?;
    // A multi-call program picks its tool by the name it runs as: the
    // Alpine guest of the VM backend has `/bin/sh` as a link to busybox.
    if is_multi_call(&real) && real.file_name() != path.file_name() {
        return Ok(path.to_path_buf());
    }
    Ok(real)
}

fn is_multi_call(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n == "busybox" || n == "toybox")
}

const SYSTEM_PREFIXES: &[&str] = &[
    "/usr/",
    "/bin/",
    "/sbin/",
    "/lib/",
    "/lib64/",
    "/opt/",
    "/nix/store/",
];

pub fn is_system_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    SYSTEM_PREFIXES.iter().any(|pre| s.starts_with(pre))
}

/// Bin directories in the home that hold single, self-contained executables
/// next to unrelated ones. The folder above them can hold credentials
/// (`~/.cargo/credentials.toml`), so only the file is mounted.
pub const HOME_BIN_DIRS: &[&str] = &[
    ".local/bin",
    ".cargo/bin",
    "go/bin",
    ".bun/bin",
    ".deno/bin",
];

/// The directory to mount for an executable. For npm packages this is the
/// outermost `node_modules` folder: it holds the package and the packages it
/// loads, and nothing else of the project or install around it (a `.env`, the
/// sources). For a binary in a shared bin directory it is the file itself, so
/// the directory is not shadowed in an image and its neighbours stay hidden.
pub fn install_root(exe: &Path) -> PathBuf {
    let comps: Vec<_> = exe.components().collect();
    if let Some(i) = comps
        .iter()
        .position(|c| matches!(c, Component::Normal(s) if *s == "node_modules"))
    {
        return comps[..=i].iter().collect();
    }
    let parent = exe.parent().unwrap_or(Path::new("/"));
    if is_shared_bin_dir(parent) {
        return exe.to_path_buf();
    }
    // A versioned install directory such as `<tool>/1.2.3/bin/<exe>`.
    if parent.file_name().is_some_and(|n| n == "bin") {
        if let Some(gp) = parent.parent() {
            return gp.to_path_buf();
        }
    }
    parent.to_path_buf()
}

/// The kinds of bin directory that hold the programs of many packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BinDir {
    /// `/usr/bin` and the other system folders, which hold every package.
    System,
    /// A bin folder in the home, such as `~/.local/bin`.
    Home,
}

fn bin_dir_kind(dir: &Path) -> Option<BinDir> {
    const SYSTEM: &[&str] = &[
        "/usr/bin",
        "/usr/sbin",
        "/bin",
        "/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ];
    if SYSTEM.iter().any(|d| dir == Path::new(d)) {
        Some(BinDir::System)
    } else if HOME_BIN_DIRS.iter().any(|d| dir.ends_with(d)) {
        Some(BinDir::Home)
    } else {
        None
    }
}

fn is_shared_bin_dir(dir: &Path) -> bool {
    bin_dir_kind(dir).is_some()
}

/// The helper programs of an executable in a shared bin directory, such as
/// `codex-exec-server` next to `codex`. The harness starts them from the
/// folder of its own binary. `listed` holds the helper names of the profile
/// (`[locate] helpers`). An executable in its own install directory gets
/// its helpers with the directory.
pub(super) fn bin_helpers(exe: &Path, listed: &[String]) -> Vec<(PathBuf, PathBuf)> {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return Vec::new();
    };
    match bin_dir_kind(dir) {
        Some(kind) => helpers_in(dir, &name.to_string_lossy(), kind, listed),
        None => Vec::new(),
    }
}

/// The helpers of `name` in `dir`. In a system folder only `<name>-<h>` for
/// a listed `h`: `/usr/bin` holds other products with the same prefix, such
/// as `claude-desktop` next to `claude`. In a home bin folder every
/// executable file named `<name>-*`. Each item is the path the harness looks
/// up and the file it resolves to, resolved like the executable. Nothing
/// else of the folder is returned.
pub(super) fn helpers_in(
    dir: &Path,
    name: &str,
    kind: BinDir,
    listed: &[String],
) -> Vec<(PathBuf, PathBuf)> {
    let prefix = format!("{name}-");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<_> = entries
        .flatten()
        .filter(|e| {
            let file = e.file_name();
            let Some(rest) = file
                .to_string_lossy()
                .strip_prefix(&prefix)
                .map(String::from)
            else {
                return false;
            };
            match kind {
                BinDir::System => listed.contains(&rest),
                BinDir::Home => !rest.is_empty(),
            }
        })
        .map(|e| e.path())
        // is_executable follows a link and takes regular files only.
        .filter(|p| is_executable(p))
        .filter_map(|p| canonical(&p).ok().map(|real| (p, real)))
        .collect();
    out.sort();
    out
}
