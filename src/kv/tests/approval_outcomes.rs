use super::*;

#[test]
fn review_e_a_stopped_timer_counts_the_sample_step_it_ran_in() {
    let w = world();
    let dir = w.env.state_dir.join("omarchy/agents/usage");
    std::fs::create_dir_all(&dir).unwrap();
    let usage = |at: u64, pct: f64| {
        std::fs::write(
            dir.join("t.json"),
            serde_json::json!({"limitsFetchedAt": at * 1000, "limits": [
                {"label": "Weekly (7-day)", "percent": pct, "resetsAt": "2030-01-05T00:00:00Z"}]})
            .to_string(),
        )
        .unwrap();
        crate::forage::record(&w.env, "t").unwrap();
    };
    let now = crate::state::now();
    usage(now - 100, 0.40);
    let t = crate::timer::Timer {
        name: "task".into(),
        owner: "user".into(),
        subject: "a".into(),
        started: now - 100,
        budget: 0,
        hard: false,
        quota: Some(crate::timer::Quota {
            agent: "t".into(),
            window: "Weekly (7-day)".into(),
            share: 0.10,
            base: 0.40,
            base_resets: crate::forage::samples(&w.env, "t", "Weekly (7-day)")[0].resets,
            base_at: now - 100,
        }),
        stopped: Some(now - 50),
        announced: false,
        enforced: false,
        extensions: Vec::new(),
    };
    // The account used 2% between now - 100 and now (while the timer ran until now - 50).
    usage(now, 0.42);
    let s = t.status_in(&w.env, now);
    assert!((s.quota.unwrap().used - 0.02).abs() < 1e-9);
}

#[test]
fn review_e_quota_timer_in_short_lived_sandbox_is_kept_in_ended() {
    let w = world();
    let now = crate::state::now();
    let (file, ns) = crate::timer::self_file(&w.env, "b").unwrap();
    let mut st = Store::open(&file).unwrap();
    let t = crate::timer::Timer {
        name: "task".into(),
        owner: "user".into(),
        subject: "b".into(),
        started: now,
        budget: 0,
        hard: false,
        quota: Some(crate::timer::Quota {
            agent: "codex".into(),
            window: "Weekly (7-day)".into(),
            share: 0.10,
            base: 0.40,
            base_resets: 9000,
            base_at: now,
        }),
        stopped: None,
        announced: false,
        enforced: false,
        extensions: Vec::new(),
    };
    crate::timer::save(&mut st, &ns, &t, None, &HOST_W, Q, now).unwrap();
    // b ends after 5 seconds, before any 30s timer_tick runs.
    crate::timer::end_runs(&w.env, "b", now + 5);
    let ended = crate::timer::read_ended(&w.env);
    assert!(
        !ended.is_empty(),
        "quota timer of b was lost from ended runs"
    );
    assert_eq!(ended[0].subject, "b");
    assert_eq!(ended[0].seen, now + 5);
    assert!(ended[0].stopped.is_none());
}

#[derive(Clone, Copy)]
enum FakeApproval {
    Allowed,
    Unavailable,
    Unknown,
    NoAnswer,
    Denied,
}

struct OutcomeOnly(FakeApproval);

impl Approver for OutcomeOnly {
    fn ask(&self, _: &HostEnv, _: &Pending, _: Duration) -> Option<Answer> {
        None
    }
    fn ask_outcome(&self, _: &HostEnv, _: &Pending, _: Duration) -> crate::broker::ApprovalOutcome {
        match self.0 {
            FakeApproval::Allowed => crate::broker::ApprovalOutcome::Answered(Verdict {
                answer: Answer::Once,
                note: None,
                who: "user",
            }),
            FakeApproval::Unavailable => crate::broker::ApprovalOutcome::Unavailable,
            FakeApproval::Unknown => crate::broker::ApprovalOutcome::Unknown,
            FakeApproval::NoAnswer => crate::broker::ApprovalOutcome::NoAnswer,
            FakeApproval::Denied => crate::broker::ApprovalOutcome::Answered(Verdict {
                answer: Answer::Deny,
                note: Some("declined".into()),
                who: "user",
            }),
        }
    }
}

