use super::*;

#[test]
fn resumed_notice_updates_status_even_without_a_connection_answer() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Ask);
    for i in 0..=crate::macvm::ask_queue::GUARD_MAX {
        fw.asker()
            .unwrap()
            .join(&triple(i), Who::default(), None, 100);
    }
    fw.settle(101);
    let before = ask_status::read(&fw.status_path());
    assert!(before.paused);
    assert!(before.open.iter().all(|q| !q.shown));
    let notice = broker::pending(&env).into_iter().next().unwrap();
    broker::answer_connect(&env, &notice.id, ConnectAnswer::AllowOnce).unwrap();
    fw.settle(102);
    let after = ask_status::read(&fw.status_path());
    assert!(!after.paused);
    assert_eq!(after.open.len(), before.open.len());
    assert_eq!(after.open.iter().filter(|q| q.shown).count(), 1);
    let report = broker::pending_report(&env).unwrap();
    let outcome = report.outcomes.iter().find(|o| o.id == notice.id).unwrap();
    assert_eq!(outcome.state, broker::LocalApplication::Applied);
}

#[test]
fn failed_status_write_is_retried_without_another_queue_change() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Ask);
    fw.asker()
        .unwrap()
        .join(&triple(0), Who::default(), None, 100);
    std::fs::create_dir_all(fw.status_path()).unwrap();
    fw.settle(101);
    assert!(fw.status_retry.load(std::sync::atomic::Ordering::SeqCst));
    std::fs::remove_dir(fw.status_path()).unwrap();
    fw.settle(102);
    assert!(!fw.status_retry.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(ask_status::read(&fw.status_path()).open.len(), 1);
}

#[test]
fn production_recheck_releases_rules_before_reading_mode() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::None);
    fw.asker()
        .unwrap()
        .join(&triple(0), Who::default(), None, 100);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        RECHECK_BEFORE_MODE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }))
        });
        other.recheck_current_rules(101);
    });
    ready_rx.recv().unwrap();
    let mode = fw.mode.write().unwrap();
    assert!(
        fw.rules.try_lock().is_ok(),
        "the rechecker must have released rules"
    );
    go_tx.send(()).unwrap();
    drop(mode);
    worker.join().unwrap();
    assert_eq!(fw.asker().unwrap().waiting(), 0);
}

#[test]
fn recheck_does_not_claim_a_rule_removed_before_the_claim() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Ask);
    let key = triple(1);
    let q = match fw.asker().unwrap().join(&key, Who::default(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a waiting question"),
    };
    let rule = firewall::new_rule(
        "*",
        None,
        &key.host,
        firewall::Port::Num(443),
        firewall::Verdict::Allow,
        Scope::Session,
        BY_USER,
        100,
    );
    fw.rules().add(rule, false).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        RECHECK_BEFORE_MODE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }));
        });
        other.recheck_current_rules(101);
    });
    ready_rx.recv().unwrap();
    fw.rules().session.clear();
    go_tx.send(()).unwrap();
    worker.join().unwrap();
    assert!(q.result().is_none());
    assert_eq!(fw.asker().unwrap().waiting(), 1);
}

#[test]
fn a_rule_based_connection_is_rejected_if_the_hold_now_requires_asking() {
    let (stream, _other) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut held = Held {
        stream,
        host: "removed.example".into(),
        port: 443,
        id: None,
        require_allow: true,
    };
    assert!(held.rejected_at_hold(Mode::Ask, None, &[]));
    held.require_allow = false;
    assert!(!held.rejected_at_hold(Mode::Ask, None, &[]));
}

#[test]
fn removing_a_rule_between_decision_and_hold_refuses_the_connection() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Ask);
    let rule = firewall::new_rule(
        "*",
        None,
        "removed.example",
        firewall::Port::Num(443),
        firewall::Verdict::Allow,
        Scope::Session,
        BY_USER,
        100,
    );
    fw.rules().add(rule, false).unwrap();
    let gate = fw.clone().gate();
    let mut opened = (gate.0)("removed.example", 443, None).unwrap();
    fw.rules().session.clear();
    let (client, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    assert!(opened.hold(&client).is_err());
    assert_eq!(fw.open_count(), 0);
    let entries = connlog::read_at(&fw.log);
    assert!(entries
        .iter()
        .any(|e| e.verdict == connlog::DENY && e.rule == RULE_STALE));
}

