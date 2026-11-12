use super::*;

fn fake_firewall(t: &std::path::Path) -> (crate::env::HostEnv, Arc<Firewall>) {
    let env = crate::privfs::tests::env_in(t);
    let asker = Asker::new(env.clone(), "fake", "Fake", t.into(), 123);
    let rules = Rules::load_at(t.join("rules.toml")).unwrap();
    (
        env,
        Arc::new(Firewall::new(
            Mode::Ask,
            None,
            rules,
            Some(asker),
            t.join("logs/connections.jsonl"),
        )),
    )
}

fn helper_who() -> Who {
    Who {
        app: "Helper-A".into(),
        parent_app: Some("Editor".into()),
        team: Some("T1".into()),
    }
}

#[test]
fn every_selected_allow_checks_process_parent_team_host_and_port_scope() {
    for answer in [
        ConnectAnswer::AllowSession,
        ConnectAnswer::AllowAlways,
        ConnectAnswer::AllowSite,
        ConnectAnswer::AllowEverything,
    ] {
        for scope in [Scope::Always, Scope::Session] {
            let tmp = tempfile::tempdir().unwrap();
            let (_env, fw) = fake_firewall(tmp.path());
            let key = Triple {
                identity: None,
                app: "Editor".into(),
                host: "api.example.com".into(),
                port: 443,
            };
            let who = helper_who();
            let independent = firewall::new_rule(
                "Editor",
                Some("T1"),
                "other.example",
                firewall::Port::Num(443),
                firewall::Verdict::Allow,
                Scope::Session,
                BY_USER,
                1,
            );
            fw.rules().add(independent.clone(), false).unwrap();
            let deny = firewall::new_rule(
                "Helper-B",
                Some("T1"),
                &key.host,
                firewall::Port::Num(443),
                firewall::Verdict::Deny,
                scope,
                BY_USER,
                2,
            );
            if scope == Scope::Always {
                let mut writer = Rules::load_at(tmp.path().join("rules.toml")).unwrap();
                writer.add(deny.clone(), false).unwrap();
                writer.save().unwrap();
            } else {
                fw.rules().add(deny.clone(), false).unwrap();
            }
            assert_eq!(
                fw.apply(answer, &who, &key, BY_USER, 3),
                RULE_STALE,
                "{answer:?} {scope:?}"
            );
            let rules = fw.rules();
            assert!(rules.session.contains(&independent));
            assert!(rules.file.contains(&deny) || rules.session.contains(&deny));
        }
    }

    for (team, port, expected_stale) in [
        (Some("T2"), 443, false),
        (Some("T1"), 8443, false),
        (None, 443, true),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (_env, fw) = fake_firewall(tmp.path());
        let key = Triple {
            identity: None,
            app: "Editor".into(),
            host: "api.example.com".into(),
            port: 443,
        };
        let deny = firewall::new_rule(
            "Helper-B",
            team,
            &key.host,
            firewall::Port::Num(port),
            firewall::Verdict::Deny,
            Scope::Session,
            BY_USER,
            1,
        );
        fw.rules().add(deny.clone(), false).unwrap();
        assert_eq!(
            fw.apply(ConnectAnswer::AllowSession, &helper_who(), &key, BY_USER, 2) == RULE_STALE,
            expected_stale
        );
        assert!(fw.rules().session.contains(&deny));
    }
}

