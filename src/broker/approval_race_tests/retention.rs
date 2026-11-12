use super::*;
use std::cell::Cell;
use std::rc::Rc;

fn completed(dir: &Path, id: &str, at: u64, retired: bool) -> Vec<u8> {
    let mut p = card("connect");
    p.id = id.into();
    p.connect.as_mut().unwrap().pid = 123;
    let answer = r#"{"reply":"deny","source":"cli"}"#;
    let done = serde_json::to_vec(&serde_json::json!({
        "kind": "connect-application-v2", "id": id, "request": &p,
        "selected": "deny", "local_allowed": false, "rule": "once", "by": "cli",
        "state": "decision_prepared", "scope": "connection", "reason": null,
        "recorded_at": at, "affected": [], "selection": answer
    }))
    .unwrap();
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec(&p).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join(format!("{id}.answer")), answer).unwrap();
    std::fs::write(dir.join(format!("{id}.done")), &done).unwrap();
    if retired {
        std::fs::write(dir.join(format!("{id}.retired")), &done).unwrap();
    }
    done
}

#[test]
fn cleanup_waits_for_local_result_retirement() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let done = completed(dir, "in-flight", 90, false);
    store::cleanup_completed(dir, 100, 0).unwrap();
    assert!(dir.join("in-flight.answer").is_file());
    assert!(dir.join("in-flight.done").is_file());
    std::fs::write(dir.join("in-flight.retired"), done).unwrap();
    store::cleanup_completed(dir, 100, 0).unwrap();
    assert!(!dir.join("in-flight.done").exists());
}

#[test]
fn retention_counts_only_currently_eligible_records() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    completed(dir, "old", 70, true);
    completed(dir, "new", 80, true);
    let interrupted = completed(dir, "interrupted", 90, true);
    completed(dir, "in-flight", 95, false);
    std::fs::write(dir.join("interrupted.cleanup"), interrupted).unwrap();
    store::cleanup_completed(dir, 100, 1).unwrap();
    assert!(!dir.join("old.done").exists());
    assert!(dir.join("new.done").is_file());
    assert!(dir.join("interrupted.cleanup").is_file());
    assert!(dir.join("in-flight.answer").is_file());
}

#[test]
fn changed_retirement_evidence_cannot_authorize_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    completed(dir, "changed", 80, true);
    std::fs::write(dir.join("changed.retired"), "different").unwrap();
    store::cleanup_completed(dir, 100, 0).unwrap();
    assert!(dir.join("changed.answer").is_file());
    assert!(dir.join("changed.done").is_file());
}

#[test]
fn retention_discovery_runs_without_the_global_store_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    completed(&dir, "eligible", 80, true);
    completed(&dir, "unresolved-a", 90, false);
    completed(&dir, "unresolved-b", 91, false);
    let seen = Rc::new(Cell::new(0));
    let count = seen.clone();
    let lock_path = dir.join(".lock");
    store::RETENTION_SCAN_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&lock_path)
                .unwrap();
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            count.set(count.get() + 1);
        }))
    });
    store::cleanup_completed(&dir, 100, 1).unwrap();
    store::RETENTION_SCAN_HOOK.with(|hook| *hook.borrow_mut() = None);
    assert!(seen.get() >= 3);
    assert!(dir.join("unresolved-a.done").is_file());
    assert!(dir.join("unresolved-b.done").is_file());
}
