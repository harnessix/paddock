use super::*;
use std::os::unix::io::AsRawFd;
const ID: &str = "01934b20-1111-7777-8888-123456789abc";
const META: &str = r#"{"timestamp":"2026-11-04T00:00:00Z","type":"session_meta","payload":{"id":"01934b20-1111-7777-8888-123456789abc","session_id":"01934b20-1111-7777-8888-123456789abc","cli_version":"0.166.0","cwd":"/home/demo/project"}}"#;
const REASON: &str = r#"{"timestamp":"2026-11-04T00:00:01Z","type":"response_item","payload":{"type":"reasoning","id":"rs_1","encrypted_content":"x\u00e9\/abc=="}}"#;
fn setup(t: &Path) -> (Store, std::path::PathBuf) {
    let p = t.join(format!(
        "input/sessions/2026/11/04/rollout-2026-11-04T00-00-00-{ID}.jsonl"
    ));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, format!("{META}\n{REASON}\n")).unwrap();
    (Store::open(&t.join("store")).unwrap(), p)
}
#[test]
fn round_trip_torn_and_versions() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    assert!(ingest(&st, &p, "0.1", None).is_err());
    let r = ingest(&st, &p, VERSION, None).unwrap();
    let out = project(&st, &r.session, &t.path().join("out"), false).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(out.path).unwrap());
    std::fs::write(&p, format!("{META}\n{REASON}\n{{\"type\":")).unwrap();
    assert!(ingest(&st, &p, VERSION, None).unwrap().torn);
    st.update_session(&r.session, |s| s.binding.version = "1".into())
        .unwrap();
    assert!(project(&st, &r.session, &t.path().join("out2"), false).is_err());
    std::fs::write(&p, format!("{}\n", META.replace(VERSION, "1"))).unwrap();
    assert!(ingest(&st, &p, VERSION, None).is_err());
}
#[test]
fn forks_and_swap() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let r = ingest(&st, &p, VERSION, None).unwrap();
    let s = st.session(&r.session).unwrap();
    let fork = st.fork(&s.id, s.head.as_ref().unwrap(), None).unwrap();
    let cold = project(&st, &fork.id, &t.path().join("cold"), false).unwrap();
    let b = std::fs::read_to_string(cold.path).unwrap();
    assert!(b.contains(REASON));
    assert!(b.contains(&format!("\"forked_from_id\":\"{ID}\"")));
    assert_ne!(cold.native_id, ID);
    let f = st.fork(&s.id, s.head.as_ref().unwrap(), None).unwrap();
    let cfg = t.path().join("input");
    let locks = cfg.join("thread-writer-locks");
    std::fs::create_dir_all(&locks).unwrap();
    let lock = std::fs::File::create(locks.join(format!("{ID}.lock"))).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(project(&st, &f.id, &cfg, true).is_err());
    // Unlock before the close: a process that another test forks in this
    // moment shares the locked file until its exec, and LOCK_UN frees the
    // lock for every copy.
    // SAFETY: flock on a descriptor that this test owns.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    drop(lock);
    let swapped = project(&st, &f.id, &cfg, true).unwrap();
    assert_eq!(swapped.native_id, ID);
    assert_eq!(
        std::fs::read(swapped.backup.unwrap()).unwrap(),
        std::fs::read(&p).unwrap()
    );
    assert_eq!(st.session(&s.id).unwrap().head, s.head);
    assert_eq!(ingest(&st, &p, VERSION, None).unwrap().session, f.id);
}

#[test]
fn unknown_outer_row_is_preserved_but_fork_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let unknown = r#"{"timestamp":"2026-11-04T00:00:02Z","type":"future_record","payload":{}}"#;
    std::fs::write(&p, format!("{META}\n{REASON}\n{unknown}\n")).unwrap();
    let result = ingest(&st, &p, VERSION, None).unwrap();
    let out = project(&st, &result.session, &t.path().join("out"), false).unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(out.path).unwrap());
    let session = st.session(&result.session).unwrap();
    let fork = st
        .fork(&session.id, session.head.as_ref().unwrap(), None)
        .unwrap();
    let error = project(&st, &fork.id, &t.path().join("fork"), false).unwrap_err();
    assert!(error
        .to_string()
        .contains("line 3 has unknown type future_record"));
}

