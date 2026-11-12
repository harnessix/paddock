use super::*;

#[test]
fn publication_is_locked_through_write_and_close_observes_the_first_answer() {
    for kind in [
        "host",
        "text",
        "choice",
        "connect",
        "connect-text",
        "connect-plain",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let env = crate::privfs::tests::env_in(tmp.path());
        let p = card(kind);
        write_pending(&env, &p).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel();
        let env2 = env.clone();
        let writer = std::thread::spawn(move || {
            ANSWER_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    ready_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                }));
            });
            publish(&env2, kind)
        });
        ready_rx.recv().unwrap(); // validation completed; publication is paused
        let dir = approvals_dir(&env);
        let probe = std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join(".lock"))
            .unwrap();
        // A separate open must not acquire the lock during the hook.
        let rc = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_ne!(rc, 0);
        assert_eq!(io::Error::last_os_error().kind(), io::ErrorKind::WouldBlock);
        let (started_tx, started_rx) = mpsc::channel();
        let env3 = env.clone();
        let closer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            close_pending(&env3, "r-synthetic")
        });
        started_rx.recv().unwrap();
        go_tx.send(()).unwrap();
        writer.join().unwrap().unwrap();
        let text = closer.join().unwrap().unwrap().unwrap();
        match kind {
            "host" => assert_eq!(
                AnswerFile::parse(&text),
                Some((Answer::Deny, Some("synthetic note".into())))
            ),
            "text" => assert!(text.contains("synthetic reply")),
            "choice" => assert!(text.contains("30m")),
            "connect" => {
                assert!(text.contains("deny-always"));
                assert!(text.contains("cli"));
            }
            "connect-text" => {
                assert!(text.contains("allow-site"));
                assert!(text.contains("panel"));
            }
            "connect-plain" => {
                assert!(text.contains("allow-once"));
                assert!(text.contains("cli"));
            }
            _ => unreachable!(),
        }
        assert!(publish(&env, kind).is_err());
        assert!(!dir.join("r-synthetic.answer").exists());
        assert!(
            update_pending(&env, &p).is_err(),
            "a late rewrite must not recreate the card"
        );

        let tmp2 = tempfile::tempdir().unwrap();
        let env4 = crate::privfs::tests::env_in(tmp2.path());
        write_pending(&env4, &p).unwrap();
        assert_eq!(close_pending(&env4, &p.id).unwrap(), None);
        assert!(
            publish(&env4, kind).is_err(),
            "closure won before validation"
        );
    }
}

#[test]
fn first_answer_and_close_failures_are_recoverable() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("host");
    write_pending(&env, &p).unwrap();
    answer_with_note(&env, &p.id, Answer::Deny, Some("first")).unwrap();
    assert!(answer(&env, &p.id, Answer::Once).is_err());
    let dir = approvals_dir(&env);
    REMOVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|path| {
            if path.extension().is_some_and(|ext| ext == "json") {
                Err(io::Error::other("injected card unlink failure"))
            } else {
                Ok(())
            }
        }))
    });
    assert!(close_pending(&env, &p.id).is_err());
    assert!(dir.join("r-synthetic.json").is_file());
    assert!(dir.join("r-synthetic.answer").is_file());
    REMOVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|path| {
            if path.extension().is_some_and(|ext| ext == "answer") {
                Err(io::Error::other("injected answer unlink failure"))
            } else {
                Ok(())
            }
        }))
    });
    assert!(close_pending(&env, &p.id).is_err());
    assert!(!dir.join("r-synthetic.json").exists());
    assert!(dir.join("r-synthetic.answer").is_file());
    assert!(
        write_pending(&env, &p).is_err(),
        "an unremoved answer reserves the id"
    );
    REMOVE_HOOK.with(|hook| *hook.borrow_mut() = None);
    let text = close_pending(&env, &p.id).unwrap().unwrap();
    assert_eq!(
        AnswerFile::parse(&text),
        Some((Answer::Deny, Some("first".into())))
    );
    assert!(dir.join(".lock").is_file());
}

#[test]
fn a_sidecar_unlink_failure_after_answer_removal_still_returns_the_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let p = card("text");
    write_pending(&env, &p).unwrap();
    answer_reply(&env, &p.id, "preserved").unwrap();
    REMOVE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|path| {
            if path.extension().is_some_and(|ext| ext == "owner") {
                Err(io::Error::other("persistent owner unlink failure"))
            } else {
                Ok(())
            }
        }))
    });
    assert!(take_answer(&env, &p.id)
        .unwrap()
        .unwrap()
        .contains("preserved"));
    let dir = approvals_dir(&env);
    assert!(!dir.join("r-synthetic.answer").exists());
    assert!(dir.join("r-synthetic.owner").exists());
    REMOVE_HOOK.with(|hook| *hook.borrow_mut() = None);
    let _ = pending(&env);
    assert!(!dir.join("r-synthetic.owner").exists());
}

