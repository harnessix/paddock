use super::*;

fn ctx(dir: &std::path::Path) -> DeviceCtx {
    // No session bus: no desktop notification from a test.
    let (c, _) = crate::devices::user::tests::ctx(dir, crate::broker::Answer::Deny);
    DeviceCtx {
        sandbox: "asker".into(),
        approver: std::sync::Arc::new(crate::broker::DenyAll),
        ..c
    }
}

fn errno(r: Result<Vec<u8>, DevError>) -> i32 {
    r.unwrap_err().errno()
}

#[test]
fn wait_returns_at_the_time_and_reports_still_waiting() {
    let now = crate::state::now();
    let call = Call::for_test("s", Duration::from_secs(60));
    // A time in the past returns at once.
    let past = waiting::format_rfc3339(now - 5);
    let out = Wait.call(&call, past.as_bytes()).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), format!("reached {past}\n"));
    // Two seconds ahead, with a longer read timeout: it waits for it.
    let at = waiting::format_rfc3339(now + 2);
    let start = Instant::now();
    let req = format!("{{\"until\": \"{at}\", \"timeout\": 10}}");
    let out = Wait.call(&call, req.as_bytes()).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(900));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(String::from_utf8(out).unwrap(), format!("reached {at}\n"));
    // An hour ahead with a read timeout of one second: still waiting.
    let start = Instant::now();
    let r = Wait.call(&call, br#"{"until": "1h", "timeout": 1}"#);
    assert_eq!(errno(r), libc::ETIME);
    assert!(start.elapsed() < Duration::from_secs(3));
    // Wrong requests.
    for bad in [
        &b"soon"[..],
        b"",
        b"9d",
        br#"{"until": "1h", "timeout": 0}"#,
        br#"{"until": "1h", "timeout": 601}"#,
        br#"{"until": "1h", "sandbox": "other"}"#,
        b"\xff",
    ] {
        assert_eq!(errno(Wait.call(&call, bad)), libc::EINVAL, "{bad:?}");
    }
}

#[test]
fn ask_puts_the_question_on_the_card_and_returns_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = std::sync::Arc::new(Ask::new(c));
    let call = Call::for_test("asker", Duration::from_secs(60));
    let req = br#"{"question": "Deploy to staging?", "choices": ["yes", "no"], "timeout": 1}"#;
    // Nobody answers: still waiting, and the card stays.
    assert_eq!(errno(ask.call(&call, req)), libc::ETIME);
    let p = broker::pending(&env);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].capability, "ask");
    assert_eq!(p[0].detail, "Deploy to staging?");
    assert_eq!(p[0].choices, ["yes", "no"]);
    assert!(!p[0].session_allowed);
    let id = p[0].id.clone();
    // The same question again finds the same card.
    assert_eq!(errno(ask.call(&call, req)), libc::ETIME);
    assert_eq!(broker::pending(&env).len(), 1);
    // A host answer word cannot answer a question; a text can.
    assert!(broker::answer(&env, &id, broker::Answer::Once).is_err());
    let a2 = ask.clone();
    let t = std::thread::spawn(move || {
        let call = Call::for_test("asker", Duration::from_secs(60));
        a2.call(
            &call,
            br#"{"question": "Deploy to staging?", "choices": ["yes", "no"], "timeout": 10}"#,
        )
    });
    std::thread::sleep(Duration::from_millis(300));
    broker::answer_reply(&env, &id, "yes, but only the API").unwrap();
    let out = t.join().unwrap().unwrap();
    assert_eq!(out, b"yes, but only the API");
    assert!(broker::pending(&env).is_empty());
    // The audit log has the sizes, not the text.
    let log = std::fs::read_to_string(broker::audit_file(&env)).unwrap();
    assert!(log.contains("\"capability\":\"ask\""));
    assert!(log.contains("an answer of 21 characters"));
    assert!(!log.contains("staging"));
    assert!(!log.contains("only the API"));
}

