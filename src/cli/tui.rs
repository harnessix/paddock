//! `paddock tui`: start the terminal interface. It is a separate program
//! (`tui/` in this repository), so the paddock binary gains no dependency. paddock looks for it in this
//! order: a `paddock-tui` binary next to paddock, then `paddock-tui` on
//! PATH. It never downloads the interface: `npx` would run code from npm
//! and from the project folder (its `node_modules` and `.npmrc`, which the
//! agent can write) outside the sandbox, with the power to answer every
//! approval.

use anyhow::Result;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

/// The oldest Node.js major version that runs it (Ink 7).
const NODE_MAJOR: u32 = 22;

/// How `paddock tui` starts the interface.
#[derive(Debug, PartialEq, Eq)]
enum Launch {
    Binary(PathBuf),
    /// No way: the text says why and what to install.
    Missing(String),
}

pub(crate) fn run(args: Vec<String>) -> Result<i32> {
    let exe = std::env::current_exe().ok();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let launch = choose(exe.as_deref(), &path, node_major);
    let mut cmd = match launch {
        Launch::Binary(bin) => std::process::Command::new(bin),
        Launch::Missing(text) => {
            eprintln!("{text}");
            return Ok(127);
        }
    };
    if let Some(exe) = exe {
        // The interface calls the same paddock, not another one on PATH.
        cmd.env("PADDOCK_BIN", exe);
    }
    let err = cmd.args(args).exec();
    anyhow::bail!("could not start the terminal UI: {err}")
}

fn choose(exe: Option<&Path>, path: &OsString, node: impl Fn(&Path) -> Option<u32>) -> Launch {
    if let Some(sibling) = exe.and_then(Path::parent).map(|d| d.join("paddock-tui")) {
        if is_executable(&sibling) {
            return Launch::Binary(sibling);
        }
    }
    if let Some(bin) = on_path(path, "paddock-tui") {
        return Launch::Binary(bin);
    }
    let install = format!(
        "Install one of them:\n  put the paddock-tui binary of a paddock release next to paddock{}\n  or build it from the paddock source tree with `cd tui && npm ci && npm run build`, then put tui/dist/cli.js on PATH as paddock-tui",
        exe.and_then(Path::parent)
            .map(|d| format!(" ({})", d.display()))
            .unwrap_or_default()
    );
    let Some(node_bin) = on_path(path, "node") else {
        return Launch::Missing(format!(
            "paddock tui needs Node.js {NODE_MAJOR} or newer, or the paddock-tui binary. Neither is installed.\n{install}"
        ));
    };
    match node(&node_bin) {
        Some(v) if v >= NODE_MAJOR => Launch::Missing(format!(
            "paddock tui found Node.js {v}, but the paddock-tui program is not installed. paddock does not download it with npx.\n{install}"
        )),
        Some(v) => Launch::Missing(format!(
            "paddock tui needs Node.js {NODE_MAJOR} or newer. {} is version {v}.\n{install}",
            node_bin.display()
        )),
        None => Launch::Missing(format!(
            "paddock tui could not read the version of {}.\n{install}",
            node_bin.display()
        )),
    }
}

fn on_path(path: &OsString, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The major version of `node --version` (`v22.11.0`).
fn node_major(node: &Path) -> Option<u32> {
    let out = std::process::Command::new(node)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_major(&String::from_utf8_lossy(&out.stdout))
}

fn parse_major(text: &str) -> Option<u32> {
    text.trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe_file(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_order_is_sibling_then_path_and_npx_is_never_used() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let tools = tmp.path().join("tools");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        let exe = exe_file(&bin, "paddock");
        let path = OsString::from(tools.as_os_str());
        let v22 = |_: &Path| Some(22);

        let none = choose(Some(&exe), &path, v22);
        assert!(
            matches!(&none, Launch::Missing(t) if t.contains("paddock-tui binary of a paddock release next to paddock") && t.contains("cd tui && npm ci && npm run build") && !t.contains("npm install")),
            "{none:?}"
        );

        exe_file(&tools, "node");
        exe_file(&tools, "npx");
        let node_only = choose(Some(&exe), &path, v22);
        assert!(
            matches!(&node_only, Launch::Missing(t) if t.contains("does not download it with npx") && t.contains("cd tui && npm ci && npm run build") && !t.contains("npm install")),
            "{node_only:?}"
        );
        let old = choose(Some(&exe), &path, |_: &Path| Some(18));
        assert!(
            matches!(&old, Launch::Missing(t) if t.contains("version 18")),
            "{old:?}"
        );

        let on_path = exe_file(&tools, "paddock-tui");
        assert_eq!(choose(Some(&exe), &path, v22), Launch::Binary(on_path));
        let sibling = exe_file(&bin, "paddock-tui");
        assert_eq!(choose(Some(&exe), &path, v22), Launch::Binary(sibling));
    }

    #[test]
    fn node_versions_parse() {
        assert_eq!(parse_major("v22.11.0\n"), Some(22));
        assert_eq!(parse_major("v8"), Some(8));
        assert_eq!(parse_major("22.1"), None);
        assert_eq!(parse_major(""), None);
    }
}
