//! A copy of `/bin/ps` that a Seatbelt sandbox can start.
//!
//! `/bin/ps` is setuid root, and a Seatbelt sandbox may not start a setuid
//! program: `ps` fails with "Operation not permitted" (exit 126). codex
//! reads the start time of its app server with `ps`, so it stops. A plain
//! copy with mode 755 is killed, because the copy breaks its signature. A
//! copy with mode 755 and an ad hoc signature (`codesign -s - -f`) works.
//!
//! paddock makes that copy once per build of `/bin/ps`, in its own state
//! folder: `STATE/seatbelt-shims/KEY/ps`, where KEY names the inode, the
//! size and the change time of `/bin/ps`. A macOS update gives a new key and
//! a new copy. The copy is made in a temporary folder, checked, and renamed
//! into place, so two starts at once are safe. A run puts the folder first
//! on PATH, and its profile allows the folder read-only. Setuid programs in
//! general stay blocked. When a step fails, the run goes on without the
//! copy, after one line in the log.

use crate::env::HostEnv;
use crate::seatbelt::{Access, Policy};
use anyhow::{bail, Context as _, Result};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The program that paddock copies.
pub const SOURCE: &str = "/bin/ps";
/// The name of the copy, the same as the program.
pub const NAME: &str = "ps";

/// The commands that make the copy. Tests give fake ones.
pub struct Tools {
    /// `cp SRC DST`.
    pub cp: PathBuf,
    /// `codesign -s - -f FILE`.
    pub codesign: PathBuf,
}

impl Tools {
    /// The commands of the Mac.
    pub fn host() -> Self {
        Self {
            cp: "/bin/cp".into(),
            codesign: "/usr/bin/codesign".into(),
        }
    }
}

/// The folder that holds one folder per key.
pub fn root(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("seatbelt-shims")
}

/// The key of a build of the program: its inode, size and modification
/// time. A
/// macOS update changes at least one of them.
pub fn key(meta: &std::fs::Metadata) -> String {
    format!(
        "{NAME}-{:x}-{:x}-{:x}.{:x}",
        meta.ino(),
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec()
    )
}

/// The folder of the copy for the current build of `source`, when the copy
/// is there. It does not make the copy.
pub fn current(root: &Path, source: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(source).ok()?;
    let dir = root.join(key(&meta));
    crate::env::is_executable(&dir.join(NAME)).then_some(dir)
}

/// Run one step of the build. The error holds the first line of stderr.
fn step(what: &str, cmd: &mut Command) -> Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let out = crate::env::spawn_retrying(cmd)
        .and_then(|c| c.wait_with_output())
        .with_context(|| format!("cannot start {what}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().next().unwrap_or("").trim();
        bail!("{what} failed ({}): {line}", out.status);
    }
    Ok(())
}

