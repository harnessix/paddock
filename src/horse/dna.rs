//! The DNA of a horse: a TOML document in the store, referenced by hash.
//!
//! DNA acts by default inside the horse's own namespace. A new horse gets
//! the default DNA. A child horse of a plain spawn inherits the DNA of its
//! parent. An edit of `self/dna.toml` (or `paddock horse dna set`) hot-loads
//! as a new DNA: the horse points at the new hash, and each running session
//! of the horse gets one `system` entry in its inbox, which the run delivers
//! at the next safe point. Nothing is rewritten: the old DNA stays in the
//! store. A move of a session or a combine of two horses grafts one DNA
//! into another (`graft`), the end of a run adapts traits by three small
//! rules (`adapt`), and `explain` shows where each trait comes from.
//! Every change is an event (`dna.default`, `dna.inherit`, `dna.edit`,
//! `dna.graft`, `dna.adapt`, `dna.revert`) in the event chain of the
//! horse, and `explain::revert` goes back to an earlier DNA.

use super::{guard, run, session::Horse, store::Lock, Hash, Store};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The media type of a DNA blob.
pub const MEDIA: &str = "application/toml+dna";
/// The media type of the text of an edit, as the writer gave it.
pub const INPUT_MEDIA: &str = "application/toml+dna-input";
/// The media type of the DNA of a horse made before DNA objects.
const LEGACY_MEDIA: &str = "toml";

/// The DNA document.
///
/// `capabilities` is recorded and shown only. Nothing reads it to grant
/// anything: a DNA never gives a sandbox a capability that the sandbox
/// does not have already.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dna {
    pub capabilities: Vec<String>,
    pub origin: Origin,
    pub policies: BTreeMap<String, String>,
    pub traits: BTreeMap<String, Trait>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// The id of the parent horse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The hash of the parent's DNA at the inheritance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_dna: Option<Hash>,
    #[serde(default)]
    pub created_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trait {
    pub value: String,
    pub from: Provenance,
    /// Other values from grafts, when the host's value won.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Alternative>,
}

impl Trait {
    pub fn new(value: impl Into<String>, from: Provenance) -> Trait {
        Trait {
            value: value.into(),
            from,
            alternatives: Vec::new(),
        }
    }
}

/// A value of a trait that a graft brought, and that the host's value
/// overrides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    pub value: String,
    pub from: Provenance,
}

/// Where a trait comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Default,
    /// An edit. `event` is the hash of the edit text in the store, which
    /// the `dna.edit` event names as `input`. (The event itself names the
    /// new DNA, so the DNA cannot name the event.)
    Edit {
        event: Hash,
    },
    /// Inherited. `event` is the DNA event of the parent that set the
    /// parent's DNA at the inheritance.
    Inherit {
        from_horse: String,
        event: Hash,
    },
    /// Grafted from another horse, on a move or a combine. `event` is
    /// the DNA event of the donor that set the donor's DNA.
    Graft {
        from_horse: String,
        event: Hash,
    },
    /// Set by an adapt rule. `event` is the `run.end` event of the run.
    Adapt {
        event: Hash,
        rule: String,
    },
    /// Grafted from a published DNA. `reference` is the pinned reference
    /// (`registry://org/name@1.2.0`), `hash` the artifact hash, `event` the
    /// `dna.fetch` event that brought it into the store.
    Registry {
        reference: String,
        hash: Hash,
        event: Hash,
    },
}

/// Why the DNA of a horse changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnaEvent {
    /// A new horse gets the default DNA.
    Default,
    /// A new child horse inherits the DNA of `parent_horse`.
    Inherit { parent_horse: String },
    /// An edit. `input`: the edit text in the store, if any.
    Edit { input: Option<Hash> },
    /// A graft of the DNA of `donor_horse` (a horse id, or a registry
    /// reference). `donor_dna`: the hash of the donor's DNA. `compat`: the
    /// compatibility class, `reason`: why (`combine`, `move <session>`,
    /// `use`).
    Graft {
        donor_horse: String,
        donor_dna: Option<Hash>,
        compat: String,
        reason: String,
    },
    /// Adapt rules changed traits after the run event `run_event`.
    Adapt { rules: Vec<String>, run_event: Hash },
    /// Back to an earlier DNA of the horse. The forward history stays.
    Revert,
    /// A published DNA came into the store. It changes no DNA: the body is
    /// written by `registry::import`.
    Fetch {
        reference: String,
        hash: Hash,
        index: String,
        signed_by: Option<String>,
    },
    /// The current DNA of a horse was published. It changes no DNA: the
    /// body is written by `registry::publish`.
    Publish {
        reference: String,
        hash: Hash,
        index: String,
        parents: Vec<Hash>,
    },
}

