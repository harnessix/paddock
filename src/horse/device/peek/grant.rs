//! Read grants and their audit records, kept in the encrypted store.

use super::super::Access;
use crate::horse::Store;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use zeroize::{Zeroize, Zeroizing};

const OWN_BYTES: u64 = 1024 * 1024 * 1024;
const OWN_LIFETIME_MS: u64 = 30 * 24 * 60 * 60 * 1000;
const MAX_READS: usize = 10_000;
const RATE_PER_MINUTE: usize = 120;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Read {
    pub at_ms: u64,
    pub operation: String,
    pub selectors: Vec<String>,
    pub bytes: u64,
    pub status: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Grant {
    pub sandbox: String,
    pub id: String,
    pub horse: String,
    pub session: String,
    pub expires_ms: u64,
    pub max_bytes: u64,
    pub used_bytes: u64,
    pub revoked: bool,
    pub reads: Vec<Read>,
    pub(super) mount_key: [u8; 32],
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.mount_key.zeroize();
    }
}

fn path(sandbox: &str, id: &str) -> Result<String> {
    if !super::super::ctl::grant::valid_sandbox(sandbox)
        || !super::super::ctl::grant::valid_sandbox(id)
    {
        bail!("invalid peek grant name");
    }
    Ok(format!("peek/grants/{sandbox}/{id}.json"))
}

pub(crate) fn get(store: &Store, sandbox: &str, id: &str) -> Result<Option<Grant>> {
    store
        .read_ref(&path(sandbox, id)?)?
        .map(|bytes| serde_json::from_slice(&Zeroizing::new(bytes)).map_err(Into::into))
        .transpose()
}

/// A revoked or expired grant still identifies a trainer until its run ends.
pub(crate) fn has_trainer(store: &Store, sandbox: &str) -> Result<bool> {
    Ok(get(store, sandbox, "trainer")?.is_some())
}

