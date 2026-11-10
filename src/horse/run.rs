//! Run ownership primitives for the horse store.
//!
//! These are deliberately separate from custody: the native session belongs
//! to the run even when the harness has no live protocol integration.

use super::{agy, claude, codex, dsh, grok, Binding, Hash, Store};
use anyhow::{bail, Result};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunHorse {
    pub horse: String,
    pub session: String,
    pub harness: String,
}

/// Check the codec before creating any objects in the store.
///
/// `parent`: a hitched child, it joins the horse of the parent run.
/// `inherit_from`: a plain spawn from a run in horse mode; the new horse
/// is a child of this horse and inherits its DNA. Neither: a new horse
/// with the default DNA. The first inbox entry of the new session tells
/// the model the DNA of its horse.
pub fn begin(
    store: &Store,
    harness: &str,
    version: &str,
    inherit_from: Option<&str>,
    parent: Option<&RunHorse>,
) -> Result<RunHorse> {
    let container = match harness {
        dsh::HARNESS => {
            dsh::check_version(version)?;
            dsh::CONTAINER
        }
        claude::HARNESS => {
            claude::check_version(version)?;
            claude::CONTAINER
        }
        codex::HARNESS => {
            codex::check_version(version)?;
            codex::CONTAINER
        }
        grok::HARNESS => {
            grok::check_version(version)?;
            grok::CONTAINER
        }
        agy::HARNESS => {
            agy::check_version(version)?;
            agy::CONTAINER
        }
        _ => bail!("no pinned horse codec for {harness}"),
    };
    let locked = store.lock()?;
    let horse = match parent {
        Some(parent) => {
            let session = store.session(&parent.session)?;
            anyhow::ensure!(
                session.horse == parent.horse && session.binding.harness == parent.harness,
                "parent run does not match its session"
            );
            store.horse(&parent.horse)?
        }
        None => match inherit_from {
            Some(from) => super::dna::new_child_horse_locked(&locked, from)?,
            None => super::dna::new_default_horse_locked(&locked)?,
        },
    };
    // The DNA entry goes in with the session, so it is the first entry
    // also when another process appends at once.
    let dna = super::dna::get_or_default(store, &horse.dna);
    let session = locked.new_session_first(
        &horse.id,
        Binding {
            harness: harness.into(),
            native_id: None,
            version: version.into(),
            container: container.into(),
        },
        Some(("system", &super::dna::render_entry(&dna, "start"))),
    )?;
    if harness == codex::HARNESS {
        locked.update_session(&session.id, |s| {
            s.codex_reader_version = Some(version.into())
        })?;
    }
    Ok(RunHorse {
        horse: horse.id,
        session: session.id,
        harness: harness.into(),
    })
}

/// Append an event to a horse-specific chain. The ref update and object
/// creation are serialized with the store's writer lock.
pub fn append_event(store: &Store, rh: &RunHorse, body: &Value) -> Result<Hash> {
    append_event_locked(&store.lock()?, rh, body)
}

/// Append an event while holding the store writer lock.
pub fn append_event_locked(
    locked: &super::store::Lock<'_>,
    rh: &RunHorse,
    body: &Value,
) -> Result<Hash> {
    let horse = locked.store.horse(&rh.horse)?;
    if !horse.sessions.contains(&rh.session) {
        bail!("session is not owned by horse");
    }
    append_horse_event_locked(locked, &rh.horse, Some(&rh.harness), body)
}

/// Append an event to the chain of a horse, for an event that belongs to
/// the horse and not to one of its sessions (a DNA change).
pub fn append_horse_event_locked(
    locked: &super::store::Lock<'_>,
    horse: &str,
    harness: Option<&str>,
    body: &Value,
) -> Result<Hash> {
    let store = locked.store;
    store.horse(horse)?;
    let path = format!("events/{horse}.json");
    let parent = store
        .read_ref(&path)?
        .map(|b| serde_json::from_slice::<Hash>(&b))
        .transpose()?;
    let meta = super::store::Meta {
        kind: "event".into(),
        harness: harness.map(String::from),
        container: None,
        version: None,
        parent,
        key: None,
        media: Some("application/json".into()),
    };
    let hash = store
        .for_horse(horse)?
        .put(&meta, &serde_json::to_vec(body)?)?;
    store.write_ref(&path, &serde_json::to_vec(&hash)?)?;
    Ok(hash)
}

pub fn events(store: &Store, horse: &str) -> Result<Vec<Value>> {
    store.horse(horse)?;
    let path = format!("events/{horse}.json");
    let head = store
        .read_ref(&path)?
        .map(|b| serde_json::from_slice::<Hash>(&b))
        .transpose()?;
    store
        .chain(head.as_ref())?
        .iter()
        .map(|node| Ok(serde_json::from_slice(&node.raw)?))
        .collect()
}

/// Add a run event to the horse's history.
pub fn record_event(
    store: &Store,
    rh: &RunHorse,
    kind: &str,
    sandbox: &str,
    extra: Value,
) -> Result<Hash> {
    record_event_locked(&store.lock()?, rh, kind, sandbox, extra)
}

/// Run the adapt rules of the DNA after the `run.end` event `end`. An
/// error does not fail the end of the run: it is a warning.
pub(crate) fn adapt_after(
    locked: &super::store::Lock<'_>,
    rh: &RunHorse,
    run_ok: bool,
    end: &Hash,
) {
    if let Err(e) = super::dna::adapt::at_run_end(locked, rh, run_ok, end) {
        log::warn!("horse DNA adapt {}: {e:#}", rh.horse);
    }
}

pub mod agy_setup;
pub mod by_sandbox;
pub mod final_only;
pub mod grok_setup;
mod locked;
pub mod resume;
pub mod setup;
pub mod snapshot;
pub use locked::{end_dsh, record_event_locked};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod begin_tests;
