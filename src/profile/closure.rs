//! What an executable needs from the host: its install, the interpreters of
//! its script header and, for a harness, its helper programs.

use super::paths::{bin_helpers, install_root, is_system_path, HOME_BIN_DIRS};
use crate::env::HostEnv;
use anyhow::Result;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Find what an executable needs from the host: its install directory and, for
/// scripts, the interpreter named in the shebang line. MCP servers get this
/// closure (mcp.rs); a harness gets `harness_closure`.
pub fn runtime_closure(env: &HostEnv, exe: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut mounts = Vec::new();
    let mut path_dirs = Vec::new();
    follow(env, exe, &install_root, &mut mounts, &mut path_dirs);
    Ok(tidy(mounts, path_dirs))
}

/// The closure of a harness: the closure of its executable, and its helper
/// programs in a shared bin folder (`bin_helpers`). The harness looks a
/// helper up by name, so the name is mounted, and the file it resolves to
/// with its own closure. Each helper has its own budget of interpreter
/// steps, so one helper's script header cannot take another's target.
/// Every step of a helper, its interpreters too, gets the `helper_root`
/// rule, so a header that names `~/bin/tool` mounts that file only. Every
/// step of the executable gets the `harness_root` rule, so a header such
/// as `#!/usr/bin/env node` that finds `~/bin/node` mounts that file only.
pub fn harness_closure(
    env: &HostEnv,
    exe: &Path,
    listed: &[String],
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut mounts = Vec::new();
    let mut path_dirs = Vec::new();
    follow(
        env,
        exe,
        &|f: &Path| harness_root(env, f),
        &mut mounts,
        &mut path_dirs,
    );
    let helpers = bin_helpers(exe, listed);
    for (_, real) in &helpers {
        follow(
            env,
            real,
            &|f: &Path| helper_root(env, f),
            &mut mounts,
            &mut path_dirs,
        );
    }
    let (mut mounts, path_dirs) = tidy(mounts, path_dirs);
    for (name, _) in helpers {
        if !mounts.contains(&name) {
            mounts.push(name);
        }
    }
    mounts.sort();
    Ok((mounts, path_dirs))
}

/// The mount for the file a helper resolves to: the file itself, or its
/// install folder when the file sits in a known tool install. Never the
/// home or a folder above it. A link from `~/.local/bin` into `~/bin` or
/// `~/dotfiles` must not mount the folder around the target. In a system
/// folder only a package folder counts (`/opt/<tool>` for
/// `/opt/<tool>/bin/<file>`, or `node_modules`), never a shared folder such
/// as `/usr/lib` or `/opt`.
pub(super) fn helper_root(env: &HostEnv, real: &Path) -> PathBuf {
    if crate::mcp::is_tool_install(env, real) {
        let root = install_root(real);
        let shared = is_system_path(real) && Some(root.as_path()) == real.parent();
        if !shared && !exposes_home(env, &root) {
            return root;
        }
    }
    real.to_path_buf()
}

/// The mount for one step of the harness executable, the executable or an
/// interpreter of its script header: its `install_root`, or the file itself
/// when that folder is too wide (`exposes_home`). An npm install keeps its
/// `node_modules` folder, a versioned install such as
/// `~/.nvm/versions/node/v20/bin/node` keeps `~/.nvm/versions/node/v20`.
pub(super) fn harness_root(env: &HostEnv, file: &Path) -> PathBuf {
    let root = install_root(file);
    if exposes_home(env, &root) {
        return file.to_path_buf();
    }
    root
}

/// Whether mounting `dir` as a folder shows more than one tool: the home or
/// a folder above it, a home bin folder (`~/bin` and `HOME_BIN_DIRS`) or a
/// folder above one, such as `~/.cargo` with its `credentials.toml`, or a
/// shared system folder such as `/opt` or `/usr/lib`. The home counts in
/// its given and its canonical spelling.
fn exposes_home(env: &HostEnv, dir: &Path) -> bool {
    if is_shared_system_dir(dir) {
        return true;
    }
    let mut homes = vec![env.home.clone()];
    if let Ok(real) = std::fs::canonicalize(&env.home) {
        if real != env.home {
            homes.push(real);
        }
    }
    homes.iter().any(|home| {
        std::iter::once(home.clone())
            .chain(std::iter::once(home.join("bin")))
            .chain(HOME_BIN_DIRS.iter().map(|b| home.join(b)))
            .any(|kept| kept.starts_with(dir))
    })
}

/// System folders that hold the files of many packages.
fn is_shared_system_dir(dir: &Path) -> bool {
    const SHARED: &[&str] = &[
        "/",
        "/bin",
        "/sbin",
        "/lib",
        "/lib32",
        "/lib64",
        "/opt",
        "/nix",
        "/nix/store",
        "/usr",
        "/usr/bin",
        "/usr/sbin",
        "/usr/lib",
        "/usr/lib32",
        "/usr/lib64",
        "/usr/libexec",
        "/usr/share",
        "/usr/local",
        "/usr/local/bin",
        "/usr/local/sbin",
        "/usr/local/lib",
        "/usr/local/lib64",
        "/usr/local/libexec",
        "/usr/local/share",
    ];
    SHARED.iter().any(|d| dir == Path::new(d))
}

/// Mount `root_of(file)` for `file`, then follow its script header up to
/// three interpreters deep. Each interpreter gets the same rule.
fn follow(
    env: &HostEnv,
    file: &Path,
    root_of: &dyn Fn(&Path) -> PathBuf,
    mounts: &mut Vec<PathBuf>,
    path_dirs: &mut Vec<PathBuf>,
) {
    let mut queue = vec![file.to_path_buf()];
    let mut seen = 0;
    while let Some(file) = queue.pop() {
        seen += 1;
        if seen > 4 {
            break;
        }
        mounts.push(root_of(&file));
        if let Some(interp) = shebang_interpreter(&file) {
            let resolved = if interp.contains('/') {
                Some(PathBuf::from(&interp))
            } else {
                env.which(&interp)
            };
            if let Some(found) = resolved {
                if let Some(dir) = found.parent() {
                    if !is_system_path(&found) {
                        path_dirs.push(dir.to_path_buf());
                    }
                }
                if let Ok(real) = std::fs::canonicalize(&found) {
                    if let Some(dir) = real.parent() {
                        path_dirs.push(dir.to_path_buf());
                    }
                    queue.push(real);
                }
            }
        }
    }
}

/// Sort the mounts and drop the ones that sit inside another mount.
fn tidy(mut mounts: Vec<PathBuf>, mut path_dirs: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    mounts.sort();
    mounts.dedup();
    let all = mounts.clone();
    mounts.retain(|m| !all.iter().any(|o| crate::paths::strictly_inside(m, o)));
    path_dirs.dedup();
    (mounts, path_dirs)
}

fn shebang_interpreter(file: &Path) -> Option<String> {
    let mut buf = [0u8; 256];
    let n = std::fs::File::open(file).ok()?.read(&mut buf).ok()?;
    let head = std::str::from_utf8(&buf[..n]).ok()?;
    let line = head.strip_prefix("#!")?.lines().next()?.trim();
    let mut parts = line.split_whitespace();
    let first = parts.next()?;
    if first.ends_with("/env") {
        parts.find(|a| !a.starts_with('-')).map(str::to_string)
    } else {
        Some(first.to_string())
    }
}