#[test]
fn real_owners_return_the_published_decision_when_either_unlink_fails() {
    use std::cell::Cell;
    use std::rc::Rc;
    for kind in ["host", "choice"] {
        for extension in ["json", "answer"] {
            let tmp = tempfile::tempdir().unwrap();
            let env = crate::privfs::tests::env_in(tmp.path());
            crate::privfs::write(&approval_ui_file(&env), state::now().to_string()).unwrap();
            let p = card(kind);
            let env2 = env.clone();
            OWNER_PUBLISHED_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || publish(&env2, kind).unwrap()));
            });
            let injected = Rc::new(Cell::new(false));
            let observed = injected.clone();
            REMOVE_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |path| {
                    if path.extension().is_some_and(|x| x == extension) {
                        observed.set(true);
                        Err(io::Error::other("synthetic unlink failure"))
                    } else {
                        Ok(())
                    }
                }));
            });
            if kind == "host" {
                let result = HostApprover.ask_verdict(&env, &p, Duration::ZERO).unwrap();
                assert_eq!(result.answer, Answer::Deny);
                assert_eq!(result.note.as_deref(), Some("synthetic note"));
            } else {
                assert!(matches!(ask_choice(&env, &p, Duration::ZERO, &|| false),
                        crate::spawn::Choice::More(ref s) if s == "30m"));
            }
            assert!(injected.get());
            OWNER_PUBLISHED_HOOK.with(|hook| *hook.borrow_mut() = None);
            let dir = approvals_dir(&env);
            // The same persistent failure also affects the recovery scan.
            let _ = pending(&env);
            assert!(dir.join("r-synthetic.owner").exists());
            REMOVE_HOOK.with(|hook| *hook.borrow_mut() = None);
            let _ = pending(&env);
            assert!(!dir.join("r-synthetic.json").exists());
            assert!(dir.join("r-synthetic.answer").exists());
            assert!(dir.join("r-synthetic.owner").exists());
            assert!(dir.join("r-synthetic.done").exists());
        }
    }
}

#[test]
fn persistent_answer_read_error_returns_unknown_and_retains_the_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    crate::privfs::write(&approval_ui_file(&env), state::now().to_string()).unwrap();
    let env2 = env.clone();
    OWNER_PUBLISHED_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || publish(&env2, "host").unwrap()));
    });
    ANSWER_READ_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|| {
            Err(io::Error::other("persistent read failure"))
        }))
    });
    let result = HostApprover.ask_outcome(&env, &card("host"), Duration::ZERO);
    assert!(matches!(result, ApprovalOutcome::Unknown));
    OWNER_PUBLISHED_HOOK.with(|hook| *hook.borrow_mut() = None);
    ANSWER_READ_HOOK.with(|hook| *hook.borrow_mut() = None);
    let _ = pending(&env);
    let dir = approvals_dir(&env);
    assert!(!dir.join("r-synthetic.json").exists());
    assert!(dir.join("r-synthetic.answer").exists());
    assert!(dir.join("r-synthetic.owner").exists());
}

#[test]
fn failed_lock_open_does_not_touch_the_card_or_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    std::fs::create_dir(dir.join(".lock")).unwrap();
    let p = card("host");
    crate::privfs::write_atomic(
        &dir.join("r-synthetic.json"),
        serde_json::to_string(&p).unwrap(),
    )
    .unwrap();
    crate::privfs::write_atomic(&dir.join("r-synthetic.answer"), "\"deny\"").unwrap();
    assert!(close_pending(&env, &p.id).is_err());
    assert!(dir.join("r-synthetic.json").is_file());
    assert!(dir.join("r-synthetic.answer").is_file());
    std::fs::remove_dir(dir.join(".lock")).unwrap();
    assert!(close_pending(&env, &p.id).unwrap().is_some());
}

