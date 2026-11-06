//! Unix socket paths that fit in `sun_path`.
//!
//! Linux keeps paddock's sockets under the run directory. macOS allows 104
//! bytes and `$TMPDIR` is long, so those sockets live in
//! `/tmp/paddock-<uid>/<run>`, with mode 0700. `<run>` is a short hash of the
//! run directory, so two paddock homes (for example a test and the real one)
//! never share a socket. A path that is still too long is an error here,
//! before the standard library reports `SUN_LEN`.

#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The limit of [`check`] on macOS, where `sun_path` has 104 bytes.
pub const MACOS_MAX: usize = 103;
/// The limit of [`check`] on Linux, where `sun_path` has 108 bytes.
pub const LINUX_MAX: usize = 107;
/// The room that [`short_dir`] keeps for the file name of a socket:
/// `/credentials.sock` and every other name of paddock fits.
pub const NAME_ROOM: usize = 24;

/// The longest path `UnixListener::bind` accepts, in bytes.
/// The standard library rejects a path whose length is at least `sun_path`.
pub fn max_len() -> usize {
    if cfg!(target_os = "macos") {
        MACOS_MAX
    } else {
        LINUX_MAX
    }
}

/// True when `path` fits in a socket address of a system whose limit is
/// `max` ([`MACOS_MAX`], [`LINUX_MAX`]).
pub fn fits(path: &Path, max: usize) -> bool {
    path.as_os_str().len() < max
}

/// Error when `path` does not fit in a Unix socket address on this system.
pub fn check(path: &Path) -> Result<()> {
    let n = path.as_os_str().len();
    let max = max_len();
    if !fits(path, max) {
        bail!(
            "unix socket path is {n} bytes, longer than the {max} byte limit on this system: {}",
            path.display()
        );
    }
    Ok(())
}

/// Directory for host sockets of run `id`. An empty id is the shared directory
/// (`gatewayd.sock`, `view.sock`).
pub fn socket_dir(run: &Path, uid: u32, id: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        short_dir(&top(uid), run, id, MACOS_MAX)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = uid;
        if id.is_empty() {
            run.to_path_buf()
        } else {
            run.join(id)
        }
    }
}

/// `name` inside [`socket_dir`] for a run of `env`. An empty id is a shared
/// socket (`gatewayd.sock`, `view.sock`). Creates the directory on macOS.
pub fn named(env: &crate::env::HostEnv, id: &str, name: &str) -> Result<PathBuf> {
    host_socket(&env.paddock_run(), env.uid, id, name)
}

/// `name` for a run of `env`, or the run-directory path when that path does
/// not fit in a socket address.
pub fn named_or_run(env: &crate::env::HostEnv, id: &str, name: &str) -> PathBuf {
    named(env, id, name).unwrap_or_else(|_| {
        let run = env.paddock_run();
        if id.is_empty() {
            run.join(name)
        } else {
            run.join(id).join(name)
        }
    })
}

/// Remove the run directory and, on macOS, the socket directory of that run.
pub fn end_run(run: &Path, uid: u32, id: &str) {
    let _ = std::fs::remove_dir_all(run.join(id));
    remove_run_sockets(run, uid, id);
}

/// `name` inside [`socket_dir`]. Creates the directory on macOS (mode 0700).
pub fn host_socket(run: &Path, uid: u32, id: &str, name: &str) -> Result<PathBuf> {
    let dir = socket_dir(run, uid, id);
    #[cfg(target_os = "macos")]
    {
        // Each level from /tmp/paddock-<uid> down is private.
        let top = top(uid);
        private_dir(&dir)?;
        for d in dir.ancestors().skip(1) {
            if !d.starts_with(&top) {
                break;
            }
            private_dir(d)?;
        }
    }
    let path = dir.join(name);
    check(&path)?;
    Ok(path)
}

/// `/tmp/paddock-<uid>`: the top of the short socket folders on macOS.
#[cfg(target_os = "macos")]
fn top(uid: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/paddock-{uid}"))
}

/// The short socket folder of run `id` under `top`: `top/<hash of run>`,
/// then `id`, or a hash of `id` when a socket under `id` would not fit
/// in `max` bytes with [`NAME_ROOM`] for its name. The hashes are stable
/// across builds. An empty id is the shared folder. This is the layout of
/// macOS, kept platform-neutral so that the Linux tests prove the rule.
pub fn short_dir(top: &Path, run: &Path, id: &str, max: usize) -> PathBuf {
    let base = top.join(hex8(run.as_os_str().as_encoded_bytes()));
    if id.is_empty() {
        return base;
    }
    let plain = base.join(id);
    let safe = !id.contains('/') && !id.contains('\0') && id != "." && id != "..";
    if safe && plain.as_os_str().len() + NAME_ROOM < max {
        return plain;
    }
    base.join(hex16(id.as_bytes()))
}

fn hex8(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hex16(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(target_os = "macos")]
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path).with_context(|| format!("cannot create {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot set the mode of {}", path.display()))?;
    Ok(())
}

/// Remove the per-run socket directory on macOS. On Linux that directory
/// is the run folder, and the caller removes the run folder itself.
pub fn remove_run_sockets(run: &Path, uid: u32, id: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = run;
        if !id.is_empty() {
            let _ = std::fs::remove_dir_all(socket_dir(run, uid, id));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (run, uid, id);
    }
}

#[cfg(test)]
#[path = "sockpath_tests.rs"]
mod tests;
