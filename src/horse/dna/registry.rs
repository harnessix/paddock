//! Published DNA in the store: `import` a resolved artifact, `use_in` to
//! graft it into a horse, and `publish` the DNA of a horse to an index.
//!
//! An artifact is stored unchanged as a DNA blob, so its registry hash is
//! its store hash. Every fetch is one `dna.fetch` event, also a repeated
//! one: the event list is the audit of what came in. A fetch for a horse
//! goes into the chain of that horse; a fetch for no horse goes into the
//! chain `registry/events.json` of the store.
//!
//! Nothing here grants anything. The `capabilities` of a fetched DNA are
//! recorded and shown, and the graft check refuses a capability that the
//! host lacks. Every write runs under the store writer lock.

use super::graft::{check, graft, Compat};
use super::{
    get, get_or_default, history, put, set_horse_dna_locked, Dna, DnaEvent, Provenance, MEDIA,
};
use crate::horse::registry::{
    artifact_hash, check_artifact, Keypair, Manifest, Reference, Resolved, Transport, Version,
};
use crate::horse::store::{Lock, Meta};
use crate::horse::{run, Hash, Store};
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};

/// The chain of the fetches that belong to no horse.
const STORE_EVENTS: &str = "registry/events.json";

/// The reference as it is recorded: an `@latest` becomes the version that
/// it resolved to, so the record names one immutable artifact.
pub fn pinned(r: &Resolved) -> String {
    match &r.reference.version {
        Version::Hash(_) => r.reference.to_string(),
        _ => Reference {
            org: r.reference.org.clone(),
            name: r.reference.name.clone(),
            version: Version::Exact(r.manifest.version.clone()),
        }
        .to_string(),
    }
}

/// The body of a `dna.fetch` or a `dna.publish` event.
fn body(horse: Option<&str>, why: &DnaEvent) -> Value {
    let mut b = match why {
        DnaEvent::Fetch {
            reference,
            hash,
            index,
            signed_by,
        } => json!({"type": why.kind(), "reference": reference, "hash": hash,
            "index": index, "signed_by": signed_by}),
        DnaEvent::Publish {
            reference,
            hash,
            index,
            parents,
        } => json!({"type": why.kind(), "reference": reference, "hash": hash,
            "index": index, "parents": parents}),
        other => unreachable!("{} is not a registry event", other.kind()),
    };
    if let Some(h) = horse {
        b["horse"] = json!(h);
    }
    b["at_ms"] = json!(crate::horse::now_ms());
    b
}

/// Append a registry event to the chain of `horse`, or to the chain of
/// the store when there is no horse.
fn append(locked: &Lock<'_>, horse: Option<&str>, why: &DnaEvent) -> Result<Hash> {
    let b = body(horse, why);
    if let Some(h) = horse {
        return run::append_horse_event_locked(locked, h, None, &b);
    }
    let store = locked.store;
    let parent = store
        .read_ref(STORE_EVENTS)?
        .map(|b| serde_json::from_slice::<Hash>(&b))
        .transpose()
        .with_context(|| format!("{STORE_EVENTS} in the horse store is damaged"))?;
    let meta = Meta {
        kind: "event".into(),
        harness: None,
        container: None,
        version: None,
        parent,
        key: None,
        media: Some("application/json".into()),
    };
    let hash = store.put(&meta, &serde_json::to_vec(&b)?)?;
    store.write_ref(STORE_EVENTS, &serde_json::to_vec(&hash)?)?;
    Ok(hash)
}

/// The registry events that belong to no horse, root first.
pub fn store_events(store: &Store) -> Result<Vec<(Hash, Value)>> {
    let head = store
        .read_ref(STORE_EVENTS)?
        .map(|b| serde_json::from_slice::<Hash>(&b))
        .transpose()
        .with_context(|| format!("{STORE_EVENTS} in the horse store is damaged"))?;
    store
        .chain(head.as_ref())?
        .into_iter()
        .map(|n| Ok((n.hash, serde_json::from_slice(&n.raw)?)))
        .collect()
}

/// The DNA of a resolved artifact, checked again: the caller may have
/// built the `Resolved` by hand.
fn fetched(r: &Resolved) -> Result<Dna> {
    check_artifact(&r.manifest, &r.bytes).with_context(|| format!("{}", r.reference))?;
    let text = std::str::from_utf8(&r.bytes).expect("check_artifact refuses non-UTF-8 text");
    super::parse(text)
}

