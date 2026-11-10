//! Migration of the default private registry, retaining its plaintext copy.

use super::snapshot;
use crate::env::HostEnv;
use crate::horse::Store;
use anyhow::{bail, Context as _, Result};
use std::ffi::OsStr;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

fn parent(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("registry")
}
fn local(env: &HostEnv) -> PathBuf {
    parent(env).join("local")
}
fn stage(env: &HostEnv) -> PathBuf {
    parent(env).join(".local-migrate")
}
pub(super) fn backup(env: &HostEnv) -> PathBuf {
    parent(env).join("local.plain")
}

pub(super) fn prepare(env: &HostEnv, store: &Store) -> Result<Option<String>> {
    let local = local(env);
    if !local.exists() {
        return Ok(None);
    }
    if backup(env).exists() || stage(env).exists() {
        bail!("a plaintext registry backup or migration stage already exists");
    }
    if local.join(".git").exists() {
        bail!("the private local registry is a git checkout; its plaintext has not been changed");
    }
    let source = snapshot::scan(&local)?;
    std::fs::DirBuilder::new().mode(0o700).create(stage(env))?;
    let root = crate::safefs::Root::open(&local)?;
    for path in &source.files {
        let name = path.to_str().context("registry path is not UTF-8")?;
        let bytes = snapshot::read(&root, path)?;
        let encrypted = store.seal_registry(name, &bytes)?;
        let to = stage(env).join(path);
        crate::privfs::write_atomic(&to, &encrypted)?;
        super::super::store::sync_dir(to.parent().context("registry file has no parent")?)?;
    }
    verify(store, &local, &stage(env), &source.digest)?;
    if snapshot::scan(&local)?.digest != source.digest {
        bail!("local registry changed during migration; nothing was swapped");
    }
    Ok(Some(source.digest))
}

/// Check every plaintext source file against the encrypted copy. Extra
/// encrypted files may have been added after migration and are harmless.
pub(super) fn verify(store: &Store, plain: &Path, encrypted: &Path, expected: &str) -> Result<()> {
    let source = snapshot::scan(plain)?;
    if source.digest != expected {
        bail!("retained plaintext registry changed; it will not be removed");
    }
    let source_root = crate::safefs::Root::open(plain)?;
    let encrypted_root = crate::safefs::Root::open(encrypted)?;
    for path in &source.files {
        let name = path.to_str().context("registry path is not UTF-8")?;
        let original = snapshot::read(&source_root, path)?;
        let cipher = snapshot::read(&encrypted_root, path)?;
        if store.open_registry(name, &cipher)? != original {
            bail!("encrypted registry verification failed for {name}");
        }
    }
    Ok(())
}

/// Swap the prepared registry into place. The old directory is moved, never
/// removed, and a retry recognizes each intermediate state.
pub(super) fn advance(env: &HostEnv, store: &Store, expected: Option<&str>) -> Result<()> {
    let Some(expected) = expected else {
        if local(env).exists() || stage(env).exists() || backup(env).exists() {
            bail!("local registry appeared during migration; nothing was removed");
        }
        return Ok(());
    };
    let dir = crate::safefs::Root::open(&parent(env))?.dir(Path::new(""), false)?;
    if backup(env).exists() {
        verify(store, &backup(env), &local(env), expected)?;
        return Ok(());
    }
    if !local(env).exists() || !stage(env).exists() {
        bail!("registry migration is incomplete; both copies were retained");
    }
    // If the stage is still encrypted, swap it. Otherwise the swap already
    // happened and the stage is the retained plaintext directory.
    if snapshot::scan(&local(env))?.digest == expected {
        verify(store, &local(env), &stage(env), expected)?;
        dir.exchange(OsStr::new("local"), OsStr::new(".local-migrate"))?;
        super::super::store::sync_dir(&parent(env))?;
    }
    verify(store, &stage(env), &local(env), expected)?;
    dir.move_in(&stage(env), OsStr::new("local.plain"))?;
    super::super::store::sync_dir(&parent(env))?;
    Ok(())
}

pub(super) fn remove_backup(env: &HostEnv) -> Result<()> {
    crate::safefs::Root::open(&parent(env))?.remove_tree(Path::new("local.plain"))?;
    super::super::store::sync_dir(&parent(env))
}