impl DnaEvent {
    fn kind(&self) -> &'static str {
        match self {
            DnaEvent::Default => "dna.default",
            DnaEvent::Inherit { .. } => "dna.inherit",
            DnaEvent::Edit { .. } => "dna.edit",
            DnaEvent::Graft { .. } => "dna.graft",
            DnaEvent::Adapt { .. } => "dna.adapt",
            DnaEvent::Revert => "dna.revert",
            DnaEvent::Fetch { .. } => "dna.fetch",
            DnaEvent::Publish { .. } => "dna.publish",
        }
    }

    /// The reason in the entry that the model sees.
    fn reason(&self) -> String {
        match self {
            DnaEvent::Edit { .. } => "hot-load".into(),
            DnaEvent::Graft {
                donor_horse,
                reason,
                ..
            } => format!("graft from {donor_horse}, {reason}"),
            DnaEvent::Adapt { .. } => "adapt".into(),
            DnaEvent::Revert => "revert".into(),
            _ => "change".into(),
        }
    }
}

/// The text form: a trait is a table `{value, from}` or a plain string,
/// which gets its provenance from paddock.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    origin: Origin,
    #[serde(default)]
    policies: BTreeMap<String, String>,
    #[serde(default)]
    traits: BTreeMap<String, TextTrait>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TextTrait {
    Plain(String),
    Full {
        value: String,
        #[serde(default)]
        from: Option<Provenance>,
        #[serde(default)]
        alternatives: Vec<Alternative>,
    },
}

/// Parse a DNA. A trait without a `from` gets `Default`; an edit sets the
/// provenance of the traits it changes (`edited`).
pub fn parse(text: &str) -> Result<Dna> {
    let t: Text = toml::from_str(text).context("the DNA is not valid DNA TOML")?;
    let traits = t
        .traits
        .into_iter()
        .map(|(k, v)| {
            let tr = match v {
                TextTrait::Plain(value) => Trait::new(value, Provenance::Default),
                TextTrait::Full {
                    value,
                    from,
                    alternatives,
                } => Trait {
                    alternatives,
                    ..Trait::new(value, from.unwrap_or(Provenance::Default))
                },
            };
            (k, tr)
        })
        .collect();
    let mut d = Dna {
        capabilities: t.capabilities,
        origin: t.origin,
        policies: t.policies,
        traits,
    };
    d.capabilities.sort();
    d.capabilities.dedup();
    Ok(d)
}

impl Dna {
    /// The canonical text: sorted keys and capabilities, so equal DNA has
    /// equal bytes and an equal hash.
    pub fn encode(&self) -> String {
        let mut d = self.clone();
        d.capabilities.sort();
        d.capabilities.dedup();
        let body = toml::to_string(&d).expect("a DNA always encodes as TOML");
        format!("# horse DNA\n{body}")
    }

    /// The hash that `put` gives this DNA.
    pub fn hash(&self) -> Hash {
        super::store::blob_hash(MEDIA, self.encode().as_bytes())
            .expect("DNA metadata always encodes")
    }
}

/// The DNA of a new horse of a run: no traits, no parent.
pub fn default_for_run() -> Dna {
    Dna {
        origin: Origin {
            parent: None,
            parent_dna: None,
            created_ms: super::now_ms(),
        },
        ..Dna::default()
    }
}

/// The DNA of a child horse: every trait of the parent, with provenance
/// `Inherit`. `event` is the parent's DNA event at the inheritance.
pub fn inherit(parent: &Dna, parent_horse: &str, event: &Hash) -> Dna {
    let traits = parent
        .traits
        .iter()
        .map(|(k, t)| {
            let from = Provenance::Inherit {
                from_horse: parent_horse.into(),
                event: event.clone(),
            };
            (k.clone(), Trait::new(t.value.clone(), from))
        })
        .collect();
    Dna {
        capabilities: parent.capabilities.clone(),
        origin: Origin {
            parent: Some(parent_horse.into()),
            parent_dna: Some(parent.hash()),
            created_ms: super::now_ms(),
        },
        policies: parent.policies.clone(),
        traits,
    }
}

/// The DNA that an edit gives. The origin stays. A trait whose value
/// stays keeps its provenance and alternatives; a new or changed trait
/// gets `Edit` and no alternatives, so a writer cannot claim another
/// provenance.
pub fn edited(current: &Dna, written: Dna, input: &Hash) -> Dna {
    let traits = written
        .traits
        .into_iter()
        .map(|(k, t)| {
            let tr = match current.traits.get(&k) {
                Some(old) if old.value == t.value => old.clone(),
                _ => Trait::new(
                    t.value,
                    Provenance::Edit {
                        event: input.clone(),
                    },
                ),
            };
            (k, tr)
        })
        .collect();
    Dna {
        capabilities: written.capabilities,
        origin: current.origin.clone(),
        policies: written.policies,
        traits,
    }
}