#[test]
fn a_dismissed_question_fails_with_eacces_and_wrong_questions_are_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = std::sync::Arc::new(Ask::new(c));
    let a2 = ask.clone();
    let t = std::thread::spawn(move || {
        let call = Call::for_test("asker", Duration::from_secs(60));
        a2.call(&call, b"Can I delete the old branches?")
    });
    let start = Instant::now();
    while broker::pending(&env).is_empty() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let id = broker::pending(&env)[0].id.clone();
    // A question cannot take a host request's text answer path the
    // other way round either.
    broker::answer(&env, &id, broker::Answer::Deny).unwrap();
    assert_eq!(errno(t.join().unwrap()), libc::EACCES);
    assert!(broker::pending(&env).is_empty());
    let call = Call::for_test("asker", Duration::from_secs(60));
    let long = "x".repeat(MAX_QUESTION + 1);
    let many: Vec<String> = (0..MAX_CHOICES + 1).map(|i| format!("c{i}")).collect();
    for bad in [
        String::new(),
        "   ".into(),
        long,
        serde_json::json!({ "question": "q", "choices": many }).to_string(),
        serde_json::json!({ "question": "q", "choices": ["a\nb"] }).to_string(),
        serde_json::json!({ "question": "q", "timeout": 9999 }).to_string(),
    ] {
        assert_eq!(errno(ask.call(&call, bad.as_bytes())), libc::EINVAL);
    }
    assert!(broker::pending(&env).is_empty());
}

