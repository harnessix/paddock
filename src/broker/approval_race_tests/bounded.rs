//! Small malformed-evidence fixtures; none create a real approval session.
use super::*;
use std::os::unix::ffi::OsStrExt;

#[test]
fn report_retains_oversize_links_and_fifo_without_blocking_other_cards() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    store::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(true))));
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();

    let mut good = card("connect");
    good.id = "healthy".into();
    good.connect.as_mut().unwrap().pid = 123;
    std::fs::write(dir.join("healthy.json"), serde_json::to_vec(&good).unwrap()).unwrap();
    std::fs::write(dir.join("oversized.json"), vec![b'x'; 512 * 1024 + 1]).unwrap();
    std::os::unix::fs::symlink("healthy.json", dir.join("linked.json")).unwrap();
    let mut fifo_card = good.clone();
    fifo_card.id = "fifo".into();
    std::fs::write(
        dir.join("fifo.json"),
        serde_json::to_vec(&fifo_card).unwrap(),
    )
    .unwrap();
    let fifo = dir.join("fifo.answer");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

    let report = pending_report(&env).unwrap();
    assert!(report.pending.iter().any(|p| p.id == "healthy"));
    assert!(report.unresolved.iter().any(|p| p.id == "oversized"));
    assert!(report.unresolved.iter().any(|p| p.id == "linked"));
    assert!(report.unresolved.iter().any(|p| p.id == "fifo"));
    assert!(std::fs::symlink_metadata(&fifo).is_ok());
    store::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = None);
}

#[test]
fn replacement_after_preopen_is_rejected_inside_the_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    store::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(true))));
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    let mut pending = card("connect");
    pending.id = "race".into();
    pending.connect.as_mut().unwrap().pid = 123;
    std::fs::write(dir.join("race.json"), serde_json::to_vec(&pending).unwrap()).unwrap();
    std::fs::write(dir.join("race.answer"), r#"{"reply":"deny"}"#).unwrap();
    let replacement = dir.join("replacement-fifo");
    let name = std::ffi::CString::new(replacement.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let target = dir.join("race.answer");
    store::EVIDENCE_PREPARED_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::rename(replacement, target).unwrap()
        }));
    });
    let report = pending_report(&env).unwrap();
    assert!(report.unresolved.iter().any(|item| item.id == "race"));
    assert!(std::fs::symlink_metadata(dir.join("race.answer")).is_ok());
    store::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = None);
}

#[test]
fn a_card_rewritten_while_an_answer_waits_is_prepared_again() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    store::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(|_| Some(true))));
    let mut pending = card("connect");
    pending.id = "rewritten".into();
    pending.connect.as_mut().unwrap().pid = 123;
    std::fs::write(
        dir.join("rewritten.json"),
        serde_json::to_vec(&pending).unwrap(),
    )
    .unwrap();
    let path = dir.join("rewritten.json");
    let mut rewrite = pending.clone();
    rewrite.connect.as_mut().unwrap().count += 1;
    // A lock holder renames a new card into place while the answer waits.
    store::EVIDENCE_PREPARED_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            crate::privfs::write_atomic(&path, serde_json::to_vec(&rewrite).unwrap()).unwrap()
        }));
    });
    answer_connect(&env, "rewritten", ConnectAnswer::Deny).unwrap();
    let answer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("rewritten.answer")).unwrap()).unwrap();
    assert_eq!(
        answer["request"]["connect"]["count"],
        pending.connect.unwrap().count + 1
    );
    store::PID_LIVENESS_HOOK.with(|hook| *hook.borrow_mut() = None);
}