#[test]
fn none_stops_connections_while_recheck_waits_for_the_approval_store() {
    use std::os::unix::io::AsRawFd;
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Egress);
    let gate = fw.clone().gate();
    let (client, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut opened = (gate.0)("open.example", 443, None).unwrap();
    opened.hold(&client).unwrap();
    fw.asker()
        .unwrap()
        .join(&triple(1), Who::default(), None, 100);
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(broker::approvals_dir(&env).join(".lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let other = fw.clone();
    let recheck = std::thread::spawn(move || {
        crate::macvm::approvals::CLAIM_CLOSE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }));
        });
        other.recheck_current_rules(101);
    });
    ready_rx.recv().unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let other = fw.clone();
    let switch = std::thread::spawn(move || done_tx.send(other.set_mode(Mode::None)).unwrap());
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    // Always release the worker, including when the assertion would fail.
    go_tx.send(()).unwrap();
    drop(lock);
    switch.join().unwrap();
    recheck.join().unwrap();
    assert_eq!(result.unwrap(), 1);
    assert_eq!(fw.mode(), Mode::None);
    use std::io::Read;
    let mut byte = [0];
    assert_eq!(peer.read(&mut byte).unwrap(), 0);
}

#[test]
fn a_withdrawn_card_is_reopened_if_the_mode_changes_before_revalidation() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Egress);
    let key = triple(2);
    let q = match fw.asker().unwrap().join(&key, Who::default(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a waiting question"),
    };
    let original = broker::pending(&env)[0].id.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        RECHECK_REVALIDATE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }))
        });
        other.recheck_current_rules(101);
    });
    ready_rx.recv().unwrap();
    // Simulate a mode change at the exact revalidation boundary. The rules
    // guard intentionally prevents concurrent rule edits at this point.
    *fw.mode.write().unwrap() = Mode::Ask;
    assert!(broker::answer_connect(&env, &original, ConnectAnswer::AllowOnce).is_err());
    go_tx.send(()).unwrap();
    worker.join().unwrap();
    assert!(q.result().is_none());
    fw.settle(102);
    let cards = broker::pending(&env);
    assert_eq!(cards.len(), 1);
    assert_ne!(cards[0].id, original);
}

#[test]
fn a_failed_persistent_deny_remains_a_session_rule_after_reload() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Ask);
    let profile = crate::macvm::session::Profile::new(t.path().join("profile"));
    let fw = Arc::new(
        Arc::try_unwrap(fw)
            .ok()
            .unwrap()
            .with_changes(profile.dir.clone(), t.path().to_path_buf()),
    );
    let key = triple(3);
    {
        let mut rules = fw.rules();
        rules
            .add(
                firewall::new_rule(
                    "*",
                    None,
                    &key.host,
                    firewall::Port::Num(443),
                    firewall::Verdict::Allow,
                    Scope::Always,
                    BY_USER,
                    100,
                ),
                false,
            )
            .unwrap();
        rules.save().unwrap();
    }
    let previous = firewall::new_rule(
        "*",
        None,
        &key.host,
        firewall::Port::Num(443),
        firewall::Verdict::Allow,
        Scope::Session,
        BY_USER,
        100,
    );
    fw.rules().add(previous.clone(), false).unwrap();
    let before = std::fs::read(t.path().join("rules.toml")).unwrap();
    let dir = t.path().to_path_buf();
    crate::privfs::ATOMIC_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("rules.toml.paddock-tmp-")
                {
                    std::fs::remove_file(entry.path()).unwrap();
                }
            }
        }))
    });
    let word = fw.apply(
        ConnectAnswer::DenyAlways,
        &Who::default(),
        &key,
        BY_USER,
        101,
    );
    crate::privfs::ATOMIC_HOOK.with(|hook| *hook.borrow_mut() = None);
    assert!(word.starts_with("session-"));
    assert_eq!(std::fs::read(t.path().join("rules.toml")).unwrap(), before);
    fw.rules().reload_if_changed().unwrap();
    assert_eq!(fw.connection(&key.host, key.port, None).unwrap_err(), word);
    let rules = fw.rules();
    assert!(rules
        .session
        .iter()
        .any(|r| r.id == word && r.verdict == firewall::Verdict::Deny));
    assert!(rules.session.iter().any(|r| r.id == previous.id));
    assert!(rules
        .file
        .iter()
        .all(|r| r.verdict == firewall::Verdict::Allow));
    let changes = crate::macvm::grants::load(&profile)
        .unwrap()
        .history_changes;
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (changes[0].action.as_str(), changes[0].value.as_str()),
        ("add", "deny session (historical; order unknown)")
    );
}

