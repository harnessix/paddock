use super::super::{get, graft::graft_horses, history, hot_load, new_child_horse_locked, parse};
use super::*;
use crate::horse::registry::{self as reg, trust::new_keypair, FileIndex, Trust, TrustedKey};
use crate::horse::{dsh, run};
use serde_json::json;
use std::path::Path;

/// Publish `text` as `org/name@version` into the index at `dir`, signed by
/// `key`, and resolve it as a fetch does.
fn resolved(dir: &Path, key: &Keypair, reference: &str, text: &str) -> Resolved {
    let r: Reference = reference.parse().unwrap();
    let bytes = parse(text).unwrap().encode().into_bytes();
    let Version::Exact(version) = &r.version else {
        panic!("{reference} has no version");
    };
    let mut m = Manifest {
        name: format!("{}/{}", r.org, r.name),
        version: version.clone(),
        hash: artifact_hash(&bytes),
        parents: Vec::new(),
        created_ms: 1,
        author: None,
        signature: None,
    };
    m.sign(key);
    let idx = FileIndex::new("test", dir);
    idx.put(&m, &bytes).unwrap();
    let trust = Trust {
        require_signature: true,
        keys: vec![TrustedKey {
            org: r.org.clone(),
            public: key.public_hex(),
            name: Some("ci".into()),
        }],
    };
    let indexes: Vec<Box<dyn Transport>> = vec![Box::new(idx)];
    reg::resolve(&indexes, &r, &trust).unwrap()
}

fn kinds(st: &Store, horse: &str) -> Vec<String> {
    history(st, horse)
        .unwrap()
        .iter()
        .map(|(_, e)| e["type"].as_str().unwrap().to_string())
        .collect()
}

fn count_objects(root: &Path) -> usize {
    std::fs::read_dir(root.join("objects"))
        .unwrap()
        .map(|d| std::fs::read_dir(d.unwrap().path()).unwrap().count())
        .sum()
}

#[test]
fn import_twice_stores_one_blob_and_two_fetch_events() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let key = new_keypair();
    let r = resolved(
        &t.path().join("idx"),
        &key,
        "registry://acme/review@1.2.0",
        "[traits]\nlang = \"en\"\n",
    );
    let horse = st.new_default_horse().unwrap();
    let a = import(&st, &r, Some(&horse.id)).unwrap();
    let objects = count_objects(&t.path().join("store"));
    let b = import(&st, &r, Some(&horse.id)).unwrap();
    assert_eq!(a, b);
    assert_eq!(a, r.manifest.hash);
    // The second fetch adds one event node and no blob.
    assert_eq!(count_objects(&t.path().join("store")), objects + 1);
    assert_eq!(
        kinds(&st, &horse.id),
        ["dna.default", "dna.fetch", "dna.fetch"]
    );
    let (_, e) = history(&st, &horse.id).unwrap().pop().unwrap();
    assert_eq!(e["reference"], "registry://acme/review@1.2.0");
    assert_eq!(e["hash"], json!(a));
    assert_eq!(e["index"], "test");
    assert_eq!(e["signed_by"], "ci");
    assert_eq!(e["horse"], json!(horse.id));
    assert!(e["at_ms"].as_u64().unwrap() > 0);
    // A fetch does not change the DNA of the horse.
    assert_eq!(st.horse(&horse.id).unwrap().dna, horse.dna);
    // The blob is the artifact, unchanged.
    assert_eq!(local_object(&st, &a).unwrap().unwrap(), r.bytes);

    // A fetch for no horse goes into the chain of the store.
    import(&st, &r, None).unwrap();
    let store_events = store_events(&st).unwrap();
    assert_eq!(store_events.len(), 1);
    assert_eq!(store_events[0].1["type"], "dna.fetch");
    assert!(store_events[0].1.get("horse").is_none());
    assert_eq!(history(&st, &horse.id).unwrap().len(), 3);

    // A resolved artifact whose bytes do not match its manifest is refused.
    let mut bad = r.clone();
    bad.bytes = parse("[traits]\nlang = \"de\"\n")
        .unwrap()
        .encode()
        .into_bytes();
    assert!(import(&st, &bad, Some(&horse.id)).is_err());
    assert!(import(&st, &r, Some("no-such-horse")).is_err());
    assert_eq!(history(&st, &horse.id).unwrap().len(), 3);
}

