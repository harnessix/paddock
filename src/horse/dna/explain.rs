//! Explain where each trait of a horse comes from, and revert the DNA of
//! a horse to an earlier hash.

use super::{get, get_or_default, history, set_horse_dna, DnaEvent, Provenance};
use crate::horse::{Hash, Store};
use anyhow::{bail, Result};
use serde_json::Value;

/// The time of an event: its `at_ms`, in local time.
fn when(ms: Option<u64>) -> String {
    ms.map_or_else(|| "-".into(), |ms| crate::storage::local_time(ms / 1000))
}

/// The `at_ms` of an event node in the store.
fn event_ms(store: &Store, h: &Hash) -> Option<u64> {
    let node = store.get(h).ok()?;
    let v: Value = serde_json::from_slice(&node.raw).ok()?;
    v["at_ms"].as_u64()
}

/// `<provenance> (<event short hash>, <when>)`.
fn provenance(store: &Store, events: &[(Hash, Value)], p: &Provenance) -> String {
    let at = |h: &Hash| format!("({}, {})", h.short(), when(event_ms(store, h)));
    match p {
        Provenance::Default => "default".into(),
        Provenance::Edit { event } => {
            // The DNA names the edit text; the dna.edit event names both.
            match events
                .iter()
                .find(|(_, e)| e["input"].as_str() == Some(event.as_str()))
            {
                Some((h, e)) => format!("edit ({}, {})", h.short(), when(e["at_ms"].as_u64())),
                None => format!("edit ({}, -)", event.short()),
            }
        }
        Provenance::Inherit { from_horse, event } => {
            format!("inherit from {from_horse} {}", at(event))
        }
        Provenance::Graft { from_horse, event } => format!("graft from {from_horse} {}", at(event)),
        Provenance::Adapt { event, rule } => format!("adapt {rule} {}", at(event)),
        Provenance::Registry {
            reference,
            hash,
            event,
        } => format!(
            "{reference} ({}, {}, {})",
            hash.short(),
            event.short(),
            when(event_ms(store, event))
        ),
    }
}

/// The explanation of a horse: one line per trait with its provenance,
/// the DNA events root first, then the lineage of each session.
pub fn explain(store: &Store, horse: &str) -> Result<String> {
    let h = store.horse(horse)?;
    let dna = get_or_default(store, &h.dna);
    let events = history(store, horse)?;
    let mut s = format!("horse {} (DNA {})\ntraits:\n", h.id, h.dna.short());
    if dna.traits.is_empty() {
        s.push_str("  none\n");
    }
    for (k, t) in &dna.traits {
        s.push_str(&format!(
            "  {k} = {}  ← {}\n",
            t.value,
            provenance(store, &events, &t.from)
        ));
        for a in &t.alternatives {
            s.push_str(&format!(
                "    alternative {}  ← {}\n",
                a.value,
                provenance(store, &events, &a.from)
            ));
        }
    }
    for (k, v) in &dna.policies {
        s.push_str(&format!("policy {k} = {v}\n"));
    }
    s.push_str("DNA events, root first:\n");
    for (hash, e) in &events {
        let f = |k: &str| {
            e[k].as_str()
                .map(|x| x.chars().take(12).collect::<String>())
        };
        let mut line = format!(
            "  {} {} to {} at {}",
            hash.short(),
            e["type"].as_str().unwrap_or("?"),
            f("to_hash").unwrap_or_else(|| "-".into()),
            when(e["at_ms"].as_u64())
        );
        for k in [
            "parent_horse",
            "child_horse",
            "donor_horse",
            "reference",
            "index",
            "compat",
            "reason",
        ] {
            if let Some(v) = e[k].as_str() {
                line.push_str(&format!(" {k} {v}"));
            }
        }
        if let Some(r) = e["rules"].as_array() {
            let r: Vec<&str> = r.iter().filter_map(Value::as_str).collect();
            line.push_str(&format!(" rules {}", r.join(",")));
        }
        s.push_str(&line);
        s.push('\n');
    }
    for id in &h.sessions {
        let session = store.session(id)?;
        s.push_str(&format!("lineage of session {id}:\n"));
        for l in &session.lineage {
            s.push_str(&format!(
                "  {} DNA {} since {}",
                l.horse,
                l.dna.short(),
                when(Some(l.since_ms))
            ));
            if let Some(u) = l.until_ms {
                s.push_str(&format!(" until {}", when(Some(u))));
            }
            s.push('\n');
        }
    }
    Ok(s)
}

/// Set the DNA of a horse back to `to`, a DNA that the horse had before.
/// The forward history stays: this is one more event, `dna.revert`.
pub fn revert(store: &Store, horse: &str, to: &Hash) -> Result<Hash> {
    get(store, to)?;
    let known = history(store, horse)?
        .iter()
        .any(|(_, e)| e["to_hash"].as_str() == Some(to.as_str()));
    if !known {
        bail!("{} is no earlier DNA of horse {horse}", to.short());
    }
    if store.horse(horse)?.dna == *to {
        bail!("the DNA of horse {horse} is {} already", to.short());
    }
    set_horse_dna(store, horse, to, DnaEvent::Revert)
}

#[cfg(test)]
#[path = "explain_tests.rs"]
mod tests;
