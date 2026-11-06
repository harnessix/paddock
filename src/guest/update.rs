//! The self-update of the agent. The host shares the agent of its own version
//! read only, and sends `agent.update {path, sha256, version}`. The agent
//! copies the file to `next/`, checks the hash, renames `current` to `prev`
//! and `next` to `current`, and exits 0 so launchd restarts it. The `launch`
//! script falls back to `prev/` after three fast failures of `current/`, so a
//! bad agent never bricks a profile.
//!
//! The pure parts live here with Linux tests: the hash check, the rename
//! order and the exit. The copy is a closure.

use super::BINARY;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder of the running agent.
pub const CURRENT: &str = "current";
/// The folder of the agent before the last update.
pub const PREV: &str = "prev";
/// The folder of an update in progress.
pub const NEXT: &str = "next";
/// The script that launchd runs. It must exist for an update, because it
/// is what falls back to `prev/`.
pub const LAUNCH: &str = "launch";

/// The SHA-256 of a file as lower-case hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// True when this agent can take an update: the `launch` script exists
/// next to `current/` and the folder takes new entries.
pub fn can_update(base: &Path) -> bool {
    if !base.join(LAUNCH).is_file() || !base.join(CURRENT).join(BINARY).is_file() {
        return false;
    }
    let probe = base.join(".update-probe");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// A plain file copy with the executable bit, the copy of the real agent.
pub fn copy_executable(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::copy(from, to)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Apply an update under `base`. `copy` copies `source` to the new
/// binary path. On any error `current/` is untouched. On success the
/// caller exits 0: launchd restarts the agent from the new `current/`.
pub fn apply(
    base: &Path,
    source: &Path,
    sha256: &str,
    copy: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<PathBuf, (ErrorCode, String)> {
    let io = |what: &str, e: std::io::Error| (ErrorCode::Io, text::io_failed(what, &e.to_string()));
    let next = base.join(NEXT);
    let current = base.join(CURRENT);
    let prev = base.join(PREV);
    if next.exists() {
        std::fs::remove_dir_all(&next).map_err(|e| io("cannot remove the old next/", e))?;
    }
    std::fs::create_dir(&next).map_err(|e| io("cannot make next/", e))?;
    let target = next.join(BINARY);
    let result = (|| {
        copy(source, &target).map_err(|e| io("cannot copy the new agent", e))?;
        let got = sha256_file(&target).map_err(|e| io("cannot read the copied agent", e))?;
        if !got.eq_ignore_ascii_case(sha256) {
            return Err((ErrorCode::BadHash, text::update_hash_mismatch(&got, sha256)));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&next);
        return Err(e);
    }
    // The rename order: next is complete, then current becomes prev, then
    // next becomes current. A crash between the two renames leaves no
    // current/, and launch falls back to prev/.
    if prev.exists() {
        std::fs::remove_dir_all(&prev).map_err(|e| io("cannot remove the old prev/", e))?;
    }
    if current.exists() {
        std::fs::rename(&current, &prev).map_err(|e| io("cannot move current/ to prev/", e))?;
    }
    std::fs::rename(&next, &current).map_err(|e| io("cannot move next/ to current/", e))?;
    Ok(current.join(BINARY))
}

/// The sha256 of the agent's own binary, once per process. On macOS only:
/// elsewhere the "agent" is a test binary, and no host compares it.
pub fn own_sha256() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        static SHA: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        SHA.get_or_init(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| sha256_file(&p).ok())
        })
        .clone()
    }
    #[cfg(not(target_os = "macos"))]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join(CURRENT)).unwrap();
        std::fs::write(d.path().join(CURRENT).join(BINARY), b"old agent").unwrap();
        std::fs::write(d.path().join(LAUNCH), b"#!/bin/sh\n").unwrap();
        d
    }

    fn source(d: &Path) -> (PathBuf, String) {
        let p = d.join("share-paddock-guest");
        std::fs::write(&p, b"new agent").unwrap();
        let h = sha256_file(&p).unwrap();
        (p, h)
    }

    #[test]
    fn wrong_hash_keeps_current() {
        let d = base();
        let (src, _) = source(d.path());
        let e = apply(d.path(), &src, &"0".repeat(64), copy_executable).unwrap_err();
        assert_eq!(e.0, ErrorCode::BadHash);
        assert!(e.1.contains("hash"), "{}", e.1);
        assert_eq!(
            std::fs::read(d.path().join(CURRENT).join(BINARY)).unwrap(),
            b"old agent"
        );
        assert!(!d.path().join(NEXT).exists(), "next/ is removed");
        assert!(!d.path().join(PREV).exists());
        // A failed copy keeps current too.
        let e = apply(d.path(), &src, &"0".repeat(64), |_, _| {
            Err(std::io::Error::other("share gone"))
        })
        .unwrap_err();
        assert_eq!(e.0, ErrorCode::Io);
        assert!(e.1.contains("share gone"));
        assert!(d.path().join(CURRENT).join(BINARY).exists());
    }

    #[test]
    fn rename_order() {
        let d = base();
        let (src, hash) = source(d.path());
        let mut order = Vec::new();
        let bin = apply(d.path(), &src, &hash.to_uppercase(), |from, to| {
            order.push(to.to_path_buf());
            copy_executable(from, to)
        })
        .unwrap();
        assert_eq!(order, vec![d.path().join(NEXT).join(BINARY)]);
        assert_eq!(bin, d.path().join(CURRENT).join(BINARY));
        assert_eq!(std::fs::read(&bin).unwrap(), b"new agent");
        assert_eq!(
            std::fs::read(d.path().join(PREV).join(BINARY)).unwrap(),
            b"old agent"
        );
        assert!(!d.path().join(NEXT).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&bin).unwrap().permissions().mode() & 0o111,
                0o111
            );
        }
        // A second update replaces prev.
        std::fs::write(&src, b"third agent").unwrap();
        let hash = sha256_file(&src).unwrap();
        apply(d.path(), &src, &hash, copy_executable).unwrap();
        assert_eq!(std::fs::read(&bin).unwrap(), b"third agent");
        assert_eq!(
            std::fs::read(d.path().join(PREV).join(BINARY)).unwrap(),
            b"new agent"
        );
    }

    #[test]
    fn missing_prev_and_stale_next_are_fine() {
        let d = base();
        std::fs::create_dir(d.path().join(NEXT)).unwrap();
        std::fs::write(d.path().join(NEXT).join("junk"), b"x").unwrap();
        let (src, hash) = source(d.path());
        apply(d.path(), &src, &hash, copy_executable).unwrap();
        assert!(d.path().join(PREV).join(BINARY).exists());
        assert!(!d.path().join(NEXT).exists());
    }

    #[test]
    fn can_update_needs_launch_and_a_writable_folder() {
        let d = base();
        assert!(can_update(d.path()));
        std::fs::remove_file(d.path().join(LAUNCH)).unwrap();
        assert!(!can_update(d.path()));
        assert!(!can_update(Path::new("/nonexistent/paddock")));
    }
}