pub fn put(store: &Store, dna: &Dna) -> Result<Hash> {
    store.put_blob(MEDIA, dna.encode().as_bytes())
}

pub fn get(store: &Store, h: &Hash) -> Result<Dna> {
    let node = store.get(h)?;
    let media = node.meta.media.as_deref().unwrap_or("");
    if node.meta.kind != "blob" || !matches!(media, MEDIA | LEGACY_MEDIA) {
        bail!("node {} is not a DNA", h.short());
    }
    let text = std::str::from_utf8(&node.raw).context("the DNA is not UTF-8")?;
    match parse(text) {
        Ok(d) => Ok(d),
        // A horse from before DNA objects: its text has another shape.
        Err(_) if media == LEGACY_MEDIA => Ok(Dna::default()),
        Err(e) => Err(e),
    }
}

/// The DNA of a hash, or the empty DNA when it cannot be read.
pub fn get_or_default(store: &Store, h: &Hash) -> Dna {
    get(store, h).unwrap_or_else(|e| {
        log::warn!("horse DNA {}: {e:#}", h.short());
        Dna::default()
    })
}

/// The text that the model sees: one short block.
pub fn render_entry(dna: &Dna, reason: &str) -> String {
    let list = |m: Vec<String>, none: &str| {
        if m.is_empty() {
            none.to_string()
        } else {
            m.join(", ")
        }
    };
    let traits = dna
        .traits
        .iter()
        .map(|(k, t)| format!("{k} = {}", t.value))
        .collect();
    let policies = dna
        .policies
        .iter()
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    let mut s = format!(
        "DNA of this horse ({reason})\ntraits: {}\npolicies: {}",
        list(traits, "none yet, this horse has no traits"),
        list(policies, "none")
    );
    if !dna.capabilities.is_empty() {
        s.push_str(&format!(
            "\ncapabilities (recorded only, they grant nothing): {}",
            dna.capabilities.join(", ")
        ));
    }
    s
}

/// Change the DNA of a horse: write the event, point the horse at `new`,
/// and queue the new DNA on each running session of the horse. Returns
/// the hash of the event.
pub fn set_horse_dna(store: &Store, horse: &str, new: &Hash, why: DnaEvent) -> Result<Hash> {
    set_horse_dna_locked(&store.lock()?, horse, new, why)
}

/// `set_horse_dna` while holding the store writer lock.
pub fn set_horse_dna_locked(
    locked: &Lock<'_>,
    horse: &str,
    new: &Hash,
    why: DnaEvent,
) -> Result<Hash> {
    let store = locked.store;
    if matches!(why, DnaEvent::Fetch { .. } | DnaEvent::Publish { .. }) {
        bail!("a {} event does not change the DNA of a horse", why.kind());
    }
    let dna = get(store, new)?;
    let before = store.horse(horse)?;
    let from = match why {
        DnaEvent::Default => None,
        DnaEvent::Inherit { .. } => dna.origin.parent_dna.clone(),
        _ => Some(before.dna.clone()),
    };
    let mut body = json!({"type": why.kind(), "horse": horse, "from_hash": from,
        "to_hash": new, "at_ms": super::now_ms()});
    match &why {
        DnaEvent::Inherit { parent_horse } => body["parent_horse"] = json!(parent_horse),
        DnaEvent::Edit { input: Some(i) } => body["input"] = json!(i),
        DnaEvent::Graft {
            donor_horse,
            donor_dna,
            compat,
            reason,
        } => {
            body["donor_horse"] = json!(donor_horse);
            if let Some(d) = donor_dna {
                body["donor_hash"] = json!(d);
            }
            body["compat"] = json!(compat);
            body["reason"] = json!(reason);
        }
        DnaEvent::Adapt { rules, run_event } => {
            body["rules"] = json!(rules);
            body["run_event"] = json!(run_event);
        }
        _ => {}
    }
    let event = run::append_horse_event_locked(locked, horse, None, &body)?;
    locked.set_dna(horse, new)?;
    let text = render_entry(&dna, &why.reason());
    for session in running_sessions(store, &before)? {
        // A DNA entry is an append at the end: the guard records it.
        match guard::check(store, &session, &guard::Op::DnaChange, super::now_ms()) {
            Ok(v) => {
                if let Err(e) = store.save_verdict(&v) {
                    log::warn!("horse guard {session}: {e:#}");
                }
            }
            Err(e) => log::debug!("horse guard {session}: {e:#}"),
        }
        store.queue_append(&session, "system", &text)?;
    }
    Ok(event)
}