/// Whether a live trainer grant is bound to this sandbox.
pub(crate) fn is_trainer(store: &Store, sandbox: &str) -> Result<bool> {
    match get(store, sandbox, "trainer")? {
        Some(grant) => {
            check(&grant, sandbox, "trainer")?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Read counters without exposing the grant's encryption key.
pub fn stats(store: &Store, sandbox: &str, id: &str) -> Result<Option<(u64, usize)>> {
    Ok(get(store, sandbox, id)?.map(|grant| (grant.used_bytes, grant.reads.len())))
}

fn put(store: &Store, grant: &Grant) -> Result<()> {
    store.write_ref(
        &path(&grant.sandbox, &grant.id)?,
        &Zeroizing::new(serde_json::to_vec(grant)?),
    )
}

/// Issue the run's own grant once. Reopening a lifecycle never restores a
/// revoked grant or resets its counters.
pub fn issue_own(access: &Access, sandbox: &str) -> Result<()> {
    let session = access.store.session(&access.session)?;
    if session.horse != access.horse {
        bail!("run session is not owned by its horse");
    }
    let _lock = access.store.lock()?;
    if let Some(existing) = get(&access.store, sandbox, "own")? {
        if existing.horse != access.horse || existing.session != access.session {
            bail!("peek grant already belongs to another session");
        }
        return Ok(());
    }
    put(
        &access.store,
        &Grant {
            sandbox: sandbox.into(),
            id: "own".into(),
            horse: access.horse.clone(),
            session: access.session.clone(),
            expires_ms: crate::horse::now_ms().saturating_add(OWN_LIFETIME_MS),
            max_bytes: OWN_BYTES,
            used_bytes: 0,
            revoked: false,
            reads: Vec::new(),
            mount_key: crate::horse::random_bytes()?,
        },
    )
}

/// Issue a scoped trainer grant from host code; no broker request can call
/// this. The encrypted grant record and its key stay on the host; the grant
/// is bound to one sandbox.
pub fn issue_trainer(
    store: &Store,
    sandbox: &str,
    id: &str,
    horse: &str,
    session: &str,
    expires_ms: u64,
    max_bytes: u64,
) -> Result<()> {
    let now = crate::horse::now_ms();
    if id == "own"
        || expires_ms <= now
        || expires_ms > now.saturating_add(OWN_LIFETIME_MS)
        || max_bytes == 0
    {
        bail!("invalid trainer peek grant");
    }
    if store.session(session)?.horse != horse {
        bail!("trainer grant target does not belong to that horse");
    }
    let _lock = store.lock()?;
    if get(store, sandbox, id)?.is_some() {
        bail!("peek grant already exists");
    }
    put(
        store,
        &Grant {
            sandbox: sandbox.into(),
            id: id.into(),
            horse: horse.into(),
            session: session.into(),
            expires_ms,
            max_bytes,
            used_bytes: 0,
            revoked: false,
            reads: Vec::new(),
            mount_key: crate::horse::random_bytes()?,
        },
    )
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn active(store: &Store, sandbox: &str) -> Result<Vec<Grant>> {
    if !super::super::ctl::grant::valid_sandbox(sandbox) {
        bail!("invalid sandbox id");
    }
    let directory = format!("peek/grants/{sandbox}");
    if !store.root().join(&directory).is_dir() {
        return Ok(Vec::new());
    }
    let mut grants = Vec::new();
    for filename in store.list_refs(&directory)? {
        let Some(id) = filename.strip_suffix(".json") else {
            continue;
        };
        if let Some(grant) = get(store, sandbox, id)? {
            if check(&grant, sandbox, id).is_ok() {
                grants.push(grant);
            }
        }
    }
    Ok(grants)
}

pub fn revoke(store: &Store, sandbox: &str) -> Result<bool> {
    if !super::super::ctl::grant::valid_sandbox(sandbox) {
        bail!("invalid sandbox id");
    }
    let directory = format!("peek/grants/{sandbox}");
    if !store.root().join(&directory).is_dir() {
        return Ok(false);
    }
    let mut found = false;
    for filename in store.list_refs(&directory)? {
        let Some(id) = filename.strip_suffix(".json") else {
            continue;
        };
        found |= revoke_one(store, sandbox, id)?;
    }
    Ok(found)
}

/// Revoke one grant and erase its mount key while retaining its audit log.
pub fn revoke_one(store: &Store, sandbox: &str, id: &str) -> Result<bool> {
    if get(store, sandbox, id)?.is_none() {
        return Ok(false);
    }
    let _lock = lock(store, sandbox, id)?;
    let Some(mut grant) = get(store, sandbox, id)? else {
        return Ok(false);
    };
    if grant.revoked {
        return Ok(false);
    }
    grant.revoked = true;
    grant.mount_key.zeroize();
    put(store, &grant)?;
    Ok(true)
}

/// Serialize reads and revocation of one grant without holding the global
/// horse writer lock while decrypting or searching a long trajectory.
pub(super) fn lock(store: &Store, sandbox: &str, id: &str) -> Result<std::fs::File> {
    path(sandbox, id)?;
    let directory = store.root().join("peek/locks").join(sandbox);
    crate::privfs::dir(&directory)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(crate::privfs::FILE_MODE)
        .open(directory.join(format!("{id}.lock")))?;
    // SAFETY: the file descriptor belongs to `file` for the duration of flock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

pub(super) fn check(grant: &Grant, sandbox: &str, id: &str) -> Result<()> {
    if grant.sandbox != sandbox
        || grant.id != id
        || grant.revoked
        || crate::horse::now_ms() >= grant.expires_ms
    {
        bail!("peek grant is revoked, expired, or belongs to another run");
    }
    Ok(())
}

pub(super) fn remaining(grant: &Grant) -> Result<u64> {
    let now = crate::horse::now_ms();
    if grant.reads.len() >= MAX_READS
        || grant
            .reads
            .iter()
            .rev()
            .take_while(|r| now.saturating_sub(r.at_ms) < 60_000)
            .count()
            >= RATE_PER_MINUTE
    {
        bail!("peek grant read limit reached");
    }
    Ok(grant.max_bytes.saturating_sub(grant.used_bytes))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn record(
    store: &Store,
    _guard: &std::fs::File,
    sandbox: &str,
    id: &str,
    horse: &str,
    session: &str,
    operation: &str,
    selectors: Vec<String>,
    bytes: u64,
    status: &str,
) -> Result<()> {
    let mut grant =
        get(store, sandbox, id)?.ok_or_else(|| anyhow::anyhow!("no peek grant for this run"))?;
    check(&grant, sandbox, id)?;
    if grant.horse != horse || grant.session != session {
        bail!("peek grant no longer matches its target");
    }
    remaining(&grant)?;
    let now = crate::horse::now_ms();
    let total = grant
        .used_bytes
        .checked_add(bytes)
        .ok_or_else(|| anyhow::anyhow!("peek byte count overflow"))?;
    if total > grant.max_bytes {
        bail!("peek grant byte limit reached");
    }
    grant.used_bytes = total;
    grant.reads.push(Read {
        at_ms: now,
        operation: operation.into(),
        selectors,
        bytes,
        status: status.into(),
    });
    put(store, &grant)
}
