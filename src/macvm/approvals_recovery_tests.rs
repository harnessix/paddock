use super::*;
use crate::macvm::connlog;
use crate::macvm::firewall::{Mode, Rules};
use crate::macvm::gate::Firewall;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn triple(index: usize) -> Triple {
    Triple {
        identity: None,
        app: "test.app".into(),
        host: format!("host-{index}.example"),
        port: 443,
    }
}

#[test]
fn pause_is_announced_after_storage_recovers_and_count_updates_follow_the_queue() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let asker = Asker::new(
        env.clone(),
        "test",
        "Test",
        t.path().into(),
        std::process::id(),
    );
    let rules = Rules::load_at(t.path().join("rules.toml")).unwrap();
    let fw = Firewall::new(
        Mode::Ask,
        None,
        rules,
        Some(asker),
        t.path().join(connlog::FILE),
    );
    let a = fw.asker().unwrap();
    let events = Rc::new(RefCell::new(Vec::<EventQuestion>::new()));
    let sink = events.clone();
    EVENT_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move |e| sink.borrow_mut().push(e))));

    let first_key = triple(0);
    let first = match a.join(&first_key, Who::default(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    for i in 1..crate::macvm::ask_queue::GUARD_MAX {
        a.join(&triple(i), Who::default(), None, 100);
    }
    let failing = Rc::new(Cell::new(true));
    let fail_notice = failing.clone();
    WRITE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |p| {
            if fail_notice.get() && p.connect.as_ref().is_some_and(|c| c.notice) {
                Err(std::io::Error::other("synthetic notice write failure"))
            } else {
                Ok(())
            }
        }))
    });
    a.join(
        &triple(crate::macvm::ask_queue::GUARD_MAX),
        Who::default(),
        None,
        100,
    );
    fw.settle(101);
    assert!(
        broker::pending(&env).is_empty(),
        "a snapshot during failure has no notice"
    );
    assert!(events
        .borrow()
        .iter()
        .all(|e| e.state != QuestionState::Paused));
    assert!(crate::macvm::ask_status::read(&fw.status_path()).paused);

    failing.set(false);
    fw.settle(102);
    let notice = broker::pending(&env).into_iter().next().unwrap();
    assert!(notice.connect.as_ref().unwrap().notice);
    let count = crate::macvm::ask_queue::GUARD_MAX as u64 + 1;
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|e| e.state == QuestionState::Paused)
            .map(|e| (e.id.clone(), e.open))
            .collect::<Vec<_>>(),
        vec![(notice.id.clone(), Some(count))]
    );

    a.join(&triple(100), Who::default(), None, 103);
    let claim = a.claim_decision(&first_key, &first);
    assert!(claim.claimed);
    a.finish(
        &first_key,
        &first,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: BY_RULE,
            word: "rule".into(),
        },
        103,
    );
    let counts: Vec<_> = events
        .borrow()
        .iter()
        .filter(|e| e.state == QuestionState::Paused)
        .map(|e| e.open.unwrap())
        .collect();
    assert_eq!(counts, vec![count, count + 1, count]);
    // Each changed target set is a new presentation with its own id.
    let current = broker::pending(&env).into_iter().next().unwrap();
    assert_eq!(current.connect.as_ref().unwrap().more, count);
    assert_ne!(current.id, notice.id);

    // Resume can change the queue even with no Answered entry. A failed
    // card publication is not shown in status; a later retry is persisted.
    broker::answer_connect(&env, &current.id, ConnectAnswer::AllowOnce).unwrap();
    WRITE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|p| {
            if p.connect.as_ref().is_some_and(|c| !c.notice) {
                Err(std::io::Error::other("synthetic question write failure"))
            } else {
                Ok(())
            }
        }))
    });
    fw.settle(104);
    let status = crate::macvm::ask_status::read(&fw.status_path());
    assert!(!status.paused);
    assert!(status.open.iter().all(|q| !q.shown));
    WRITE_HOOK.with(|hook| *hook.borrow_mut() = None);
    fw.settle(105);
    let status = crate::macvm::ask_status::read(&fw.status_path());
    assert!(!status.paused);
    assert_eq!(status.open.iter().filter(|q| q.shown).count(), 1);
    EVENT_HOOK.with(|hook| *hook.borrow_mut() = None);
}

#[test]
fn published_question_and_notice_rewrites_retry_without_queue_changes() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let a = Asker::new(
        env.clone(),
        "test",
        "Test",
        t.path().into(),
        std::process::id(),
    );
    let first = triple(0);
    a.join(&first, Who::default(), None, 100);
    assert_eq!(broker::pending(&env)[0].connect.as_ref().unwrap().count, 1);
    let fail = Rc::new(Cell::new(true));
    let fail_write = fail.clone();
    WRITE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |_| {
            if fail_write.get() {
                Err(std::io::Error::other("persistent rewrite failure"))
            } else {
                Ok(())
            }
        }))
    });
    a.join(&first, Who::default(), None, 101);
    a.poll(102);
    assert_eq!(broker::pending(&env)[0].connect.as_ref().unwrap().count, 1);
    fail.set(false);
    a.poll(103);
    assert_eq!(broker::pending(&env)[0].connect.as_ref().unwrap().count, 2);

    for i in 1..=crate::macvm::ask_queue::GUARD_MAX {
        a.join(&triple(i), Who::default(), None, 104);
    }
    let notice = broker::pending(&env).into_iter().next().unwrap();
    assert!(notice.connect.as_ref().unwrap().notice);
    let initial = notice.connect.unwrap().more;
    fail.set(true);
    a.join(&triple(100), Who::default(), None, 105);
    a.poll(106);
    // The old target set is withdrawn before its successor is written, so
    // a failed write leaves no notice with a stale count on the screen.
    assert!(broker::pending(&env).is_empty());
    assert!(a.paused());
    fail.set(false);
    a.poll(107);
    let current = broker::pending(&env).into_iter().next().unwrap();
    assert_ne!(current.id, notice.id);
    assert_eq!(current.connect.as_ref().unwrap().more, initial + 1);
    WRITE_HOOK.with(|hook| *hook.borrow_mut() = None);
}

#[test]
fn failed_revalidation_withdraws_before_opening_a_new_presentation() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let a = Asker::new(
        env.clone(),
        "test",
        "Test",
        t.path().into(),
        std::process::id(),
    );
    let events = Rc::new(RefCell::new(Vec::<EventQuestion>::new()));
    let sink = events.clone();
    EVENT_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move |e| sink.borrow_mut().push(e))));
    let key = triple(20);
    let q = match a.join(&key, Who::default(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let old = broker::pending(&env)[0].id.clone();
    let (claim, decision) = a.claim_decision_if(&key, &q, || None::<()>);
    assert!(!claim.claimed);
    assert!(decision.is_none());
    assert!(broker::answer_connect(&env, &old, ConnectAnswer::Deny).is_err());
    a.poll(101);
    let new = broker::pending(&env)[0].id.clone();
    assert_ne!(old, new);
    let sequence: Vec<_> = events
        .borrow()
        .iter()
        .map(|event| (event.state, event.id.clone()))
        .collect();
    assert_eq!(
        sequence,
        vec![
            (QuestionState::Open, old.clone()),
            (QuestionState::Withdrawn, old),
            (QuestionState::Open, new.clone()),
        ]
    );
    broker::answer_connect(&env, &new, ConnectAnswer::Deny).unwrap();
    EVENT_HOOK.with(|hook| *hook.borrow_mut() = None);
}
