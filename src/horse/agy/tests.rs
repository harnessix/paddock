use super::fixture::{Fixture, ID};
use super::*;
use crate::horse::guard::{self, Op};

fn node_by_key(st: &Store, session: &str, key: &str) -> Hash {
    let s = st.session(session).unwrap();
    st.resolve_node(&s, key).unwrap()
}

#[test]
fn round_trip_keeps_every_row_column_by_column_and_blob_by_blob() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let src = Fixture::two_turns().write(&t.path().join("in"));
    let r = ingest(&st, &src, VERSION, None).unwrap();
    // 6 steps, 2 gen_metadata rows, 1 executor_metadata row.
    assert_eq!((r.added, r.kept, r.updated, r.dropped), (9, 0, 0, 0));
    let out = project(&st, &r.session, &t.path().join("out")).unwrap();
    assert_eq!(out.native_id, ID);
    assert!(!out.forked);
    let a = db::read(&src).unwrap();
    let b = db::read(&out.path).unwrap();
    assert_eq!(a.rows, b.rows);
    assert_eq!(a.header, b.header);
    assert_eq!(a.summary, b.summary);
    assert!(a.summary.is_some());
    assert_eq!(
        request::body(&a.rows, i64::MAX),
        request::body(&b.rows, i64::MAX)
    );
    // A second ingest adds nothing, and a projection never overwrites.
    let again = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!(
        (again.session.as_str(), again.added, again.kept),
        (r.session.as_str(), 0, 9)
    );
    assert!(project(&st, &r.session, &t.path().join("out")).is_err());
    // The projection reads back into the same session.
    let back = ingest(&st, &out.path, VERSION, None).unwrap();
    assert_eq!((back.session, back.added), (r.session, 0));
}

#[test]
fn keys_are_the_table_and_the_step_index() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let src = Fixture::two_turns().write(t.path());
    let r = ingest(&st, &src, VERSION, None).unwrap();
    let nodes = st.session_nodes(&st.session(&r.session).unwrap()).unwrap();
    let keys: Vec<&str> = nodes.iter().filter_map(|n| n.meta.key.as_deref()).collect();
    assert_eq!(
        keys,
        [
            "steps:0",
            "executor_metadata:1",
            "steps:1",
            "gen_metadata:2",
            "steps:2",
            "steps:3",
            "steps:4",
            "gen_metadata:5",
            "steps:5"
        ]
    );
    assert!(describe(&nodes[0]).contains("user_input"));
    assert!(describe(&nodes[4]).contains("planner_response, done"));
}

#[test]
fn a_row_update_and_a_truncation_lose_no_node() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    f.running();
    let src = f.write(t.path());
    let r = ingest(&st, &src, VERSION, None).unwrap();
    let running = node_by_key(&st, &r.session, "steps:5");

    // agy finishes the step at the same index: a new node after the old one.
    let mut f = Fixture::two_turns();
    f.write(t.path());
    let u = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((u.added, u.updated, u.dropped), (1, 1, 0));
    let s = st.session(&r.session).unwrap();
    let nodes = st.session_nodes(&s).unwrap();
    assert!(nodes.iter().any(|n| n.hash == running));
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.meta.key.as_deref() == Some("steps:5"))
            .count(),
        2
    );

    let before: Vec<Hash> = nodes.iter().map(|n| n.hash.clone()).collect();

    // A rewind to step 4, then a new turn at the same indexes.
    f.truncate(4);
    f.write(t.path());
    let d = ingest(&st, &src, VERSION, None).unwrap();
    // steps 4 and 5, gen_metadata 5.
    assert_eq!((d.added, d.dropped), (0, 3));
    f.user("Try another way").reply("Done another way.");
    f.write(t.path());
    let n = ingest(&st, &src, VERSION, None).unwrap();
    assert_eq!((n.added, n.dropped), (3, 0));
    let s = st.session(&r.session).unwrap();
    let nodes = st.session_nodes(&s).unwrap();
    // Every node seen before is still in the chain.
    for h in &before {
        assert!(nodes.iter().any(|n| &n.hash == h));
    }
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.meta.key.as_deref() == Some("steps:5"))
            .count(),
        3
    );
    assert_eq!(replay(&nodes).unwrap(), db::read(&src).unwrap().rows);
    // The old branch is still there: its rows replay from its node.
    let old = st
        .chain(Some(
            &nodes
                .iter()
                .rev()
                .find(|n| n.meta.container.as_deref() == Some(DROP))
                .unwrap()
                .meta
                .parent
                .clone()
                .unwrap(),
        ))
        .unwrap();
    let rows = replay(&old).unwrap();
    assert!(steps(&rows).any(|r| r
        .get("step_payload")
        .and_then(row::Cell::bytes)
        .is_some_and(|b| b.windows(6).any(|w| w == b"Fixed."))));
}