#[test]
fn interrupted_lock_attempt_is_retried_and_other_errors_are_returned() {
    let mut calls = 0;
    retry_interrupted(|| {
        calls += 1;
        if calls == 1 {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(calls, 2);
    let mut calls = 0;
    assert!(retry_interrupted(|| {
        calls += 1;
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    })
    .is_err());
    assert_eq!(calls, 1);
}

#[test]
fn stale_scan_revalidates_and_keeps_an_accepted_answer() {
    store::PID_LIVENESS_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(|_| Some(false))));
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut old = card("connect");
    old.connect.as_mut().unwrap().pid = 2_000_000_000;
    write_pending(&env, &old).unwrap();
    let mut current = old.clone();
    current.detail = "updated".into();
    update_pending(&env, &current).unwrap();
    assert!(!reap_stale(&env, &old).unwrap());
    assert!(approvals_dir(&env).join("r-synthetic.json").is_file());
    answer_connect(&env, &current.id, ConnectAnswer::Deny).unwrap();
    assert!(reap_stale(&env, &current).unwrap());
    assert!(approvals_dir(&env).join("r-synthetic.answer").is_file());
    assert!(approvals_dir(&env).join("r-synthetic.json").is_file());
    assert!(answer_connect(&env, &current.id, ConnectAnswer::AllowAlways).is_err());
}

#[test]
fn stale_ask_retains_an_answer_when_its_recorded_owner_is_dead() {
    store::PID_LIVENESS_HOOK
        .with(|h| *h.borrow_mut() = Some(Box::new(|pid| Some(pid != 2_000_000_000))));
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut p = card("text");
    p.sandbox = "synthetic-no-registration".into();
    p.created = state::now().saturating_sub(120);
    write_pending(&env, &p).unwrap();
    answer_reply(&env, &p.id, "accepted").unwrap();
    let dir = approvals_dir(&env);
    assert!(!reap_stale(&env, &p).unwrap(), "the actual owner is alive");
    assert!(dir.join("r-synthetic.answer").exists());
    std::fs::write(dir.join("r-synthetic.owner"), "unknown").unwrap();
    assert!(
        !reap_stale(&env, &p).unwrap(),
        "unknown ownership is not death"
    );
    assert!(dir.join("r-synthetic.answer").exists());
    std::fs::write(dir.join("r-synthetic.owner"), "2000000000").unwrap();
    assert!(reap_stale(&env, &p).unwrap());
    assert!(!dir.join("r-synthetic.json").exists());
    assert!(dir.join("r-synthetic.answer").exists());
    assert!(dir.join("r-synthetic.owner").exists());
    store::abandon_owner(&env, &p.id);
}

#[test]
fn cancelled_choice_does_not_wait_for_a_contended_store_lock() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    crate::privfs::write(&approval_ui_file(&env), state::now().to_string()).unwrap();
    let held = Rc::new(RefCell::new(None));
    let hold = held.clone();
    let cancelled = Rc::new(Cell::new(false));
    let cancel_after_publication = cancelled.clone();
    let lock_path = approvals_dir(&env).join(".lock");
    OWNER_PUBLISHED_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&lock_path)
                .unwrap();
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            *hold.borrow_mut() = Some(file);
            cancel_after_publication.set(true);
        }))
    });
    assert!(matches!(
        ask_choice(&env, &card("choice"), Duration::from_secs(30), &|| {
            cancelled.get()
        }),
        crate::spawn::Choice::Unknown
    ));
    OWNER_PUBLISHED_HOOK.with(|hook| *hook.borrow_mut() = None);
    drop(held.borrow_mut().take());
    let _ = pending(&env);
    assert!(!approvals_dir(&env).join("r-synthetic.json").exists());
}

#[test]
fn initial_store_contention_retries_for_host_and_choice_cards() {
    for kind in ["host", "choice"] {
        let tmp = tempfile::tempdir().unwrap();
        let env = crate::privfs::tests::env_in(tmp.path());
        crate::privfs::write(&approval_ui_file(&env), state::now().to_string()).unwrap();
        let dir = approvals_dir(&env);
        let held = lock_approvals(&dir, "r-synthetic").unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel();
        let other = env.clone();
        let worker = std::thread::spawn(move || {
            PUBLISH_CONTENDED_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    ready_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                }))
            });
            let answer_env = other.clone();
            OWNER_PUBLISHED_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    publish(&answer_env, kind).unwrap();
                }))
            });
            if kind == "host" {
                matches!(
                    HostApprover.ask_outcome(&other, &card(kind), Duration::from_secs(30)),
                    ApprovalOutcome::Answered(Verdict {
                        answer: Answer::Deny,
                        ..
                    })
                )
            } else {
                matches!(ask_choice(&other, &card(kind), Duration::from_secs(30), &|| false),
                    crate::spawn::Choice::More(ref s) if s == "30m")
            }
        });
        ready_rx.recv().unwrap();
        assert!(!dir.join("r-synthetic.json").exists());
        drop(held);
        go_tx.send(()).unwrap();
        assert!(worker.join().unwrap());
    }
}