#[test]
fn swap_refuses_unknown_outer_row_before_writing() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let unknown = r#"{"timestamp":"2026-11-04T00:00:02Z","type":"future_record","payload":{}}"#;
    std::fs::write(&p, format!("{META}\n{REASON}\n{unknown}\n")).unwrap();
    let result = ingest(&st, &p, VERSION, None).unwrap();
    let session = st.session(&result.session).unwrap();
    let target = session.head.unwrap();
    let first = st
        .session_nodes(&st.session(&result.session).unwrap())
        .unwrap()[0]
        .hash
        .clone();
    st.update_session(&result.session, |s| s.head = Some(first))
        .unwrap();
    let before = std::fs::read(&p).unwrap();
    let error =
        swap::swap_file(&st, &result.session, &target, &t.path().join("input")).unwrap_err();
    assert!(error
        .to_string()
        .contains("line 3 has unknown type future_record"));
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

#[test]
fn swap_refuses_unknown_outer_row_in_current_history() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let unknown = r#"{"timestamp":"2026-11-04T00:00:02Z","type":"future_record","payload":{}}"#;
    std::fs::write(&p, format!("{META}\n{REASON}\n{unknown}\n")).unwrap();
    let result = ingest(&st, &p, VERSION, None).unwrap();
    let session = st.session(&result.session).unwrap();
    let target = st.session_nodes(&session).unwrap()[0].hash.clone();
    let before = std::fs::read(&p).unwrap();
    let error =
        swap::swap_file(&st, &result.session, &target, &t.path().join("input")).unwrap_err();
    assert!(error
        .to_string()
        .contains("line 3 has unknown type future_record"));
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

#[test]
fn nested_unknown_content_blocks_fork_and_swap() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let unknown = r#"{"timestamp":"2026-11-04T00:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"future_content"}]}}"#;
    std::fs::write(&p, format!("{META}\n{REASON}\n{unknown}\n")).unwrap();
    let result = ingest(&st, &p, VERSION, None).unwrap();
    let session = st.session(&result.session).unwrap();
    let fork = st
        .fork(&session.id, session.head.as_ref().unwrap(), None)
        .unwrap();
    let error = project(&st, &fork.id, &t.path().join("fork"), false).unwrap_err();
    assert!(format!("{error:#}").contains("line 3"));
    let target = st.session_nodes(&session).unwrap()[0].hash.clone();
    let before = std::fs::read(&p).unwrap();
    let error = swap::swap_file(&st, &session.id, &target, &t.path().join("input")).unwrap_err();
    assert!(format!("{error:#}").contains("line 3"));
    assert_eq!(std::fs::read(&p).unwrap(), before);
}

#[test]
fn swap_refuses_a_new_uningested_record_on_disk() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let result = ingest(&st, &p, VERSION, None).unwrap();
    let session = st.session(&result.session).unwrap();
    let first = st.session_nodes(&session).unwrap()[0].hash.clone();
    let mut file = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    use std::io::Write as _;
    file.write_all(b"{\"timestamp\":\"t\",\"type\":\"future_record\",\"payload\":{}}\n")
        .unwrap();
    let before = std::fs::read(&p).unwrap();
    let error = swap::swap_file(&st, &session.id, &first, &t.path().join("input")).unwrap_err();
    assert!(format!("{error:#}").contains("line 3"));
    assert_eq!(std::fs::read(&p).unwrap(), before);
    assert_eq!(st.session(&session.id).unwrap().head, session.head);
}

#[test]
fn fork_projection_preserves_reader_provenance() {
    let t = tempfile::tempdir().unwrap();
    let (st, p) = setup(t.path());
    let result = ingest(&st, &p, "0.167.0", None).unwrap();
    let session = st.session(&result.session).unwrap();
    let fork = st
        .fork(&session.id, session.head.as_ref().unwrap(), None)
        .unwrap();
    project(&st, &fork.id, &t.path().join("fork"), false).unwrap();
    let fork = st.session(&fork.id).unwrap();
    assert_eq!(fork.binding.version, "0.166.0");
    assert_eq!(fork.codex_reader_version.as_deref(), Some("0.167.0"));
    assert!(st
        .session_nodes(&fork)
        .unwrap()
        .iter()
        .all(|n| n.meta.version.as_deref() == Some("0.167.0")));
}
