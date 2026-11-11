//! Explicit, verified migration of a plaintext horse store.
//!
//! The new encrypted tree is built and verified beside the source. An atomic
//! exchange puts it in service, and the original is retained as `horse.plain`
//! until an explicit `--drop-plain`. Interrupted exchanges are recoverable.

mod copy;
mod registry;
mod snapshot;

use crate::env::HostEnv;
use crate::horse::Store;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

const MARKER: &str = ".paddock-migration/verified.json";
const DROP_MARKER: &str = ".paddock-migration/drop-intent.json";
const STAGE: &str = ".horse-migrate";
const BACKUP: &str = "horse.plain";

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Verified {
    version: u32,
    source: String,
    registry: Option<String>,
    nodes: usize,
}

fn parent(root: &Path) -> Result<&Path> {
    if root.file_name() != Some(OsStr::new("horse")) {
        bail!("unexpected horse store path");
    }
    root.parent().context("horse store has no parent")
}

pub(crate) fn backup_path(root: &Path) -> Result<PathBuf> {
    Ok(parent(root)?.join(BACKUP))
}
pub(crate) fn stage_root(root: &Path) -> Result<PathBuf> {
    Ok(parent(root)?.join(STAGE))
}

fn marker(store: &Store) -> Result<Verified> {
    let bytes = store
        .read_ref(MARKER)?
        .context("migration stage is incomplete; all files were retained")?;
    let verified: Verified =
        serde_json::from_slice(&bytes).context("migration verification marker is damaged")?;
    if verified.version != 1 {
        bail!("unknown migration verification version");
    }
    Ok(verified)
}

fn try_lock(root: &Path, create: bool) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(create)
        .truncate(false)
        .mode(crate::privfs::FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join("lock"))?;
    // SAFETY: `file` owns this descriptor, and flock does not outlive it.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!(
            "cannot lock the horse store; stop its writers before migration: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(file)
}

fn directory_or_missing(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => bail!("a plaintext backup path is not a directory; nothing was removed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn no_live_runs(source: &Path, snapshot: &snapshot::Snapshot) -> Result<()> {
    let root = crate::safefs::Root::open(source)?;
    for file in &snapshot.files {
        if file
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == "live")
        {
            let bytes = snapshot::read(&root, file)?;
            let record: crate::horse::live::Record = serde_json::from_slice(&bytes)
                .context("cannot verify a legacy live-run record; migration was not started")?;
            if record.pid == 0 || record.pid > i32::MAX as u32 {
                bail!("legacy live-run record has an invalid process id");
            }
            // SAFETY: signal zero only checks the process; it sends no signal.
            let result = unsafe { libc::kill(record.pid as libc::pid_t, 0) };
            if result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                bail!(
                    "horse run {} is still active; stop it before migration",
                    record.run
                );
            }
        }
    }
    Ok(())
}

fn check_source(source: &Path, verified: &Verified, target: &Store, all_refs: bool) -> Result<()> {
    let snapshot = snapshot::scan(source)?;
    if snapshot.digest != verified.source {
        bail!("the retained plaintext store changed; nothing was removed");
    }
    let nodes = if all_refs {
        copy::verify_all(source, &snapshot, target)?
    } else {
        copy::verify_archive(source, &snapshot, target)?
    };
    if nodes != verified.nodes {
        bail!("the encrypted store does not contain every verified legacy node");
    }
    Ok(())
}

