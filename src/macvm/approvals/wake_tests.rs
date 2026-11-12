use super::fake_state::{add, fixture};
use super::*;

#[test]
fn wake_retires_real_open_presentation_without_a_retry() {
    let (_tmp, env, a) = fixture();
    let (_, q) = add(&a, "old", 100);
    a.note_wake(110);
    assert_eq!(q.result().unwrap().word, "stale");
    assert!(broker::answer_connect(&env, "old", ConnectAnswer::AllowAlways).is_err());
    assert!(!broker::approvals_dir(&env).join("old.json").exists());
}

#[test]
fn wake_preserves_a_previously_claimed_answer() {
    let (_tmp, env, a) = fixture();
    let (t, q) = add(&a, "old", 100);
    broker::answer_connect_cli(&env, "old", ConnectAnswer::Deny).unwrap();
    let selected = a.poll(101).answers;
    assert_eq!(selected.len(), 1);
    a.note_wake(110);
    assert!(q.result().is_none());
    a.finish(
        &t,
        &q,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: "cli",
            word: "once".into(),
        },
        111,
    );
    assert_eq!(q.result().unwrap().answer, ConnectAnswer::Deny);
}

#[test]
fn wake_keeps_unobserved_selection_as_unknown_without_a_rule() {
    let (_tmp, env, a) = fixture();
    let (_, q) = add(&a, "old", 100);
    broker::answer_connect_cli(&env, "old", ConnectAnswer::AllowAlways).unwrap();
    a.note_wake(110);
    assert!(!q.result().unwrap().allowed);
    assert!(a.poll(111).answers.is_empty());
    let dir = broker::approvals_dir(&env);
    assert!(dir.join("old.answer").is_file());
    let outcome: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("old.done")).unwrap()).unwrap();
    assert_eq!(outcome["selected"], "allow-always");
    assert_eq!(outcome["state"], "unknown");
}

#[test]
fn wake_retries_an_unreadable_presentation_without_accepting_it() {
    let (_tmp, env, a) = fixture();
    let (_, q) = add(&a, "old", 100);
    let path = broker::approvals_dir(&env).join("old.json");
    let valid = std::fs::read(&path).unwrap();
    std::fs::write(&path, "broken").unwrap();
    a.note_wake(110);
    assert!(q.result().is_none());
    std::fs::write(&path, valid).unwrap();
    assert!(a.poll(111).answers.is_empty());
    assert_eq!(q.result().unwrap().word, "stale");
}

#[test]
fn a_prepared_result_with_a_waiter_is_not_retired() {
    let (_tmp, env, a) = fixture();
    let (t, q) = add(&a, "waiting", 100);
    broker::answer_connect_cli(&env, "waiting", ConnectAnswer::Deny).unwrap();
    assert_eq!(a.poll(101).answers.len(), 1);
    a.finish(
        &t,
        &q,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: "cli",
            word: "once".into(),
        },
        102,
    );
    let dir = broker::approvals_dir(&env);
    assert!(dir.join("waiting.done").is_file());
    assert!(!dir.join("waiting.retired").exists());
}

#[test]
fn a_late_once_result_with_no_waiter_can_be_retired() {
    let (_tmp, env, a) = fixture();
    let (t, q) = add(&a, "late", 100);
    assert!(a.timeout(&t, &q, 101).is_none());
    broker::answer_connect_cli(&env, "late", ConnectAnswer::Deny).unwrap();
    assert_eq!(a.poll(102).answers.len(), 1);
    a.finish(
        &t,
        &q,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: "cli",
            word: "once".into(),
        },
        103,
    );
    let dir = broker::approvals_dir(&env);
    assert!(dir.join("late.done").is_file());
    assert!(dir.join("late.retired").is_file());
}