#[test]
fn deny_committed_before_poll_rejects_stale_allows_and_keeps_other_grants() {
    for answer in [
        ConnectAnswer::AllowOnce,
        ConnectAnswer::AllowSession,
        ConnectAnswer::AllowAlways,
        ConnectAnswer::AllowEverything,
        ConnectAnswer::AllowSite,
    ] {
        let t = tempfile::tempdir().unwrap();
        let (env, fw) = firewall(t.path(), Mode::Ask);
        fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
        let key = triple(4);
        let who = Who {
            app: key.app.clone(),
            team: Some("T1".into()),
            ..Default::default()
        };
        let q = match fw.asker().unwrap().join(&key, who.clone(), None, 100) {
            Join::Wait(q, false) => q,
            _ => panic!("expected a question"),
        };
        let card = broker::pending(&env).pop().unwrap();
        broker::answer_connect(&env, &card.id, answer).unwrap();
        let broad = firewall::new_rule(
            &key.app,
            Some("T1"),
            "*",
            firewall::Port::Any,
            firewall::Verdict::Allow,
            Scope::Session,
            BY_USER,
            100,
        );
        let exact = firewall::new_rule(
            &key.app,
            Some("T1"),
            &key.host,
            firewall::Port::Num(key.port),
            firewall::Verdict::Allow,
            Scope::Session,
            BY_USER,
            100,
        );
        {
            let mut rules = fw.rules();
            rules.add(broad.clone(), true).unwrap();
            rules.add(exact.clone(), false).unwrap();
        }
        let mut writer = Rules::load_at(t.path().join("rules.toml")).unwrap();
        writer
            .add(
                firewall::new_rule(
                    &key.app,
                    Some("T1"),
                    &key.host,
                    firewall::Port::Num(key.port),
                    firewall::Verdict::Deny,
                    Scope::Always,
                    BY_USER,
                    101,
                ),
                false,
            )
            .unwrap();
        writer.save().unwrap();
        fw.settle(101);
        let settled = q.result().unwrap();
        assert_eq!(settled.answer, answer);
        assert!(!settled.allowed, "{answer:?}");
        assert_eq!(settled.word, RULE_STALE);
        let rules = fw.rules();
        assert!(rules.session.contains(&broad));
        assert!(rules.session.contains(&exact));
        let current = who.conn(&key);
        assert_eq!(
            firewall::winner(&rules.file, &current).unwrap().verdict,
            firewall::Verdict::Deny
        );
        assert_eq!(
            firewall::winner(&rules.all(), &current).unwrap().id,
            exact.id,
            "an existing independent session exception remains effective"
        );
        let other = Triple {
            identity: None,
            host: "other.example".into(),
            ..key.clone()
        };
        assert_eq!(
            firewall::winner(&rules.all(), &who.conn(&other))
                .unwrap()
                .id,
            broad.id
        );
    }
}