#[test]
fn queued_and_hook_session_answers_recheck_another_process_deny() {
    for hook in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let (env, fw) = fake_firewall(tmp.path());
        fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
        let key = Triple {
            identity: None,
            app: "Editor".into(),
            host: "api.example.com".into(),
            port: 443,
        };
        let who = helper_who();
        let path = tmp.path().join("rules.toml");
        let install = move || {
            let mut writer = Rules::load_at(path.clone()).unwrap();
            writer
                .add(
                    firewall::new_rule(
                        "Helper-B",
                        Some("T1"),
                        "api.example.com",
                        firewall::Port::Num(443),
                        firewall::Verdict::Deny,
                        Scope::Always,
                        BY_USER,
                        1,
                    ),
                    false,
                )
                .unwrap();
            writer.save().unwrap();
        };
        let result = if hook {
            crate::macvm::approvals::TEST_CONNECT_HOOK.with(|h| {
                *h.borrow_mut() = Some(Box::new(move || {
                    install();
                    Some(ConnectAnswer::AllowSession)
                }));
            });
            let result = fw.ask(fw.asker().unwrap(), &key, who, None, 2).unwrap();
            crate::macvm::approvals::TEST_CONNECT_HOOK.with(|h| *h.borrow_mut() = None);
            result
        } else {
            let q = match fw.asker().unwrap().join(&key, who, None, 1) {
                Join::Wait(q, false) => q,
                _ => panic!(),
            };
            broker::answer_connect(&env, &q.id, ConnectAnswer::AllowSession).unwrap();
            install();
            fw.settle(2);
            q.result().unwrap()
        };
        assert_eq!(result.answer, ConnectAnswer::AllowSession);
        assert!(!result.allowed);
        assert_eq!(result.word, RULE_STALE);
        assert!(fw.rules().session.is_empty());
    }
}

#[test]
fn once_answers_do_not_cross_process_or_team_question_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
    let base = Triple {
        identity: None,
        app: "Editor".into(),
        host: "api.example.com".into(),
        port: 443,
    };
    let first = helper_who();
    let id = Identity {
        pid: Some(71),
        app: Some("Helper-A".into()),
        team: Some("T1".into()),
        parent_app: Some("Editor".into()),
        path: Some("/fake/helper-a".into()),
    };
    let first_key = question_key(&base, &first, Some(&id));
    assert_ne!(
        first_key,
        question_key(&base, &first, Some(&id)),
        "the same PID may be reused"
    );
    let mut other_id = id.clone();
    other_id.pid = Some(72);
    other_id.app = Some("Helper-B".into());
    other_id.path = Some("/fake/helper-b".into());
    let other = Who {
        app: "Helper-B".into(),
        ..first.clone()
    };
    let process_key = question_key(&base, &other, Some(&other_id));
    let mut team = other.clone();
    team.team = Some("T2".into());
    let team_key = question_key(&base, &team, Some(&other_id));
    assert_ne!(first_key, process_key);
    assert_ne!(process_key, team_key);
    assert_ne!(
        question_key(&base, &first, None),
        question_key(&base, &first, None)
    );
    let asker = fw.asker().unwrap();
    let q1 = match asker.join(&first_key, first, Some(&id), 1) {
        Join::Wait(q, false) => q,
        _ => panic!(),
    };
    let q2 = match asker.join(&process_key, other, Some(&other_id), 1) {
        Join::Wait(q, false) => q,
        _ => panic!(),
    };
    let q3 = match asker.join(&team_key, team, Some(&other_id), 1) {
        Join::Wait(q, false) => q,
        _ => panic!(),
    };
    broker::answer_connect(&env, &q1.id, ConnectAnswer::AllowOnce).unwrap();
    fw.settle(2);
    assert!(q1.result().unwrap().allowed);
    assert!(q2.result().is_none());
    assert!(q3.result().is_none());
    assert!(fw.rules().session.is_empty());
}

#[test]
fn missing_or_reused_pid_cannot_receive_a_late_once_decision() {
    for pid in [None, Some(71)] {
        let tmp = tempfile::tempdir().unwrap();
        let (env, fw) = fake_firewall(tmp.path());
        fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
        let asker = fw.asker().unwrap();
        let identity = Identity {
            pid,
            app: Some("Helper-A".into()),
            ..Default::default()
        };
        let base = triple(1);
        let who = helper_who();
        let key = question_key(&base, &who, Some(&identity));
        let q = match asker.join(&key, who.clone(), Some(&identity), 1) {
            Join::Wait(q, false) => q,
            _ => panic!("expected a waiting connection"),
        };
        assert!(asker.timeout(&key, &q, 2).is_none());
        broker::answer_connect(&env, &q.id, ConnectAnswer::AllowOnce).unwrap();
        fw.settle(3);
        assert!(q.result().unwrap().allowed);
        let report = broker::pending_report(&env).unwrap();
        let outcome = report.outcomes.iter().find(|o| o.id == q.id).unwrap();
        assert_eq!(outcome.state, broker::LocalApplication::NoWaiter);
        assert_eq!(outcome.local_allowed, None);
        let new_key = question_key(&base, &who, Some(&identity));
        assert_ne!(key, new_key);
        assert!(matches!(
            asker.join(&new_key, who, Some(&identity), 4),
            Join::Wait(_, false)
        ));
    }
}

