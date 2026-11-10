//! Device mounts that a killed paddock left behind, and the owner mark
//! that keeps the mount of a sandbox that is still starting.
//!
//! A sandbox registers as running only after its sandbox process started,
//! and its device mount comes before that. Two sandboxes that start at
//! the same moment (two spawned children, for example) must not take the
//! fresh mount of the other one for a stale mount. So each mount gets an
//! owner mark with the pid of its paddock process before the mount, and
//! the mark goes away with the mount.

use super::lazy_unmount;
use crate::env::HostEnv;
use std::path::{Path, PathBuf};

/// The owner mark of the mount at `dir`: a file next to the folder.
pub(super) fn owner_file(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(".owner");
    dir.with_file_name(name)
}

/// Mark this process as the owner of the mount at `dir`.
pub(super) fn claim(dir: &Path) -> std::io::Result<()> {
    crate::privfs::write(&owner_file(dir), std::process::id().to_string())
}

/// Remove the owner mark of the mount at `dir`.
pub(super) fn release(dir: &Path) {
    let _ = std::fs::remove_file(owner_file(dir));
}

/// The paddock process that owns the mount at `dir` still runs.
fn owner_alive(dir: &Path) -> bool {
    std::fs::read_to_string(owner_file(dir))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(crate::state::alive)
}

/// The paddock device mounts in `mountinfo` under the run folder `run`
/// whose sandbox is not in `running` and whose owner process is gone.
pub(super) fn stale_mounts(mountinfo: &str, run: &Path, running: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in mountinfo.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else {
            continue;
        };
        if f.get(sep + 1) != Some(&"fuse.paddock") || f.len() < 5 {
            continue;
        }
        let mp = Path::new(f[4]);
        let Ok(rel) = mp.strip_prefix(run) else {
            continue;
        };
        let mut parts = rel.components();
        let (Some(id), Some(dev), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        // <id>/dev, or devices/<id> for srt.
        let (id, dev) = if id.as_os_str() == "devices" {
            (
                dev.as_os_str().to_string_lossy(),
                std::ffi::OsStr::new("dev"),
            )
        } else {
            (id.as_os_str().to_string_lossy(), dev.as_os_str())
        };
        if dev == "dev" && !running.iter().any(|r| *r == id) && !owner_alive(mp) {
            out.push(mp.to_path_buf());
        }
    }
    out
}

/// Unmount device file systems of sandboxes that no longer run. A paddock
/// process that was killed leaves its mount behind, and every access to it
/// fails until it is unmounted. A mount whose owner still runs stays: its
/// sandbox may be starting and not registered yet.
pub fn clear_stale(env: &HostEnv) {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return;
    };
    let running: Vec<String> = crate::state::Running::list(env)
        .into_iter()
        .map(|r| r.id)
        .collect();
    for mp in stale_mounts(&info, &env.paddock_run(), &running) {
        log::debug!("unmounting the stale device mount {}", mp.display());
        lazy_unmount(&mp);
        release(&mp);
    }
}

#[cfg(test)]
#[path = "stale_tests.rs"]
mod tests;
