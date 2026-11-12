use super::*;
use crate::broker;
use std::sync::mpsc;

fn firewall(t: &std::path::Path, mode: Mode) -> (crate::env::HostEnv, Arc<Firewall>) {
    let env = crate::privfs::tests::env_in(t);
    let asker = Asker::new(env.clone(), "test", "Test", t.into(), std::process::id());
    let rules = Rules::load_at(t.join("rules.toml")).unwrap();
    let fw = Firewall::new(
        mode,
        None,
        rules,
        Some(asker),
        t.join("logs").join(connlog::FILE),
    );
    (env, Arc::new(fw))
}

fn triple(index: usize) -> Triple {
    Triple {
        identity: None,
        app: "test.app".into(),
        host: format!("host-{index}.example"),
        port: 443,
    }
}

#[path = "gate_recovery_tests/recheck.rs"]
mod recheck;
#[path = "gate_recovery_tests/unapplied.rs"]
mod unapplied;

#[test]
fn an_older_mode_change_waiting_on_rules_cannot_undo_none() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Egress);
    let (started_tx, started_rx) = mpsc::channel();
    let (read_tx, read_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let other = fw.clone();
    let worker = std::thread::spawn(move || {
        MODE_STARTED_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                started_tx.send(()).unwrap();
            }));
        });
        let first = std::cell::Cell::new(true);
        firewall::READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                if first.replace(false) {
                    read_tx.send(()).unwrap();
                    go_rx.recv().unwrap();
                }
            }));
        });
        other.set_mode(Mode::Ask)
    });
    started_rx.recv().unwrap();
    read_rx.recv().unwrap();
    assert_eq!(fw.set_mode(Mode::None), 0);
    go_tx.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), 0);
    assert_eq!(fw.mode(), Mode::None);
}

#[test]
fn a_later_mode_change_waiting_for_rules_does_not_cancel_none() {
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Egress);
    let gate = fw.clone().gate();
    let (client, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut opened = (gate.0)("open.example", 443, None).unwrap();
    opened.hold(&client).unwrap();
    let rules = fw.rules();
    let mode = fw.mode.write().unwrap();
    let (none_started_tx, none_started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let other = fw.clone();
    let switch = std::thread::spawn(move || {
        MODE_STARTED_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                none_started_tx.send(()).unwrap();
            }));
        });
        done_tx.send(other.set_mode(Mode::None)).unwrap();
    });
    none_started_rx.recv().unwrap();
    let (ask_started_tx, ask_started_rx) = mpsc::channel();
    let other = fw.clone();
    let later = std::thread::spawn(move || {
        MODE_STARTED_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ask_started_tx.send(()).unwrap();
            }));
        });
        other.set_mode(Mode::Ask)
    });
    ask_started_rx.recv().unwrap();
    drop(mode);
    let result = done_rx.recv_timeout(Duration::from_secs(2));
    drop(rules);
    switch.join().unwrap();
    later.join().unwrap();
    assert_eq!(result.unwrap(), 1);
    assert_eq!(fw.open_count(), 0);
}