#[test]
fn the_end_of_the_sandbox_withdraws_its_questions() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = Ask::new(c);
    let call = Call::for_test("asker", Duration::from_secs(60));
    assert_eq!(
        errno(ask.call(&call, br#"{"question": "q?", "timeout": 1}"#)),
        libc::ETIME
    );
    assert_eq!(broker::pending(&env).len(), 1);
    drop(ask);
    assert!(broker::pending(&env).is_empty());
}

fn wait_for_card(env: &crate::env::HostEnv) -> String {
    let start = Instant::now();
    while broker::pending(env).is_empty() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    broker::pending(env)[0].id.clone()
}

#[test]
fn every_read_of_one_question_gets_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = std::sync::Arc::new(Ask::new(c));
    let reads: Vec<_> = (0..2)
        .map(|_| {
            let a = ask.clone();
            std::thread::spawn(move || {
                let call = Call::for_test("asker", Duration::from_secs(60));
                a.call(&call, br#"{"question": "Which port?", "timeout": 10}"#)
            })
        })
        .collect();
    let id = wait_for_card(&env);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(broker::pending(&env).len(), 1);
    broker::answer_reply(&env, &id, "-p 8080").unwrap();
    for r in reads {
        assert_eq!(r.join().unwrap().unwrap(), b"-p 8080");
    }
    assert!(broker::pending(&env).is_empty());
}

#[test]
fn after_a_dismissal_a_new_question_waits() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = std::sync::Arc::new(Ask::new(c));
    let a = ask.clone();
    let t = std::thread::spawn(move || {
        let call = Call::for_test("asker", Duration::from_secs(60));
        a.call(&call, b"May I?")
    });
    let id = wait_for_card(&env);
    broker::answer(&env, &id, broker::Answer::Deny).unwrap();
    assert_eq!(errno(t.join().unwrap()), libc::EACCES);
    // A new question at once is declined without a new card.
    let call = Call::for_test("asker", Duration::from_secs(60));
    assert_eq!(
        errno(ask.call(&call, br#"{"question": "And now?", "timeout": 1}"#)),
        libc::EACCES
    );
    assert!(broker::pending(&env).is_empty());
}

#[test]
fn control_and_bidi_characters_never_reach_the_card() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = Ask::new(c);
    let call = Call::for_test("asker", Duration::from_secs(60));
    let q = serde_json::json!({
        "question": "Allow\u{1b}[2J\r this\u{202e}gnp.exe\u{200b}?\nLine two",
        "timeout": 1
    });
    assert_eq!(
        errno(ask.call(&call, q.to_string().as_bytes())),
        libc::ETIME
    );
    assert_eq!(
        broker::pending(&env)[0].detail,
        "Allow[2J thisgnp.exe?\nLine two"
    );
    for bad in ["\u{1b}[31mred", "a\u{202e}b"] {
        let r = serde_json::json!({ "question": "q", "choices": [bad] });
        assert_eq!(
            errno(ask.call(&call, r.to_string().as_bytes())),
            libc::EINVAL
        );
    }
}

#[test]
fn questions_of_all_sandboxes_are_limited() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    for i in 0..MAX_ALL_QUESTIONS {
        broker::write_pending(
            &env,
            &broker::Pending {
                id: format!("r-other-{i}"),
                sandbox: format!("other-{i}"),
                harness: "fake".into(),
                project: dir.path().into(),
                capability: broker::ASK_CAPABILITY.into(),
                summary: String::new(),
                detail: "q".into(),
                created: crate::state::now(),
                expires: 0,
                session_allowed: false,
                ..Default::default()
            },
        )
        .unwrap();
    }
    let ask = Ask::new(c);
    let call = Call::for_test("asker", Duration::from_secs(60));
    assert_eq!(
        errno(ask.call(&call, br#"{"question": "One more?", "timeout": 1}"#)),
        libc::EIO
    );
    assert_eq!(broker::pending(&env).len(), MAX_ALL_QUESTIONS);
}

#[test]
fn an_abandoned_question_keeps_an_accepted_answer_for_the_next_reader() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = Ask::new(c);
    let p = broker::Pending {
        id: "r-abandoned".into(),
        sandbox: "asker".into(),
        capability: broker::ASK_CAPABILITY.into(),
        ..Default::default()
    };
    broker::write_pending(&env, &p).unwrap();
    broker::answer_reply(&env, &p.id, "accepted").unwrap();
    let mut open = HashMap::new();
    open.insert(
        "key".into(),
        Open {
            id: p.id.clone(),
            chars: 1,
            choices: 0,
            readers: 0,
            left: Instant::now(),
            note: None,
            done: None,
        },
    );
    ask.sweep_with(&mut open, |_| true);
    assert!(open.contains_key("key"));
    assert!(!broker::approvals_dir(&env)
        .join("r-abandoned.answer")
        .exists());
    assert!(
        matches!(open["key"].done.as_ref(), Some((Reply::Text(text), _)) if text == "accepted")
    );
}

#[test]
fn an_abandoned_question_is_retained_after_lock_failure_and_retried() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    let ask = Ask::new(c);
    let store = broker::approvals_dir(&env);
    crate::privfs::dir(&store).unwrap();
    let p = broker::Pending {
        id: "r-abandoned".into(),
        sandbox: "asker".into(),
        capability: broker::ASK_CAPABILITY.into(),
        ..Default::default()
    };
    crate::privfs::write_atomic(
        &store.join("r-abandoned.json"),
        serde_json::to_string(&p).unwrap(),
    )
    .unwrap();
    std::fs::create_dir(store.join(".lock")).unwrap();
    let mut open = HashMap::new();
    open.insert(
        "key".into(),
        Open {
            id: p.id.clone(),
            chars: 1,
            choices: 0,
            readers: 0,
            left: Instant::now(),
            note: None,
            done: None,
        },
    );
    ask.sweep_with(&mut open, |_| true);
    assert!(open.contains_key("key"));
    assert!(store.join("r-abandoned.json").is_file());
    std::fs::remove_dir(store.join(".lock")).unwrap();
    ask.sweep_with(&mut open, |_| true);
    assert!(!open.contains_key("key"));
    assert!(!store.join("r-abandoned.json").exists());
    assert!(store.join(".lock").is_file());
}