#[test]
fn timer_gate_failures_after_user_allow_have_two_distinct_audits() {
    for (outcome, expected) in [
        (FakeApproval::Denied, "denied"),
        (FakeApproval::NoAnswer, "timed out"),
        (FakeApproval::Unavailable, "approval unavailable"),
        (FakeApproval::Unknown, "approval outcome unknown"),
    ] {
        let w = world();
        let (owner, _) = service(
            &w,
            "a",
            None,
            KvCfg {
                user: Mode::Allow,
                ..KvCfg::default()
            },
            None,
        );
        let start = Request {
            budget: Some(3600),
            ..timer_req("start", Scope::User, "fix")
        };
        assert_eq!(owner.handle(&start, None).status, Status::Done);
        let (mut actor, _) = service(&w, "b", None, KvCfg::default(), None);
        let gates = Arc::new(TwoGates {
            user: FakeApproval::Allowed,
            timer: outcome,
            calls: Mutex::new(Vec::new()),
        });
        actor.approver = gates.clone();
        let before = std::fs::read_to_string(audit_file(&w.env))
            .unwrap()
            .lines()
            .count();
        actor.handle(
            &Request {
                by: Some(600),
                ..timer_req("extend", Scope::User, "fix")
            },
            None,
        );
        assert_eq!(
            &*gates.calls.lock().unwrap(),
            &[
                USER_CAPABILITY.to_string(),
                crate::timer::CAPABILITY.to_string()
            ]
        );
        let text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .skip(before)
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0]["gate"].as_str(), rows[0]["decision"].as_str()),
            (Some("user_scope"), Some("allowed once"))
        );
        assert_eq!(
            (rows[1]["gate"].as_str(), rows[1]["decision"].as_str()),
            (Some("timer_approval"), Some(expected))
        );
    }
}

#[test]
fn unresolved_approvals_remain_distinct_for_user_principal_and_timer() {
    for (outcome, expected, audit) in [
        (
            FakeApproval::Unavailable,
            Status::Failed,
            "approval unavailable",
        ),
        (
            FakeApproval::Unknown,
            Status::Failed,
            "approval outcome unknown",
        ),
        (FakeApproval::NoAnswer, Status::Timeout, "timed out"),
        (FakeApproval::Denied, Status::Denied, "denied"),
    ] {
        let w = world();
        let (mut actor, _) = service(&w, "b", None, KvCfg::default(), None);
        actor.approver = Arc::new(OutcomeOnly(outcome));
        let response = actor.handle(&put(Scope::User, "key", "value"), None);
        assert_eq!(response.status, expected);
        assert!(!actor.user_granted.load(Ordering::SeqCst));
        assert!(actor.user_denied.lock().unwrap().is_none());
        let audit_text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
        assert!(audit_text.contains(audit), "{audit_text}");

        let cfg = KvCfg::parse(PRINCIPALS).unwrap();
        let (mut ci, _) = principal(&w, "ci", None);
        ci.approver = Arc::new(OutcomeOnly(outcome));
        let response = ci.handle(&w.env, &cfg, &put(Scope::Own, "task", "value"));
        assert_eq!(response.status, expected);
        assert!(!w.env.paddock_run().join("kv-principals.json").exists());
        let audit_text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
        assert!(audit_text.contains(audit), "{audit_text}");

        let (owner, _) = service(&w, "a", None, KvCfg::default(), None);
        let start = Request {
            budget: Some(3600),
            ..timer_req("start", Scope::Pasture, "fix")
        };
        assert_eq!(owner.handle(&start, None).status, Status::Done);
        let extend = Request {
            by: Some(600),
            ..timer_req("extend", Scope::Pasture, "fix")
        };
        let response = actor.handle(&extend, None);
        assert_eq!(response.status, expected);
        let audit_text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
        let last: serde_json::Value =
            serde_json::from_str(audit_text.lines().last().unwrap()).unwrap();
        assert_eq!(last["decision"], audit);
        let status = owner.handle(&timer_req("status", Scope::Pasture, "fix"), None);
        assert_eq!(status.result["budget"], 3600);
    }
}

struct TwoGates {
    user: FakeApproval,
    timer: FakeApproval,
    calls: Mutex<Vec<String>>,
}