#[test]
fn unique_unverified_connections_have_a_fixed_queue_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let (_env, fw) = fake_firewall(tmp.path());
    let asker = fw.asker().unwrap();
    let base = triple(2);
    let who = helper_who();
    for _ in 0..crate::macvm::approvals::MAX_OPEN_QUESTIONS {
        let key = question_key(&base, &who, None);
        assert!(matches!(
            asker.join(&key, who.clone(), None, 1),
            Join::Wait(_, false)
        ));
    }
    let key = question_key(&base, &who, None);
    assert!(matches!(asker.join(&key, who, None, 1), Join::Stale));
    assert_eq!(asker.waiting(), crate::macvm::approvals::MAX_OPEN_QUESTIONS);
}

#[test]
fn pause_deny_crash_keeps_affected_decisions_as_unconfirmed_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let asker = fw.asker().unwrap();
    let mut questions = Vec::new();
    for i in 0..=crate::macvm::ask_queue::GUARD_MAX {
        let key = triple(i);
        match asker.join(&key, helper_who(), None, 1) {
            Join::Wait(q, false) => questions.push(q),
            _ => panic!("expected a queued question"),
        }
    }
    let notice = broker::pending(&env)
        .into_iter()
        .find(|p| p.connect.as_ref().is_some_and(|c| c.notice))
        .unwrap();
    broker::answer_connect(&env, &notice.id, ConnectAnswer::Deny).unwrap();
    AFTER_POLL_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(|| panic!("synthetic crash after notice polling")))
    });
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fw.settle(2)));
    AFTER_POLL_HOOK.with(|h| *h.borrow_mut() = None);
    assert!(crash.is_err());
    assert!(questions.iter().all(|q| q.result().is_none()));
    let report = broker::pending_report(&env).unwrap();
    let outcome = report.outcomes.iter().find(|o| o.id == notice.id).unwrap();
    assert_eq!(outcome.state, broker::LocalApplication::AffectedUnconfirmed);
    assert_eq!(outcome.affected.len(), questions.len());
    assert!(outcome
        .affected
        .iter()
        .all(|a| a.app == "Helper-A" && a.waiting == 1));
    assert!(report.unresolved.iter().any(|u| u.id == notice.id));
}

#[test]
fn failed_pause_outcome_write_reconciles_after_storage_recovers() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let asker = fw.asker().unwrap();
    for i in 0..=crate::macvm::ask_queue::GUARD_MAX {
        assert!(matches!(
            asker.join(&triple(i), helper_who(), None, 1),
            Join::Wait(_, false)
        ));
    }
    let notice = broker::pending(&env)
        .into_iter()
        .find(|p| p.connect.as_ref().is_some_and(|c| c.notice))
        .unwrap();
    broker::answer_connect(&env, &notice.id, ConnectAnswer::Deny).unwrap();
    let dir = broker::approvals_dir(&env);
    let done = dir.join(format!("{}.done", notice.id));
    std::fs::create_dir(&done).unwrap();
    fw.settle(2);
    std::fs::remove_dir(&done).unwrap();
    fw.settle(3);
    assert!(!asker.paused());
    assert!(done.exists());
    assert!(dir.join(format!("{}.answer", notice.id)).is_file());
    assert!(broker::pending_report(&env)
        .unwrap()
        .unresolved
        .iter()
        .any(|u| u.id == notice.id));
}

