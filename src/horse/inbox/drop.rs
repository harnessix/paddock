//! Take an entry out of the inbox with no delivery: the harness refused
//! it too often. The caller records why.

use super::dir;
use crate::horse::store::Store;
use anyhow::{Context as _, Result};

impl Store {
    /// Remove entry `id` from the inbox of `session`. False when it is
    /// not there (any more).
    pub fn drop_entry(&self, session: &str, id: &str) -> Result<bool> {
        let _l = self.lock()?;
        let path = self.root().join(format!("{}/{id}.json", dir(session)));
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(x) => Err(x).with_context(|| format!("cannot remove {}", path.display())),
        }
    }
}
