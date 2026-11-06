//! `registry.fetch` through the broker: the card, the ledger row and the
//! audit line.
use super::*;
use crate::horse::registry::{artifact_hash, host as rhost, trust::new_keypair, FileIndex};
use crate::horse::registry::{Manifest, Transport as _};

fn rows(b: &Broker) -> Vec<(String, String)> {
    let c = rusqlite::Connection::open(crate::devices::ledger::path(&b.env)).unwrap();
    let mut s = c
        .prepare("SELECT device, status FROM requests ORDER BY id")
        .unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A horse for the broker, and `t/x@1.0.0` signed by a trusted key in the
/// local index of the broker's state folder. Returns the artifact hash.
fn give_horse(b: &Broker, tmp: &Path) -> crate::horse::Hash {
    let store = crate::horse::Store::open(&tmp.join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let state = b.env.paddock_state();
    let key = new_keypair();
    rhost::trust_add(&state, "t", &key.public_hex(), None).unwrap();
    let bytes = crate::horse::dna::parse("[traits]\nstyle = \"broker\"\n")
        .unwrap()
        .encode()
        .into_bytes();
    let mut m = Manifest {
        name: "t/x".into(),
        version: "1.0.0".into(),
        hash: artifact_hash(&bytes),
        parents: Vec::new(),
        created_ms: 1,
        author: None,
        signature: None,
    };
    m.sign(&key);
    FileIndex::new("local", rhost::dir(&state).join("local"))
        .put(&m, &bytes)
        .unwrap();
    b.horse
        .set(crate::horse::device::Access {
            store,
            horse: h.id,
            session: String::new(),
        })
        .ok()
        .unwrap();
    m.hash
}

fn fetch() -> Request {
    req(
        "horse",
        serde_json::json!({"op": "registry.fetch", "ref": "registry://t/x@1.0.0", "use": true}),
    )
}

#[test]
fn without_a_horse_the_fetch_is_refused_without_a_card() {
    let (_tmp, b, fixed) = setup(Some(Answer::Once), HostPolicy::default());
    let r = b.handle(&fetch());
    assert_eq!(r.status, Status::Failed);
    assert_eq!(r.message, "this sandbox has no horse");
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_denied_fetch_writes_one_ledger_row_and_an_audit_line() {
    let (tmp, b, fixed) = setup(Some(Answer::Deny), HostPolicy::default());
    give_horse(&b, tmp.path());
    let access = b.horse.get().unwrap();
    let before = access.store.horse(&access.horse).unwrap().dna;
    let r = b.handle(&fetch());
    assert_eq!(r.status, Status::Denied);
    assert_eq!(r.message, "fetch of registry://t/x@1.0.0 was not approved");
    assert_eq!(fixed.asked.load(Ordering::SeqCst), 1);
    assert_eq!(
        rows(&b),
        [(
            "horse".to_string(),
            "registry.fetch denied registry://t/x@1.0.0".to_string()
        )]
    );
    assert_eq!(access.store.horse(&access.horse).unwrap().dna, before);
    let audit = std::fs::read_to_string(audit_file(&b.env)).unwrap();
    let line: serde_json::Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(line["capability"], "horse");
    assert_eq!(line["decision"], "denied");
    assert_eq!(line["detail"], "registry.fetch registry://t/x@1.0.0");
}

#[test]
fn an_approved_fetch_grafts_and_the_ledger_row_names_the_hash() {
    let (tmp, b, _) = setup(Some(Answer::Once), HostPolicy::default());
    let hash = give_horse(&b, tmp.path());
    let access = b.horse.get().unwrap();
    let before = access.store.horse(&access.horse).unwrap().dna;
    let r = b.handle(&fetch());
    assert_eq!(r.status, Status::Done, "{}", r.message);
    assert_ne!(access.store.horse(&access.horse).unwrap().dna, before);
    assert_eq!(
        rows(&b),
        [(
            "horse".to_string(),
            format!("registry.fetch registry://t/x@1.0.0 {hash} trusted")
        )]
    );
    let audit = std::fs::read_to_string(audit_file(&b.env)).unwrap();
    let line: serde_json::Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(line["decision"], "allowed once");
}
