//! The index of the host sessions: (harness, home, native id) to session.
//!
//! A hook of an adopted harness looks up one host session per event. A
//! scan of every session would read and decrypt each session file of the
//! store, so its cost would grow with the store. The index is one small
//! ref, `host/index.json`, that the writer lock keeps current: each new
//! host session goes in when it is made (target.rs).
//!
//! A store without the index, or with an index that paddock cannot read,
//! falls back to the scan. The first new host session after that writes
//! the index from a full scan, so no older host session is left out.

use super::super::store::Lock;
use super::*;
use std::collections::BTreeMap;
use std::path::Path;

const INDEX: &str = "host/index.json";

#[cfg(test)]
thread_local! {
    /// Tests: the write of the index fails.
    pub(crate) static FAIL_INDEX_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests: paddock dies after the index entry of a new host session is
    /// written, before the session is (session/locked.rs).
    pub(crate) static CRASH_AFTER_INDEX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    /// key (see [`key`]) to session id.
    sessions: BTreeMap<String, String>,
}

/// The key of a host session. The home is part of it: a copy of a home
/// (`paddock horse import --home`) has the same native ids as the home.
fn key(harness: &str, home: &Path, native: &str) -> String {
    format!("{harness}\n{}\n{native}", home.display())
}

fn key_of(s: &Session) -> Option<String> {
    let h = s.host.as_ref()?;
    Some(key(
        &s.binding.harness,
        &h.home,
        s.binding.native_id.as_deref()?,
    ))
}

impl Store {
    fn host_index(&self) -> Option<Index> {
        match self.read_ref(INDEX) {
            Ok(Some(b)) => match serde_json::from_slice(&b) {
                Ok(ix) => Some(ix),
                Err(e) => {
                    log::debug!("horse: the host session index is damaged: {e}");
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                log::debug!("horse: cannot read the host session index: {e:#}");
                None
            }
        }
    }

    /// Every host session, from a scan of the store.
    fn scan_host_sessions(&self) -> Result<Vec<Session>> {
        Ok(self
            .sessions()?
            .into_iter()
            .filter(|s| s.host.is_some())
            .collect())
    }

    /// The session that an entry of the index names. The entry is written
    /// before the session (session/locked.rs): a crash between the two
    /// leaves an entry without a session. That is no session, and the next
    /// one of the key takes the entry. Any other error is an error, so a
    /// session that cannot be read now does not get a second one.
    fn indexed_session(&self, id: &str) -> Result<Option<Session>> {
        check_id(id)?;
        match self.read_ref(&format!("sessions/{id}.json"))? {
            None => {
                log::debug!("horse: the host session index names {id}, which is not there");
                Ok(None)
            }
            Some(_) => Ok(Some(self.session(id)?)),
        }
    }

    /// The host session of this native id of a harness, of any home.
    /// A session of the index that cannot be read is an error, as in
    /// [`Store::host_session_at`].
    pub fn host_session(&self, harness: &str, native_id: &str) -> Result<Option<Session>> {
        let found: Vec<Session> = match self.host_index() {
            Some(ix) => {
                let mut v = Vec::new();
                for (k, id) in &ix.sessions {
                    let mut parts = k.splitn(2, '\n');
                    let same_harness = parts.next() == Some(harness);
                    if same_harness && k.ends_with(&format!("\n{native_id}")) {
                        v.extend(self.indexed_session(id)?);
                    }
                }
                v
            }
            None => self.scan_host_sessions()?,
        }
        .into_iter()
        .filter(|s| {
            s.binding.harness == harness && s.binding.native_id.as_deref() == Some(native_id)
        })
        .collect();
        anyhow::ensure!(
            found.len() <= 1,
            "the host session {harness} {native_id} is in the store more than once; name the home"
        );
        Ok(found.into_iter().next())
    }

    /// The host session of this native id of a harness in the host home
    /// `home`. It reads the index and one session file.
    pub fn host_session_at(
        &self,
        harness: &str,
        native_id: &str,
        home: &Path,
    ) -> Result<Option<Session>> {
        let k = key(harness, home, native_id);
        let candidates = match self.host_index() {
            Some(ix) => match ix.sessions.get(&k) {
                Some(id) => self.indexed_session(id)?.into_iter().collect(),
                None => Vec::new(),
            },
            None => self.scan_host_sessions()?,
        };
        let mut found = candidates
            .into_iter()
            .filter(|s| key_of(s).as_deref() == Some(k.as_str()));
        let first = found.next();
        anyhow::ensure!(
            found.next().is_none(),
            "the host session {harness} {native_id} of {} is in the store more than once",
            home.display()
        );
        Ok(first)
    }
}

impl Lock<'_> {
    /// Put the host session `s` into the index. An index that is not there
    /// (or that paddock cannot read) is made again from a scan first.
    pub(crate) fn index_host_session(&self, s: &Session) -> Result<()> {
        let store = self.store;
        let mut ix = match store.host_index() {
            Some(ix) => ix,
            None => {
                let mut ix = Index::default();
                for o in store.scan_host_sessions()? {
                    if let Some(k) = key_of(&o) {
                        ix.sessions.insert(k, o.id);
                    }
                }
                ix
            }
        };
        let k = key_of(s).context("a host session needs a host origin and a native id")?;
        ix.sessions.insert(k, s.id.clone());
        #[cfg(test)]
        if FAIL_INDEX_WRITE.with(std::cell::Cell::get) {
            anyhow::bail!("test: the write of the host session index fails");
        }
        store.write_ref(INDEX, &serde_json::to_vec_pretty(&ix)?)
    }
}

impl Lock<'_> {
    /// Take out a host session that a failed read made just now: its ref,
    /// its place in its horse and in the index. Its objects stay (they are
    /// shared by hash and harmless).
    pub(crate) fn discard_host_session(&self, id: &str) -> Result<()> {
        let store = self.store;
        let s = store.session(id)?;
        anyhow::ensure!(s.host.is_some(), "session {id} is no host session");
        let mut h = store.horse(&s.horse)?;
        h.sessions.retain(|x| x != id);
        store.save_horse(&h)?;
        if let Some(mut ix) = store.host_index() {
            ix.sessions.retain(|_, v| v != id);
            store.write_ref(INDEX, &serde_json::to_vec_pretty(&ix)?)?;
        }
        store.remove_ref(&format!("sessions/{id}.json"))
    }
}
