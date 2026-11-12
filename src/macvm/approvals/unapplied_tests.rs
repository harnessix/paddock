//! Production has no wake fence: every claimed human selection must still
//! close its question, so the queue, the pause and the bound move on.
use super::fake_state::{add, card, fixture, notice, notice_card};
use super::streak::STREAK_SECONDS;
use super::*;

fn denied(answer: ConnectAnswer, who: &'static str, word: &str) -> Settled {
    Settled {
        answer,
        allowed: false,
        who,
        word: word.into(),
    }
}

#[test]
fn production_asker_has_no_wake_fence() {
    let (_tmp, env, _a) = fixture();
    let a = Asker::new(env, "fresh", "Fresh", "/home/demo".into(), 1);
    assert!(!a.wake_order_fenced());
}

#[test]
fn an_unverified_allow_closes_its_question_and_shows_the_next() {
    let (_tmp, env, a) = fixture();
    let (first, q) = add(&a, "first", 100);
    add(&a, "second", 100);
    let dir = broker::approvals_dir(&env);
    assert!(!dir.join("second.json").exists());
    broker::answer_connect(&env, "first", ConnectAnswer::AllowAlways).unwrap();
    let polled = a.poll(101);
    assert_eq!(polled.answers.len(), 1);
    let got = &polled.answers[0];
    assert_eq!(got.unapplied, Some(Unapplied::WakeOrder));
    assert_eq!(got.answer, ConnectAnswer::AllowAlways);
    a.finish(
        &first,
        &q,
        denied(ConnectAnswer::AllowAlways, got.who, broker::UNVERIFIED_RULE),
        101,
    );
    assert!(!q.result().unwrap().allowed);
    assert_eq!(a.waiting(), 1);
    assert_eq!(card(&env, "second").connect.unwrap().host, "second.example");
    let outcome: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("first.done")).unwrap()).unwrap();
    assert_eq!(outcome["state"], "refused");
    assert_eq!(outcome["local_allowed"], false);
    a.acknowledge_waiter(&q);
    assert!(dir.join("first.retired").is_file());
    let unapplied = a.unapplied();
    assert_eq!(unapplied.len(), 1);
    assert_eq!(unapplied[0].target, "first.example:443");
    assert_eq!(unapplied[0].selected, "allow-always");
    assert_eq!(unapplied[0].reason, "unverified");
}

#[test]
fn an_unverified_pause_deny_frees_the_open_question_bound() {
    let (_tmp, env, a) = fixture();
    for i in 0..MAX_OPEN_QUESTIONS {
        add(&a, &format!("q{i}"), 100);
    }
    let extra = Triple {
        app: "app".into(),
        host: "extra.example".into(),
        port: 443,
        identity: None,
    };
    assert!(matches!(
        a.join(&extra, Who::default(), None, 101),
        Join::Stale
    ));
    assert!(a.paused());
    let selected = notice_card(&a, &env);
    broker::answer_connect(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let polled = a.poll(102);
    assert!(!a.paused());
    assert_eq!(polled.answers.len(), MAX_OPEN_QUESTIONS);
    for got in polled.answers {
        a.finish(
            &got.triple,
            &got.question,
            denied(ConnectAnswer::Deny, got.who, "once"),
            103,
        );
    }
    assert_eq!(a.waiting(), 0);
    assert!(matches!(
        a.join(&extra, Who::default(), None, 104),
        Join::Wait(_, false)
    ));
}

#[test]
fn an_unverified_pause_resume_goes_on_one_by_one() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let selected = notice_card(&a, &env);
    broker::answer_connect(&env, &selected.id, ConnectAnswer::AllowOnce).unwrap();
    assert!(a.poll(102).answers.is_empty());
    assert!(!a.paused());
    assert_eq!(card(&env, "first").connect.unwrap().host, "first.example");
}

