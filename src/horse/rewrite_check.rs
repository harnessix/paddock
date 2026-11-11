//! Test helpers for ingest across rewrites: a harness rewrites, renames,
//! truncates or migrates its session files, and no node may be lost or
//! appear twice.

use super::store::{Node, Store};
use std::collections::{BTreeMap, HashSet};

/// The nodes of a session, in chain order.
pub(crate) fn nodes(st: &Store, session: &str) -> Vec<Node> {
    let s = st.session(session).unwrap();
    st.session_nodes(&s).unwrap()
}

/// Every node of `before` is still in the store, byte for byte, and no
/// identity-keyed node of the session appears twice.
pub(crate) fn assert_no_loss(st: &Store, session: &str, before: &[Node]) {
    for n in before {
        let got = st
            .get(&n.hash)
            .unwrap_or_else(|e| panic!("node {} left the store: {e}", n.hash.short()));
        assert_eq!(got.raw, n.raw, "node {} changed", n.hash.short());
    }
    assert_once(&nodes(st, session));
}

/// No node of the chain with an identity key (uuid, seq, event id, chat
/// item, step index) appears twice with the same bytes, unless an agy drop
/// node removed it in between. Rows without an identity (Claude Code
/// metadata rows, index and drop nodes) may repeat.
pub(crate) fn assert_once(chain: &[Node]) {
    let mut seen = HashSet::new();
    for n in chain {
        let Some(key) = n.meta.key.as_deref() else {
            continue;
        };
        // An agy drop node removes rows; a row may come back after it.
        if key.starts_with("drop:") {
            let gone: BTreeMap<String, Vec<i64>> = serde_json::from_slice(&n.raw).unwrap();
            let keys: HashSet<String> = gone
                .iter()
                .flat_map(|(t, idx)| idx.iter().map(move |i| format!("{t}:{i}")))
                .collect();
            seen.retain(|(k, _)| !keys.contains(k));
            continue;
        }
        if ["type:", "rewrite:", "drop:"]
            .iter()
            .any(|p| key.starts_with(p))
        {
            continue;
        }
        assert!(
            seen.insert((key.to_string(), n.raw.clone())),
            "node {key} appears twice"
        );
    }
}

/// The node of the chain with `key`, if it is there.
pub(crate) fn has_key(chain: &[Node], key: &str) -> bool {
    chain.iter().any(|n| n.meta.key.as_deref() == Some(key))
}
