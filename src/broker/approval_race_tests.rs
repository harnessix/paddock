use super::*;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc;

fn card(kind: &str) -> Pending {
    Pending {
        id: "r-synthetic".into(),
        capability: match kind {
            "text" => ASK_CAPABILITY,
            "choice" => TIMEOUT_CAPABILITY,
            "connect" | "connect-text" | "connect-plain" => CONNECT_CAPABILITY,
            _ => "exec",
        }
        .into(),
        choices: if kind == "choice" {
            vec!["30m".into()]
        } else {
            vec![]
        },
        session_allowed: true,
        connect: (kind.starts_with("connect")).then(Connect::default),
        ..Default::default()
    }
}

fn publish(env: &HostEnv, kind: &str) -> Result<()> {
    match kind {
        "text" => answer_reply(env, "r-synthetic", "synthetic reply"),
        "choice" => answer_reply(env, "r-synthetic", "30m"),
        "connect" => answer_connect_cli(env, "r-synthetic", ConnectAnswer::DenyAlways),
        "connect-text" => answer_reply(env, "r-synthetic", "allow-site"),
        "connect-plain" => answer_with_note_cli(env, "r-synthetic", Answer::Once, None),
        _ => answer_with_note(env, "r-synthetic", Answer::Deny, Some("synthetic note")),
    }
}

#[path = "approval_race_tests/publication.rs"]
mod publication;

#[path = "approval_race_tests/bounded.rs"]
mod bounded;

#[path = "approval_race_tests/retention.rs"]
mod retention;

#[test]
fn checked_listing_reports_a_dangling_root_and_unknown_owner() {
    store::PID_LIVENESS_HOOK
        .with(|h| *h.borrow_mut() = Some(Box::new(|pid| Some(pid != i32::MAX as u32))));
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    assert!(pending_checked(&env).unwrap().is_empty());
    let dir = approvals_dir(&env);
    crate::privfs::dir(dir.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("missing-approval-root", &dir).unwrap();
    assert_eq!(
        pending_checked(&env).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(std::fs::symlink_metadata(&dir).is_ok());
    std::fs::remove_file(&dir).unwrap();
    crate::privfs::dir(&dir).unwrap();
    let p = card("host");
    let path = dir.join(format!("{}.json", p.id));
    let owner = dir.join(format!("{}.owner", p.id));
    let answer = dir.join(format!("{}.answer", p.id));
    crate::privfs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
    crate::privfs::write(&answer, "unconfirmed answer").unwrap();
    for unknown in ["malformed".to_string(), std::process::id().to_string()] {
        crate::privfs::write(&owner, unknown).unwrap();
        let error = pending_checked(&env).unwrap_err();
        assert!(error.to_string().contains("liveness is unknown"));
        assert!(path.is_file());
        assert!(answer.is_file());
        assert_eq!(pending(&env), vec![p.clone()]);
    }
    let mut stale_connect = card("connect");
    stale_connect.connect.as_mut().unwrap().pid = i32::MAX as u32;
    crate::privfs::write(&path, serde_json::to_vec(&stale_connect).unwrap()).unwrap();
    // A selected connect card is not actionable, but its files stay.
    assert!(pending(&env).is_empty());
    assert!(path.is_file());
    assert!(answer.is_file());
    // A PID which the kernel confirms absent is still reaped, but its
    // accepted answer remains available for recovery.
    crate::privfs::write(&owner, i32::MAX.to_string()).unwrap();
    assert!(pending_checked(&env).unwrap().is_empty());
    assert!(!path.exists());
    assert!(answer.is_file());
}

#[test]
fn dangling_answer_is_not_reaped_as_an_absent_answer() {
    store::PID_LIVENESS_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(|_| Some(false))));
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    let p = card("host");
    let path = dir.join(format!("{}.json", p.id));
    let answer = dir.join(format!("{}.answer", p.id));
    crate::privfs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
    crate::privfs::write(&dir.join(format!("{}.owner", p.id)), i32::MAX.to_string()).unwrap();
    std::os::unix::fs::symlink("missing-answer", &answer).unwrap();
    assert!(pending_checked(&env).is_err());
    assert!(path.is_file());
    assert!(std::fs::symlink_metadata(answer).is_ok());
}