#[test]
fn pause_resume_crash_before_queue_change_is_unconfirmed() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let asker = fw.asker().unwrap();
    for i in 0..=crate::macvm::ask_queue::GUARD_MAX {
        assert!(matches!(
            asker.join(&triple(i), helper_who(), None, 1),
            Join::Wait(_, false)
        ));
    }
    let notice = broker::pending(&env)
        .into_iter()
        .find(|p| p.connect.as_ref().is_some_and(|c| c.notice))
        .unwrap();
    broker::answer_connect(&env, &notice.id, ConnectAnswer::AllowOnce).unwrap();
    crate::macvm::approvals::PAUSE_RECORDED_HOOK
        .with(|h| *h.borrow_mut() = Some(Box::new(|| panic!("synthetic crash before resume"))));
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fw.settle(2)));
    crate::macvm::approvals::PAUSE_RECORDED_HOOK.with(|h| *h.borrow_mut() = None);
    assert!(crash.is_err());
    let report = broker::pending_report(&env).unwrap();
    let outcome = report.outcomes.iter().find(|o| o.id == notice.id).unwrap();
    assert_eq!(outcome.state, broker::LocalApplication::NoticePending);
    assert_eq!(outcome.rule, "pause-notice");
    assert_eq!(outcome.local_allowed, None);
    assert!(report.unresolved.iter().any(|u| u.id == notice.id));
}

#[test]
fn failed_pause_resume_confirmation_is_not_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let asker = fw.asker().unwrap();
    for i in 0..=crate::macvm::ask_queue::GUARD_MAX {
        assert!(matches!(
            asker.join(&triple(i), helper_who(), None, 1),
            Join::Wait(_, false)
        ));
    }
    let notice = broker::pending(&env)
        .into_iter()
        .find(|p| p.connect.as_ref().is_some_and(|c| c.notice))
        .unwrap();
    broker::answer_connect(&env, &notice.id, ConnectAnswer::AllowOnce).unwrap();
    let dir = broker::approvals_dir(&env);
    let prefix = format!("{}.done.paddock-tmp-", notice.id);
    crate::privfs::ATOMIC_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    std::fs::remove_file(entry.path()).unwrap();
                }
            }
        }))
    });
    fw.settle(2);
    crate::privfs::ATOMIC_HOOK.with(|h| *h.borrow_mut() = None);
    assert!(!asker.paused());
    fw.settle(3);
    let report = broker::pending_report(&env).unwrap();
    let outcome = report.outcomes.iter().find(|o| o.id == notice.id).unwrap();
    assert_eq!(outcome.state, broker::LocalApplication::NoticePending);
    assert!(report.unresolved.iter().any(|u| u.id == notice.id));
}

#[test]
fn session_grant_selected_before_a_later_file_deny_wins_at_hold() {
    use crate::eventstream::schema::QuestionState;
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
    let gate = fw.clone().gate();
    let identity = Identity {
        pid: Some(71),
        app: Some("Editor".into()),
        team: Some("T".into()),
        path: Some("/fake/editor".into()),
        parent_app: None,
    };
    let (shown_tx, shown_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        crate::macvm::approvals::EVENT_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |event| {
                if event.state == QuestionState::Open {
                    let _ = shown_tx.send(event.id);
                }
            }))
        });
        (gate.0)("hold.example", 443, Some(&identity))
    });
    let id = shown_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    broker::answer_connect(&env, &id, ConnectAnswer::AllowSession).unwrap();
    let mut opened = worker.join().unwrap().unwrap();
    assert_eq!(fw.rules().session.len(), 1);
    let mut writer = Rules::load_at(tmp.path().join("rules.toml")).unwrap();
    writer
        .add(
            firewall::new_rule(
                "Editor",
                Some("T"),
                "hold.example",
                firewall::Port::Num(443),
                firewall::Verdict::Deny,
                Scope::Always,
                BY_USER,
                2,
            ),
            false,
        )
        .unwrap();
    writer.save().unwrap();
    let (client, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    opened.hold(&client).unwrap();
    assert_eq!(fw.open_count(), 1);
    assert_eq!(fw.rules().session.len(), 1);
    assert_eq!(fw.rules().file.len(), 1);
}