/// `import` while holding the store writer lock. Returns the artifact
/// hash and the hash of the `dna.fetch` event.
fn import_locked(locked: &Lock<'_>, r: &Resolved, horse: Option<&str>) -> Result<(Hash, Hash)> {
    let store = locked.store;
    fetched(r)?;
    if let Some(h) = horse {
        store.horse(h)?;
    }
    let hash = store.put_blob(MEDIA, &r.bytes)?;
    if hash != r.manifest.hash {
        // check_artifact compared the same hash function: a bug.
        bail!(
            "the store hash {hash} of {} differs from its manifest hash {}; it is refused",
            r.reference,
            r.manifest.hash
        );
    }
    let why = DnaEvent::Fetch {
        reference: pinned(r),
        hash: hash.clone(),
        index: r.index.clone(),
        signed_by: r.signed_by.clone(),
    };
    let event = append(locked, horse, &why)?;
    Ok((hash, event))
}

/// Put a resolved artifact into the store as a DNA blob, unchanged, and
/// write one `dna.fetch` event, in the chain of `horse` when given.
/// Returns the artifact hash, which is its store hash. A second import
/// of the same artifact stores nothing new but writes one more event.
pub fn import(store: &Store, r: &Resolved, horse: Option<&str>) -> Result<Hash> {
    Ok(import_locked(&store.lock()?, r, horse)?.0)
}

/// Graft a resolved artifact into `horse`. The compatibility check comes
/// first: an incompatible DNA is refused with the list of conflicts, and
/// nothing is written, not even the fetch. Otherwise one `dna.fetch` and
/// then one `dna.graft` event. The traits that the artifact brings get
/// the provenance `Registry`. Returns the new DNA hash of the horse; when
/// the artifact adds nothing, the DNA stays, and only the fetch is
/// written.
pub fn use_in(store: &Store, horse: &str, r: &Resolved) -> Result<Hash> {
    let locked = store.lock()?;
    let h = store.horse(horse)?;
    let host = get_or_default(store, &h.dna);
    let donor = fetched(r)?;
    let reference = pinned(r);
    let compat = check(&host, &donor);
    if let Compat::Incompatible(list) = &compat {
        bail!(
            "{reference} cannot graft into horse {horse}: the DNAs are incompatible: {}",
            list.join(", ")
        );
    }
    let (hash, event) = import_locked(&locked, r, Some(horse))?;
    let mut new = graft(&host, &donor, &reference, &event)?;
    let marker = Provenance::Graft {
        from_horse: reference.clone(),
        event: event.clone(),
    };
    let from = Provenance::Registry {
        reference: reference.clone(),
        hash: hash.clone(),
        event,
    };
    for t in new.traits.values_mut() {
        if t.from == marker {
            t.from = from.clone();
        }
        for a in &mut t.alternatives {
            if a.from == marker {
                a.from = from.clone();
            }
        }
    }
    if new.hash() == h.dna {
        return Ok(h.dna);
    }
    let to = put(store, &new)?;
    let why = DnaEvent::Graft {
        donor_horse: reference,
        donor_dna: Some(hash),
        compat: compat.name().into(),
        reason: "use".into(),
    };
    set_horse_dna_locked(&locked, horse, &to, why)?;
    Ok(to)
}

/// The DNAs that the current DNA of `horse` came from, each once: the
/// parent DNA of its origin, the donor DNA of every `dna.graft` event
/// root first, and the artifact of every `dna.fetch` event.
pub fn lineage_parents(store: &Store, horse: &str) -> Result<Vec<Hash>> {
    let h = store.horse(horse)?;
    let mut out: Vec<Hash> = Vec::new();
    let mut add = |x: Hash| {
        if !out.contains(&x) {
            out.push(x);
        }
    };
    if let Some(p) = get_or_default(store, &h.dna).origin.parent_dna {
        add(p);
    }
    let events = history(store, horse)?;
    let hash_of = |e: &Value, k: &str| -> Result<Option<Hash>> {
        e[k].as_str()
            .map(Hash::parse)
            .transpose()
            .with_context(|| format!("a {} event of horse {horse} has a bad {k}", e["type"]))
    };
    for (_, e) in events.iter().filter(|(_, e)| e["type"] == "dna.graft") {
        if let Some(d) = hash_of(e, "donor_hash")? {
            add(d);
        }
    }
    for (_, e) in events.iter().filter(|(_, e)| e["type"] == "dna.fetch") {
        if let Some(d) = hash_of(e, "hash")? {
            add(d);
        }
    }
    Ok(out)
}