#[test]
fn a_writer_after_poll_but_before_application_is_revalidated() {
    use std::os::unix::io::AsRawFd;
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Ask);
    fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
    let key = triple(5);
    let who = Who {
        app: key.app.clone(),
        ..Default::default()
    };
    let q = match fw.asker().unwrap().join(&key, who, None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let card = broker::pending(&env).pop().unwrap();
    broker::answer_connect(&env, &card.id, ConnectAnswer::AllowSession).unwrap();
    let (polled_tx, polled_rx) = mpsc::channel();
    let (continue_tx, continue_rx) = mpsc::channel();
    let worker_fw = fw.clone();
    let worker = std::thread::spawn(move || {
        AFTER_POLL_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                polled_tx.send(()).unwrap();
                continue_rx.recv().unwrap();
            }));
        });
        worker_fw.settle(101);
    });
    polled_rx.recv().unwrap();
    let probe = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(t.path().join("rules.toml.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    drop(probe);
    let mut writer = Rules::load_at(t.path().join("rules.toml")).unwrap();
    writer
        .add(
            firewall::new_rule(
                &key.app,
                None,
                &key.host,
                firewall::Port::Num(key.port),
                firewall::Verdict::Deny,
                Scope::Always,
                BY_USER,
                102,
            ),
            false,
        )
        .unwrap();
    writer.save().unwrap();
    continue_tx.send(()).unwrap();
    worker.join().unwrap();
    let result = q.result().unwrap();
    assert_eq!(result.answer, ConnectAnswer::AllowSession);
    assert!(!result.allowed);
    assert_eq!(result.word, RULE_STALE);
    assert!(fw.rules().session.is_empty());
}

#[test]
fn answer_revalidation_respects_file_rule_expiry_and_specificity() {
    for expired in [true, false] {
        let t = tempfile::tempdir().unwrap();
        let (env, fw) = firewall(t.path(), Mode::Ask);
        fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
        let key = triple(8);
        let who = Who {
            app: key.app.clone(),
            ..Default::default()
        };
        let q = match fw.asker().unwrap().join(&key, who, None, 100) {
            Join::Wait(q, false) => q,
            _ => panic!("expected a question"),
        };
        let card = broker::pending(&env).pop().unwrap();
        broker::answer_connect(&env, &card.id, ConnectAnswer::AllowSession).unwrap();
        let mut writer = Rules::load_at(t.path().join("rules.toml")).unwrap();
        let mut deny = firewall::new_rule(
            &key.app,
            None,
            if expired { &key.host } else { "*" },
            if expired {
                firewall::Port::Num(key.port)
            } else {
                firewall::Port::Any
            },
            firewall::Verdict::Deny,
            Scope::Always,
            BY_USER,
            101,
        );
        if expired {
            deny.expires = Some(0);
        } else {
            writer
                .add(
                    firewall::new_rule(
                        &key.app,
                        None,
                        &key.host,
                        firewall::Port::Num(key.port),
                        firewall::Verdict::Allow,
                        Scope::Always,
                        BY_USER,
                        101,
                    ),
                    false,
                )
                .unwrap();
        }
        writer.add(deny, false).unwrap();
        writer.save().unwrap();
        fw.settle(101);
        if expired {
            assert!(q.result().unwrap().allowed);
            assert_eq!(fw.rules().session.len(), 1);
        } else {
            // The active `deny *` can overlap the new session allow, so the
            // allow fails closed, even with an exact allow rule in the file.
            assert!(!q.result().unwrap().allowed);
            assert!(fw.rules().session.is_empty());
        }
    }
}

#[test]
fn a_failed_persistent_allow_is_not_promoted_to_a_session_allow() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Ask);
    let key = triple(6);
    let dir = t.path().to_path_buf();
    crate::privfs::ATOMIC_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("rules.toml.paddock-tmp-")
                {
                    std::fs::remove_file(entry.path()).unwrap();
                }
            }
        }));
    });
    let result = fw.apply(
        ConnectAnswer::AllowAlways,
        &Who::default(),
        &key,
        BY_USER,
        101,
    );
    crate::privfs::ATOMIC_HOOK.with(|hook| *hook.borrow_mut() = None);
    assert_eq!(result, RULE_STALE);
    assert!(fw.rules().session.is_empty());
}