#[test]
fn wake_preserves_an_unobserved_pause_selection_as_unknown() {
    let (_tmp, env, a) = fixture();
    super::fake_state::notice(&a);
    let (_, q) = add(&a, "queued", 101);
    let notice_id = a.lock().notice.clone().unwrap();
    broker::answer_connect_cli(&env, &notice_id, ConnectAnswer::Deny).unwrap();
    a.note_wake(110);
    assert_eq!(q.result().unwrap().word, "stale");
    assert!(!a.paused());
    assert!(broker::approvals_dir(&env)
        .join(format!("{notice_id}.answer"))
        .is_file());
    assert!(!broker::approvals_dir(&env)
        .join(format!("{notice_id}.done"))
        .exists());
}

#[test]
fn the_wake_second_is_treated_as_unordered() {
    let (_tmp, _env, a) = fixture();
    let (t, q) = add(&a, "same-second", 100);
    a.note_wake(100);
    assert_eq!(q.result().unwrap().word, "stale");
    assert!(matches!(a.join(&t, Who::default(), None, 100), Join::Stale));
}

#[test]
fn physical_wake_before_detector_cannot_apply_an_old_allow() {
    let (_tmp, env, a) = fixture();
    let (t, q) = add(&a, "unordered", 100);
    broker::answer_connect_cli(&env, "unordered", ConnectAnswer::AllowAlways).unwrap();
    // Model a physical wake that has not yet reached note_wake. No later
    // wall-clock observation can supply a platform fence retroactively, so
    // the allow closes its question as a once denial without a rule.
    let selected = a.poll(101).answers;
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].unapplied, Some(Unapplied::WakeOrder));
    assert!(q.taken.load(Ordering::SeqCst));
    assert!(!a.claim_decision(&t, &q).claimed);
    assert!(broker::approvals_dir(&env)
        .join("unordered.answer")
        .is_file());
}

#[test]
fn a_result_retires_only_after_the_last_handler_observes_it() {
    let (_tmp, env, a) = fixture();
    let (t, q) = add(&a, "two-waiters", 100);
    q.count.store(2, Ordering::SeqCst);
    broker::answer_connect_cli(&env, "two-waiters", ConnectAnswer::Deny).unwrap();
    assert_eq!(a.poll(101).answers.len(), 1);
    a.finish(
        &t,
        &q,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: "cli",
            word: "once".into(),
        },
        102,
    );
    let retired = broker::approvals_dir(&env).join("two-waiters.retired");
    assert!(!retired.exists());
    a.acknowledge_waiter(&q);
    assert!(!retired.exists());
    a.acknowledge_waiter(&q);
    assert!(retired.is_file());
}

#[test]
fn replaced_ordinary_card_cannot_select_another_question() {
    let (_tmp, env, a) = fixture();
    let (_, q) = add(&a, "original", 100);
    let path = broker::approvals_dir(&env).join("original.json");
    let mut other = super::fake_state::card(&env, "original");
    other.connect.as_mut().unwrap().host = "other.example".into();
    std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    broker::answer_connect_cli(&env, "original", ConnectAnswer::AllowAlways).unwrap();
    assert!(a.poll(101).answers.is_empty());
    assert!(!q.taken.load(Ordering::SeqCst));
    assert!(broker::approvals_dir(&env).join("original.answer").exists());
}

#[test]
fn replaced_ordinary_answer_cannot_be_recorded_as_the_frozen_selection() {
    let (_tmp, env, a) = fixture();
    let (t, q) = add(&a, "changed-answer", 100);
    broker::answer_connect_cli(&env, "changed-answer", ConnectAnswer::Deny).unwrap();
    assert_eq!(a.poll(101).answers.len(), 1);
    let dir = broker::approvals_dir(&env);
    let path = dir.join("changed-answer.answer");
    let mut changed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    changed["source"] = "panel".into();
    std::fs::write(&path, changed.to_string()).unwrap();
    a.finish(
        &t,
        &q,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: "cli",
            word: "once".into(),
        },
        102,
    );
    assert!(!dir.join("changed-answer.done").exists());
    assert!(path.is_file());
    a.acknowledge_waiter(&q);
    assert!(!dir.join("changed-answer.retired").exists());
}
