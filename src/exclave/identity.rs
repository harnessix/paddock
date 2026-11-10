//! Opaque, persistent project capabilities, kept only on the local host.
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Return a peer-specific capability which does not expose a project path and
/// cannot be derived by another project from a public repository name.
pub fn project(env: &HostEnv, peer: &str, project: &Path) -> Result<String> {
    let dir = env.paddock_data().join("exclave-client");
    crate::privfs::dir(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join("key.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).context("lock exclave identity");
    }
    check_private(&lock)?;
    let file = dir.join("key");
    let key = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&file)
    {
        Ok(f) => {
            check_private(&f)?;
            let mut bytes = Vec::new();
            f.take(33).read_to_end(&mut bytes)?;
            if bytes.len() != 32 {
                bail!("invalid exclave identity key");
            }
            bytes
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut bytes = vec![0u8; 32];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&file)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            bytes
        }
        Err(e) => return Err(e.into()),
    };
    let key: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid exclave identity key"))?;
    use std::os::unix::ffi::OsStrExt;
    let path = crate::storage::package_cache::identity(project)?;
    let mut hasher = blake3::Hasher::new_keyed(&key);
    hasher.update(b"paddock exclave project v1\0");
    hasher.update(&(peer.len() as u64).to_be_bytes());
    hasher.update(peer.as_bytes());
    hasher.update(path.as_os_str().as_bytes());
    if crate::storage::layers::share(env)? != crate::storage::layers::Share::Project {
        let mut nonce = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        hasher.update(b"\0isolated cache\0");
        hasher.update(&nonce);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn check_private(file: &std::fs::File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        bail!("insecure exclave identity file");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn worktrees_share_peer_identity() {
        let temp = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            temp.path().join("home"),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        let repo = temp.path().join("repo");
        let wt = temp.path().join("worktree");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "init",
        ]);
        git(&["worktree", "add", "-q", "-b", "test", wt.to_str().unwrap()]);
        assert_eq!(
            project(&env, "peer", &repo).unwrap(),
            project(&env, "peer", &wt).unwrap()
        );
        assert_ne!(
            project(&env, "peer", &repo).unwrap(),
            project(&env, "other", &wt).unwrap()
        );
        let peer_cache = temp.path().join("fake-peer");
        crate::privfs::dir(&peer_cache).unwrap();
        let first = project(&env, "peer", &repo).unwrap();
        let second = project(&env, "peer", &wt).unwrap();
        let cache =
            crate::exclave::cache::ProjectCache::open(&peer_cache, first.as_bytes()).unwrap();
        std::fs::write(cache.target_dir().join("fake"), b"cached").unwrap();
        drop(cache);
        let cache =
            crate::exclave::cache::ProjectCache::open(&peer_cache, second.as_bytes()).unwrap();
        assert_eq!(
            std::fs::read(cache.target_dir().join("fake")).unwrap(),
            b"cached"
        );
    }

    #[test]
    fn restricted_sharing_never_reuses_a_peer_cache() {
        let temp = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            temp.path().join("home"),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let config = env.paddock_config().join("config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        for choice in ["verified", "none"] {
            std::fs::write(&config, format!("[cache]\nshare = \"{choice}\"\n")).unwrap();
            assert_ne!(
                project(&env, "peer", &repo).unwrap(),
                project(&env, "peer", &repo).unwrap()
            );
        }
    }

    #[test]
    fn identities_are_stable_separate_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(
            temp.path().join("home"),
            "test".into(),
            1,
            1,
            BTreeMap::new(),
        );
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let id = project(&env, "peer", &a).unwrap();
        assert_eq!(id, project(&env, "peer", &a).unwrap());
        assert_ne!(id, project(&env, "peer", &b).unwrap());
        assert_ne!(id, project(&env, "other", &a).unwrap());
        assert_eq!(
            std::fs::metadata(env.paddock_data().join("exclave-client/key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
}
