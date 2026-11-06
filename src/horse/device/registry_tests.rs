use super::*;
use crate::horse::registry::{
    artifact_hash, host as rhost, trust::new_keypair, FileIndex, Keypair, Manifest, Transport,
};
use crate::horse::Store;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// An approval stub: a fixed answer, and the ledger lines it got.
struct Stub {
    state: PathBuf,
    answer: Answer,
    asked: Mutex<Vec<String>>,
    lines: Mutex<Vec<String>>,
}

impl Host for Stub {
    fn state_dir(&self) -> PathBuf {
        self.state.clone()
    }
    fn ask(&self, summary: &str, _detail: &str) -> Answer {
        self.asked.lock().unwrap().push(summary.into());
        self.answer
    }
    fn record(&self, line: &str) {
        self.lines.lock().unwrap().push(line.into());
    }
}

struct World {
    _tmp: tempfile::TempDir,
    store: Store,
    state: PathBuf,
    access: Access,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("store")).unwrap();
    let state = tmp.path().join("state");
    let h = store.new_default_horse().unwrap();
    let access = Access {
        store: store.clone(),
        horse: h.id.clone(),
        session: String::new(),
    };
    World {
        _tmp: tmp,
        store,
        state,
        access,
    }
}

/// Publish `text` as `t/x@<version>` into the default local index of
/// `state`, signed by `key` when given.
fn publish(state: &Path, key: Option<&Keypair>, version: &str, text: &str) -> Manifest {
    let bytes = dna::parse(text).unwrap().encode().into_bytes();
    let mut m = Manifest {
        name: "t/x".into(),
        version: version.into(),
        hash: artifact_hash(&bytes),
        parents: Vec::new(),
        created_ms: 7,
        author: None,
        signature: None,
    };
    if let Some(k) = key {
        m.sign(k);
    }
    FileIndex::new("local", rhost::dir(state).join("local"))
        .put(&m, &bytes)
        .unwrap();
    m
}

fn stub(state: &Path, answer: Answer) -> Stub {
    Stub {
        state: state.to_path_buf(),
        answer,
        asked: Mutex::new(Vec::new()),
        lines: Mutex::new(Vec::new()),
    }
}

fn fetch_req(r: &str, graft_it: bool) -> Request {
    let mut args: std::collections::BTreeMap<String, serde_json::Value> =
        [("op".into(), json!(FETCH)), ("ref".into(), json!(r))]
            .into_iter()
            .collect();
    if graft_it {
        args.insert("use".into(), json!(true));
    }
    Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args,
        reason: String::new(),
    }
}

fn output(resp: &Response) -> String {
    let bytes: Vec<u8> = serde_json::from_value(resp.result["bytes"].clone()).unwrap();
    String::from_utf8(bytes).unwrap()
}

fn kinds(w: &World) -> Vec<String> {
    dna::history(&w.store, &w.access.horse)
        .unwrap()
        .iter()
        .map(|(_, e)| e["type"].as_str().unwrap().to_string())
        .collect()
}

const REF: &str = "registry://t/x@1.0.0";
const TEXT: &str = "[traits]\nstyle = \"registry-test\"\n";

#[test]
fn a_denied_fetch_writes_nothing_and_one_denied_line() {
    let w = world();
    let key = new_keypair();
    rhost::trust_add(&w.state, "t", &key.public_hex(), None).unwrap();
    publish(&w.state, Some(&key), "1.0.0", TEXT);
    let before = w.store.horse(&w.access.horse).unwrap().dna;
    let events = kinds(&w);
    let host = stub(&w.state, Answer::Denied);
    let resp = handle(Some(&w.access), &fetch_req(REF, true), &host);
    assert_eq!(resp.status, Status::Denied);
    assert_eq!(resp.message, format!("fetch of {REF} was not approved"));
    assert_eq!(
        *host.lines.lock().unwrap(),
        [format!("registry.fetch denied {REF}")]
    );
    let short = &w.access.horse[..12];
    assert_eq!(
        *host.asked.lock().unwrap(),
        [format!("fetch DNA {REF} for horse {short}")]
    );
    assert_eq!(w.store.horse(&w.access.horse).unwrap().dna, before);
    assert_eq!(kinds(&w), events);
    assert!(dna::registry::store_events(&w.store).unwrap().is_empty());
}

