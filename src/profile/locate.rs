//! Where the locate step looks for a harness when PATH does not have it.
//!
//! An app started from Finder, a service, or `env -i` gives paddock a thin
//! PATH such as `/usr/bin:/bin`. The installers of most agents put them in
//! a folder that only the login shell adds. After PATH the locate step tries
//! the folders of the profile (`[locate] dirs`), then the common folders
//! below. `paddock list --json` and `paddock doctor` say which step found
//! the program.

use crate::env::{capture, is_executable, HostEnv};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Which step of the locate order found a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FoundBy {
    /// PATH, or `mise which` with mise on PATH.
    Path,
    /// `[locate] paths` or `[locate] dirs` of the profile.
    Profile,
    /// One of the common install folders (`common_dirs`).
    Common,
}

impl FoundBy {
    pub fn as_str(self) -> &'static str {
        match self {
            FoundBy::Path => "path",
            FoundBy::Profile => "profile",
            FoundBy::Common => "common",
        }
    }
}

/// How the locate step found a harness: the step, and the folder of the
/// program as found, before links are resolved. `paddock doctor` tells the
/// user to add that folder to PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub by: FoundBy,
    pub dir: PathBuf,
}

/// The common install folders that do not depend on another program, in
/// order. Homebrew on Apple silicon and on Intel, then the home folders of
/// the usual installers.
pub fn fixed_dirs(env: &HostEnv) -> Vec<PathBuf> {
    // The folders of the system. A test sets PADDOCK_TEST_SYSTEM_BIN (a
    // PATH-like list, empty for none) so that the npm and node of the test
    // machine do not hide its fake ones.
    let mut dirs: Vec<PathBuf> = match env.var("PADDOCK_TEST_SYSTEM_BIN") {
        Some(list) => std::env::split_paths(&list)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ],
    };
    for rel in [".local/bin", ".bun/bin", ".npm-global/bin", ".cargo/bin"] {
        dirs.push(env.home.join(rel));
    }
    dirs
}

/// The folder of mise's shims: `MISE_DATA_DIR/shims`, else in the XDG data folder.
pub fn mise_shims(env: &HostEnv) -> PathBuf {
    env.var("MISE_DATA_DIR")
        .filter(|d| Path::new(d).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(|| env.data_dir.join("mise"))
        .join("shims")
}

/// A program on PATH or, after PATH, in `dirs`.
pub fn which_in(env: &HostEnv, dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    env.which(name)
        .or_else(|| dirs.iter().map(|d| d.join(name)).find(|p| is_executable(p)))
}

/// The `bin` folder of npm's global prefix, when npm is found. npm is a
/// node script, so its own folder goes first on the PATH it runs with.
pub fn npm_bin(env: &HostEnv, fixed: &[PathBuf]) -> Option<PathBuf> {
    let npm = which_in(env, fixed, "npm")?;
    let mut path: Vec<PathBuf> = npm.parent().map(Path::to_path_buf).into_iter().collect();
    path.extend(env.path.iter().cloned());
    path.extend(fixed.iter().cloned());
    let path = std::env::join_paths(path).ok()?;
    let mut cmd = std::process::Command::new(&npm);
    // No log file in ~/.npm/_logs for this question.
    cmd.args(["prefix", "-g", "--logs-max=0"])
        .current_dir("/")
        .env("PATH", path)
        .stdin(std::process::Stdio::null());
    let prefix = capture(&mut cmd).ok()?;
    let prefix = PathBuf::from(prefix.lines().last()?.trim());
    prefix.is_absolute().then(|| prefix.join("bin"))
}

/// All the common folders, in order: the fixed ones, npm's global `bin`
/// folder and mise's shims. A folder that does not exist is left out.
pub fn common_dirs(env: &HostEnv) -> Vec<PathBuf> {
    let mut dirs = fixed_dirs(env);
    if let Some(bin) = npm_bin(env, &dirs) {
        if !dirs.contains(&bin) {
            dirs.push(bin);
        }
    }
    dirs.push(mise_shims(env));
    dirs.retain(|d| d.is_dir());
    dirs
}

/// The PATH that the closure of a harness searches for the interpreters of
/// its script header: PATH, then the folder where the harness was found,
/// then the common folders. A harness in `~/.npm-global/bin` names
/// `#!/usr/bin/env node`, and node is in `/opt/homebrew/bin`, not on a thin PATH.
pub fn closure_env(env: &HostEnv, found: &Found, common: &[PathBuf]) -> HostEnv {
    let mut e = env.clone();
    if found.by == FoundBy::Path {
        return e;
    }
    for d in std::iter::once(&found.dir).chain(common) {
        if !e.path.contains(d) && !super::paths::is_shim(d) {
            e.path.push(d.clone());
        }
    }
    e
}

/// The line of `paddock doctor` that tells the user how to put `dir` on
/// PATH, for the shell in SHELL.
pub fn path_hint(env: &HostEnv, dir: &Path) -> String {
    let home = env.home.display().to_string();
    let shown = match dir.strip_prefix(&env.home) {
        Ok(rest) if !home.is_empty() => format!("$HOME/{}", rest.display()),
        _ => dir.display().to_string(),
    };
    let shell = env
        .var("SHELL")
        .and_then(|s| Path::new(s).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match shell.as_str() {
        "fish" => format!("run once: fish_add_path \"{shown}\""),
        "zsh" => format!("add to ~/.zshrc: export PATH=\"{shown}:$PATH\""),
        "bash" => format!("add to ~/.bashrc: export PATH=\"{shown}:$PATH\""),
        _ => format!("add to the startup file of your shell: export PATH=\"{shown}:$PATH\""),
    }
}

#[cfg(test)]
#[path = "locate_tests.rs"]
mod tests;