#[test]
fn unpublished_contention_is_distinct_from_an_unknown_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    let held = lock_approvals(&dir, "r-synthetic").unwrap();
    assert!(matches!(
        HostApprover.ask_outcome(&env, &card("host"), Duration::ZERO),
        ApprovalOutcome::Unavailable
    ));
    assert!(matches!(
        ask_choice(&env, &card("choice"), Duration::ZERO, &|| false),
        crate::spawn::Choice::Unavailable
    ));
    assert!(!dir.join("r-synthetic.json").exists());
    assert!(!dir.join("r-synthetic.answer").exists());
    drop(held);
}

#[test]
fn cancellation_during_publication_contention_prevents_a_retry() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    let held = Rc::new(RefCell::new(Some(
        lock_approvals(&dir, "r-synthetic").unwrap(),
    )));
    let cancelled = Rc::new(Cell::new(false));
    let release = held.clone();
    let cancel = cancelled.clone();
    PUBLISH_CONTENDED_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            cancel.set(true);
            drop(release.borrow_mut().take());
        }))
    });
    let result = ask_choice(&env, &card("choice"), Duration::from_secs(30), &|| {
        cancelled.get()
    });
    PUBLISH_CONTENDED_HOOK.with(|hook| *hook.borrow_mut() = None);
    assert_eq!(result, crate::spawn::Choice::Unavailable);
    assert!(!dir.join("r-synthetic.json").exists());
}

#[test]
fn checked_listing_preserves_directory_and_card_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    assert!(pending_checked(&env).unwrap().is_empty());
    let dir = approvals_dir(&env);
    crate::privfs::dir(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, "not a directory").unwrap();
    assert!(pending_checked(&env).is_err());
    std::fs::remove_file(&dir).unwrap();
    crate::privfs::dir(&dir).unwrap();
    let valid = card("host");
    std::fs::write(
        dir.join("r-synthetic.json"),
        serde_json::to_vec(&valid).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("bad.json"), "{broken").unwrap();
    assert_eq!(
        pending_checked(&env).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        pending(&env).len(),
        1,
        "the legacy best-effort caller remains unchanged"
    );
    std::fs::remove_file(dir.join("bad.json")).unwrap();
    std::fs::create_dir(dir.join("unreadable.json")).unwrap();
    assert!(pending_checked(&env).is_err());
    std::fs::remove_dir(dir.join("unreadable.json")).unwrap();
    assert_eq!(pending_checked(&env).unwrap().len(), 1);
    std::fs::write(
        dir.join("wrong-id.json"),
        serde_json::to_vec(&valid).unwrap(),
    )
    .unwrap();
    assert_eq!(
        pending_checked(&env).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn checked_listing_reports_dangling_cards_and_preserves_unknown_legacy_owners() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let dir = approvals_dir(&env);
    crate::privfs::dir(&dir).unwrap();
    std::os::unix::fs::symlink("missing-target", dir.join("dangling.json")).unwrap();
    assert_eq!(
        pending_checked(&env).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(std::fs::symlink_metadata(dir.join("dangling.json")).is_ok());
    std::fs::remove_file(dir.join("dangling.json")).unwrap();

    let mut p = card("text");
    p.sandbox = "legacy-sandbox".into();
    p.created = state::now().saturating_sub(120);
    crate::privfs::write(
        &dir.join(format!("{}.json", p.id)),
        serde_json::to_vec(&p).unwrap(),
    )
    .unwrap();
    let info = state::Running::file(&env, &p.sandbox);
    crate::privfs::write(&info, "{malformed").unwrap();
    assert!(pending_checked(&env).is_err());
    assert_eq!(pending(&env), vec![p.clone()]);
    assert!(dir.join(format!("{}.json", p.id)).exists());
    std::fs::remove_file(&info).unwrap();
    assert!(pending_checked(&env).is_err());
    assert!(dir.join(format!("{}.json", p.id)).exists());

    let owner = dir.join(format!("{}.owner", p.id));
    std::os::unix::fs::symlink("missing-owner", &owner).unwrap();
    assert!(pending_checked(&env).is_err());
    assert!(dir.join(format!("{}.json", p.id)).exists());
    std::fs::remove_file(owner).unwrap();

    let live = state::Running {
        id: p.sandbox.clone(),
        pid: std::process::id(),
        ..Default::default()
    };
    crate::privfs::write(&info, serde_json::to_vec(&live).unwrap()).unwrap();
    assert_eq!(pending_checked(&env).unwrap(), vec![p]);
}

#[test]
fn a_card_open_notice_remains_factual_after_withdrawal() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut p = card("host");
    p.sandbox = "synthetic-mail".into();
    p.summary = "original request".into();
    crate::mail::create(
        &env,
        &p.sandbox,
        &crate::mail::Starter {
            kind: "host".into(),
            caller: "test".into(),
            parent: None,
            created: 0,
        },
    )
    .unwrap();
    let card_path = approvals_dir(&env).join(format!("{}.json", p.id));
    crate::privfs::write(&card_path, serde_json::to_vec(&p).unwrap()).unwrap();
    let mailbox = crate::mail::dir(&env, &p.sandbox).unwrap();
    let held = crate::mail::lock(&mailbox).unwrap();
    assert!(!store::try_card_mail(&env, &p).unwrap());
    assert_eq!(close_pending(&env, &p.id).unwrap(), None);
    drop(held);
    // Mail I/O must not acquire the approval lock, even after closure.
    let store_lock = lock_approvals(&approvals_dir(&env), &p.id).unwrap();
    assert!(store::try_card_mail(&env, &p).unwrap());
    drop(store_lock);
    let mail = crate::mail::read(&env, &p.sandbox, crate::mail::Dir::Up);
    assert_eq!(mail.len(), 1);
    assert!(mail[0].text.contains("approval card r-synthetic opened"));
    assert!(mail[0].text.contains("original request"));
    assert!(mail[0].text.contains("paddock approvals"));
    p.detail = "x".repeat(1024 * 1024 + 1);
    assert!(store::try_card_mail(&env, &p).unwrap());
    assert_eq!(
        crate::mail::read(&env, &p.sandbox, crate::mail::Dir::Up).len(),
        1
    );
}