/// The sessions of a horse whose last run event is `run.begin`.
pub fn running_sessions(store: &Store, horse: &Horse) -> Result<Vec<String>> {
    let mut state: BTreeMap<String, bool> = BTreeMap::new();
    for e in run::events(store, &horse.id)? {
        if let (Some(s), Some(t)) = (e["session"].as_str(), e["type"].as_str()) {
            match t {
                "run.begin" => {
                    state.insert(s.into(), true);
                }
                "run.end" => {
                    state.insert(s.into(), false);
                }
                _ => {}
            }
        }
    }
    Ok(horse
        .sessions
        .iter()
        .filter(|s| state.get(*s) == Some(&true))
        .cloned()
        .collect())
}

/// A new horse with the default DNA and its `dna.default` event.
pub fn new_default_horse_locked(locked: &Lock<'_>) -> Result<Horse> {
    let h = put(locked.store, &default_for_run())?;
    let horse = locked.new_horse(&h)?;
    set_horse_dna_locked(locked, &horse.id, &h, DnaEvent::Default)?;
    locked.store.horse(&horse.id)
}

impl Store {
    /// A new horse with the default DNA and its `dna.default` event.
    pub fn new_default_horse(&self) -> Result<Horse> {
        new_default_horse_locked(&self.lock()?)
    }
}

/// The event that set the current DNA of a horse. A horse from before
/// DNA events has none, so the DNA hash stands in.
pub fn current_event(store: &Store, h: &Horse) -> Result<Hash> {
    Ok(history(store, &h.id)?
        .into_iter()
        .rev()
        .find(|(_, e)| e["to_hash"].as_str() == Some(h.dna.as_str()))
        .map_or_else(|| h.dna.clone(), |(h, _)| h))
}

/// A new child horse of `parent`: its DNA inherits the parent's DNA. One
/// `dna.inherit` event goes into the child and one into the parent.
pub fn new_child_horse_locked(locked: &Lock<'_>, parent: &str) -> Result<Horse> {
    let store = locked.store;
    let p = store.horse(parent)?;
    let pdna = get_or_default(store, &p.dna);
    let pevent = current_event(store, &p)?;
    let mut dna = inherit(&pdna, parent, &pevent);
    dna.origin.parent_dna = Some(p.dna.clone());
    let h = put(store, &dna)?;
    let child = locked.new_horse(&h)?;
    set_horse_dna_locked(
        locked,
        &child.id,
        &h,
        DnaEvent::Inherit {
            parent_horse: parent.into(),
        },
    )?;
    let body = json!({"type": "dna.inherit", "horse": parent, "child_horse": child.id,
        "from_hash": p.dna, "to_hash": h, "parent_horse": parent, "at_ms": super::now_ms()});
    run::append_horse_event_locked(locked, parent, None, &body)?;
    store.horse(&child.id)
}

/// Hot-load an edit of `self/dna.toml`: parse, store the text and the new
/// DNA, and set it. None when the DNA does not change. An invalid text is
/// an error and changes nothing.
pub fn hot_load(store: &Store, horse: &str, text: &str) -> Result<Option<Hash>> {
    let written = parse(text)?;
    let locked = store.lock()?;
    let h = store.horse(horse)?;
    let current = get_or_default(store, &h.dna);
    let input = super::store::blob_hash(INPUT_MEDIA, text.as_bytes())?;
    let new = edited(&current, written, &input);
    if new.hash() == h.dna {
        return Ok(None);
    }
    store.put_blob(INPUT_MEDIA, text.as_bytes())?;
    let to = put(store, &new)?;
    set_horse_dna_locked(&locked, horse, &to, DnaEvent::Edit { input: Some(input) })?;
    Ok(Some(to))
}

/// Restore an earlier DNA by its hash: a `dna.revert` event back to it.
pub fn restore(store: &Store, horse: &str, to: &Hash) -> Result<Hash> {
    explain::revert(store, horse, to)
}

/// The DNA events of a horse with their hashes, root first.
pub fn history(store: &Store, horse: &str) -> Result<Vec<(Hash, Value)>> {
    store.horse(horse)?;
    let head = store
        .read_ref(&format!("events/{horse}.json"))?
        .map(|b| serde_json::from_slice::<Hash>(&b))
        .transpose()?;
    let mut out = Vec::new();
    for node in store.chain(head.as_ref())? {
        let v: Value = serde_json::from_slice(&node.raw)?;
        if v["type"].as_str().is_some_and(|t| t.starts_with("dna.")) {
            out.push((node.hash, v));
        }
    }
    Ok(out)
}

pub mod adapt;
pub mod explain;
pub mod graft;
pub mod registry;

#[cfg(test)]
mod tests;