/// Publish the current DNA of `horse` as `reference`, with its lineage,
/// signed with `key` when given, into `index`, and write one
/// `dna.publish` event. The reference must name a version. A refusal of
/// the index (the version is published with another DNA) is returned
/// unchanged. A publish of the same DNA, lineage and signer again is a
/// no-op that returns the manifest that is there.
pub fn publish(
    store: &Store,
    horse: &str,
    index: &dyn Transport,
    reference: &Reference,
    key: Option<&Keypair>,
) -> Result<Manifest> {
    let Version::Exact(version) = &reference.version else {
        bail!("{reference}: a publish needs a version such as @1.0.0, not @latest or @sha256:");
    };
    let locked = store.lock()?;
    let h = store.horse(horse)?;
    let dna = get(store, &h.dna)
        .with_context(|| format!("cannot read the DNA of horse {horse} to publish it"))?;
    let bytes = dna.encode().into_bytes();
    let hash = artifact_hash(&bytes);
    if hash != h.dna {
        bail!(
            "the DNA of horse {horse} is stored as {} but encodes as {hash}; it is not published (an edit of the DNA stores it again in canonical form)",
            h.dna
        );
    }
    let parents = lineage_parents(store, horse)?;
    let name = format!("{}/{}", reference.org, reference.name);
    let author = key.map(Keypair::public_hex);
    let there = index.manifest(&reference.org, &reference.name, version)?;
    let manifest = match there {
        Some(m) if m.hash == hash && m.parents == parents && m.author == author => {
            if has_publish(store, horse, &reference.to_string(), &hash, index.name())? {
                return Ok(m);
            }
            // The index has it, but the event is missing (a crash after
            // the put): write the event now.
            m
        }
        _ => {
            let mut m = Manifest {
                name,
                version: version.clone(),
                hash: hash.clone(),
                parents: parents.clone(),
                created_ms: crate::horse::now_ms(),
                author: None,
                signature: None,
            };
            if let Some(k) = key {
                m.sign(k);
            }
            index.put(&m, &bytes)?;
            m
        }
    };
    let why = DnaEvent::Publish {
        reference: reference.to_string(),
        hash,
        index: index.name().into(),
        parents,
    };
    append(&locked, Some(horse), &why)?;
    Ok(manifest)
}

/// Whether `horse` has a `dna.publish` event of `reference` and `hash`
/// into `index`.
fn has_publish(
    store: &Store,
    horse: &str,
    reference: &str,
    hash: &Hash,
    index: &str,
) -> Result<bool> {
    Ok(history(store, horse)?.iter().any(|(_, e)| {
        e["type"] == "dna.publish"
            && e["reference"].as_str() == Some(reference)
            && e["hash"].as_str() == Some(hash.as_str())
            && e["index"].as_str() == Some(index)
    }))
}

/// The artifact bytes of `h` when the store has it as a DNA blob, so a
/// `@sha256:` reference can be answered before any index. None when the
/// store has no such object, or the object is not a DNA. A damaged
/// object is an error.
pub fn local_object(store: &Store, h: &Hash) -> Result<Option<Vec<u8>>> {
    let node = match store.get(h) {
        Ok(n) => n,
        Err(e)
            if e.root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(None)
        }
        Err(e) => return Err(e),
    };
    if node.meta.kind != "blob" || node.meta.media.as_deref() != Some(MEDIA) {
        return Ok(None);
    }
    Ok(Some(node.raw))
}

/// Resolve `r` on the host: an `@sha256:` reference that the store holds
/// as a DNA comes from the store (the hash pins the bytes, and the store
/// admitted them before; `signed_by` stays None). Otherwise `resolve`
/// over the indexes of `state_dir`, only the index `only` when given, with
/// the trust policy of `state_dir`.
pub fn resolve_on_host(
    store: &Store,
    state_dir: &std::path::Path,
    r: &Reference,
    only: Option<&str>,
) -> Result<Resolved> {
    use crate::horse::registry::{host, open_indexes, resolve, Trust};
    if let Version::Hash(h) = &r.version {
        if let Some(bytes) = local_object(store, h)? {
            let manifest = Manifest {
                name: format!("{}/{}", r.org, r.name),
                version: format!("sha256:{h}"),
                hash: h.clone(),
                parents: Vec::new(),
                created_ms: 0,
                author: None,
                signature: None,
            };
            return Ok(Resolved {
                reference: r.clone(),
                manifest,
                bytes,
                index: "store".into(),
                signed_by: None,
            });
        }
    }
    let mut indexes = open_indexes(state_dir)?;
    if let Some(name) = only {
        indexes.retain(|t| t.name() == name);
        if indexes.is_empty() {
            bail!("there is no index {name}: `paddock horse registry list` shows the indexes");
        }
    }
    let policy = Trust::load(&host::trust_path(state_dir))?;
    resolve(&indexes, r, &policy)
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
