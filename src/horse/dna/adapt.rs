//! Adapt: traits follow the recorded outcomes of the horse's runs.
//!
//! Three small rules run at the end of every run, and each one names
//! itself in the provenance of the trait it sets:
//! - `runs`: the number of runs,
//! - `harnesses`: the sorted set of harnesses that ran the horse,
//! - `reliability`: ok runs divided by runs, with two decimals. The rule
//!   name keeps the exact counts (`reliability 1/2`), so the next run
//!   does not round twice.
//!
//! The rules read only the DNA and the outcome of the run: nothing
//! outside the horse.

use super::{get_or_default, put, set_horse_dna_locked, Dna, DnaEvent, Provenance, Trait};
use crate::horse::{run::RunHorse, store::Lock, Hash};
use anyhow::Result;

/// What a run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The harness exited with code 0.
    pub run_ok: bool,
    pub harness: String,
    /// The nodes of the session of the run.
    pub nodes: u64,
    /// The tools that the run used, when the codec knows them. No rule
    /// reads them yet.
    pub tools: Vec<String>,
}

fn count(d: &Dna, key: &str) -> u64 {
    d.traits
        .get(key)
        .and_then(|t| t.value.parse().ok())
        .unwrap_or(0)
}

/// The ok runs before this run: from the rule name when the trait came
/// from the rule, else from the value.
fn ok_runs(d: &Dna, runs: u64) -> u64 {
    let Some(t) = d.traits.get("reliability") else {
        return 0;
    };
    if let Provenance::Adapt { rule, .. } = &t.from {
        let n = rule
            .strip_prefix("reliability ")
            .and_then(|r| r.split_once('/'))
            .and_then(|(ok, _)| ok.parse().ok());
        if let Some(n) = n {
            return n;
        }
    }
    let r: f64 = t.value.parse().unwrap_or(0.0);
    ((r * runs as f64).round() as u64).min(runs)
}

/// The DNA after the run `o`, or None when nothing changes. A run that
/// recorded no node is no outcome. `event` is the `run.end` event.
pub fn adapt(dna: &Dna, o: &Outcome, event: &Hash) -> Option<Dna> {
    if o.nodes == 0 && o.tools.is_empty() {
        return None;
    }
    let before = count(dna, "runs");
    let runs = before + 1;
    let ok = ok_runs(dna, before) + u64::from(o.run_ok);
    let mut harnesses: Vec<String> = dna
        .traits
        .get("harnesses")
        .map(|t| {
            t.value
                .split(',')
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    harnesses.push(o.harness.clone());
    harnesses.sort();
    harnesses.dedup();
    let reliability = format!("{:.2}", ok as f64 / runs as f64);
    let mut out = dna.clone();
    let mut set = |key: &str, value: String, rule: String| {
        // An equal value keeps its trait, unless the counts in the rule
        // name moved on.
        let keep = out.traits.get(key).is_some_and(|t| {
            t.value == value && !matches!(&t.from, Provenance::Adapt { rule: r, .. } if *r != rule)
        });
        if keep {
            return;
        }
        let from = Provenance::Adapt {
            event: event.clone(),
            rule,
        };
        out.traits.insert(key.into(), Trait::new(value, from));
    };
    set("runs", runs.to_string(), "runs".into());
    set("harnesses", harnesses.join(", "), "harnesses".into());
    set(
        "reliability",
        reliability,
        format!("reliability {ok}/{runs}"),
    );
    (out != *dna).then_some(out)
}

/// The names of the rules that changed a trait from `a` to `b`.
fn rules(a: &Dna, b: &Dna) -> Vec<String> {
    b.traits
        .iter()
        .filter(|(k, t)| a.traits.get(*k) != Some(*t))
        .filter_map(|(_, t)| match &t.from {
            Provenance::Adapt { rule, .. } => rule.split(' ').next().map(String::from),
            _ => None,
        })
        .collect()
}

/// Run the adapt rules at the end of run `rh`, while holding the store
/// writer lock. `end` is its `run.end` event. Returns the new DNA hash.
pub fn at_run_end(
    locked: &Lock<'_>,
    rh: &RunHorse,
    run_ok: bool,
    end: &Hash,
) -> Result<Option<Hash>> {
    let store = locked.store;
    let horse = store.horse(&rh.horse)?;
    let dna = get_or_default(store, &horse.dna);
    let nodes = store.session_nodes(&store.session(&rh.session)?)?.len() as u64;
    let o = Outcome {
        run_ok,
        harness: rh.harness.clone(),
        nodes,
        tools: Vec::new(),
    };
    let Some(new) = adapt(&dna, &o, end) else {
        return Ok(None);
    };
    let to = put(store, &new)?;
    let why = DnaEvent::Adapt {
        rules: rules(&dna, &new),
        run_event: end.clone(),
    };
    set_horse_dna_locked(locked, &rh.horse, &to, why)?;
    Ok(Some(to))
}

#[cfg(test)]
#[path = "adapt_tests.rs"]
mod tests;