#[test]
fn use_grafts_with_registry_provenance_and_tells_running_sessions() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let key = new_keypair();
    let host = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    run::record_event(&st, &host, "run.begin", "sb-a", json!({})).unwrap();
    hot_load(&st, &host.horse, "[traits]\nstyle = \"short\"\n").unwrap();
    let before = st.inbox(&host.session).unwrap().len();
    let r = resolved(
        &t.path().join("idx"),
        &key,
        "registry://acme/review@1.2.0",
        "[traits]\nlang = \"en\"\nstyle = \"long\"\ntone = \"calm\"\n",
    );
    let to = use_in(&st, &host.horse, &r).unwrap();
    assert_eq!(st.horse(&host.horse).unwrap().dna, to);
    let events = history(&st, &host.horse).unwrap();
    let n = events.len();
    assert_eq!(events[n - 2].1["type"], "dna.fetch");
    assert_eq!(events[n - 1].1["type"], "dna.graft");
    let fetch = events[n - 2].0.clone();
    let g = &events[n - 1].1;
    assert_eq!(g["donor_horse"], "registry://acme/review@1.2.0");
    assert_eq!(g["donor_hash"], json!(r.manifest.hash));
    assert_eq!(g["compat"], "requires-adaptation");
    assert_eq!(g["reason"], "use");

    let from = Provenance::Registry {
        reference: "registry://acme/review@1.2.0".into(),
        hash: r.manifest.hash.clone(),
        event: fetch,
    };
    let d = get(&st, &to).unwrap();
    assert_eq!(d.traits["lang"].from, from);
    assert_eq!(d.traits["tone"].from, from);
    // The host's value wins; the artifact's value is an alternative.
    assert_eq!(d.traits["style"].value, "short");
    assert_eq!(d.traits["style"].alternatives[0].from, from);
    assert_eq!(parse(&d.encode()).unwrap(), d);

    let inbox = st.inbox(&host.session).unwrap();
    assert_eq!(inbox.len(), before + 1);
    let last = inbox.last().unwrap();
    assert_eq!(last.role, "system");
    assert!(last.text.contains("lang = en"), "{}", last.text);
    assert!(
        last.text.contains("registry://acme/review@1.2.0"),
        "{}",
        last.text
    );

    // The same artifact again adds nothing: one more fetch, no graft.
    assert_eq!(use_in(&st, &host.horse, &r).unwrap(), to);
    let k = kinds(&st, &host.horse);
    assert_eq!(&k[k.len() - 3..], ["dna.fetch", "dna.graft", "dna.fetch"]);
    assert_eq!(st.inbox(&host.session).unwrap().len(), before + 1);
}

#[test]
fn an_incompatible_artifact_is_refused_and_writes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("store");
    let st = Store::open(&root).unwrap();
    let key = new_keypair();
    let horse = st.new_default_horse().unwrap();
    hot_load(&st, &horse.id, "[policies]\nreview = \"always\"\n").unwrap();
    let dir = t.path().join("idx");
    let policy = resolved(
        &dir,
        &key,
        "registry://acme/a@1.0.0",
        "[policies]\nreview = \"never\"\n",
    );
    let cap = resolved(
        &dir,
        &key,
        "registry://acme/b@1.0.0",
        "capabilities = [\"net\"]\n",
    );
    let before = st.horse(&horse.id).unwrap();
    let events = run::events(&st, &horse.id).unwrap();
    let objects = count_objects(&root);
    let e = format!("{:#}", use_in(&st, &horse.id, &policy).unwrap_err());
    assert!(e.contains("incompatible: policy review"), "{e}");
    // A capability that the host lacks is never granted by a graft.
    let e = format!("{:#}", use_in(&st, &horse.id, &cap).unwrap_err());
    assert!(e.contains("capability net"), "{e}");
    assert_eq!(st.horse(&horse.id).unwrap(), before);
    assert_eq!(run::events(&st, &horse.id).unwrap(), events);
    assert_eq!(count_objects(&root), objects);
    assert!(store_events(&st).unwrap().is_empty());
}

