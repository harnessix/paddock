//! Store operations performed under an existing writer lock.

use super::*;

impl super::super::store::Lock<'_> {
    /// Create a horse while holding the store writer lock. `dna` is the
    /// hash of a DNA in the store (`dna::put`).
    pub fn new_horse(&self, dna: &Hash) -> Result<Horse> {
        let store = self.store;
        super::super::dna::get(store, dna).context("the horse needs a DNA in the store")?;
        let h = Horse {
            id: super::super::new_id("h")?,
            dna: dna.clone(),
            sessions: Vec::new(),
            created_ms: super::super::now_ms(),
        };
        if let Some(keys) = &store.keyring {
            keys.create_horse_key(&h.id)?;
        }
        store.save_horse(&h)?;
        Ok(h)
    }
}

impl super::super::store::Lock<'_> {
    /// Point a horse at another DNA in the store. Only `dna::set_horse_dna`
    /// calls it, so every change is an event.
    pub(crate) fn set_dna(&self, horse: &str, dna: &Hash) -> Result<Horse> {
        let mut h = self.store.horse(horse)?;
        h.dna = dna.clone();
        self.store.save_horse(&h)?;
        Ok(h)
    }

    /// Create a session while holding the store writer lock.
    pub fn new_session(&self, horse: &str, binding: Binding) -> Result<Session> {
        self.new_session_first(horse, binding, None)
    }

    /// Create a session whose inbox holds `first` (role, text) before the
    /// session can be seen: `paddock horse append` takes no lock, so an
    /// entry that it queues comes after this one.
    pub fn new_session_first(
        &self,
        horse: &str,
        binding: Binding,
        first: Option<(&str, &str)>,
    ) -> Result<Session> {
        self.make_session(horse, binding, first, None)
    }

    /// Create a host session of `origin`. Its entry in the index of the
    /// host sessions is written first, then the session: a failed write of
    /// the index makes no session, and a crash after it leaves an entry
    /// that names no session, which the next read makes anew. So no host
    /// session is ever there without its entry, and the lookup of the
    /// index (`host_session_at`) never misses one and makes a second
    /// session for the same native id.
    pub(crate) fn new_host_session(
        &self,
        horse: &str,
        binding: Binding,
        origin: HostOrigin,
    ) -> Result<Session> {
        anyhow::ensure!(
            binding.native_id.is_some(),
            "a host session needs a native id"
        );
        self.make_session(horse, binding, None, Some(origin))
    }

    fn make_session(
        &self,
        horse: &str,
        binding: Binding,
        first: Option<(&str, &str)>,
        host: Option<HostOrigin>,
    ) -> Result<Session> {
        let store = self.store;
        let mut h = store.horse(horse)?;
        let s = Session {
            id: super::super::new_id("s")?,
            horse: h.id.clone(),
            head: None,
            binding,
            codex_reader_version: None,
            native_header: None,
            fork_of: None,
            lineage: vec![LineageStep {
                horse: h.id.clone(),
                dna: h.dna.clone(),
                since_ms: super::super::now_ms(),
                until_ms: None,
            }],
            created_ms: super::super::now_ms(),
            host,
            origin_run: None,
        };
        if s.host.is_some() {
            self.index_host_session(&s)?;
            #[cfg(test)]
            if super::host_index::CRASH_AFTER_INDEX.with(std::cell::Cell::get) {
                anyhow::bail!("test: paddock dies after the index entry, before the session");
            }
        }
        if let Some((role, text)) = first {
            store
                .for_horse(&h.id)?
                .write_entry(&s.id, role, text, false)?;
        }
        store.save_session(&s)?;
        h.sessions.push(s.id.clone());
        store.save_horse(&h)?;
        Ok(s)
    }

    /// Change a session while holding the store writer lock.
    pub fn update_session(&self, id: &str, f: impl FnOnce(&mut Session)) -> Result<Session> {
        let mut s = self.store.session(id)?;
        f(&mut s);
        self.store.save_session(&s)?;
        Ok(s)
    }
}

impl super::super::store::Lock<'_> {
    /// Bind an unbound session to a native id, without changing its owner.
    pub fn bind_native(&self, id: &str, native: &str) -> Result<Session> {
        let session = self.store.session(id)?;
        if session
            .binding
            .native_id
            .as_deref()
            .is_some_and(|n| n != native)
        {
            anyhow::bail!("session {id} is already bound to another native id");
        }
        if self
            .store
            .session_by_native(&session.binding.harness, native)?
            .is_some_and(|other| other.id != id)
        {
            anyhow::bail!("native id is already bound to another session");
        }
        self.update_session(id, |s| s.binding.native_id = Some(native.into()))
    }
}
