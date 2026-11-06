use super::*;
use crate::horse::Binding;

#[test]
fn grants_and_memory_are_scoped_to_one_horse() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let a = store.new_default_horse().unwrap();
    let b = store.new_default_horse().unwrap();
    let binding = Binding {
        harness: "dsh".into(),
        native_id: None,
        version: "test".into(),
        container: "test".into(),
    };
    let allowed = store.new_session(&a.id, binding.clone()).unwrap();
    let hidden = store.new_session(&a.id, binding.clone()).unwrap();
    let other = store.new_session(&b.id, binding).unwrap();
    let grants = Grants {
        sessions: vec![allowed.id.clone(), other.id.clone()],
    };
    write_memory(&store, &a.id, "note", b"hello").unwrap();
    assert!(write_memory(&store, &a.id, "../escape", b"bad").is_err());
    let tree = build(&store, &a.id, &grants).unwrap();
    assert!(tree
        .paths
        .contains_key(&format!("self/sessions/{}", allowed.id)));
    assert!(!tree
        .paths
        .contains_key(&format!("self/sessions/{}", hidden.id)));
    assert!(!tree
        .paths
        .contains_key(&format!("self/sessions/{}", other.id)));
    assert_eq!(
        tree.paths["self/memory/note"],
        Entry::File(b"hello".to_vec())
    );
}

#[test]
fn ingested_messages_expose_only_normalized_views() {
    use crate::horse::{dsh, live::fake};
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("store")).unwrap();
    let a = store.new_default_horse().unwrap();
    let b = store.new_default_horse().unwrap();
    let root = tmp.path().join("sessions");
    let rows = fake::turn_rows(0, 1, "hello from a", "msg-a");
    fake::write_file(&root, "session-a", &rows);
    let path = dsh::session_dir(&root, Some(fake::CWD), "session-a");
    let ingested = dsh::ingest(&store, &path, dsh::VERSION, Some(&a.id)).unwrap();
    fake::write_file(
        &root,
        "session-b",
        &fake::turn_rows(0, 1, "private b", "msg-b"),
    );
    let path_b = dsh::session_dir(&root, Some(fake::CWD), "session-b");
    let other = dsh::ingest(&store, &path_b, dsh::VERSION, Some(&b.id)).unwrap();
    let tree = build(
        &store,
        &a.id,
        &Grants {
            sessions: vec![ingested.session.clone(), other.session.clone()],
        },
    )
    .unwrap();
    let Entry::File(log) = &tree.paths[&format!("self/sessions/{}/log", ingested.session)] else {
        panic!("log is not a file")
    };
    let text = std::str::from_utf8(log).unwrap();
    assert!(text.contains("user hello from a"));
    assert!(!text.contains("private b"));
    assert!(!tree
        .paths
        .contains_key(&format!("self/sessions/{}", other.session)));
    let Entry::File(jsonl) = &tree.paths[&format!("self/sessions/{}/log.jsonl", ingested.session)]
    else {
        panic!("jsonl is not a file")
    };
    for line in std::str::from_utf8(jsonl).unwrap().lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(value.get("role").is_some());
        assert!(value.get("data").is_none());
    }
}

#[test]
fn broker_matches_tree_and_rejects_missing_horse() {
    use crate::broker::{Request, Status, PROTOCOL};
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let horse = store.new_default_horse().unwrap();
    let session = store
        .new_session(
            &horse.id,
            Binding {
                harness: "dsh".into(),
                native_id: None,
                version: "test".into(),
                container: "test".into(),
            },
        )
        .unwrap();
    let setup = crate::horse::device::Access {
        store: store.clone(),
        horse: horse.id.clone(),
        session: session.id.clone(),
    };
    let request = |op: &str| Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args: [("op".into(), serde_json::json!(op))].into_iter().collect(),
        reason: String::new(),
    };
    let missing = handle(None, &request("sessions"));
    assert_eq!(missing.status, Status::Failed);
    assert_eq!(missing.message, "this sandbox has no horse");
    let response = handle(Some(&setup), &request("sessions"));
    assert_eq!(response.status, Status::Done);
    let bytes: Vec<u8> = serde_json::from_value(response.result["bytes"].clone()).unwrap();
    assert_eq!(bytes, format!("{}\n", session.id).as_bytes());
    let mut log = request("log");
    log.args
        .insert("session".into(), serde_json::json!(session.id));
    let response = handle(Some(&setup), &log);
    let bytes: Vec<u8> = serde_json::from_value(response.result["bytes"].clone()).unwrap();
    let tree = build(
        &store,
        &horse.id,
        &Grants {
            sessions: vec![session.id.clone()],
        },
    )
    .unwrap();
    assert_eq!(
        tree.paths[&format!("self/sessions/{}/log", session.id)],
        Entry::File(bytes)
    );
}