#[test]
fn answered_timed_out_reads_free_both_quotas_and_a_reader_gets_its_reply() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    crate::privfs::write(
        &broker::approval_ui_file(&env),
        crate::state::now().to_string(),
    )
    .unwrap();
    let first = Ask::new(c.clone());
    let second = Ask::new(DeviceCtx {
        sandbox: "other".into(),
        ..c
    });
    let devices = [&first, &second];
    // Answers arrive only after all the synthetic reads have timed out.
    EXPIRE_READ.with(|expire| expire.set(true));
    for (i, device) in devices.iter().enumerate() {
        for j in 0..MAX_OPEN_QUESTIONS {
            let call = Call::for_test("asker", Duration::from_secs(60));
            let request = format!("question-{i}-{j}");
            assert_eq!(errno(device.call(&call, request.as_bytes())), libc::ETIME);
        }
    }
    let cards = broker::pending(&env);
    assert_eq!(cards.len(), MAX_ALL_QUESTIONS);
    for card in &cards {
        broker::answer_reply(&env, &card.id, &format!("reply for {}", card.detail)).unwrap();
    }
    assert!(
        broker::pending(&env).is_empty(),
        "answered cards are not visible"
    );
    for (i, device) in devices.iter().enumerate() {
        let call = Call::for_test("asker", Duration::from_secs(60));
        assert_eq!(
            errno(device.call(&call, format!("new-{i}").as_bytes())),
            libc::ETIME
        );
    }
    assert_eq!(broker::pending(&env).len(), 2);
    let call = Call::for_test("asker", Duration::from_secs(60));
    assert_eq!(
        first.call(&call, b"question-0-0").unwrap(),
        b"reply for question-0-0"
    );
    drop(first);
    drop(second);
    EXPIRE_READ.with(|expire| expire.set(false));
    assert!(broker::pending(&env).is_empty());
    for card in cards {
        for ext in ["json", "answer", "owner"] {
            assert!(!broker::approvals_dir(&env)
                .join(format!("{}.{ext}", card.id))
                .exists());
        }
    }
}

#[test]
fn retained_answers_are_bounded_without_discarding_existing_replies() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    crate::privfs::write(
        &broker::approval_ui_file(&env),
        crate::state::now().to_string(),
    )
    .unwrap();
    let ask = Ask::new(c);
    EXPIRE_READ.with(|expire| expire.set(true));
    let call = Call::for_test("asker", Duration::from_secs(60));
    for i in 0..MAX_RETAINED_QUESTIONS {
        let question = format!("retained-{i}");
        assert_eq!(errno(ask.call(&call, question.as_bytes())), libc::ETIME);
        let card = broker::pending(&env).into_iter().next().unwrap();
        broker::answer_reply(&env, &card.id, &format!("answer-{i}")).unwrap();
    }
    assert_eq!(errno(ask.call(&call, b"one too many")), libc::EIO);
    assert_eq!(ask.call(&call, b"retained-0").unwrap(), b"answer-0");
    assert_eq!(errno(ask.call(&call, b"space is free")), libc::ETIME);
    EXPIRE_READ.with(|expire| expire.set(false));
}

#[test]
fn drop_removes_an_accepted_reply_with_no_reader() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let env = c.env.clone();
    crate::privfs::write(
        &broker::approval_ui_file(&env),
        crate::state::now().to_string(),
    )
    .unwrap();
    let ask = Ask::new(c);
    EXPIRE_READ.with(|expire| expire.set(true));
    let call = Call::for_test("asker", Duration::from_secs(60));
    assert_eq!(errno(ask.call(&call, b"last question")), libc::ETIME);
    let id = broker::pending(&env)[0].id.clone();
    broker::answer_reply(&env, &id, "accepted").unwrap();
    drop(ask);
    EXPIRE_READ.with(|expire| expire.set(false));
    for ext in ["json", "answer", "owner"] {
        assert!(!broker::approvals_dir(&env)
            .join(format!("{id}.{ext}"))
            .exists());
    }
}