#[test]
fn transient_store_contention_can_retry_while_the_owner_is_waiting() {
    use std::cell::RefCell;
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("host");
    write_pending(&env, &p).unwrap();
    let dir = approvals_dir(&env);
    let held = RefCell::new(Some(lock_approvals(&dir, &p.id).unwrap()));
    let result = close_owned(&env, &p.id, &|| {
        drop(held.borrow_mut().take());
        true
    });
    assert!(matches!(result, Ok(None)));
    assert!(!dir.join("r-synthetic.json").exists());
}

#[test]
fn persistent_unanswered_unlink_failure_returns_unknown_and_recovers() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    crate::privfs::write(&approval_ui_file(&env), state::now().to_string()).unwrap();
    REMOVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|path| {
            if path.extension().is_some_and(|ext| ext == "json") {
                Err(io::Error::other("persistent unlink failure"))
            } else {
                Ok(())
            }
        }))
    });
    assert!(matches!(
        ask_choice(&env, &card("choice"), Duration::ZERO, &|| false),
        crate::spawn::Choice::Unknown
    ));
    let dir = approvals_dir(&env);
    assert!(dir.join("r-synthetic.json").exists());
    assert!(answer_reply(&env, "r-synthetic", "30m").is_err());
    let _ = pending(&env);
    assert!(dir.join("r-synthetic.json").exists());
    REMOVE_HOOK.with(|hook| *hook.borrow_mut() = None);
    let _ = pending(&env);
    assert!(!dir.join("r-synthetic.json").exists());
    assert!(!dir.join("r-synthetic.owner").exists());
}

#[test]
fn an_in_flight_writer_after_an_unknown_close_is_preserved_for_recovery() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("host");
    write_pending(&env, &p).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let writer_env = env.clone();
    let writer = std::thread::spawn(move || {
        ANSWER_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }))
        });
        answer(&writer_env, "r-synthetic", Answer::Once)
    });
    ready_rx.recv().unwrap();
    assert!(close_owned(&env, &p.id, &|| false).is_err());
    go_tx.send(()).unwrap();
    writer.join().unwrap().unwrap();
    let _ = pending(&env);
    let dir = approvals_dir(&env);
    assert!(!dir.join("r-synthetic.json").exists());
    assert!(dir.join("r-synthetic.answer").exists());
    assert!(!dir.join("r-synthetic.done").exists());
}

#[test]
fn legacy_live_pid_is_not_proof_of_owner_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("text");
    write_pending(&env, &p).unwrap();
    let dir = approvals_dir(&env);
    std::fs::write(
        dir.join("r-synthetic.owner"),
        std::process::id().to_string(),
    )
    .unwrap();
    assert!(answer_reply(&env, &p.id, "must not be accepted").is_err());
    store::abandon_owner(&env, &p.id);
}

#[test]
fn orphaned_owner_is_removed_but_an_ownerless_answer_is_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    std::fs::write(dir.join("orphan.owner"), "lease-v1").unwrap();
    std::fs::write(dir.join("unidentified.answer"), "accepted").unwrap();
    let _ = pending(&env);
    assert!(!dir.join("orphan.owner").exists());
    assert!(dir.join("unidentified.answer").exists());
}

#[test]
fn owner_exit_child() {
    let Some(root) = std::env::var_os("PADDOCK_APPROVAL_EXIT_PROBE") else {
        return;
    };
    let env = crate::privfs::tests::env_in(Path::new(&root));
    let p = card("host");
    write_pending(&env, &p).unwrap();
    answer(&env, &p.id, Answer::Deny).unwrap();
    if std::env::var_os("PADDOCK_APPROVAL_CONSUMED_PROBE").is_some() {
        REMOVE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|path| {
                if path.extension().is_some_and(|ext| ext == "json") {
                    Err(io::Error::other("persistent unlink failure"))
                } else {
                    Ok(())
                }
            }))
        });
        assert!(close_owned(&env, &p.id, &|| false).unwrap().is_some());
    }
    // Simulate abrupt exit without running any Rust destructors.
    std::process::exit(0);
}