/// Encrypt a plaintext store and verify it before swapping it into place.
pub fn verify(env: &HostEnv) -> Result<usize> {
    let root = crate::horse::root(env);
    let parent = parent(&root)?;
    let stage = stage_root(&root)?;
    let backup = backup_path(&root)?;
    if !parent.is_dir() {
        bail!("there is no horse store to migrate");
    }
    // Reject a replaced root before opening its lock or reading any contents.
    crate::safefs::Root::open(&root).context("cannot open the horse store for migration")?;

    if Store::is_encrypted_path(&root)? {
        let store = Store::open_host(env)?;
        let verified = marker(&store)?;
        if store.read_ref(DROP_MARKER)?.is_some() && !backup.exists() && !stage.exists() {
            bail!("the retained plaintext copy has already been removed");
        }
        if verified.registry.is_some() {
            registry::advance(env, &store, verified.registry.as_deref())?;
        }
        if !backup.exists() && stage.exists() {
            // The exchange completed but moving the original to its backup
            // name did not. Authenticate it before completing that move.
            let _lock = try_lock(&stage, false)?;
            check_source(&stage, &verified, &store, false)?;
            crate::safefs::Root::open(parent)?
                .dir(Path::new(""), false)?
                .move_in(&stage, OsStr::new(BACKUP))?;
            crate::horse::store::sync_dir(parent)?;
        }
        if !backup.is_dir() {
            bail!("there is no retained plaintext copy to verify");
        }
        check_source(&backup, &verified, &store, false)?;
        return Ok(verified.nodes);
    }

    // Wrong or missing keys are checked before touching the legacy store.
    crate::horse::keys::Keyring::for_host(env).preflight()?;
    let source = Store::open_plain_for_migration(&root)?;
    let _lock = try_lock(source.root(), true)?;
    if Store::is_encrypted_path(&root)? {
        bail!("another migration completed; run --verify again");
    }
    if backup.exists() {
        bail!("a plaintext horse backup already exists; it was not replaced");
    }
    let before = snapshot::scan(&root)?;
    no_live_runs(&root, &before)?;

    let (target, verified) = if stage.exists() {
        if !Store::is_encrypted_path(&stage)? {
            bail!("an unrecognized migration stage exists; it was not changed");
        }
        let target = Store::open_migration_stage(env, &stage)?;
        let verified = marker(&target)?;
        check_source(&root, &verified, &target, true)?;
        (target, verified)
    } else {
        std::fs::DirBuilder::new().mode(0o700).create(&stage)?;
        let target = Store::open_migration_stage(env, &stage)?;
        let nodes = copy::build(&root, &before, &target)?;
        let registry = registry::prepare(env, &target)?;
        if snapshot::scan(&root)?.digest != before.digest {
            bail!("plaintext horse store changed during migration; nothing was swapped");
        }
        let verified = Verified {
            version: 1,
            source: before.digest,
            registry,
            nodes,
        };
        target.write_ref(MARKER, &serde_json::to_vec(&verified)?)?;
        check_source(&root, &verified, &target, true)?;
        (target, verified)
    };
    crate::horse::keys::Keyring::for_host(env).preflight()?;
    registry::advance(env, &target, verified.registry.as_deref())?;
    check_source(&root, &verified, &target, true)?;
    crate::horse::keys::Keyring::for_host(env).preflight()?;

    // The exchange is one filesystem operation: at every instant the horse
    // path names either the complete plaintext source or the verified target.
    let dir = crate::safefs::Root::open(parent)?.dir(Path::new(""), false)?;
    dir.exchange(OsStr::new("horse"), OsStr::new(STAGE))?;
    crate::horse::store::sync_dir(parent)?;
    if snapshot::scan(&stage)?.digest != verified.source {
        dir.exchange(OsStr::new("horse"), OsStr::new(STAGE))?;
        crate::horse::store::sync_dir(parent)?;
        bail!("plaintext horse store changed during the swap; both copies were retained");
    }
    dir.move_in(&stage, OsStr::new(BACKUP))?;
    crate::horse::store::sync_dir(parent)?;
    Ok(verified.nodes)
}

/// Remove the retained plaintext copies only after checking the encrypted
/// store and confirming that neither copy changed after verification.
pub fn drop_plain(env: &HostEnv) -> Result<usize> {
    let root = crate::horse::root(env);
    let backup = backup_path(&root)?;
    let store = Store::open_host(env)?;
    let verified = marker(&store)?;
    let horse_backup = directory_or_missing(&backup)?;
    let registry_backup = directory_or_missing(&registry::backup(env))?;
    let intent = store
        .read_ref(DROP_MARKER)?
        .map(|bytes| serde_json::from_slice::<Verified>(&bytes))
        .transpose()?;
    if intent.as_ref().is_some_and(|saved| saved != &verified) {
        bail!("plaintext removal record does not match the verified migration");
    }
    if intent.is_none() && !horse_backup {
        bail!("there is no verified plaintext horse copy to remove");
    }
    let _lock = if horse_backup {
        Some(try_lock(&backup, false)?)
    } else {
        None
    };
    if horse_backup {
        no_live_runs(&backup, &snapshot::scan(&backup)?)?;
        check_source(&backup, &verified, &store, false)?;
    }
    if let Some(expected) = &verified.registry {
        if intent.is_none() && !registry_backup {
            bail!("there is no verified plaintext registry copy to remove");
        }
        if registry_backup {
            registry::verify(
                &store,
                &registry::backup(env),
                &env.paddock_state().join("registry/local"),
                expected,
            )?;
        }
    }
    if intent.is_none() {
        // Make an explicit removal retryable if a later deletion fails.
        store.write_ref(DROP_MARKER, &serde_json::to_vec(&verified)?)?;
    }
    // The caller explicitly chose --drop-plain. The no-follow remover refuses
    // symlinks along the path and never reaches outside the chosen backup.
    if verified.registry.is_some() && registry_backup {
        registry::remove_backup(env)?;
    }
    if horse_backup {
        crate::safefs::Root::open(parent(&root)?)?.remove_tree(Path::new(BACKUP))?;
        crate::horse::store::sync_dir(parent(&root)?)?;
    }
    Ok(verified.nodes)
}

#[cfg(test)]
mod tests;