#[test]
fn hitched_views_hide_other_sessions_and_show_only_metadata() {
    use crate::horse::{dsh, hitch, run};
    use serde_json::json;
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let parent = run::begin(&store, "dsh", dsh::VERSION, None, None).unwrap();
    let a = hitch::spawn_child(&store, &parent, "dsh", dsh::VERSION).unwrap();
    let b = hitch::spawn_child(&store, &parent, "dsh", dsh::VERSION).unwrap();
    run::record_event(&store, &a, "run.end", "sandbox", json!({})).unwrap();
    for own in [&parent, &a, &b] {
        let access = Access {
            store: store.clone(),
            horse: own.horse.clone(),
            session: own.session.clone(),
        };
        let tree = access.tree().unwrap();
        for instance in [&parent, &a, &b] {
            assert_eq!(
                tree.paths
                    .contains_key(&format!("self/sessions/{}/log", instance.session)),
                own.session == instance.session
            );
        }
        let Entry::File(instances) = &tree.paths["self/instances"] else {
            panic!()
        };
        let instances = std::str::from_utf8(instances).unwrap();
        assert!(instances.contains(&format!("{} dsh finished", a.session)));
        assert!(instances.contains(&format!("{} dsh running", b.session)));
        let Entry::File(edges) = &tree.paths["self/edges"] else {
            panic!()
        };
        let edges = std::str::from_utf8(edges).unwrap();
        assert_eq!(edges.lines().count(), 2);
        assert!(edges.contains(&format!("SpawnedFrom {} {}", a.session, parent.session)));
    }
}

#[test]
fn dna_toml_reads_the_current_dna_and_a_write_hot_loads() {
    use crate::broker::{Request, Status, PROTOCOL};
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    use crate::horse::run;
    let rh = run::begin(&store, "dsh", dsh::VERSION, None, None).unwrap();
    run::record_event(&store, &rh, "run.begin", "sb", serde_json::json!({})).unwrap();
    let setup = Access {
        store: store.clone(),
        horse: rh.horse.clone(),
        session: rh.session.clone(),
    };
    let dna_file = || match &setup.tree().unwrap().paths[DNA_PATH] {
        Entry::File(b) => String::from_utf8(b.clone()).unwrap(),
        e => panic!("{e:?}"),
    };
    assert!(dna_file().starts_with("# horse DNA\n"));
    assert!(writable(DNA_PATH) && !writable("self/dna") && !writable("self/id"));
    let request = |op: &str, bytes: Option<&[u8]>| {
        let mut args: std::collections::BTreeMap<String, serde_json::Value> =
            [("op".into(), serde_json::json!(op))].into_iter().collect();
        if let Some(b) = bytes {
            args.insert("bytes".into(), serde_json::json!(b));
        }
        Request {
            v: PROTOCOL,
            cap: "horse".into(),
            args,
            reason: String::new(),
        }
    };
    let bad = handle(Some(&setup), &request("dna_put", Some(b"not = [toml")));
    assert_eq!(bad.status, Status::Failed);
    let ok = handle(
        Some(&setup),
        &request("dna_put", Some(b"[traits]\nstyle = \"careful\"\n")),
    );
    assert_eq!(ok.status, Status::Done, "{}", ok.message);
    assert!(dna_file().contains("careful"));
    let read = handle(Some(&setup), &request("dna", None));
    let bytes: Vec<u8> = serde_json::from_value(read.result["bytes"].clone()).unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), dna_file());
    // The running session gets the hot-load entry after its start entry.
    let inbox = store.inbox(&rh.session).unwrap();
    assert_eq!(inbox.len(), 2);
    assert!(inbox[1].text.contains("style = careful"));
}
