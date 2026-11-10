use super::*;

#[test]
fn final_ingest_ignores_a_transcript_owned_by_another_horse() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let first = super::super::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    let second = super::super::begin(&store, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    store
        .lock()
        .unwrap()
        .bind_native(&first.session, "session-other")
        .unwrap();
    store
        .lock()
        .unwrap()
        .bind_native(&second.session, "session-own")
        .unwrap();
    let home = dir.path().join("home/demo");
    let before = Snapshot::capture(&home, dsh::HARNESS).unwrap();
    let root = home.join(".dsh/sessions");
    let rows = crate::horse::live::fake::turn_rows(0, 1, "hello", "m1");
    crate::horse::live::fake::write_file(&root, "session-other", &rows);
    end(&store, &second, "sandbox", &home, &before, true, None).unwrap();
    assert_eq!(
        store.horse(&second.horse).unwrap().sessions,
        vec![second.session.clone()]
    );
    assert!(
        store
            .session(&second.session)
            .unwrap()
            .binding
            .native_id
            .as_deref()
            == Some("session-own")
    );
    assert!(store
        .session_nodes(&store.session(&first.session).unwrap())
        .unwrap()
        .is_empty());
}

#[test]
fn bound_claude_run_ignores_another_runs_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let first = super::super::begin(&store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    let child = super::super::begin(&store, claude::HARNESS, claude::VERSION, None, None).unwrap();
    let locked = store.lock().unwrap();
    let other = "01934b20-1111-7777-8888-123456789abc";
    let own = "01934b20-1111-7777-8888-123456789abd";
    locked.bind_native(&first.session, other).unwrap();
    locked.bind_native(&child.session, own).unwrap();
    drop(locked);
    let home = dir.path().join("home/demo");
    let before = Snapshot::capture(&home, claude::HARNESS).unwrap();
    let root = home.join(".claude/projects/project");
    std::fs::create_dir_all(&root).unwrap();
    let row = |id: &str| {
        format!("{{\"type\":\"user\",\"uuid\":\"{id}\",\"sessionId\":\"{id}\",\"version\":\"{}\",\"message\":{{\"role\":\"user\",\"content\":\"hello\"}}}}\n", claude::VERSION)
    };
    for id in [own, other] {
        std::fs::write(root.join(format!("{id}.jsonl")), row(id)).unwrap();
    }
    end(&store, &child, "sandbox", &home, &before, true, None).unwrap();
    assert_eq!(
        store.horse(&child.horse).unwrap().sessions,
        vec![child.session.clone()]
    );
    assert_eq!(
        store
            .session_nodes(&store.session(&child.session).unwrap())
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .session_nodes(&store.session(&first.session).unwrap())
        .unwrap()
        .is_empty());
}