#[test]
fn process_exit_recovers_confirmed_cleanup_and_preserves_unknown_answers() {
    for consumed in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "broker::approval::race_tests::owner_exit_child"])
            .env("PADDOCK_APPROVAL_EXIT_PROBE", tmp.path());
        if consumed {
            child.env("PADDOCK_APPROVAL_CONSUMED_PROBE", "1");
        }
        assert!(child.status().unwrap().success());
        let env = crate::privfs::tests::env_in(tmp.path());
        let dir = approvals_dir(&env);
        assert!(dir.join("r-synthetic.answer").exists());
        let _ = pending(&env);
        assert!(!dir.join("r-synthetic.json").exists());
        assert!(dir.join("r-synthetic.answer").exists());
        assert!(dir.join("r-synthetic.owner").exists());
        assert_eq!(dir.join("r-synthetic.done").exists(), consumed);
        assert!(answer(&env, "r-synthetic", Answer::Once).is_err());
    }
}

#[test]
fn connect_rewrite_keeps_the_old_card_whole_until_atomic_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut old = card("connect");
    old.detail = "old".into();
    write_pending(&env, &old).unwrap();
    let mut new = old.clone();
    new.detail = "new".into();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let env2 = env.clone();
    let new2 = new.clone();
    let writer = std::thread::spawn(move || {
        crate::privfs::ATOMIC_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }));
        });
        update_pending(&env2, &new2)
    });
    ready_rx.recv().unwrap();
    let path = approvals_dir(&env).join("r-synthetic.json");
    let before: Pending = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(before, old);
    let probe = std::fs::OpenOptions::new()
        .write(true)
        .open(approvals_dir(&env).join(".lock"))
        .unwrap();
    assert_ne!(
        unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    go_tx.send(()).unwrap();
    writer.join().unwrap().unwrap();
    let after: Pending = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after, new);
    assert_eq!(before.id, after.id);
    close_pending(&env, &new.id).unwrap();
    assert!(update_pending(&env, &old).is_err());
    assert!(!path.exists());
}

#[test]
fn lock_child() {
    let Some(dir) = std::env::var_os("PADDOCK_APPROVAL_LOCK_PROBE") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join(".lock"))
        .unwrap();
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_ne!(rc, 0);
    assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
    std::fs::write(dir.join("probe-ok"), b"ok").unwrap();
}

#[test]
fn a_separate_process_observes_the_same_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    let _lock = lock_approvals(&dir, "r-synthetic").unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "broker::approval::race_tests::lock_child"])
        .env("PADDOCK_APPROVAL_LOCK_PROBE", &dir)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        dir.join("probe-ok").is_file(),
        "the child probe must actually run"
    );
}

#[test]
fn incomplete_listing_keeps_healthy_cards_and_unknown_sidecars() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut healthy = card("host");
    healthy.id = "r-healthy".into();
    write_pending(&env, &healthy).unwrap();
    let dir = approvals_dir(&env);
    std::fs::write(dir.join("r-orphan.owner"), "unknown owner").unwrap();
    let mut unknown = card("connect");
    unknown.id = "r-unknown".into();
    unknown.connect.as_mut().unwrap().pid = std::process::id();
    std::fs::write(
        dir.join("r-unknown.json"),
        serde_json::to_vec(&unknown).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("r-unknown.owner"), "unknown owner").unwrap();
    let report = pending_report(&env).unwrap();
    assert_eq!(report.pending, vec![healthy]);
    assert!(report
        .diagnostics
        .iter()
        .any(|s| s.contains("r-orphan") && s.contains("unknown")));
    assert!(report
        .diagnostics
        .iter()
        .any(|s| s.contains("r-unknown") && s.contains("unknown")));
    assert!(answer_connect(&env, "r-unknown", ConnectAnswer::AllowOnce).is_err());
    assert!(dir.join("r-unknown.json").is_file());
    assert!(dir.join("r-unknown.owner").is_file());
    assert!(dir.join("r-orphan.owner").is_file());
}