#[test]
fn deny_selection_survives_a_crash_after_poll_with_the_rules_lock_held() {
    use std::os::unix::io::AsRawFd;
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let key = triple(99);
    let q = match fw.asker().unwrap().join(&key, helper_who(), None, 1) {
        Join::Wait(q, false) => q,
        _ => panic!(),
    };
    broker::answer_connect(&env, &q.id, ConnectAnswer::DenyAlways).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(tmp.path().join("rules.toml.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    AFTER_POLL_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(|| panic!("simulated helper crash"))));
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fw.settle(2)));
    AFTER_POLL_HOOK.with(|h| *h.borrow_mut() = None);
    assert!(crash.is_err());
    let dir = broker::approvals_dir(&env);
    assert!(dir.join(format!("{}.json", q.id)).is_file());
    let answer = std::fs::read_to_string(dir.join(format!("{}.answer", q.id))).unwrap();
    assert_eq!(
        ConnectAnswer::parse_file(&answer),
        Some(ConnectAnswer::DenyAlways)
    );
    assert!(!dir.join(format!("{}.done", q.id)).exists());
    let report = broker::pending_report(&env).unwrap();
    assert!(report
        .diagnostics
        .iter()
        .any(|d| d.contains(&q.id) && d.contains("unconfirmed")));
    assert!(q.result().is_none());
    drop(lock);
}

#[test]
fn rule_lock_wait_retains_selection_without_holding_approval_lock() {
    use std::os::unix::io::AsRawFd;
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let q = match fw.asker().unwrap().join(&triple(98), helper_who(), None, 1) {
        Join::Wait(q, false) => q,
        _ => panic!(),
    };
    broker::answer_connect(&env, &q.id, ConnectAnswer::DenyAlways).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(tmp.path().join("rules.toml.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let (polled, observed) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        AFTER_POLL_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                polled.send(()).unwrap();
            }))
        });
        other.settle(2);
    });
    observed.recv().unwrap();
    let dir = broker::approvals_dir(&env);
    assert!(broker::claim_connect(&env, &q.id, false).unwrap().is_some());
    assert!(!dir.join(format!("{}.done", q.id)).exists());
    drop(lock);
    worker.join().unwrap();
    assert_eq!(q.result().unwrap().answer, ConnectAnswer::DenyAlways);
    assert!(dir.join(format!("{}.answer", q.id)).is_file());
    assert!(dir.join(format!("{}.done", q.id)).is_file());
    assert_eq!(fw.rules().file[0].verdict, firewall::Verdict::Deny);
}

