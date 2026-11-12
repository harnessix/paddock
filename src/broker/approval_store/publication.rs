//! Publish and update approval presentations.

use super::*;

pub fn write_pending(env: &HostEnv, p: &Pending) -> std::io::Result<PathBuf> {
    write_pending_with(env, p, false).map_err(|e| match e {
        PublishError::Contended => io::Error::from(io::ErrorKind::WouldBlock),
        PublishError::Failed(e) => e,
    })
}

pub(in super::super) fn try_write_pending(
    env: &HostEnv,
    p: &Pending,
) -> std::result::Result<PathBuf, PublishError> {
    write_pending_with(env, p, true)
}

fn write_pending_with(
    env: &HostEnv,
    p: &Pending,
    nonblocking: bool,
) -> std::result::Result<PathBuf, PublishError> {
    let p = &p.cleaned();
    if !valid_id(&p.id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid request id").into());
    }
    let dir = approvals_dir(env);
    let pending = dir.join(format!("{}.json", p.id));
    // The lease is opened and locked before taking the global store lock.
    let mut prepared_lease = if p.capability != CONNECT_CAPABILITY {
        Some(lease::PreparedLease::new(&dir)?)
    } else {
        None
    };
    let mail_key = {
        let _lock = match lock_approvals_with(&dir, &p.id, None, nonblocking) {
            Ok(lock) => lock,
            Err(e) if nonblocking && e.kind() == io::ErrorKind::WouldBlock => {
                return Err(PublishError::Contended);
            }
            Err(e) => return Err(e.into()),
        };
        let answer = dir.join(format!("{}.answer", p.id));
        let owner = dir.join(format!("{}.owner", p.id));
        let done = dir.join(format!("{}.done", p.id));
        let cleanup = dir.join(format!("{}.cleanup", p.id));
        let retired = dir.join(format!("{}.retired", p.id));
        for path in [&pending, &answer, &owner, &done, &cleanup, &retired] {
            match std::fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "approval id is in use",
                    )
                    .into())
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        let json = serde_json::to_string(p).map_err(io::Error::other)?;
        let lease = match prepared_lease.take() {
            Some(lease) => Some(lease.publish(&owner)?),
            None => None,
        };
        if let Err(e) = crate::privfs::write_atomic(&pending, &json) {
            if lease.is_some() {
                let _ = remove_if_present(&owner);
            }
            return Err(e.into());
        }
        if let Some(file) = lease {
            owners()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(owner, file);
        }
        card_mail_key(p, &json)
    };
    notify_card(env, mail_key);
    Ok(pending)
}

/// Atomically update an existing presentation. It must never recreate a
/// card that was closed while an update was being prepared.
pub(crate) fn update_pending(env: &HostEnv, p: &Pending) -> io::Result<()> {
    let p = &p.cleaned();
    if !valid_id(&p.id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid request id",
        ));
    }
    let dir = approvals_dir(env);
    {
        let _lock = lock_approvals(&dir, &p.id)?;
        let path = dir.join(format!("{}.json", p.id));
        let old: Pending =
            serde_json::from_str(&evidence::read_text(&path)?).map_err(io::Error::other)?;
        if old.id != p.id || old.capability != p.capability {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "approval identity changed",
            ));
        }
        if old.capability == CONNECT_CAPABILITY && answer_text(&dir, &p.id)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "answered connect card cannot be updated",
            ));
        }
        let json = serde_json::to_string(p).map_err(io::Error::other)?;
        crate::privfs::write_atomic(&path, &json)?;
    }
    Ok(())
}