#[test]
fn a_fork_at_a_step_projects_as_a_native_fork() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let home = t.path().join("home");
    let src = Fixture::two_turns().write(&home);
    let r = ingest(&st, &src, VERSION, None).unwrap();
    let at = node_by_key(&st, &r.session, "steps:2");
    let fork = st.fork(&r.session, &at, None).unwrap();
    let p = project(&st, &fork.id, &home).unwrap();
    assert!(p.forked);
    assert_ne!(p.native_id, ID);
    crate::horse::claude::check_session_id(&p.native_id).unwrap();
    let a = db::read(&src).unwrap();
    let b = db::read(&p.path).unwrap();
    assert_eq!(b.id, p.native_id);
    // Contiguous steps 0..=2, byte for byte the source rows, with their
    // metadata rows and none after the fork point.
    let want: db::Rows = a
        .rows
        .iter()
        .filter(|((i, _), _)| *i <= 2)
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    assert_eq!(b.rows, want);
    // The trajectory and its metadata name the new conversation.
    let tm = &db::header_rows(&b.header, "trajectory_meta")[0];
    let old_tm = &db::header_rows(&a.header, "trajectory_meta")[0];
    assert_eq!(
        tm.get("cascade_id").and_then(row::Cell::text),
        Some(p.native_id.as_str())
    );
    assert_ne!(tm.get("trajectory_id"), old_tm.get("trajectory_id"));
    assert_eq!(tm.get("source"), old_tm.get("source"));
    let blob = db::header_rows(&b.header, "trajectory_metadata_blob")[0]
        .get("data")
        .and_then(row::Cell::bytes)
        .unwrap()
        .to_vec();
    assert_eq!(
        proto::bytes(&blob, 6).unwrap(),
        Some(p.native_id.as_bytes())
    );
    assert_eq!(
        proto::bytes(&blob, 18).unwrap(),
        Some(&b"default-cli-project"[..])
    );
    // Its own summaries row, next to the parent's.
    let s = b.summary.unwrap();
    let int = |c: &str| s.get(c).and_then(row::Cell::int);
    let text = |c: &str| s.get(c).and_then(row::Cell::text).map(String::from);
    assert_eq!(int("step_count"), Some(3));
    assert_eq!(int("last_user_input_step_index"), Some(0));
    assert_eq!(text("parent_conversation_id").as_deref(), Some(""));
    assert_eq!(text("title").as_deref(), Some("Fix the warning"));
    assert_eq!(
        text("workspace_uris").as_deref(),
        Some("[\"file:///home/demo/project\"]")
    );
    assert!(a.summary.is_some());
    // The model sees the same steps up to the fork point.
    assert_eq!(request::body(&a.rows, 3), request::body(&b.rows, i64::MAX));
    // The fork is bound to its new id; reading it back adds nothing.
    let f = st.session(&fork.id).unwrap();
    assert_eq!(f.binding.native_id.as_deref(), Some(p.native_id.as_str()));
    let back = ingest(&st, &p.path, VERSION, None).unwrap();
    assert_eq!((back.session, back.added, back.dropped), (fork.id, 0, 0));
}