impl Approver for TwoGates {
    fn ask(&self, _: &HostEnv, _: &Pending, _: Duration) -> Option<Answer> {
        None
    }
    fn ask_outcome(
        &self,
        env: &HostEnv,
        p: &Pending,
        wait: Duration,
    ) -> crate::broker::ApprovalOutcome {
        self.calls.lock().unwrap().push(p.capability.clone());
        let selected = if p.capability == USER_CAPABILITY {
            self.user
        } else {
            self.timer
        };
        OutcomeOnly(selected).ask_outcome(env, p, wait)
    }
}

#[test]
fn timer_audits_identify_the_gate_that_was_actually_asked() {
    for outcome in [
        FakeApproval::Denied,
        FakeApproval::NoAnswer,
        FakeApproval::Unavailable,
        FakeApproval::Unknown,
    ] {
        let w = world();
        let (mut actor, _) = service(&w, "b", None, KvCfg::default(), None);
        let gates = Arc::new(TwoGates {
            user: outcome,
            timer: FakeApproval::Denied,
            calls: Mutex::new(Vec::new()),
        });
        actor.approver = gates.clone();
        let extend = Request {
            by: Some(600),
            ..timer_req("extend", Scope::User, "fix")
        };
        actor.handle(&extend, None);
        assert_eq!(
            &*gates.calls.lock().unwrap(),
            &[USER_CAPABILITY.to_string()]
        );
        let text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["gate"], "user_scope");
        assert_eq!(rows[0]["action"], "extend");
    }

    let w = world();
    let allow = KvCfg {
        user: Mode::Allow,
        ..KvCfg::default()
    };
    let (owner, _) = service(&w, "a", None, allow, None);
    assert_eq!(
        owner
            .handle(
                &Request {
                    budget: Some(3600),
                    ..timer_req("start", Scope::User, "fix")
                },
                None
            )
            .status,
        Status::Done
    );
    let (mut actor, _) = service(&w, "b", None, KvCfg::default(), None);
    // The fixed approver allows the first gate and denies the second.
    struct AllowThenDeny(Mutex<Vec<String>>);
    impl Approver for AllowThenDeny {
        fn ask(&self, _: &HostEnv, _: &Pending, _: Duration) -> Option<Answer> {
            None
        }
        fn ask_outcome(
            &self,
            _: &HostEnv,
            p: &Pending,
            _: Duration,
        ) -> crate::broker::ApprovalOutcome {
            self.0.lock().unwrap().push(p.capability.clone());
            crate::broker::ApprovalOutcome::Answered(Verdict {
                answer: if p.capability == USER_CAPABILITY {
                    Answer::Once
                } else {
                    Answer::Deny
                },
                note: None,
                who: "user",
            })
        }
    }
    let gates = Arc::new(AllowThenDeny(Mutex::new(Vec::new())));
    actor.approver = gates.clone();
    let before = std::fs::read_to_string(audit_file(&w.env))
        .unwrap()
        .lines()
        .count();
    assert_eq!(
        actor
            .handle(
                &Request {
                    by: Some(600),
                    ..timer_req("extend", Scope::User, "fix")
                },
                None
            )
            .status,
        Status::Denied
    );
    assert_eq!(
        &*gates.0.lock().unwrap(),
        &[
            USER_CAPABILITY.to_string(),
            crate::timer::CAPABILITY.to_string()
        ]
    );
    let text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .skip(before)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0]["gate"].as_str(), rows[0]["decision"].as_str()),
        (Some("user_scope"), Some("allowed once"))
    );
    assert_eq!(
        (rows[1]["gate"].as_str(), rows[1]["decision"].as_str()),
        (Some("timer_approval"), Some("denied"))
    );

    let (child, _) = service(&w, "child", Some("root"), KvCfg::default(), None);
    let before = std::fs::read_to_string(audit_file(&w.env))
        .unwrap()
        .lines()
        .count();
    assert_eq!(
        child.handle(&put(Scope::User, "k", "v"), None).status,
        Status::Denied
    );
    let text = std::fs::read_to_string(audit_file(&w.env)).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .skip(before)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["gate"], "user_scope_policy");
}
