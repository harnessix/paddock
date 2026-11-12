//! Which session a native file goes into when a codec reads it.
//!
//! - `Any`: the session with the native id anywhere in the store, else a
//!   new one in the horse, or in a new horse.
//! - `Run`: a session of the run's horse with the native id, else a new
//!   one there.
//! - `Host`: the host session of the native id in the home of the
//!   origin, else a new host session in the horse.
//!
//! `Any` and `Run` never find a host session, and `Host` finds nothing
//! else. So the rows of a run never go into a host session, and the rows
//! of the host file never go into the session of a run.

use super::store::Lock;
use super::{Binding, HostOrigin, Session};
use anyhow::Result;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Target<'a> {
    Any(Option<&'a str>),
    Run(&'a str),
    Host {
        horse: &'a str,
        origin: &'a HostOrigin,
    },
}

impl Target<'_> {
    /// The scoped forms ignore the active-session refs of Codex and Grok.
    pub(crate) fn scoped(&self) -> bool {
        !matches!(self, Target::Any(_))
    }

    /// The existing session of `native`, if any.
    pub(crate) fn find(
        &self,
        locked: &Lock<'_>,
        harness: &str,
        native: &str,
    ) -> Result<Option<Session>> {
        let store = locked.store;
        match self {
            Target::Any(_) => store.session_by_native(harness, native),
            Target::Run(horse) => Ok(store.sessions()?.into_iter().find(|s| {
                s.host.is_none()
                    && s.horse == *horse
                    && s.binding.harness == harness
                    && s.binding.native_id.as_deref() == Some(native)
            })),
            Target::Host { origin, .. } => {
                let Some(s) = store.host_session_at(harness, native, &origin.home)? else {
                    return Ok(None);
                };
                let origin = (*origin).clone();
                Ok(Some(locked.update_session(&s.id, |s| {
                    if let Some(h) = &mut s.host {
                        h.read_ms = origin.read_ms;
                        h.path = origin.path.clone();
                    }
                })?))
            }
        }
    }

    /// A new session for `binding`.
    pub(crate) fn create(&self, locked: &Lock<'_>, binding: Binding) -> Result<Session> {
        let horse = match self {
            Target::Any(Some(h)) | Target::Run(h) | Target::Host { horse: h, .. } => {
                locked.store.horse(h)?
            }
            Target::Any(None) => super::dna::new_default_horse_locked(locked)?,
        };
        match self {
            // The index entry and the host origin come with the session,
            // not after it (session/locked.rs).
            Target::Host { origin, .. } => {
                locked.new_host_session(&horse.id, binding, (*origin).clone())
            }
            _ => locked.new_session(&horse.id, binding),
        }
    }

    /// The existing session of `native`, else a new one for `binding`.
    pub(crate) fn session(
        &self,
        locked: &Lock<'_>,
        harness: &str,
        native: &str,
        binding: impl FnOnce() -> Binding,
    ) -> Result<Session> {
        match self.find(locked, harness, native)? {
            Some(s) => Ok(s),
            None => self.create(locked, binding()),
        }
    }
}
