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