#[test]
fn publish_writes_a_signed_manifest_with_lineage() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let key = new_keypair();
    let parent = st.new_default_horse().unwrap();
    hot_load(&st, &parent.id, "[traits]\nstyle = \"short\"\n").unwrap();
    let parent_dna = st.horse(&parent.id).unwrap().dna;
    let child = new_child_horse_locked(&st.lock().unwrap(), &parent.id).unwrap();
    let donor = st.new_default_horse().unwrap();
    hot_load(&st, &donor.id, "[traits]\nlang = \"en\"\n").unwrap();
    let donor_dna = st.horse(&donor.id).unwrap().dna;
    graft_horses(&st, &child.id, &donor.id, "combine")
        .unwrap()
        .unwrap();
    assert_eq!(
        lineage_parents(&st, &child.id).unwrap(),
        [parent_dna.clone(), donor_dna.clone()]
    );

    let idx = FileIndex::new("local", t.path().join("idx"));
    let r: Reference = "registry://acme/review@1.0.0".parse().unwrap();
    let m = publish(&st, &child.id, &idx, &r, Some(&key)).unwrap();
    let h = st.horse(&child.id).unwrap();
    assert_eq!(m.hash, h.dna);
    assert_eq!(m.parents, [parent_dna.clone(), donor_dna.clone()]);
    assert_eq!(m.name, "acme/review");
    assert_eq!(m.version, "1.0.0");
    assert_eq!(m.author.as_deref(), Some(key.public_hex().as_str()));
    assert!(m.verify(&key.public_hex()));
    assert!(!m.verify(&new_keypair().public_hex()));
    // The index has the manifest and the artifact, and the store hash of
    // the DNA is the registry hash.
    assert_eq!(idx.manifest("acme", "review", "1.0.0").unwrap().unwrap(), m);
    let bytes = idx.object(&m.hash).unwrap().unwrap();
    assert_eq!(local_object(&st, &h.dna).unwrap().unwrap(), bytes);
    let (_, e) = history(&st, &child.id).unwrap().pop().unwrap();
    assert_eq!(e["type"], "dna.publish");
    assert_eq!(e["reference"], "registry://acme/review@1.0.0");
    assert_eq!(e["hash"], json!(h.dna));
    assert_eq!(e["index"], "local");
    assert_eq!(e["parents"], json!([parent_dna, donor_dna]));

    // The same publish again is a no-op.
    let n = history(&st, &child.id).unwrap().len();
    assert_eq!(publish(&st, &child.id, &idx, &r, Some(&key)).unwrap(), m);
    assert_eq!(history(&st, &child.id).unwrap().len(), n);

    // The version with another DNA: the index refuses, nothing changes.
    hot_load(
        &st,
        &child.id,
        "[traits]\nstyle = \"short\"\nlang = \"en\"\ntone = \"calm\"\n",
    )
    .unwrap()
    .unwrap();
    let n = history(&st, &child.id).unwrap().len();
    let err = format!(
        "{:#}",
        publish(&st, &child.id, &idx, &r, Some(&key)).unwrap_err()
    );
    assert!(err.contains("a version cannot change"), "{err}");
    assert_eq!(history(&st, &child.id).unwrap().len(), n);
    assert_eq!(idx.manifest("acme", "review", "1.0.0").unwrap().unwrap(), m);

    // A publish needs an exact version.
    let latest: Reference = "registry://acme/review@latest".parse().unwrap();
    assert!(publish(&st, &child.id, &idx, &latest, None).is_err());
    // An unsigned publish of a new version has no author.
    let r2: Reference = "registry://acme/review@1.1.0".parse().unwrap();
    let m2 = publish(&st, &child.id, &idx, &r2, None).unwrap();
    assert!(m2.author.is_none() && m2.signature.is_none());
}

#[test]
fn a_fetch_is_a_lineage_parent() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let key = new_keypair();
    let horse = st.new_default_horse().unwrap();
    let r = resolved(
        &t.path().join("idx"),
        &key,
        "registry://acme/review@1.2.0",
        "[traits]\nlang = \"en\"\n",
    );
    use_in(&st, &horse.id, &r).unwrap();
    // The graft donor and the fetch are one artifact: listed once.
    assert_eq!(lineage_parents(&st, &horse.id).unwrap(), [r.manifest.hash]);
}

#[test]
fn local_object_round_trips_dna_and_ignores_other_objects() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let d = parse("[traits]\nlang = \"en\"\n").unwrap();
    let h = put(&st, &d).unwrap();
    let bytes = local_object(&st, &h).unwrap().unwrap();
    assert_eq!(bytes, d.encode().into_bytes());
    assert_eq!(artifact_hash(&bytes), h);
    assert_eq!(local_object(&st, &Hash::of(b"nothing")).unwrap(), None);
    let other = st.put_blob("text/plain", b"hello").unwrap();
    assert_eq!(local_object(&st, &other).unwrap(), None);
}
