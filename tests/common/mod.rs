//! The folder that the end-to-end tests work in.
//!
//! A sandbox gets its own /tmp, so a test project cannot live there. The
//! tests use `~/.cache/paddock-tests` (or `$XDG_CACHE_HOME/paddock-tests`),
//! not the home folder itself. A test that ends normally removes its
//! folder. A folder that stays, for example after a crash, is removed by a
//! later test run once it is a day old, after any device mount in it is
//! unmounted.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

pub fn root() -> PathBuf {
    let cache = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|v| Path::new(v).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".cache"));
    cache.join("paddock-tests")
}

/// A new folder for one test.
pub fn tempdir() -> tempfile::TempDir {
    let root = root();
    std::fs::create_dir_all(&root).unwrap();
    prune(&root);
    tempfile::Builder::new()
        .prefix("t-")
        .tempdir_in(&root)
        .unwrap()
}

/// Remove the folders of earlier runs that are older than a day.
fn prune(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > MAX_AGE);
        if !old {
            continue;
        }
        let dir = e.path();
        for mp in mounts_under(&dir) {
            let _ = std::process::Command::new("fusermount3")
                .args(["-u", "-z"])
                .arg(&mp)
                .status();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The `fuse.paddock` mount points under `dir`.
fn mounts_under(dir: &Path) -> Vec<String> {
    let prefix = format!("{}/", dir.display());
    std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(" - fuse.paddock "))
        .filter_map(|l| l.split(' ').nth(4).map(str::to_string))
        .filter(|mp| mp.starts_with(&prefix))
        .collect()
}

/// bubblewrap is on PATH and can start a sandbox with a user namespace. It
/// cannot when the tests themselves run in a sandbox that allows no nested
/// namespace.
#[allow(dead_code)]
pub fn bwrap_works() -> bool {
    std::process::Command::new("bwrap")
        .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A real FUSE mount works here: paddock mounts an empty device file
/// system in a test folder, the mount answers, and paddock unmounts it.
/// A host shell with NoNewPrivs has /dev/fuse and fusermount3, but the
/// setuid helper cannot mount there. The check runs once per test binary.
#[allow(dead_code)]
pub fn fuse_mount_works() -> bool {
    static WORKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WORKS.get_or_init(|| {
        use std::os::unix::fs::MetadataExt;
        let dir = tempdir();
        let mp = dir.path().join("dev");
        let fs = paddock::devices::DeviceFs::new("fuse-check", Vec::new());
        let Ok(m) = paddock::devices::mount(&mp, fs) else {
            return false;
        };
        // The mount point is on another file system now, and it answers.
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev()).ok();
        let mounted = dev(&mp).is_some() && dev(&mp) != dev(dir.path());
        let answers = std::fs::read_dir(&mp).is_ok();
        m.unmount();
        mounted && answers
    })
}

/// bubblewrap starts a sandbox and a FUSE mount works, so paddock can give
/// a sandbox its /dev/paddock. The check runs once per test binary.
#[allow(dead_code)]
pub fn sandbox_works() -> bool {
    static WORKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WORKS.get_or_init(|| bwrap_works() && fuse_mount_works())
}

/// A systemd user instance runs, and the session bus is known.
#[allow(dead_code)]
pub fn systemd_user() -> bool {
    std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
        && std::process::Command::new("systemctl")
            .args(["--user", "is-system-running"])
            .output()
            .is_ok_and(|o| {
                matches!(
                    String::from_utf8_lossy(&o.stdout).trim(),
                    "running" | "degraded"
                )
            })
}
