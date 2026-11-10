//! Host-side wiring of hitch mode: the link that a hitched child carries,
//! and the finish that runs once per child.
//!
//! The parent's host process makes the child's session before the child
//! starts and keeps the link in the child's spawn record and job file.
//! Nothing of it reaches the child sandbox.

use super::{child_finished_locked, edges, spawn_child, EdgeKind};
use crate::horse::{claude, codex, device::Access, dsh, inbox::Entry, run::RunHorse, Store};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// A hitched child's place in its parent's horse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hitched {
    pub horse: String,
    pub session: String,
    pub harness: String,
    pub parent_session: String,
}

impl Hitched {
    pub fn run(&self) -> RunHorse {
        RunHorse {
            horse: self.horse.clone(),
            session: self.session.clone(),
            harness: self.harness.clone(),
        }
    }
}

/// The codec version that a hitched child of this harness must run.
pub fn pinned_version(harness: &str) -> Result<&'static str> {
    Ok(match harness {
        dsh::HARNESS => dsh::VERSION,
        claude::HARNESS => claude::VERSION,
        codex::HARNESS => codex::VERSION,
        _ => bail!("{harness} has no horse codec, so it cannot be hitched"),
    })
}

/// The run of the parent, from its device access.
pub fn parent_run(access: &Access) -> Result<RunHorse> {
    let session = access.store.session(&access.session)?;
    Ok(RunHorse {
        horse: access.horse.clone(),
        session: access.session.clone(),
        harness: session.binding.harness,
    })
}

/// Make the child's session in the parent's horse before the child starts.
pub fn begin(store: &Store, parent: &RunHorse, harness: &str) -> Result<Hitched> {
    let child = spawn_child(store, parent, harness, pinned_version(harness)?)?;
    Ok(Hitched {
        horse: child.horse,
        session: child.session,
        harness: child.harness,
        parent_session: parent.session.clone(),
    })
}

/// Queue the child's result to the parent once. The first caller wins: a
/// second call finds the Append edge and does nothing.
pub fn finish_once(store: &Store, hitched: &Hitched, text: &str) -> Result<Option<Entry>> {
    let locked = store.lock()?;
    let done = edges(store, &hitched.horse)?.iter().any(|edge| {
        edge.kind == EdgeKind::Append
            && edge.from == hitched.session
            && edge.to == hitched.parent_session
    });
    if done {
        return Ok(None);
    }
    child_finished_locked(&locked, &hitched.run(), &hitched.parent_session, text).map(Some)
}

/// The hitch link of a run, read from its own SpawnedFrom edge. None for a
/// run that was not hitched.
pub fn link_of(access: &Access) -> Result<Option<Hitched>> {
    let Some(edge) = edges(&access.store, &access.horse)?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::SpawnedFrom && edge.from == access.session)
    else {
        return Ok(None);
    };
    let session = access.store.session(&access.session)?;
    Ok(Some(Hitched {
        horse: access.horse.clone(),
        session: access.session.clone(),
        harness: session.binding.harness,
        parent_session: edge.to,
    }))
}

/// `paddock-host done` of a run: when it is a hitched child, its text goes
/// to the parent now. True when this run is a hitched child. Errors are
/// logged, the done itself goes on as before.
pub fn report_done(access: &Access, status: &str, text: &str) -> bool {
    let text = if status == "ok" {
        text.to_string()
    } else {
        format!("{status}: {text}")
    };
    let text = if text.is_empty() { "done".into() } else { text };
    let link = match link_of(access) {
        Ok(Some(link)) => link,
        Ok(None) => return false,
        Err(e) => {
            log::warn!("hitch: cannot read the link to the parent: {e:#}");
            return false;
        }
    };
    match finish_once(&access.store, &link, &text) {
        Ok(Some(_)) => log::info!("hitch: the result of {} goes to its parent", access.session),
        Ok(None) => {}
        Err(e) => log::warn!("hitch: cannot queue the result to the parent: {e:#}"),
    }
    true
}

#[cfg(test)]
mod race_tests;
#[cfg(test)]
mod tests;
