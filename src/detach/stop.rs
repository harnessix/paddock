//! Stop requests survive concurrent writes to a run record.
use super::{records_dir, Record};
use crate::env::HostEnv;
use anyhow::{bail, Result};
use std::path::PathBuf;

pub(super) fn stop_file(env: &HostEnv, id: &str) -> PathBuf {
    records_dir(env).join(format!("{id}.stop"))
}

/// Whether `paddock stop` recorded a stop request for this run.
pub fn was_stopped(env: &HostEnv, id: &str) -> bool {
    crate::view::valid_id(id) && stop_file(env, id).is_file()
}

/// `paddock stop` asked this run to end. The marker is a separate file so a
/// later write of the run record cannot drop it.
pub fn mark_stopped(env: &HostEnv, id: &str) -> Result<()> {
    if !crate::view::valid_id(id) {
        bail!("{id} is not a sandbox id");
    }
    crate::privfs::dir(&records_dir(env))?;
    crate::privfs::write(&stop_file(env, id), b"stopped\n")?;
    if Record::load(env, id).is_some() {
        let _ = Record::update(env, id, |r| r.stopped = true);
    }
    Ok(())
}
