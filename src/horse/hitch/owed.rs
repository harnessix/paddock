//! The results that hitched children still owe their parent session. A
//! run in horse mode does not end while one is owed: the result of each
//! hitched child reaches the parent's session once, before the run ends.

use super::{edges, EdgeKind};
use crate::horse::Store;
use anyhow::Result;

/// The first words of the entry that brings the result of `child` to its
/// parent.
pub fn result_head(child: &str) -> String {
    format!("hitched {} (", &child[..child.len().min(8)])
}

/// The hitched children of `parent` in `horse` whose result did not reach
/// `parent` yet: a child with no Append edge to it, or a child whose result
/// entry still waits in the inbox of `parent`.
pub fn owed(store: &Store, horse: &str, parent: &str) -> Result<Vec<String>> {
    // The edges first: a result entry is queued before its Append edge.
    let edges = edges(store, horse)?;
    let mut inbox = None;
    let mut out: Vec<String> = Vec::new();
    for e in &edges {
        if e.kind != EdgeKind::SpawnedFrom || e.to != parent || out.contains(&e.from) {
            continue;
        }
        let appended = edges
            .iter()
            .any(|a| a.kind == EdgeKind::Append && a.from == e.from && a.to == parent);
        let waits = appended && {
            if inbox.is_none() {
                inbox = Some(store.inbox(parent)?);
            }
            let head = result_head(&e.from);
            inbox.iter().flatten().any(|x| x.text.starts_with(&head))
        };
        if !appended || waits {
            out.push(e.from.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
