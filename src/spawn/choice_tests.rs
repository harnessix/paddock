use super::*;

#[test]
fn a_card_with_choices_takes_only_its_choices() {
    let s = setup();
    let p = Pending {
        id: "lim-1".into(),
        sandbox: "codex-spawn-1".into(),
        harness: "codex".into(),
        project: s.project.clone(),
        capability: crate::broker::TIMEOUT_CAPABILITY.into(),
        summary: "s".into(),
        detail: "d".into(),
        created: state::now(),
        expires: state::now() + 60,
        session_allowed: false,
        started_by: vec!["p-1".into()],
        choices: vec!["15m".into(), "30m".into()],
        ..Default::default()
    };
    let (p_again, p_last) = (p.clone(), p.clone());
    let validation = Pending {
        id: "lim-validation".into(),
        ..p.clone()
    };
    crate::broker::write_pending(&s.env, &validation).unwrap();
    assert!(crate::broker::answer_reply(&s.env, &validation.id, "2h").is_err());
    assert!(crate::broker::answer(&s.env, &validation.id, Answer::Once).is_err());
    assert_eq!(
        crate::broker::close_pending(&s.env, &validation.id).unwrap(),
        None
    );
    let s2 = s.env.clone();
    let t = std::thread::spawn(move || {
        crate::broker::ask_choice(&s2, &p, Duration::from_secs(10), &|| false)
    });
    std::thread::sleep(Duration::from_millis(300));
    crate::broker::answer_reply(&s.env, "lim-1", "30m").unwrap();
    assert_eq!(t.join().unwrap(), Choice::More("30m".into()));
    assert!(crate::broker::pending(&s.env).is_empty());
    // Deny stops the child.
    let s2 = s.env.clone();
    let p2 = Pending {
        id: "lim-2".into(),
        ..p_again
    };
    let t = std::thread::spawn(move || {
        crate::broker::ask_choice(&s2, &p2, Duration::from_secs(10), &|| false)
    });
    std::thread::sleep(Duration::from_millis(300));
    crate::broker::answer(&s.env, "lim-2", Answer::Deny).unwrap();
    assert_eq!(t.join().unwrap(), Choice::Stop);
    // Without an answer, the card goes away after the wait.
    let p3 = Pending {
        id: "lim-3".into(),
        ..p_last
    };
    assert_eq!(
        crate::broker::ask_choice(&s.env, &p3, Duration::from_millis(300), &|| false),
        Choice::NoAnswer
    );
    assert!(crate::broker::pending(&s.env).is_empty());
}