#[test]
fn a_notice_that_keeps_failing_lets_asking_go_on() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    add(&a, "first", 101);
    let selected = notice_card(&a, &env);
    broker::answer_connect_cli(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let dir = broker::approvals_dir(&env);
    let mut changed = selected.clone();
    changed.connect.as_mut().unwrap().more += 1;
    std::fs::write(
        dir.join(format!("{}.json", selected.id)),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    let last = 102 + u64::from(STREAK_SECONDS) - 1;
    for now in 102..last {
        assert!(a.poll(now).answers.is_empty());
        assert!(a.paused());
    }
    assert!(a.poll(last).answers.is_empty());
    assert!(!a.paused());
    assert_eq!(a.waiting(), 1, "no question was denied");
    assert!(!dir.join(format!("{}.done", selected.id)).exists());
    assert!(dir.join(format!("{}.answer", selected.id)).is_file());
    assert_eq!(a.unapplied()[0].target, "pause notice");
    assert_eq!(card(&env, "first").connect.unwrap().host, "first.example");
}

#[test]
fn the_status_names_an_unapplied_selection() {
    let mut s = crate::macvm::ask_status::AskStatus::default();
    s.unapplied.push(UnappliedAnswer {
        target: "api.example:443".into(),
        selected: "allow-always".into(),
        reason: "unverified".into(),
        result: text::UNAPPLIED_DENIED.into(),
    });
    let block = crate::macvm::status::questions_block(&s);
    assert!(
        block.contains("not applied: allow-always for api.example:443 (unverified), denied once")
    );
}

/// Every waiting connection polls the notice. A failure of less than the
/// streak, seen by many of them at once, must not drop the user's choice.
#[test]
fn many_pollers_of_a_short_failure_keep_the_pause_selection() {
    let (_tmp, env, a) = fixture();
    notice(&a);
    let open: Vec<_> = (0..20).map(|i| add(&a, &format!("q{i}"), 101)).collect();
    let selected = notice_card(&a, &env);
    broker::answer_connect(&env, &selected.id, ConnectAnswer::Deny).unwrap();
    let fail = Arc::new(AtomicBool::new(true));
    let f = fail.clone();
    super::notice::BEFORE_OUTCOME_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            if f.load(Ordering::SeqCst) {
                // The outcome write fails: its card is briefly unreadable.
                let _ = std::fs::rename(
                    broker::approvals_dir(&env).join(format!("{}.json", selected.id)),
                    broker::approvals_dir(&env).join("moved.json"),
                );
            }
        }))
    });
    let dir = broker::approvals_dir(&a.env);
    let id = notice_card_id(&a);
    // Twenty connections and the tick poll within the same second.
    for (t, q) in &open {
        assert!(!a.claim_decision(t, q).claimed);
        restore(&dir, &id);
        assert!(a.poll(102).answers.is_empty());
        restore(&dir, &id);
    }
    assert!(a.paused(), "a failure of one second keeps the selection");
    fail.store(false, Ordering::SeqCst);
    let polled = a.poll(103);
    assert!(!a.paused());
    assert_eq!(polled.answers.len(), open.len(), "Deny all applies");
    assert!(polled
        .answers
        .iter()
        .all(|got| got.answer == ConnectAnswer::Deny));
    assert!(a.unapplied().is_empty());
}

fn notice_card_id(a: &Asker) -> String {
    a.lock().notice.clone().unwrap()
}

fn restore(dir: &std::path::Path, id: &str) {
    let moved = dir.join("moved.json");
    if moved.exists() {
        std::fs::rename(moved, dir.join(format!("{id}.json"))).unwrap();
    }
}

/// A damaged answer on the card on the screen must not keep every later
/// question behind it.
#[test]
fn a_damaged_answer_closes_its_question_after_a_few_seconds() {
    let (_tmp, env, a) = fixture();
    let (first, q) = add(&a, "first", 100);
    add(&a, "second", 100);
    let dir = broker::approvals_dir(&env);
    std::fs::write(dir.join("first.answer"), "garbage").unwrap();
    let last = 101 + u64::from(STREAK_SECONDS) - 1;
    for now in 101..last {
        for _ in 0..20 {
            assert!(a.poll(now).answers.is_empty());
        }
        assert!(!dir.join("second.json").exists(), "still retried at {now}");
    }
    let polled = a.poll(last);
    assert_eq!(polled.answers.len(), 1);
    let got = &polled.answers[0];
    assert_eq!(got.unapplied, Some(Unapplied::Invalid));
    assert_eq!(got.answer, ConnectAnswer::Deny);
    a.finish(
        &first,
        &q,
        denied(ConnectAnswer::Deny, got.who, "invalid"),
        last,
    );
    assert!(!q.result().unwrap().allowed);
    assert_eq!(a.waiting(), 1);
    assert_eq!(card(&env, "second").connect.unwrap().host, "second.example");
    // The damaged answer stays as evidence; the store records no outcome.
    assert_eq!(std::fs::read(dir.join("first.answer")).unwrap(), b"garbage");
    assert!(!dir.join("first.done").exists());
    let unapplied = a.unapplied();
    assert_eq!(unapplied.len(), 1);
    assert_eq!(unapplied[0].target, "first.example:443");
    assert_eq!(unapplied[0].reason, "invalid");
}

/// A card on the screen that another reader closed or deleted must not
/// keep every later question behind it: it closes after the streak.
#[test]
fn a_missing_card_closes_its_question_after_a_few_seconds() {
    let (_tmp, env, a) = fixture();
    let (first, q) = add(&a, "first", 100);
    add(&a, "second", 100);
    let dir = broker::approvals_dir(&env);
    std::fs::remove_file(dir.join("first.json")).unwrap();
    let last = 101 + u64::from(STREAK_SECONDS) - 1;
    for now in 101..last {
        for _ in 0..20 {
            assert!(a.poll(now).answers.is_empty());
        }
        assert!(!dir.join("second.json").exists(), "still retried at {now}");
    }
    let polled = a.poll(last);
    assert_eq!(polled.answers.len(), 1);
    let got = &polled.answers[0];
    assert_eq!(got.triple, first);
    assert_eq!(got.unapplied, Some(Unapplied::Invalid));
    assert_eq!(got.answer, ConnectAnswer::Deny);
    a.finish(
        &first,
        &q,
        denied(ConnectAnswer::Deny, got.who, "invalid"),
        last,
    );
    assert!(!q.result().unwrap().allowed);
    assert_eq!(a.waiting(), 1);
    assert_eq!(card(&env, "second").connect.unwrap().host, "second.example");
}