#[test]
fn broad_queued_and_hook_allows_do_not_replace_overlapping_denies() {
    for (answer, denied_host) in [
        (ConnectAnswer::AllowSite, "example.com"),
        (ConnectAnswer::AllowSite, "*.example.com"),
        (ConnectAnswer::AllowEverything, "*"),
    ] {
        for hook in [false, true] {
            let t = tempfile::tempdir().unwrap();
            let (env, fw) = firewall(t.path(), Mode::Ask);
            fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
            let key = Triple {
                identity: None,
                app: "test.app".into(),
                host: "api.example.com".into(),
                port: 443,
            };
            let who = Who {
                app: key.app.clone(),
                team: Some("T1".into()),
                ..Default::default()
            };
            let independent = firewall::new_rule(
                &key.app,
                Some("T1"),
                "independent.example",
                firewall::Port::Any,
                firewall::Verdict::Allow,
                Scope::Session,
                BY_USER,
                99,
            );
            fw.rules().add(independent.clone(), false).unwrap();
            let write_deny = {
                let path = t.path().join("rules.toml");
                let key = key.clone();
                move || {
                    let mut writer = Rules::load_at(path.clone()).unwrap();
                    writer
                        .add(
                            firewall::new_rule(
                                &key.app,
                                Some("T1"),
                                denied_host,
                                firewall::Port::Any,
                                firewall::Verdict::Deny,
                                Scope::Always,
                                BY_USER,
                                101,
                            ),
                            false,
                        )
                        .unwrap();
                    // For wildcard denies, an exact file allow wins for the
                    // original host, but not for the rest of the new scope.
                    writer
                        .add(
                            firewall::new_rule(
                                &key.app,
                                Some("T1"),
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
                    writer.save().unwrap();
                }
            };
            let result = if hook {
                super::super::approvals::TEST_CONNECT_HOOK.with(|h| {
                    *h.borrow_mut() = Some(Box::new(move || {
                        write_deny();
                        Some(answer)
                    }));
                });
                let result = fw
                    .ask(fw.asker().unwrap(), &key, who.clone(), None, 100)
                    .unwrap();
                super::super::approvals::TEST_CONNECT_HOOK.with(|h| *h.borrow_mut() = None);
                result
            } else {
                let q = match fw.asker().unwrap().join(&key, who.clone(), None, 100) {
                    Join::Wait(q, false) => q,
                    _ => panic!("expected a question"),
                };
                let card = broker::pending(&env).pop().unwrap();
                broker::answer_connect(&env, &card.id, answer).unwrap();
                write_deny();
                fw.settle(102);
                q.result().unwrap()
            };
            assert_eq!(result.answer, answer);
            assert!(!result.allowed);
            assert_eq!(result.word, RULE_STALE);
            let rules = fw.rules();
            assert!(rules.session.contains(&independent));
            assert!(rules
                .file
                .iter()
                .any(|r| r.host == denied_host && r.verdict == firewall::Verdict::Deny));
            assert_eq!(rules.file.len(), 2);
        }
    }
}

#[test]
fn an_unanswered_stalled_store_does_not_hold_rule_locks() {
    use std::os::unix::io::AsRawFd;
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Ask);
    fw.asker()
        .unwrap()
        .join(&triple(0), Who::default(), None, 100);
    let permit = firewall::new_rule(
        "*",
        None,
        "permitted.example",
        firewall::Port::Num(443),
        firewall::Verdict::Allow,
        Scope::Session,
        BY_USER,
        100,
    );
    fw.rules().add(permit, false).unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker_fw = fw.clone();
    let worker = std::thread::spawn(move || {
        super::super::approvals::POLL_STORE_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }));
        });
        worker_fw.settle(101);
    });
    entered_rx.recv().unwrap();
    assert!(fw.rules.try_lock().is_ok());
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
    assert!(fw.connection("permitted.example", 443, None).is_ok());
    let mut writer = Rules::load_at(t.path().join("rules.toml")).unwrap();
    writer
        .add(
            firewall::new_rule(
                "*",
                None,
                "different.example",
                firewall::Port::Any,
                firewall::Verdict::Deny,
                Scope::Always,
                BY_USER,
                101,
            ),
            false,
        )
        .unwrap();
    writer.save().unwrap();
    release_tx.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn deny_always_falls_back_to_a_session_deny_on_read_or_lock_failure() {
    for fail_lock in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let (env, fw) = firewall(t.path(), Mode::Ask);
        let key = triple(10);
        let who = Who {
            app: key.app.clone(),
            ..Default::default()
        };
        let q = match fw.asker().unwrap().join(&key, who.clone(), None, 100) {
            Join::Wait(q, false) => q,
            _ => panic!("expected a question"),
        };
        let card = broker::pending(&env).pop().unwrap();
        broker::answer_connect(&env, &card.id, ConnectAnswer::DenyAlways).unwrap();
        let broken = if fail_lock {
            t.path().join("rules.toml.lock")
        } else {
            t.path().join("rules.toml")
        };
        if fail_lock {
            std::fs::create_dir(&broken).unwrap();
        } else {
            std::fs::write(&broken, "invalid = [").unwrap();
        }
        fw.settle(101);
        let selected = q.result().unwrap();
        assert_eq!(selected.answer, ConnectAnswer::DenyAlways);
        assert!(!selected.allowed);
        assert!(selected.word.starts_with("session-"));
        let report = broker::pending_report(&env).unwrap();
        let outcome = report.outcomes.iter().find(|o| o.id == card.id).unwrap();
        assert_eq!(outcome.scope, broker::ApplicationScope::Session);
        assert!(outcome.reason.as_deref().unwrap().contains("fallback"));
        assert_eq!(outcome.request.connect.as_ref().unwrap().host, key.host);
        assert!(fw
            .rules()
            .session
            .iter()
            .any(|r| r.id == selected.word && r.verdict == firewall::Verdict::Deny));
        if fail_lock {
            std::fs::remove_dir(&broken).unwrap();
        } else {
            std::fs::remove_file(&broken).unwrap();
        }
        let id = Identity {
            app: Some(key.app.clone()),
            ..Default::default()
        };
        assert_eq!(
            fw.connection(&key.host, key.port, Some(&id)).unwrap_err(),
            selected.word
        );
    }
}