#[test]
fn listing_identifies_dangling_ancestors_and_invalid_live_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let ancestor = env.paddock_run();
    std::fs::create_dir_all(ancestor.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("missing-runtime", &ancestor).unwrap();
    assert!(pending_checked(&env).is_err());
    assert!(pending_report(&env).is_err());
    std::fs::remove_file(&ancestor).unwrap();
    let mut p = card("host");
    p.id = "r-live".into();
    write_pending(&env, &p).unwrap();
    let answer = approvals_dir(&env).join("r-live.answer");
    std::os::unix::fs::symlink("missing-answer", &answer).unwrap();
    assert!(pending_checked(&env).is_err());
    let report = pending_report(&env).unwrap();
    assert!(report.pending.is_empty());
    assert!(report.diagnostics.iter().any(|s| s.contains("r-live")));
    assert!(std::fs::symlink_metadata(&answer).is_ok());
    std::fs::remove_file(&answer).unwrap();
    std::fs::write(&answer, "not a valid selection").unwrap();
    assert!(pending_checked(&env).is_err());
    assert!(pending_report(&env)
        .unwrap()
        .diagnostics
        .iter()
        .any(|s| s.contains("invalid answer")));
    assert!(answer.is_file());
}

#[test]
fn incomplete_listing_keeps_accepted_answers_and_reports_their_uncertain_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut p = card("host");
    p.id = "r-answered".into();
    write_pending(&env, &p).unwrap();
    answer(&env, &p.id, Answer::Deny).unwrap();
    let report = pending_report(&env).unwrap();
    assert!(report.pending.is_empty());
    assert!(report
        .diagnostics
        .iter()
        .any(|s| s.contains("r-answered") && s.contains("unconfirmed")));
    assert!(approvals_dir(&env).join("r-answered.answer").is_file());
}

#[test]
fn orphaned_read_marker_is_retained_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    let p = card("host");
    std::fs::write(
        dir.join("r-synthetic.json"),
        serde_json::to_vec(&p).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("r-synthetic.owner"), "lease-v1").unwrap();
    std::fs::write(dir.join("r-synthetic.done"), "read").unwrap();
    let report = pending_report(&env).unwrap();
    assert!(report.pending.is_empty());
    assert!(report
        .diagnostics
        .iter()
        .any(|s| s.contains("marker") && s.contains("unconfirmed")));
    assert!(dir.join("r-synthetic.json").is_file());
    assert!(dir.join("r-synthetic.owner").is_file());
    assert!(dir.join("r-synthetic.done").is_file());
}

#[test]
fn connect_application_record_keeps_selection_and_reports_write_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("connect");
    write_pending(&env, &p).unwrap();
    answer_connect_cli(&env, &p.id, ConnectAnswer::DenyAlways).unwrap();
    let selected = claim_connect(&env, &p.id, false).unwrap().unwrap();
    assert_eq!(
        ConnectAnswer::parse_file(&selected),
        Some(ConnectAnswer::DenyAlways)
    );
    let dir = approvals_dir(&env);
    assert!(dir.join("r-synthetic.json").is_file());
    assert!(dir.join("r-synthetic.answer").is_file());
    let report = pending_report(&env).unwrap();
    assert!(report.pending.is_empty());
    assert!(report.diagnostics.iter().any(|d| d.contains("unconfirmed")));
    let unresolved = report.unresolved.iter().find(|u| u.id == p.id).unwrap();
    assert_eq!(unresolved.selected.as_deref(), Some("deny-always"));
    assert_eq!(unresolved.request.as_ref().unwrap().id, p.id);

    assert!(record_connect_outcome(
        &env,
        &p.id,
        ConnectAnswer::AllowOnce,
        true,
        "once",
        "cli",
        false
    )
    .is_err());
    std::fs::create_dir(dir.join("r-synthetic.done")).unwrap();
    assert!(record_connect_outcome(
        &env,
        &p.id,
        ConnectAnswer::DenyAlways,
        false,
        "stale",
        "cli",
        false
    )
    .is_err());
    assert!(dir.join("r-synthetic.answer").is_file());
    assert!(pending_report(&env)
        .unwrap()
        .diagnostics
        .iter()
        .any(|d| d.contains("r-synthetic")));
    std::fs::remove_dir(dir.join("r-synthetic.done")).unwrap();
    record_connect_outcome(
        &env,
        &p.id,
        ConnectAnswer::DenyAlways,
        false,
        "stale",
        "cli",
        false,
    )
    .unwrap();
    let report = pending_report(&env).unwrap();
    assert!(report.pending.is_empty());
    assert!(report.diagnostics.iter().any(|d| d.contains("Failed")));
    assert_eq!(report.outcomes[0].state, LocalApplication::Failed);
    assert!(dir.join("r-synthetic.json").is_file());
    assert!(dir.join("r-synthetic.answer").is_file());
    let outcome: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("r-synthetic.done")).unwrap()).unwrap();
    assert_eq!(outcome["selected"], "deny-always");
    assert_eq!(outcome["rule"], "stale");
}