#[test]
fn an_approved_fetch_with_use_changes_the_dna_and_names_the_hash() {
    let w = world();
    let key = new_keypair();
    rhost::trust_add(&w.state, "t", &key.public_hex(), Some("ci")).unwrap();
    let m = publish(&w.state, Some(&key), "1.0.0", TEXT);
    let before = w.store.horse(&w.access.horse).unwrap().dna;
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(Some(&w.access), &fetch_req(REF, true), &host);
    assert_eq!(resp.status, Status::Done, "{}", resp.message);
    let after = w.store.horse(&w.access.horse).unwrap().dna;
    assert_ne!(after, before);
    assert_eq!(
        *host.lines.lock().unwrap(),
        [format!("registry.fetch {REF} {} trusted", m.hash)]
    );
    let out = output(&resp);
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some(m.hash.as_str()));
    assert_eq!(
        lines.next().unwrap(),
        format!(
            "{REF} hash {} parents - created_ms 7 verdict Trusted",
            m.hash
        )
    );
    assert_eq!(
        lines.next().unwrap(),
        format!("the DNA of this horse is now {after}")
    );
    // No path, no key, no trust entry, no index name.
    for secret in [
        w.state.to_str().unwrap(),
        key.public_hex().as_str(),
        "ci",
        "local",
    ] {
        assert!(!out.contains(secret), "{secret} in {out}");
    }
    assert_eq!(kinds(&w), ["dna.default", "dna.fetch", "dna.graft"]);
    let explained = dna::explain::explain(&w.store, &w.access.horse).unwrap();
    assert!(explained.contains("registry-test"), "{explained}");
}

#[test]
fn an_approved_fetch_without_use_only_stores_the_artifact() {
    let w = world();
    let key = new_keypair();
    rhost::trust_add(&w.state, "t", &key.public_hex(), None).unwrap();
    let m = publish(&w.state, Some(&key), "1.2.0", TEXT);
    let before = w.store.horse(&w.access.horse).unwrap().dna;
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(
        Some(&w.access),
        &fetch_req("registry://t/x@latest", false),
        &host,
    );
    assert_eq!(resp.status, Status::Done, "{}", resp.message);
    assert_eq!(w.store.horse(&w.access.horse).unwrap().dna, before);
    // `latest` is recorded as the version that it resolved to.
    assert_eq!(
        *host.lines.lock().unwrap(),
        [format!(
            "registry.fetch registry://t/x@1.2.0 {} trusted",
            m.hash
        )]
    );
    assert_eq!(kinds(&w), ["dna.default", "dna.fetch"]);
    assert!(dna::registry::local_object(&w.store, &m.hash)
        .unwrap()
        .is_some());
}

#[test]
fn a_trust_refusal_names_only_the_verdict_after_the_approval() {
    let w = world();
    // Unsigned, and the default policy requires a signature.
    publish(&w.state, None, "1.0.0", TEXT);
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(Some(&w.access), &fetch_req(REF, true), &host);
    assert_eq!(resp.status, Status::Failed);
    assert_eq!(
        resp.message,
        format!("{REF} was refused by the trust policy of the host: Unsigned")
    );
    assert_eq!(host.asked.lock().unwrap().len(), 1);
    assert_eq!(
        *host.lines.lock().unwrap(),
        [format!("registry.fetch refused {REF} Unsigned")]
    );
    // An untrusted key: the key itself stays out of the answer.
    let other = new_keypair();
    publish(&w.state, Some(&other), "2.0.0", TEXT);
    let resp = handle(
        Some(&w.access),
        &fetch_req("registry://t/x@2.0.0", true),
        &host,
    );
    assert_eq!(
        resp.message,
        "registry://t/x@2.0.0 was refused by the trust policy of the host: UntrustedKey"
    );
    assert!(!resp.message.contains(&other.public_hex()));
    assert_eq!(kinds(&w), ["dna.default"]);
}

