//! Connections between instances of a horse.

use super::{
    inbox::Entry,
    run::{self, RunHorse},
    Hash, Store,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    SpawnedFrom,
    Append,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: String,
    pub to: String,
    pub at: Option<Hash>,
    pub role: Option<String>,
    pub granted_by: String,
    pub at_ms: u64,
}

pub fn spawn_child(
    store: &Store,
    parent: &RunHorse,
    harness: &str,
    version: &str,
) -> Result<RunHorse> {
    let child = run::begin(store, harness, version, None, Some(parent))?;
    let locked = store.lock()?;
    let edge = Edge {
        kind: EdgeKind::SpawnedFrom,
        from: child.session.clone(),
        to: parent.session.clone(),
        at: store.session(&parent.session)?.head,
        role: None,
        granted_by: parent.session.clone(),
        at_ms: super::now_ms(),
    };
    run::append_event_locked(
        &locked,
        &child,
        &json!({"type": "edge.spawned_from", "edge": edge}),
    )?;
    Ok(child)
}

pub fn child_finished(
    store: &Store,
    child: &RunHorse,
    parent_session: &str,
    result_text: &str,
) -> Result<Entry> {
    child_finished_locked(&store.lock()?, child, parent_session, result_text)
}

/// `child_finished` while holding the store writer lock.
pub fn child_finished_locked(
    locked: &super::store::Lock<'_>,
    child: &RunHorse,
    parent_session: &str,
    result_text: &str,
) -> Result<Entry> {
    let store = locked.store;
    let session = store.session(&child.session)?;
    ensure!(
        session.horse == child.horse && session.binding.harness == child.harness,
        "child run does not match its session"
    );
    ensure!(
        edges(store, &child.horse)?
            .iter()
            .any(|edge| edge.kind == EdgeKind::SpawnedFrom
                && edge.from == child.session
                && edge.to == parent_session),
        "child was not spawned from parent session"
    );
    let edge = Edge {
        kind: EdgeKind::Append,
        from: child.session.clone(),
        to: parent_session.into(),
        at: session.head,
        role: Some("note".into()),
        granted_by: parent_session.into(),
        at_ms: super::now_ms(),
    };
    let text = format!(
        "hitched {} ({}) finished\n{}",
        &child.session[..child.session.len().min(8)],
        child.harness,
        result_text
    );
    let entry = store.queue_append(parent_session, "note", &text)?;
    run::append_event_locked(locked, child, &json!({"type": "edge.append", "edge": edge}))?;
    Ok(entry)
}

pub fn edges(store: &Store, horse: &str) -> Result<Vec<Edge>> {
    run::events(store, horse)?
        .into_iter()
        .filter(|event| {
            matches!(
                event["type"].as_str(),
                Some("edge.spawned_from" | "edge.append" | "edge.reference")
            )
        })
        .map(|event| Ok(serde_json::from_value(event["edge"].clone())?))
        .collect()
}

pub fn visible_sessions(store: &Store, horse: &str, own_session: &str) -> Vec<String> {
    match store.session(own_session) {
        Ok(session) if session.horse == horse => vec![own_session.into()],
        _ => Vec::new(),
    }
}

pub mod wire;
pub use wire::Hitched;

#[cfg(test)]
mod tests;