#[test]
fn malformed_connect_selection_is_retained_as_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("connect");
    write_pending(&env, &p).unwrap();
    let dir = approvals_dir(&env);
    std::fs::write(dir.join("r-synthetic.answer"), "malformed").unwrap();
    assert!(claim_connect(&env, &p.id, false).is_err());
    assert!(claim_connect(&env, &p.id, true).is_err());
    assert!(dir.join("r-synthetic.json").is_file());
    assert!(dir.join("r-synthetic.answer").is_file());
    assert!(pending_report(&env)
        .unwrap()
        .diagnostics
        .iter()
        .any(|d| d.contains("invalid answer")));
}

#[test]
fn legacy_connect_result_without_identity_snapshot_remains_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("connect");
    write_pending(&env, &p).unwrap();
    answer_connect_cli(&env, &p.id, ConnectAnswer::DenyAlways).unwrap();
    let dir = approvals_dir(&env);
    std::fs::write(dir.join("r-synthetic.done"), r#"{"kind":"connect-application-v1","selected":"deny-always","allowed":false,"rule":"rule-old","by":"cli"}"#).unwrap();
    let report = pending_report(&env).unwrap();
    assert_eq!(report.outcomes[0].state, LocalApplication::Unknown);
    assert_eq!(report.outcomes[0].local_allowed, None);
    assert!(report.unresolved.iter().any(|item| item.id == p.id));
    assert!(dir.join("r-synthetic.answer").is_file());
}

#[test]
fn marker_closure_requires_the_exact_validated_card_or_retirement() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    let mut p = card("connect");
    p.connect.as_mut().unwrap().pid = 71;
    p.connect.as_mut().unwrap().profile = "notes".into();
    let path = dir.join("r-synthetic.json");
    std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
    assert!(!connect_card_retired(&env, &p).unwrap());
    std::fs::write(dir.join("r-synthetic.answer"), "invalid").unwrap();
    assert!(connect_card_retired(&env, &p).is_err());
    std::fs::write(
        dir.join("r-synthetic.answer"),
        r#"{"reply":"deny","source":"cli"}"#,
    )
    .unwrap();
    assert!(connect_card_retired(&env, &p).unwrap());
    let mut newer = p.clone();
    newer.connect.as_mut().unwrap().host = "newer.example".into();
    assert!(!connect_card_retired(&env, &newer).unwrap());
    std::fs::remove_file(&path).unwrap();
    assert!(!connect_card_retired(&env, &p).unwrap());
    std::fs::remove_file(dir.join("r-synthetic.answer")).unwrap();
    assert!(connect_card_retired(&env, &p).unwrap());
}

