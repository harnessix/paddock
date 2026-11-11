//! Move a session to another horse.
//!
//! A move closes the current lineage step of the session and opens one in
//! the target horse. The DNA of the source horse grafts into the target
//! (the source is the donor), after the compatibility check. The session
//! gets the DNA of its new horse as one `system` entry at the end, so the
//! prefix guard records a move as a safe append.

use super::dna::{self, graft};
use super::guard::{self, Op};
use super::store::Lock;
use super::{run, Hash, LineageStep, Session, Store};
use anyhow::{bail, Result};
use serde_json::json;

impl Lock<'_> {
    /// Move a session to horse `to` while holding the store writer lock:
    /// close its lineage step, open one with the DNA of `to`, and move it
    /// between the session lists of the two horses.
    pub fn move_session(&self, session: &str, to: &str) -> Result<Session> {
        let store = self.store;
        let mut s = store.session(session)?;
        if s.horse == to {
            bail!("session {session} is in horse {to} already");
        }
        let mut from = store.horse(&s.horse)?;
        let mut target = store.horse(to)?;
        let now = super::now_ms();
        if let Some(last) = s.lineage.last_mut() {
            last.until_ms.get_or_insert(now);
        }
        s.lineage.push(LineageStep {
            horse: target.id.clone(),
            dna: target.dna.clone(),
            since_ms: now,
            until_ms: None,
        });
        s.horse = target.id.clone();
        from.sessions.retain(|x| x != session);
        if !target.sessions.iter().any(|x| x == session) {
            target.sessions.push(session.into());
        }
        store.save_session(&s)?;
        store.save_horse(&from)?;
        store.save_horse(&target)?;
        Ok(s)
    }
}

impl Store {
    /// Move a session to horse `to`. Only the lineage and the session
    /// lists change: `move_to` adds the graft and the entry.
    pub fn move_session(&self, session: &str, to: &str) -> Result<Session> {
        self.lock()?.move_session(session, to)
    }
}

/// The result of `move_to`.
#[derive(Debug)]
pub struct Moved {
    pub session: Session,
    pub from: String,
    /// The new DNA of the target, when the graft changed it.
    pub graft: Option<Hash>,
    pub verdict: Option<guard::Verdict>,
}

/// Move a session to horse `to`: compatibility check, guard verdict,
/// graft of the source DNA into the target, lineage step, one `system`
/// entry and one `session.move` event in each horse. A running session or
/// a session with hitch edges does not move: its run belongs to the
/// source horse.
pub fn move_to(store: &Store, session: &str, to: &str) -> Result<Moved> {
    let locked = store.lock()?;
    let s = store.session(session)?;
    let from = store.horse(&s.horse)?;
    let target = store.horse(to)?;
    if from.id == target.id {
        bail!("session {session} is in horse {to} already");
    }
    if dna::running_sessions(store, &from)?.contains(&s.id) {
        bail!("session {session} runs now; move it when its run ends");
    }
    if super::hitch::edges(store, &from.id)?
        .iter()
        .any(|e| e.from == s.id || e.to == s.id)
    {
        bail!("session {session} has hitch edges in horse {}", from.id);
    }
    let compat = graft::check(
        &dna::get_or_default(store, &target.dna),
        &dna::get_or_default(store, &from.dna),
    );
    if let graft::Compat::Incompatible(list) = compat {
        bail!(
            "the DNA of horse {} is incompatible with horse {to}: {}",
            from.id,
            list.join(", ")
        );
    }
    let verdict = match guard::check(
        store,
        session,
        &Op::MoveHorse { to: to.into() },
        super::now_ms(),
    ) {
        Ok(v) => {
            store.save_verdict(&v)?;
            Some(v)
        }
        Err(e) => {
            log::debug!("horse guard {session}: {e:#}");
            None
        }
    };
    let grafted = graft::graft_horses_locked(&locked, to, &from.id, &format!("move {session}"))?;
    let moved = locked.move_session(session, to)?;
    let now = store.horse(to)?;
    let text = dna::render_entry(
        &dna::get_or_default(store, &now.dna),
        &format!("move from {}", from.id),
    );
    store.queue_append(session, "system", &text)?;
    let body = json!({"type": "session.move", "session": session, "from_horse": from.id,
        "to_horse": to, "at_ms": super::now_ms()});
    run::append_horse_event_locked(&locked, &from.id, None, &body)?;
    run::append_horse_event_locked(&locked, to, None, &body)?;
    Ok(Moved {
        session: moved,
        from: from.id,
        graft: grafted,
        verdict,
    })
}

#[cfg(test)]
mod tests;
