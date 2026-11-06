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
        let store = self.store;
        let mut h = store.horse(horse)?;
        let s = Session {
            id: super::super::new_id("s")?,
            horse: h.id.clone(),
            head: None,
            binding,
            native_header: None,
            fork_of: None,
            lineage: vec![LineageStep {
                horse: h.id.clone(),
                dna: h.dna.clone(),
                since_ms: super::super::now_ms(),
                until_ms: None,
            }],
            created_ms: super::super::now_ms(),
        };
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