#[test]
fn a_stalled_mail_job_does_not_hold_the_approval_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(tmp.path());
    let mut p = card("host");
    p.sandbox = "synthetic-stall".into();
    crate::mail::create(
        &env,
        &p.sandbox,
        &crate::mail::Starter {
            kind: "host".into(),
            caller: "test".into(),
            parent: None,
            created: 0,
        },
    )
    .unwrap();
    let mailbox = crate::mail::dir(&env, &p.sandbox).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    CARD_MAIL_HOOK.with(|hook| {
        *hook.borrow_mut() = Some((
            Box::new(move || {
                let _mailbox = crate::mail::lock(&mailbox).unwrap();
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }),
            Box::new(move || finished_tx.send(()).unwrap()),
        ));
    });
    write_pending(&env, &p).unwrap();
    entered_rx.recv().unwrap();
    // A different recipient is attempted while the first worker is stuck.
    let mut healthy = card("host");
    healthy.id = "r-healthy".into();
    healthy.sandbox = "synthetic-healthy".into();
    crate::mail::create(
        &env,
        &healthy.sandbox,
        &crate::mail::Starter {
            kind: "host".into(),
            caller: "test".into(),
            parent: None,
            created: 0,
        },
    )
    .unwrap();
    let (healthy_tx, healthy_rx) = mpsc::channel();
    CARD_MAIL_HOOK.with(|hook| {
        *hook.borrow_mut() = Some((
            Box::new(|| {}),
            Box::new(move || healthy_tx.send(()).unwrap()),
        ));
    });
    write_pending(&env, &healthy).unwrap();
    healthy_rx.recv().unwrap();
    assert_eq!(
        crate::mail::read(&env, &healthy.sandbox, crate::mail::Dir::Up).len(),
        1
    );
    assert_eq!(close_pending(&env, &healthy.id).unwrap(), None);
    // Simulate a stalled mailbox operation while another operation closes
    // the original approval, using no real approval or user state.
    assert_eq!(close_pending(&env, &p.id).unwrap(), None);
    release_tx.send(()).unwrap();
    finished_rx.recv().unwrap();
    let mail = crate::mail::read(&env, &p.sandbox, crate::mail::Dir::Up);
    assert_eq!(mail.len(), 1);
    assert!(mail[0].text.contains("approval card r-synthetic opened"));
}