/// The copy works outside the sandbox: `ps -o pid= -p PID` of paddock
/// itself prints that PID. A copy with a broken signature is killed here.
fn check(file: &Path) -> Result<()> {
    let pid = std::process::id().to_string();
    let mut cmd = Command::new(file);
    cmd.args(["-o", "pid=", "-p", &pid])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let out = crate::env::spawn_retrying(&mut cmd)
        .and_then(|c| c.wait_with_output())
        .with_context(|| format!("cannot start the copy {}", file.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || text.trim() != pid {
        bail!("the copy does not work ({})", out.status);
    }
    Ok(())
}

/// Copy, chmod 755, sign ad hoc and check in the folder `tmp`.
fn build(source: &Path, tmp: &Path, tools: &Tools) -> Result<()> {
    std::fs::create_dir(tmp).with_context(|| format!("cannot create {}", tmp.display()))?;
    let file = tmp.join(NAME);
    step("cp", Command::new(&tools.cp).arg(source).arg(&file))?;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("cannot chmod {}", file.display()))?;
    step(
        "codesign",
        Command::new(&tools.codesign)
            .args(["-s", "-", "-f"])
            .arg(&file),
    )?;
    check(&file)
}

/// Remove the folders of other keys, and temporary folders older than a
/// day. Errors are ignored: an old folder does no harm.
fn prune(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == keep {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if !name.starts_with('.') || old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// The folder of the copy for the current build of `source`. It makes the
/// copy when it is not there yet.
pub fn ensure(root: &Path, source: &Path, tools: &Tools) -> Result<PathBuf> {
    let meta =
        std::fs::metadata(source).with_context(|| format!("cannot read {}", source.display()))?;
    let key = key(&meta);
    let dir = root.join(&key);
    if crate::env::is_executable(&dir.join(NAME)) {
        return Ok(dir);
    }
    crate::privfs::dir(root).with_context(|| format!("cannot create {}", root.display()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = root.join(format!(".{key}.{}.{nanos}", std::process::id()));
    if let Err(e) = build(source, &tmp, tools) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    match std::fs::rename(&tmp, &dir) {
        Ok(()) => {}
        // Another start was first: its copy is the same.
        Err(_) if crate::env::is_executable(&dir.join(NAME)) => {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e).with_context(|| format!("cannot rename into {}", dir.display()));
        }
    }
    prune(root, &key);
    Ok(dir)
}

/// The copy is needed: macOS, and a setuid `/bin/ps`.
fn wanted(source: &Path) -> bool {
    cfg!(target_os = "macos")
        && std::fs::metadata(source).is_ok_and(|m| m.permissions().mode() & 0o4000 != 0)
}

/// Make the copy before a run, or log one line why not.
pub fn prepare(env: &HostEnv) {
    let source = Path::new(SOURCE);
    if !wanted(source) {
        return;
    }
    if let Err(e) = ensure(&root(env), source, &Tools::host()) {
        log::warn!("no ps for the sandbox, ps will fail in it: {e:#}");
    }
}

/// The folder of the copy for a run, when the copy is ready.
pub fn for_run(env: &HostEnv) -> Option<PathBuf> {
    let source = Path::new(SOURCE);
    if !wanted(source) {
        return None;
    }
    current(&root(env), source)
}

/// `path` with `dir` first, and not again later.
pub fn path_first(path: &str, dir: &Path) -> String {
    let d = dir.display().to_string();
    std::iter::once(d.as_str())
        .chain(path.split(':').filter(|p| !p.is_empty() && *p != d))
        .collect::<Vec<_>>()
        .join(":")
}

/// The profile rule for the folder: read and start, never write.
pub fn allow(p: &mut Policy, dir: &Path) {
    p.add(
        dir,
        Access::ReadOnly,
        "paddock's signed copy of /bin/ps: the sandbox cannot start a setuid program",
    );
}

/// The line of `paddock doctor` for a result of `ensure`.
pub fn status_line(r: &Result<PathBuf>) -> String {
    match r {
        Ok(dir) => format!(
            "  ok       ps for the sandbox: {}",
            dir.join(NAME).display()
        ),
        Err(e) => format!(
            "  no       ps for the sandbox: {e:#}. In a Seatbelt run ps fails, and codex stops"
        ),
    }
}

/// The lines of `paddock doctor` on macOS. Makes the copy when it is not
/// there.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    let source = Path::new(SOURCE);
    if !wanted(source) {
        return vec![format!(
            "  ok       ps for the sandbox: {SOURCE} is not setuid, the sandbox starts it"
        )];
    }
    vec![status_line(&ensure(&root(env), source, &Tools::host()))]
}

/// The section of `paddock doctor` on macOS: codex stops when ps fails in
/// the sandbox.
pub fn print_doctor(env: &HostEnv) {
    println!("\nSetuid programs in a Seatbelt run:");
    for l in doctor_lines(env) {
        println!("{l}");
    }
}

#[cfg(test)]
#[path = "suid_shim_tests.rs"]
mod tests;

/// The copy in a real Seatbelt sandbox. Only on macOS.
#[cfg(all(test, target_os = "macos"))]
#[path = "suid_shim_macos_tests.rs"]
mod macos_tests;