#[test]
fn delayed_effects_and_session_snapshots_are_marked_historical() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("logs")).unwrap();
    let profile = tmp.path().join("profile");
    let fw = Arc::new(
        Firewall::new(
            Mode::Ask,
            None,
            Rules::load_at(tmp.path().join("second.toml")).unwrap(),
            None,
            tmp.path().join("logs/connections.jsonl"),
        )
        .with_changes(profile.clone(), tmp.path().into()),
    );
    let key = triple(97);
    let who = helper_who();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first = fw.clone();
    let first_key = key.clone();
    let first_who = who.clone();
    let worker = std::thread::spawn(move || {
        RULE_EFFECTS_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }))
        });
        first.apply(
            ConnectAnswer::AllowSession,
            &first_who,
            &first_key,
            BY_USER,
            1,
        )
    });
    entered_rx.recv().unwrap();
    assert!(fw
        .apply(ConnectAnswer::AllowSession, &who, &key, BY_USER, 2)
        .starts_with("session-"));
    let snapshot: Vec<crate::macvm::firewall::Rule> =
        serde_json::from_slice(&std::fs::read(tmp.path().join("logs/session-rules.json")).unwrap())
            .unwrap();
    assert_eq!(snapshot.len(), 2, "the legacy vector remains readable");
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(tmp.path().join("logs/session-rules.meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(metadata["version"], 1);
    assert_eq!(metadata["complete"], false);
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    let grants = crate::macvm::grants::load(&crate::macvm::session::Profile::new(profile)).unwrap();
    assert_eq!(grants.history_changes.len(), 2);
    assert!(grants
        .history_changes
        .iter()
        .all(|c| c.current_state.as_deref() == Some("unknown") && c.value.contains("historical")));
    assert_eq!(
        fw.rules().session.len(),
        2,
        "both independent session grants are retained"
    );
}

#[test]
fn hook_deny_selection_is_retained_before_rule_application() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    crate::macvm::approvals::TEST_CONNECT_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(|| Some(ConnectAnswer::DenyAlways)));
    });
    HOOK_SELECTION_RECORDED
        .with(|h| *h.borrow_mut() = Some(Box::new(|| panic!("simulated crash"))));
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fw.ask(fw.asker().unwrap(), &triple(96), helper_who(), None, 1)
    }));
    HOOK_SELECTION_RECORDED.with(|h| *h.borrow_mut() = None);
    crate::macvm::approvals::TEST_CONNECT_HOOK.with(|h| *h.borrow_mut() = None);
    assert!(crash.is_err());
    let dir = broker::approvals_dir(&env);
    let answers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "answer"))
        .collect();
    assert_eq!(answers.len(), 1);
    let answer: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&answers[0]).unwrap()).unwrap();
    assert_eq!(answer["reply"], "deny-always");
    assert_eq!(answer["source"], "approver");
    assert_eq!(answer["request"]["connect"]["host"], "host-96.example");
    assert!(!answers[0].with_extension("done").exists());
    assert!(broker::pending_report(&env)
        .unwrap()
        .diagnostics
        .iter()
        .any(|d| d.contains("unconfirmed")));
    assert!(fw.rules().file.is_empty());
}

#[test]
fn unavailable_hook_record_does_not_install_a_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let (env, fw) = fake_firewall(tmp.path());
    let dir = broker::approvals_dir(&env);
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, "synthetic obstruction").unwrap();
    crate::macvm::approvals::TEST_CONNECT_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(|| Some(ConnectAnswer::AllowSession)));
    });
    let result = fw
        .ask(fw.asker().unwrap(), &triple(95), helper_who(), None, 1)
        .unwrap();
    crate::macvm::approvals::TEST_CONNECT_HOOK.with(|h| *h.borrow_mut() = None);
    assert_eq!(result.answer, ConnectAnswer::AllowSession);
    assert!(!result.allowed);
    assert_eq!(result.word, RULE_STALE);
    assert!(fw.rules().session.is_empty());
    assert!(fw.rules().file.is_empty());
}

#[test]
fn fallback_deny_retains_an_independent_same_scope_session_exception() {
    let tmp = tempfile::tempdir().unwrap();
    let (_env, fw) = fake_firewall(tmp.path());
    let key = Triple {
        identity: None,
        app: "Editor".into(),
        host: "api.example.com".into(),
        port: 443,
    };
    let who = helper_who();
    let allow = firewall::new_rule(
        "Editor",
        Some("T1"),
        &key.host,
        firewall::Port::Num(443),
        firewall::Verdict::Allow,
        Scope::Session,
        BY_USER,
        1,
    );
    let mut rules = fw.rules();
    rules.add(allow.clone(), false).unwrap();
    let mut changes = Vec::new();
    assert!(fw
        .fallback_deny(&who, &key, BY_USER, 2, &mut rules, &mut changes)
        .starts_with("session-"));
    assert!(rules.session.contains(&allow));
    assert_eq!(rules.session.len(), 2);
    assert_eq!(
        firewall::winner(&rules.all(), &who.conn(&key))
            .unwrap()
            .verdict,
        firewall::Verdict::Deny
    );
    assert!(changes.iter().all(|(_, action)| *action != "remove"));
}