#[test]
fn a_missing_reference_and_a_bad_index_do_not_leak_paths() {
    let w = world();
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(Some(&w.access), &fetch_req(REF, false), &host);
    assert_eq!(resp.status, Status::Failed);
    assert_eq!(
        resp.message,
        format!("{REF} is not in any index of the host")
    );
    assert_eq!(
        *host.lines.lock().unwrap(),
        [format!("registry.fetch failed {REF}")]
    );
    let mut req = fetch_req(REF, false);
    req.args.insert("index".into(), json!("../x"));
    let resp = handle(Some(&w.access), &req, &host);
    assert_eq!(resp.status, Status::Invalid);
    // A bad reference is refused before any card.
    let resp = handle(
        Some(&w.access),
        &fetch_req("registry://T/x@1", false),
        &host,
    );
    assert_eq!(resp.status, Status::Invalid);
    assert_eq!(host.asked.lock().unwrap().len(), 1);
    // A broken registry.toml: the error on the host names its path, the
    // answer to the sandbox does not.
    let file = rhost::config_path(&w.state);
    std::fs::create_dir_all(rhost::dir(&w.state)).unwrap();
    std::fs::write(&file, "not toml [").unwrap();
    let resp = handle(Some(&w.access), &fetch_req(REF, false), &host);
    assert_eq!(resp.status, Status::Failed);
    assert_eq!(
        resp.message,
        format!("fetch of {REF} failed on the host; the user sees why with `paddock horse registry resolve {REF}`")
    );
    assert!(!resp.message.contains(w.state.to_str().unwrap()));
}

#[test]
fn a_hash_reference_comes_from_the_store_first() {
    let w = world();
    let d = dna::parse(TEXT).unwrap();
    let h = dna::put(&w.store, &d).unwrap();
    // No index has it, and the trust policy requires a signature: the
    // store answers before either.
    let r = format!("registry://t/x@sha256:{h}");
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(Some(&w.access), &fetch_req(&r, true), &host);
    assert_eq!(resp.status, Status::Done, "{}", resp.message);
    assert_eq!(
        *host.lines.lock().unwrap(),
        [format!("registry.fetch {r} {h} unsigned")]
    );
    assert!(output(&resp).starts_with(&format!("{h}\n")));
}

#[test]
fn an_incompatible_dna_is_refused_with_its_conflicts() {
    let w = world();
    let key = new_keypair();
    rhost::trust_add(&w.state, "t", &key.public_hex(), None).unwrap();
    publish(&w.state, Some(&key), "1.0.0", "capabilities = [\"net\"]\n");
    let before = w.store.horse(&w.access.horse).unwrap().dna;
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(Some(&w.access), &fetch_req(REF, true), &host);
    assert_eq!(resp.status, Status::Failed);
    assert!(resp.message.contains("incompatible"), "{}", resp.message);
    assert!(resp.message.contains("net"), "{}", resp.message);
    assert_eq!(w.store.horse(&w.access.horse).unwrap().dna, before);
    assert_eq!(kinds(&w), ["dna.default"]);
}

#[test]
fn no_horse_gets_the_device_error_and_no_card() {
    let w = world();
    let host = stub(&w.state, Answer::Approved);
    for req in [fetch_req(REF, true), sync_req("local")] {
        let resp = handle(None, &req, &host);
        assert_eq!(resp.status, Status::Failed);
        assert_eq!(resp.message, "this sandbox has no horse");
    }
    assert!(host.asked.lock().unwrap().is_empty());
    assert!(host.lines.lock().unwrap().is_empty());
}

fn sync_req(index: &str) -> Request {
    Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: [("op".into(), json!(SYNC)), ("index".into(), json!(index))]
            .into_iter()
            .collect(),
        reason: String::new(),
    }
}

#[test]
fn sync_asks_first_and_a_plain_folder_is_a_no_op() {
    let w = world();
    std::fs::create_dir_all(rhost::dir(&w.state).join("local")).unwrap();
    let denied = stub(&w.state, Answer::Denied);
    let resp = handle(Some(&w.access), &sync_req("local"), &denied);
    assert_eq!(resp.status, Status::Denied);
    assert_eq!(resp.message, "sync of local was not approved");
    assert_eq!(
        *denied.lines.lock().unwrap(),
        ["registry.sync denied local"]
    );
    let late = stub(&w.state, Answer::TimedOut(5));
    let resp = handle(Some(&w.access), &sync_req("local"), &late);
    assert_eq!(resp.status, Status::Timeout);
    assert_eq!(
        *late.lines.lock().unwrap(),
        ["registry.sync timed out local"]
    );
    let host = stub(&w.state, Answer::Approved);
    let resp = handle(Some(&w.access), &sync_req("local"), &host);
    assert_eq!(resp.status, Status::Done, "{}", resp.message);
    assert_eq!(
        output(&resp),
        "index local is a plain folder: nothing to sync\n"
    );
    assert_eq!(*host.lines.lock().unwrap(), ["registry.sync local plain"]);
    let resp = handle(Some(&w.access), &sync_req("nope"), &host);
    assert_eq!(resp.status, Status::Failed);
    assert!(!resp.message.contains(w.state.to_str().unwrap()));
}