#[test]
fn unreadable_rules_at_poll_reject_an_allow_without_deleting_session_grants() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Ask);
    fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
    let key = triple(7);
    let who = Who {
        app: key.app.clone(),
        ..Default::default()
    };
    let q = match fw.asker().unwrap().join(&key, who, None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let card = broker::pending(&env).pop().unwrap();
    broker::answer_connect(&env, &card.id, ConnectAnswer::AllowSession).unwrap();
    let broad = firewall::new_rule(
        &key.app,
        None,
        "*",
        firewall::Port::Any,
        firewall::Verdict::Allow,
        Scope::Session,
        BY_USER,
        100,
    );
    fw.rules().add(broad.clone(), true).unwrap();
    std::fs::write(t.path().join("rules.toml"), "not valid = [").unwrap();
    fw.settle(101);
    assert_eq!(q.result().unwrap().word, RULE_STALE);
    assert!(!q.result().unwrap().allowed);
    assert!(fw.rules().session.contains(&broad));
    assert_eq!(
        fw.connection(&key.host, key.port, None).unwrap_err(),
        RULE_STALE
    );
}

#[test]
fn dangling_rules_file_is_not_treated_as_an_empty_rule_set() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Ask);
    fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
    let key = triple(9);
    let who = Who {
        app: key.app.clone(),
        ..Default::default()
    };
    let q = match fw.asker().unwrap().join(&key, who, None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let card = broker::pending(&env).pop().unwrap();
    broker::answer_connect(&env, &card.id, ConnectAnswer::AllowSession).unwrap();
    let path = t.path().join("rules.toml");
    std::os::unix::fs::symlink("missing-rules", &path).unwrap();
    fw.settle(101);
    assert_eq!(q.result().unwrap().word, RULE_STALE);
    assert!(!q.result().unwrap().allowed);
    assert!(fw.rules().session.is_empty());
    assert!(std::fs::symlink_metadata(path).is_ok());
}

#[test]
fn none_closes_streams_and_the_card_stays_answerable_during_a_rules_lock() {
    let t = tempfile::tempdir().unwrap();
    let (env, fw) = firewall(t.path(), Mode::Egress);
    let gate = fw.clone().gate();
    let (client, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut opened = (gate.0)("open.example", 443, None).unwrap();
    opened.hold(&client).unwrap();
    let key = triple(5);
    fw.asker().unwrap().join(&key, Who::default(), None, 100);
    let card = broker::pending(&env).pop().unwrap();
    let held = fw.rules().decision_lock().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        RECHECK_BEFORE_FILE_LOCK_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
            }));
        });
        other.recheck_current_rules(101);
    });
    ready_rx.recv().unwrap();
    assert_eq!(broker::pending(&env)[0].id, card.id);
    broker::answer_connect(&env, &card.id, ConnectAnswer::AllowOnce).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let other = fw.clone();
    let switch = std::thread::spawn(move || done_tx.send(other.set_mode(Mode::None)).unwrap());
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    drop(held);
    switch.join().unwrap();
    worker.join().unwrap();
    assert_eq!(result.unwrap(), 1);
    use std::io::Read;
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    fw.settle(102);
    assert_eq!(fw.mode(), Mode::None);
}

#[test]
fn none_closes_streams_while_a_rules_read_is_blocked() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Egress);
    let gate = fw.clone().gate();
    let (client, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut opened = (gate.0)("open.example", 443, None).unwrap();
    opened.hold(&client).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        let first = std::cell::Cell::new(true);
        firewall::READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if first.replace(false) {
                    ready_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                }
            }));
        });
        other.connection("blocked.example", 443, None)
    });
    ready_rx.recv().unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let other = fw.clone();
    let switch = std::thread::spawn(move || done_tx.send(other.set_mode(Mode::None)).unwrap());
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    go_tx.send(()).unwrap();
    switch.join().unwrap();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(result.unwrap(), 1);
    use std::io::Read;
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