#[test]
fn broad_grants_do_not_reject_disjoint_persistent_denies() {
    for (answer, denied_host, denied_team) in [
        (ConnectAnswer::AllowSite, "other.example.net", Some("T1")),
        (ConnectAnswer::AllowEverything, "*", Some("T2")),
    ] {
        let t = tempfile::tempdir().unwrap();
        let (_env, fw) = firewall(t.path(), Mode::Ask);
        let key = Triple {
            identity: None,
            app: "test.app".into(),
            host: "api.example.com".into(),
            port: 443,
        };
        let who = Who {
            app: key.app.clone(),
            team: Some("T1".into()),
            ..Default::default()
        };
        let mut writer = Rules::load_at(t.path().join("rules.toml")).unwrap();
        let deny = firewall::new_rule(
            &key.app,
            denied_team,
            denied_host,
            firewall::Port::Any,
            firewall::Verdict::Deny,
            Scope::Always,
            BY_USER,
            100,
        );
        writer.add(deny.clone(), false).unwrap();
        writer.save().unwrap();
        let applied = fw.apply_with_bus(answer, &who, &key, BY_USER, 101, None);
        assert_ne!(applied, RULE_STALE);
        assert!(fw.rules().file.contains(&deny));
    }
}

#[test]
fn host_scope_overlap_handles_wildcards_and_disjoint_sites() {
    for (a, b, overlap) in [
        ("example.com", "*.example.com", false),
        ("api.example.com", "*.example.com", true),
        ("*.api.example.com", "*.example.com", true),
        ("*.example.com", "*.other.example", false),
        ("*", "outside.example", true),
    ] {
        assert_eq!(firewall::hosts_overlap(a, b), overlap, "{a} versus {b}");
        assert_eq!(firewall::hosts_overlap(b, a), overlap, "{b} versus {a}");
    }
}

#[test]
fn stalled_rule_effect_delivery_does_not_hold_rule_locks() {
    use std::os::unix::io::AsRawFd;
    let t = tempfile::tempdir().unwrap();
    let (_env, fw) = firewall(t.path(), Mode::Egress);
    let key = triple(11);
    let who = Who {
        app: key.app.clone(),
        ..Default::default()
    };
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker_fw = fw.clone();
    let worker = std::thread::spawn(move || {
        RULE_EFFECTS_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }));
        });
        worker_fw.apply_with_bus(ConnectAnswer::AllowSession, &who, &key, BY_USER, 100, None)
    });
    entered_rx.recv().unwrap();
    assert!(fw.rules.try_lock().is_ok());
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
    assert!(fw.connection("independent.example", 443, None).is_ok());
    let mut writer = Rules::load_at(t.path().join("rules.toml")).unwrap();
    writer
        .add(
            firewall::new_rule(
                "*",
                None,
                "another.example",
                firewall::Port::Any,
                firewall::Verdict::Deny,
                Scope::Always,
                BY_USER,
                101,
            ),
            false,
        )
        .unwrap();
    writer.save().unwrap();
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().starts_with("session-"));
}

#[test]
fn new_allow_everything_does_not_replace_a_session_deny() {
    for hook in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let (env, fw) = firewall(t.path(), Mode::Ask);
        fw.asker().unwrap().set_wake_fence_for_test(true); // Models a platform wake fence.
        let key = Triple {
            identity: None,
            app: "test.app".into(),
            host: "api.example.com".into(),
            port: 443,
        };
        let who = Who {
            app: key.app.clone(),
            ..Default::default()
        };
        let deny = firewall::new_rule(
            &key.app,
            None,
            "*",
            firewall::Port::Any,
            firewall::Verdict::Deny,
            Scope::Session,
            BY_USER,
            100,
        );
        let allow = firewall::new_rule(
            &key.app,
            None,
            &key.host,
            firewall::Port::Num(443),
            firewall::Verdict::Allow,
            Scope::Session,
            BY_USER,
            100,
        );
        let install = {
            let fw = fw.clone();
            let deny = deny.clone();
            let allow = allow.clone();
            move || {
                let mut rules = fw.rules();
                rules.add(deny.clone(), false).unwrap();
                rules.add(allow.clone(), false).unwrap();
            }
        };
        let result = if hook {
            super::super::approvals::TEST_CONNECT_HOOK.with(|h| {
                *h.borrow_mut() = Some(Box::new(move || {
                    install();
                    Some(ConnectAnswer::AllowEverything)
                }));
            });
            let result = fw.ask(fw.asker().unwrap(), &key, who, None, 100).unwrap();
            super::super::approvals::TEST_CONNECT_HOOK.with(|h| *h.borrow_mut() = None);
            result
        } else {
            let q = match fw.asker().unwrap().join(&key, who, None, 100) {
                Join::Wait(q, false) => q,
                _ => panic!("expected a question"),
            };
            let card = broker::pending(&env).pop().unwrap();
            broker::answer_connect(&env, &card.id, ConnectAnswer::AllowEverything).unwrap();
            install();
            fw.settle(101);
            q.result().unwrap()
        };
        assert_eq!(result.word, RULE_STALE);
        assert!(!result.allowed);
        let rules = fw.rules();
        assert!(rules.session.contains(&deny));
        assert!(rules.session.contains(&allow));
        assert_eq!(rules.session.len(), 2);
    }
}

// These tests use synthetic identities and temporary rule and approval stores.
// They never consult a guest process or a real user session.
#[path = "gate_recovery_tests/selection.rs"]
mod selection;