#[test]
fn completed_retention_preserves_unresolved_and_replaced_identities() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    let now = state::now();
    for (id, word, at) in [
        ("r-old", "once", now.saturating_sub(20)),
        ("r-new", "once", now.saturating_sub(10)),
        ("r-replaced", "once", now.saturating_sub(25)),
        ("r-failed", "stale", now.saturating_sub(28)),
        ("r-owner", "once", now.saturating_sub(27)),
        ("r-invalid", "once", now.saturating_sub(26)),
        ("r-future", "once", now.saturating_add(30 * 24 * 60 * 60)),
    ] {
        let mut p = card("connect");
        p.id = id.into();
        write_pending(&env, &p).unwrap();
        answer_connect_cli(&env, id, ConnectAnswer::Deny).unwrap();
        record_connect_outcome(&env, id, ConnectAnswer::Deny, false, word, "cli", false).unwrap();
        let path = dir.join(format!("{id}.done"));
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["recorded_at"] = at.into();
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        if word == "once" {
            retire_connect_outcome(&env, id).unwrap();
        }
    }
    let mut selected = card("connect");
    selected.id = "r-unconfirmed".into();
    write_pending(&env, &selected).unwrap();
    answer_connect_cli(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let path = dir.join("r-replaced.json");
    let mut replacement: Pending = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    replacement.summary = "a newer identity".into();
    std::fs::write(&path, serde_json::to_vec(&replacement).unwrap()).unwrap();
    std::fs::write(dir.join("r-owner.owner"), "unknown owner").unwrap();
    let invalid = dir.join("r-invalid.done");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&invalid).unwrap()).unwrap();
    value["local_allowed"] = true.into();
    std::fs::write(&invalid, serde_json::to_vec(&value).unwrap()).unwrap();
    store::cleanup_completed(&dir, now, 1).unwrap();
    assert!(!dir.join("r-old.json").exists());
    assert!(dir.join("r-new.json").exists());
    assert!(path.exists());
    assert!(dir.join("r-failed.answer").exists());
    assert!(dir.join("r-owner.answer").exists());
    assert!(dir.join("r-invalid.answer").exists());
    assert!(dir.join("r-future.answer").exists());
    assert!(dir.join("r-unconfirmed.answer").exists());
    store::cleanup_completed(&dir, now + 7 * 24 * 60 * 60 + 1, 256).unwrap();
    assert!(
        !dir.join("r-new.json").exists(),
        "the age limit also prunes completed records"
    );
    assert!(path.exists(), "age cannot override an identity mismatch");
    let report = pending_report(&env).unwrap();
    assert!(report.unresolved.iter().any(|u| u.id == "r-replaced"));
    assert!(report.unresolved.iter().any(|u| u.id == "r-failed"));
    assert!(report.unresolved.iter().any(|u| u.id == "r-owner"));
    assert!(report.unresolved.iter().any(|u| u.id == "r-invalid"));
    assert!(report.unresolved.iter().any(|u| u.id == "r-future"));
    assert!(report.unresolved.iter().any(|u| u.id == "r-unconfirmed"));
}

#[test]
fn interrupted_completed_cleanup_keeps_evidence_and_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("connect");
    write_pending(&env, &p).unwrap();
    answer_connect_cli(&env, &p.id, ConnectAnswer::Deny).unwrap();
    record_connect_outcome(
        &env,
        &p.id,
        ConnectAnswer::Deny,
        false,
        "once",
        "cli",
        false,
    )
    .unwrap();
    let dir = approvals_dir(&env);
    retire_connect_outcome(&env, &p.id).unwrap();
    REMOVE_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(|path| {
            if path.extension().is_some_and(|ext| ext == "answer") {
                Err(io::Error::other("synthetic interruption"))
            } else {
                Ok(())
            }
        }))
    });
    assert!(store::cleanup_completed(&dir, state::now(), 0).is_err());
    REMOVE_HOOK.with(|h| *h.borrow_mut() = None);
    assert!(dir.join("r-synthetic.cleanup").is_file());
    assert!(dir.join("r-synthetic.answer").is_file());
    store::cleanup_completed(&dir, state::now(), 0).unwrap();
    assert!(dir.join("r-synthetic.cleanup").is_file());
    let report = pending_report(&env).unwrap();
    let retained = report.unresolved.iter().find(|u| u.id == p.id).unwrap();
    assert!(retained.reason.contains("cleanup interrupted"));
    assert_eq!(retained.selected.as_deref(), Some("deny"));
    assert_eq!(retained.request.as_ref().unwrap().id, p.id);
}