/// A store that cannot be locked says nothing about the answer:
/// the question on the screen stays open however long the failure lasts,
/// and its answer applies once the store works again.
#[test]
fn a_store_that_cannot_be_locked_never_closes_the_question() {
    let (_tmp, env, a) = fixture();
    let (first, q) = add(&a, "first", 100);
    add(&a, "second", 100);
    let dir = broker::approvals_dir(&env);
    let lock = dir.join(".lock");
    std::fs::remove_file(&lock).ok();
    std::fs::create_dir(&lock).unwrap();
    for now in 101..101 + 3 * u64::from(STREAK_SECONDS) {
        for _ in 0..20 {
            assert!(a.poll(now).answers.is_empty(), "closed at {now}");
        }
    }
    assert!(q.result().is_none());
    assert!(a.unapplied().is_empty());
    assert!(!dir.join("second.json").exists());
    std::fs::remove_dir(&lock).unwrap();
    broker::answer_connect(&env, "first", ConnectAnswer::Deny).unwrap();
    let polled = a.poll(200);
    assert_eq!(polled.answers.len(), 1);
    let got = &polled.answers[0];
    assert_eq!(got.triple, first);
    assert_eq!(got.answer, ConnectAnswer::Deny);
    assert_eq!(got.unapplied, None);
}

/// The bound of open questions is freed by an unverified allow too.
#[test]
fn an_unverified_allow_frees_the_open_question_bound() {
    let (_tmp, env, a) = fixture();
    let mut open = Vec::new();
    for i in 0..MAX_OPEN_QUESTIONS {
        open.push(add(&a, &format!("q{i}"), 100));
    }
    let extra = Triple {
        app: "app".into(),
        host: "extra.example".into(),
        port: 443,
        identity: None,
    };
    assert!(matches!(
        a.join(&extra, Who::default(), None, 100),
        Join::Stale
    ));
    // Ask one by one: the bound stays full, and one card comes up.
    assert!(a.paused());
    let selected = notice_card(&a, &env);
    broker::answer_connect(&env, &selected.id, ConnectAnswer::AllowOnce).unwrap();
    assert!(a.poll(101).answers.is_empty());
    assert!(!a.paused());
    let shown = a.lock().queue.shown().cloned().expect("a card is shown");
    let (t, q) = open.iter().find(|(t, _)| *t == shown).unwrap();
    broker::answer_connect(&env, &q.card_id(), ConnectAnswer::AllowAlways).unwrap();
    let polled = a.poll(102);
    assert_eq!(polled.answers.len(), 1);
    assert_eq!(polled.answers[0].unapplied, Some(Unapplied::WakeOrder));
    a.finish(
        t,
        q,
        denied(ConnectAnswer::AllowAlways, "user", broker::UNVERIFIED_RULE),
        102,
    );
    assert_eq!(a.waiting(), MAX_OPEN_QUESTIONS - 1);
    assert!(matches!(
        a.join(&extra, Who::default(), None, 103),
        Join::Wait(_, false)
    ));
}

/// `paddock approvals answer ID --allow-always` says when the allow will
/// not apply.
#[test]
fn the_cli_notes_an_allow_that_will_not_apply() {
    let (_tmp, env, a) = fixture();
    add(&a, "first", 100);
    let shown = card(&env, "first");
    let note = cli_answer_note(&shown, ConnectAnswer::AllowAlways).expect("a note");
    assert!(note.contains("will not apply"), "{note}");
    assert!(note.contains("paddock app rules fake/fixture add first.example --port 443"));
    assert!(cli_answer_note(&shown, ConnectAnswer::AllowOnce).is_some());
    assert!(cli_answer_note(&shown, ConnectAnswer::Deny).is_none());
    assert!(cli_answer_note(&shown, ConnectAnswer::DenyAlways).is_none());
    // A pause notice applies `allow-once`: it only resumes asking.
    let (_tmp2, env2, b) = fixture();
    notice(&b);
    let paused = notice_card(&b, &env2);
    assert!(cli_answer_note(&paused, ConnectAnswer::AllowOnce).is_none());
    // A session with a wake fence applies the allow.
    let (_tmp3, env3, c) = fixture();
    c.set_wake_fence_for_test(true);
    add(&c, "fenced", 100);
    assert!(cli_answer_note(&card(&env3, "fenced"), ConnectAnswer::AllowAlways).is_none());
}
