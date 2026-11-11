//! Metadata visible to all instances of the same horse.
use super::{Store, Tree};
use crate::horse::{hitch, run, session::Horse};
use anyhow::Result;
use std::collections::HashMap;

pub(super) fn add(store: &Store, horse: &Horse, tree: &mut Tree) -> Result<()> {
    let mut states = HashMap::new();
    for event in run::events(store, &horse.id)? {
        if let (Some(id), Some(kind)) = (event["session"].as_str(), event["type"].as_str()) {
            match kind {
                "run.begin" => {
                    states.insert(id.to_owned(), "running");
                }
                "run.end" => {
                    states.insert(id.to_owned(), "finished");
                }
                _ => {}
            }
        }
    }
    let mut instances = String::new();
    for id in &horse.sessions {
        let session = store.session(id)?;
        if session.horse == horse.id {
            instances.push_str(&format!(
                "{} {} {}\n",
                id,
                session.binding.harness,
                states.get(id).unwrap_or(&"running")
            ));
        }
    }
    tree.file("self/instances", instances);
    let mut edges = String::new();
    for edge in hitch::edges(store, &horse.id)? {
        edges.push_str(&format!("{:?} {} {}\n", edge.kind, edge.from, edge.to));
    }
    tree.file("self/edges", edges);
    Ok(())
}