#[test]
fn a_fork_inside_a_turn_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let mut f = Fixture::two_turns();
    f.running();
    let src = f.write(t.path());
    let r = ingest(&st, &src, VERSION, None).unwrap();
    for (key, why) in [("steps:4", "user message"), ("steps:5", "not finished")] {
        let at = node_by_key(&st, &r.session, key);
        let fork = st.fork(&r.session, &at, None).unwrap();
        let e = project(&st, &fork.id, &t.path().join("out")).unwrap_err();
        assert!(format!("{e:#}").contains(why), "{e:#}");
    }
    assert!(
        !t.path().join("out/conversations").exists()
            || std::fs::read_dir(t.path().join("out/conversations"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[test]
fn another_version_or_schema_refuses_ingest_and_projection() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let src = Fixture::two_turns().write(&t.path().join("a"));
    let e = ingest(&st, &src, "1.4.11", None).unwrap_err();
    assert!(e.to_string().contains("1.4.12"), "{e}");

    let mut f = Fixture::two_turns();
    f.user_version = 3;
    let p = f.write(&t.path().join("b"));
    assert!(format!("{:#}", ingest(&st, &p, VERSION, None).unwrap_err()).contains("user_version"));

    for sql in [
        "ALTER TABLE steps ADD COLUMN extra blob",
        "CREATE TRIGGER t AFTER INSERT ON steps BEGIN SELECT 1; END",
        "CREATE TABLE more (x)",
        "DROP INDEX idx_steps_status",
    ] {
        let p = Fixture::two_turns().write(&t.path().join("c"));
        rusqlite::Connection::open(&p)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
        assert!(ingest(&st, &p, VERSION, None).is_err(), "{sql}");
    }
    // A payload whose type disagrees with its row refuses too.
    let p = Fixture::two_turns().write(&t.path().join("d"));
    rusqlite::Connection::open(&p)
        .unwrap()
        .execute_batch("UPDATE steps SET step_type = 15 WHERE idx = 0")
        .unwrap();
    assert!(ingest(&st, &p, VERSION, None).is_err());

    let r = ingest(&st, &src, VERSION, None).unwrap();
    st.update_session(&r.session, |s| s.binding.version = "1.4.11".into())
        .unwrap();
    let e = project(&st, &r.session, &t.path().join("out")).unwrap_err();
    assert!(e.to_string().contains("1.4.12"), "{e}");
    assert!(!t.path().join("out").exists());
}

#[test]
fn the_request_steps_are_the_model_facing_steps() {
    let t = tempfile::tempdir().unwrap();
    let src = Fixture::two_turns().write(t.path());
    let rows = db::read(&src).unwrap().rows;
    let model: Vec<i64> = request::model_steps(&rows, i64::MAX)
        .iter()
        .map(|r| r.idx().unwrap())
        .collect();
    // Step 3 is an ephemeral step that agy keeps out of the request.
    assert_eq!(model, [0, 1, 2, 4, 5]);
    let body = request::body(&rows, i64::MAX);
    // One changed byte in a payload changes the body.
    let mut other = rows.clone();
    let r = other.get_mut(&(1, db::STEPS)).unwrap();
    let Some(row::Cell::Blob(b)) = r.get("step_payload").cloned() else {
        panic!()
    };
    let mut b = b;
    *b.last_mut().unwrap() ^= 1;
    r.set("step_payload", row::Cell::Blob(b));
    assert_ne!(request::body(&other, i64::MAX), body);
}

#[test]
fn the_guard_treats_a_fork_as_cold_and_says_the_cache_scope_is_unknown() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let src = Fixture::two_turns().write(t.path());
    let r = ingest(&st, &src, VERSION, None).unwrap();
    let at = node_by_key(&st, &r.session, "steps:2");
    let v = guard::check(&st, &r.session, &Op::Fork { at }, 0).unwrap();
    assert!(!v.keeps);
    assert!(v.lost_tokens > 0);
    let m = v.message();
    assert!(m.contains("unknown"), "{m}");
    assert!(m.contains("cold"), "{m}");
    let v = guard::check(&st, &r.session, &Op::Append { after: None }, 0).unwrap();
    assert!(v.keeps);
}
