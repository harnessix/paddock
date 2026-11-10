//! Graft: the DNA of a donor horse joins the DNA of a host horse, when a
//! session moves to another horse or two horses are combined.
//!
//! A compatibility check comes first. Different values of one policy, or
//! a donor capability that the host lacks, make the DNAs incompatible:
//! the graft is refused and nothing is written. Different values of one
//! trait require adaptation: the host's value wins and the donor's value
//! stays as an alternative. The graft is one `dna.graft` event of the host.

use super::{get_or_default, put, set_horse_dna_locked, Alternative, Dna, DnaEvent, Provenance};
use crate::horse::{store::Lock, Hash, Store};
use anyhow::{bail, Result};

/// The compatibility class of two DNAs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compat {
    Compatible,
    /// The trait keys with different values.
    RequiresAdaptation(Vec<String>),
    /// What conflicts: `policy KEY` and `capability NAME` items.
    Incompatible(Vec<String>),
}

impl Compat {
    /// The name in the `dna.graft` event.
    pub fn name(&self) -> &'static str {
        match self {
            Compat::Compatible => "compatible",
            Compat::RequiresAdaptation(_) => "requires-adaptation",
            Compat::Incompatible(_) => "incompatible",
        }
    }
}

/// Can the donor `b` graft into the host `a`?
pub fn check(a: &Dna, b: &Dna) -> Compat {
    let mut bad: Vec<String> = b
        .policies
        .iter()
        .filter(|(k, v)| a.policies.get(*k).is_some_and(|x| x != *v))
        .map(|(k, _)| format!("policy {k}"))
        .collect();
    bad.extend(
        b.capabilities
            .iter()
            .filter(|c| !a.capabilities.contains(c))
            .map(|c| format!("capability {c}")),
    );
    if !bad.is_empty() {
        return Compat::Incompatible(bad);
    }
    let differ: Vec<String> = b
        .traits
        .iter()
        .filter(|(k, t)| a.traits.get(*k).is_some_and(|x| x.value != t.value))
        .map(|(k, _)| k.clone())
        .collect();
    if differ.is_empty() {
        Compat::Compatible
    } else {
        Compat::RequiresAdaptation(differ)
    }
}

/// The DNA of `host` with the traits of `donor`. A new trait gets
/// provenance `Graft`. Where the values differ, the host's value stays and
/// the donor's value becomes an alternative. `event` is the donor's DNA
/// event. An incompatible donor is an error.
pub fn graft(host: &Dna, donor: &Dna, donor_horse: &str, event: &Hash) -> Result<Dna> {
    if let Compat::Incompatible(list) = check(host, donor) {
        bail!(
            "the DNA of {donor_horse} is incompatible: {}",
            list.join(", ")
        );
    }
    let from = Provenance::Graft {
        from_horse: donor_horse.into(),
        event: event.clone(),
    };
    let mut out = host.clone();
    for (k, t) in &donor.traits {
        match out.traits.get_mut(k) {
            None => {
                out.traits
                    .insert(k.clone(), super::Trait::new(t.value.clone(), from.clone()));
            }
            Some(mine) if mine.value == t.value => {}
            Some(mine) => {
                // One alternative per value: a second graft adds nothing.
                if !mine.alternatives.iter().any(|a| a.value == t.value) {
                    mine.alternatives.push(Alternative {
                        value: t.value.clone(),
                        from: from.clone(),
                    });
                }
            }
        }
    }
    for (k, v) in &donor.policies {
        out.policies.entry(k.clone()).or_insert_with(|| v.clone());
    }
    Ok(out)
}

/// Graft the DNA of horse `donor` into horse `host` and set it with a
/// `dna.graft` event. `reason` names the cause (`combine`, `move S`).
/// None when the DNA of the host does not change.
pub fn graft_horses(store: &Store, host: &str, donor: &str, reason: &str) -> Result<Option<Hash>> {
    graft_horses_locked(&store.lock()?, host, donor, reason)
}

/// `graft_horses` while holding the store writer lock.
pub fn graft_horses_locked(
    locked: &Lock<'_>,
    host: &str,
    donor: &str,
    reason: &str,
) -> Result<Option<Hash>> {
    let store = locked.store;
    if host == donor {
        bail!("a horse cannot graft its own DNA");
    }
    let h = store.horse(host)?;
    let d = store.horse(donor)?;
    let (hdna, ddna) = (get_or_default(store, &h.dna), get_or_default(store, &d.dna));
    let compat = check(&hdna, &ddna);
    let new = graft(&hdna, &ddna, donor, &super::current_event(store, &d)?)?;
    if new.hash() == h.dna {
        return Ok(None);
    }
    let to = put(&store.for_horse(host)?, &new)?;
    let why = DnaEvent::Graft {
        donor_horse: donor.into(),
        donor_dna: Some(d.dna.clone()),
        compat: compat.name().into(),
        reason: reason.into(),
    };
    set_horse_dna_locked(locked, host, &to, why)?;
    Ok(Some(to))
}

#[cfg(test)]
#[path = "graft_tests.rs"]
mod tests;
