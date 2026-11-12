use super::*;
use crate::broker;
use crate::broker::{Pending, CONNECT_CAPABILITY};
use crate::macvm::connlog;
use crate::macvm::firewall::{Mode, Rules};
use crate::macvm::gate::Firewall;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::AtomicUsize;

fn asker(t: &std::path::Path, wait: Duration) -> (HostEnv, Asker) {
    let env = crate::privfs::tests::env_in(t);
    let a = Asker::new(
        env.clone(),
        "sample-notes",
        "Sample Notes",
        t.join("profile"),
        std::process::id(),
    )
    .timing(wait, Duration::from_millis(20));
    (env, a)
}

/// A firewall in mode ask around the asker, with no rules.
fn firewall(t: &std::path::Path, a: Asker) -> Arc<Firewall> {
    let rules = Rules::load_at(t.join("firewall").join("app.toml")).unwrap();
    Arc::new(Firewall::new(
        Mode::Ask,
        None,
        rules,
        Some(a),
        t.join("logs").join(connlog::FILE),
    ))
}

fn triple() -> Triple {
    host("telemetry.example.com")
}

fn host(h: &str) -> Triple {
    Triple {
        identity: None,
        app: "com.example.notes".into(),
        host: h.into(),
        port: 443,
    }
}

fn who() -> Who {
    Who {
        app: "com.example.notes".into(),
        parent_app: None,
        team: Some("EXAMPLE123".into()),
    }
}

fn identity() -> Identity {
    Identity {
        pid: Some(612),
        app: Some("com.example.notes".into()),
        team: Some("EXAMPLE123".into()),
        path: Some("/Applications/Sample Notes.app/Contents/MacOS/Sample Notes".into()),
        parent_app: None,
    }
}

fn ask(fw: &Firewall, t: &Triple, id: Option<&Identity>, now: u64) -> Result<Settled, String> {
    fw.ask(fw.asker().unwrap(), t, who(), id, now)
}

fn questions(env: &HostEnv) -> Vec<Pending> {
    broker::pending(env)
        .into_iter()
        .filter(|p| p.capability == CONNECT_CAPABILITY)
        .collect()
}

/// The one connect question of the store, when it is there.
fn question(env: &HostEnv) -> Option<Pending> {
    questions(env).into_iter().next()
}

fn wait_for_question(env: &HostEnv) -> Pending {
    // Generous: the full suite runs thousands of tests at once.
    for _ in 0..2000 {
        if let Some(q) = question(env) {
            return q;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no connect question in the store");
}

fn settled(answer: ConnectAnswer, word: &str) -> Settled {
    Settled {
        answer,
        allowed: answer.allows(),
        who: "user",
        word: word.into(),
    }
}

#[test]
fn separate_connections_get_separate_questions_and_session_rule_rechecks_them() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(10));
    a.set_wake_fence_for_test(true); // Models a platform wake fence.
                                     // The test clock and the settle clock differ; the rate is not under test.
    let fw = firewall(t.path(), a.rate(100));
    let id = identity();
    // Even identical PIDs cannot establish a shared process generation.
    let waiters: Vec<_> = (0..2)
        .map(|_| {
            let (fw, id) = (fw.clone(), id.clone());
            std::thread::spawn(move || ask(&fw, &triple(), Some(&id), 1000))
        })
        .collect();
    let q = wait_for_question(&env);
    for _ in 0..2000 {
        if fw.asker().unwrap().waiting() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let q2 = question(&env).unwrap();
    assert_eq!(q2.id, q.id);
    let c = q2.connect.unwrap();
    assert_eq!(c.count, 1);
    assert_eq!(c.asked_at, 1000);
    assert_eq!(c.pid, std::process::id());
    assert_eq!(c.profile, "sample-notes");
    assert_eq!(
        (c.label.as_str(), c.more, c.notice),
        ("Sample Notes", 1, false)
    );
    assert_eq!(c.team.as_deref(), Some("EXAMPLE123"));
    assert_eq!(q2.sandbox, "macvm-sample-notes");
    assert_eq!(
        q2.summary,
        "Sample Notes wants to connect to telemetry.example.com port 443"
    );
    assert!(
        q2.detail.contains(
            "Why? The process /Applications/Sample Notes.app/Contents/MacOS/Sample Notes"
        ),
        "{}",
        q2.detail
    );
    let a = fw.asker().unwrap();
    assert_eq!(a.waiting(), 2);
    // The card file is private.
    let file = broker::approvals_dir(&env).join(format!("{}.json", q.id));
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // A session rule can settle the other question through normal recheck.
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowSession).unwrap();
    let mut sources = Vec::new();
    for w in waiters {
        let s = w.join().unwrap().unwrap();
        sources.push(s.who);
        assert!(s.word.starts_with("session-"), "{}", s.word);
    }
    sources.sort();
    assert_eq!(sources, ["rule", "user"]);
    assert!(question(&env).is_none());
    assert!(file.exists());
    assert!(broker::approvals_dir(&env)
        .join(format!("{}.answer", q.id))
        .exists());
    assert!(broker::approvals_dir(&env)
        .join(format!("{}.done", q.id))
        .exists());
    assert_eq!(a.waiting(), 0);
    assert_eq!(fw.rules().session.len(), 1, "one rule for both waiters");
    // A second answer to the same id fails: the question is gone.
    assert!(broker::answer_connect(&env, &q.id, ConnectAnswer::Deny).is_err());
    // The plain card answers map: once, session, deny; deny-session not.
    let f2 = fw.clone();
    let w = std::thread::spawn(move || ask(&f2, &host("b.example"), None, 1100));
    let q = wait_for_question(&env);
    assert!(broker::answer(&env, &q.id, broker::Answer::DenySession).is_err());
    assert!(broker::answer_with_note(&env, &q.id, broker::Answer::Deny, Some("why")).is_err());
    broker::answer(&env, &q.id, broker::Answer::Once).unwrap();
    assert_eq!(
        w.join().unwrap(),
        Ok(settled(ConnectAnswer::AllowOnce, "once"))
    );
    // `paddock approvals answer ID --text WORD` goes through answer_reply.
    let f2 = fw.clone();
    let w = std::thread::spawn(move || ask(&f2, &host("c.example"), None, 1200));
    let q = wait_for_question(&env);
    assert!(q.connect.unwrap().path.is_none());
    assert!(broker::answer_reply(&env, &q.id, "maybe").is_err());
    broker::answer_reply_cli(&env, &q.id, "deny-always").unwrap();
    let s = w.join().unwrap().unwrap();
    assert_eq!(s.answer, ConnectAnswer::DenyAlways);
    assert_eq!(s.who, "cli");
    // A malformed answer remains unknown until it is repaired.
    let f2 = fw.clone();
    let w = std::thread::spawn(move || ask(&f2, &host("d.example"), None, 1300));
    let q = wait_for_question(&env);
    std::fs::write(
        broker::approvals_dir(&env).join(format!("{}.answer", q.id)),
        "garbage",
    )
    .unwrap();
    fw.settle(1301);
    assert!(!w.is_finished());
    std::fs::remove_file(broker::approvals_dir(&env).join(format!("{}.answer", q.id))).unwrap();
    broker::answer_connect(&env, &q.id, ConnectAnswer::Deny).unwrap();
    assert_eq!(w.join().unwrap(), Ok(settled(ConnectAnswer::Deny, "once")));
}

#[test]
fn timeout_keeps_question_without_transferring_a_late_once_answer() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_millis(150));
    a.set_wake_fence_for_test(true); // Models a platform wake fence.
    let nudges = Arc::new(AtomicUsize::new(0));
    let n = nudges.clone();
    let fw = firewall(
        t.path(),
        a.on_wait(Box::new(move || {
            n.fetch_add(1, Ordering::SeqCst);
        })),
    );
    assert_eq!(
        ask(&fw, &triple(), Some(&identity()), 2000),
        Err("timeout".into())
    );
    assert_eq!(
        nudges.load(Ordering::SeqCst),
        1,
        "the marker is started once per wait"
    );
    // The question stays on the screen, with nobody waiting, through
    // many looks at the store.
    let q = question(&env).expect("the question stays");
    assert_eq!(q.connect.as_ref().unwrap().count, 0);
    for _ in 0..5 {
        fw.settle(2001);
        assert_eq!(question(&env).map(|p| p.id), Some(q.id.clone()));
    }
    let a = fw.asker().unwrap();
    assert_eq!(a.waiting(), 1);
    // A late persistent answer can still create its requested rule.
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowAlways).unwrap();
    fw.settle(2001);
    assert!(question(&env).is_none());
    assert_eq!(a.waiting(), 0);
    // A late once answer has no waiter and is never transferred.
    assert_eq!(
        ask(&fw, &host("late.example"), Some(&identity()), 2100),
        Err("timeout".into())
    );
    let q = question(&env).unwrap();
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowOnce).unwrap();
    fw.settle(2101);
    assert!(
        question(&env).is_none(),
        "the answered panel does not come back"
    );
    assert_eq!(
        ask(&fw, &host("late.example"), Some(&identity()), 2102),
        Err("timeout".into())
    );
    let retry = question(&env).unwrap();
    assert_ne!(retry.id, q.id);
    broker::answer_connect(&env, &retry.id, ConnectAnswer::Deny).unwrap();
    fw.settle(2103);
    assert_eq!(a.waiting(), 0);
    // A late always answer makes its rule, which then decides alone.
    assert_eq!(
        ask(&fw, &host("rule.example"), None, 2200),
        Err("timeout".into())
    );
    let q = question(&env).unwrap();
    broker::answer_connect(&env, &q.id, ConnectAnswer::DenyAlways).unwrap();
    fw.settle(2201);
    assert_eq!(fw.rules().file.len(), 2);
}

#[test]
fn stale_after_wake_denied_once() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    let old_key = host("before.example");
    let q = match a.join(&old_key, who(), None, 3000) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let old = question(&env).unwrap();
    a.note_wake(3500);
    assert_eq!(q.result().unwrap().word, "stale");
    assert!(broker::answer_connect(&env, &old.id, ConnectAnswer::Deny).is_err());
    let fresh_key = host("after.example");
    assert!(matches!(
        a.join(&fresh_key, who(), None, 3600),
        Join::Wait(_, false)
    ));
    assert_ne!(question(&env).unwrap().id, old.id);
    a.note_wake(3000);
    assert_eq!(a.last_wake(), 3500);
}

#[test]
fn a_dead_helper_takes_its_question_along() {
    crate::broker::PID_LIVENESS_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(|pid| Some(pid != 2_000_000_000)));
    });
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    // A question of a pid that is not alive: `pending` removes it.
    let dead = Asker::new(env.clone(), "app", "App", t.path().into(), 2_000_000_000)
        .timing(Duration::from_millis(50), Duration::from_millis(10));
    let fw = firewall(t.path(), dead);
    assert_eq!(ask(&fw, &triple(), None, 1), Err("timeout".into()));
    let dir = broker::approvals_dir(&env);
    let lock = dir.join(".lock");
    use std::os::unix::fs::MetadataExt;
    let lock_inode = std::fs::metadata(&lock).unwrap().ino();
    let cards = || {
        std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x == "json" || x == "answer")
            })
            .count()
    };
    assert_eq!(cards(), 1);
    assert!(question(&env).is_none());
    assert_eq!(cards(), 0);
    assert_eq!(std::fs::metadata(&lock).unwrap().ino(), lock_inode);
}

#[test]
fn the_approver_hook_answers_first() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_millis(300));
    let script = t.path().join("hook.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\ncat > \"$0.in\"\necho '{\"answer\":\"allow-always\",\"note\":\"known host\"}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hook = crate::approver::HookConfig {
        command: vec![script.display().to_string()],
        capabilities: vec!["connect".into()],
        timeout: 5,
    };
    let fw = firewall(t.path(), a.hook(Some(hook.clone())));
    let s = ask(&fw, &triple(), Some(&identity()), 5000).unwrap();
    assert_eq!((s.answer, s.who), (ConnectAnswer::AllowAlways, "approver"));
    let input = std::fs::read_to_string(t.path().join("hook.sh.in")).unwrap();
    assert!(input.contains("\"capability\":\"connect\""), "{input}");
    assert!(
        input.contains("\"host\":\"telemetry.example.com\""),
        "{input}"
    );
    assert!(
        question(&env).is_none(),
        "an answered question never reaches the store"
    );
    // A hook without the capability does not run; a hook that abstains
    // leaves the question to the user.
    let t2 = tempfile::tempdir().unwrap();
    let (env, a) = asker(t2.path(), Duration::from_millis(100));
    let mut other = hook.clone();
    other.capabilities = vec!["exec".into()];
    let fw = firewall(t2.path(), a.hook(Some(other)));
    assert_eq!(ask(&fw, &triple(), None, 5001), Err("timeout".into()));
    assert!(question(&env).is_some());
    std::fs::write(&script, "#!/bin/sh\necho '{\"answer\":\"abstain\"}'\n").unwrap();
    let t3 = tempfile::tempdir().unwrap();
    let (env, a) = asker(t3.path(), Duration::from_millis(100));
    let fw = firewall(t3.path(), a.hook(Some(hook.clone())));
    assert_eq!(ask(&fw, &triple(), None, 5002), Err("timeout".into()));
    assert!(
        question(&env).is_some(),
        "an abstained question joins the queue"
    );
    // The plain words of a card work too, and an unknown word abstains.
    std::fs::write(&script, "#!/bin/sh\necho '{\"answer\":\"session\"}'\n").unwrap();
    let t4 = tempfile::tempdir().unwrap();
    let (_env, a) = asker(t4.path(), Duration::from_millis(100));
    let fw = firewall(t4.path(), a.hook(Some(hook.clone())));
    let s = ask(&fw, &triple(), None, 5003).unwrap();
    assert_eq!((s.answer, s.who), (ConnectAnswer::AllowSession, "approver"));
    std::fs::write(&script, "#!/bin/sh\necho '{\"answer\":\"maybe\"}'\n").unwrap();
    let t5 = tempfile::tempdir().unwrap();
    let (_env, a) = asker(t5.path(), Duration::from_millis(100));
    let fw = firewall(t5.path(), a.hook(Some(hook)));
    assert_eq!(ask(&fw, &triple(), None, 5004), Err("timeout".into()));
}

/// Answer the card on the screen and let the asker see it at `now`.
fn answer_shown(env: &HostEnv, a: &Asker, answer: ConnectAnswer, now: u64) -> Vec<String> {
    let q = question(env).expect("a card on the screen");
    broker::answer_connect(env, &q.id, answer).unwrap();
    let got = a.poll(now).answers;
    for x in &got {
        a.finish(&x.triple, &x.question, settled(x.answer, "once"), now);
    }
    got.into_iter().map(|x| x.triple.host).collect()
}

#[test]
fn one_card_per_session_the_rest_queue_with_a_count() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    for (i, h) in ["a", "b", "c", "d", "e"].iter().enumerate() {
        assert!(matches!(
            a.join(&host(h), who(), None, 100 + i as u64),
            Join::Wait(_, false)
        ));
    }
    let qs = questions(&env);
    assert_eq!(qs.len(), 1, "one card, not five");
    let c = qs[0].connect.clone().unwrap();
    assert_eq!((c.host.as_str(), c.more), ("a", 4));
    assert!(
        qs[0].detail.contains("and 4 more waiting"),
        "{}",
        qs[0].detail
    );
    let (paused, open) = a.snapshot();
    assert!(!paused);
    assert_eq!(open.len(), 5);
    assert!(open[0].shown && !open[1].shown);
    // An answer brings the next one up, in order, with the lower count.
    assert_eq!(answer_shown(&env, &a, ConnectAnswer::Deny, 110), ["a"]);
    let c = question(&env).unwrap().connect.unwrap();
    assert_eq!((c.host.as_str(), c.more), ("b", 3));
    // Three came up in this minute: the fourth waits silently.
    assert_eq!(answer_shown(&env, &a, ConnectAnswer::Deny, 111), ["b"]);
    assert_eq!(question(&env).unwrap().connect.unwrap().host, "c");
    assert_eq!(answer_shown(&env, &a, ConnectAnswer::Deny, 112), ["c"]);
    assert!(question(&env).is_none(), "the rate limit holds d back");
    assert!(a.poll(150).answers.is_empty());
    assert!(question(&env).is_none());
    a.poll(160);
    assert_eq!(
        question(&env).unwrap().connect.unwrap().host,
        "d",
        "a minute after the first, the next comes up"
    );
    assert_eq!(a.waiting(), 2);
}

#[test]
fn too_many_questions_pause_with_one_notice() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    let n = crate::macvm::ask_queue::GUARD_MAX + 1;
    for i in 0..n {
        a.join(&host(&format!("h{i}.example")), who(), None, 100);
    }
    let qs = questions(&env);
    assert_eq!(qs.len(), 1, "one notice, no question card");
    let c = qs[0].connect.clone().unwrap();
    assert!(c.notice);
    assert_eq!(c.more, n as u64);
    assert!(
        qs[0].summary.contains("many new connections"),
        "{}",
        qs[0].summary
    );
    assert!(qs[0].detail.contains("paddock app status sample-notes"));
    assert_eq!(qs[0].choices, ["allow-once", "deny"]);
    assert!(a.paused());
    // More questions while paused: still the one notice, with the count.
    a.join(&host("more.example"), who(), None, 101);
    let qs = questions(&env);
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0].connect.clone().unwrap().more, n as u64 + 1);
    // "Ask one by one": the first question comes up again.
    broker::answer_connect(&env, &qs[0].id, ConnectAnswer::AllowOnce).unwrap();
    assert!(a.poll(102).answers.is_empty());
    assert!(!a.paused());
    let q = question(&env).unwrap().connect.unwrap();
    assert!(!q.notice);
    assert_eq!(q.host, "h0.example");
    // Twenty more pause again; "deny all" answers every open question.
    for i in 0..crate::macvm::ask_queue::GUARD_MAX + 1 {
        a.join(&host(&format!("x{i}.example")), who(), None, 103);
    }
    assert!(a.paused());
    let notice = question(&env).unwrap();
    assert!(notice.connect.unwrap().notice);
    broker::answer_connect(&env, &notice.id, ConnectAnswer::Deny).unwrap();
    let got = a.poll(104).answers;
    assert_eq!(got.len(), n + 1 + crate::macvm::ask_queue::GUARD_MAX + 1);
    assert!(got.iter().all(|x| x.answer == ConnectAnswer::Deny));
    for x in &got {
        a.finish(&x.triple, &x.question, settled(x.answer, "once"), 104);
    }
    assert!(question(&env).is_none());
    assert_eq!(a.waiting(), 0);
    assert!(!a.paused());
}

#[test]
fn pause_resume_invalidates_the_old_presentation() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    a.join(&host("first.example"), who(), None, 100);
    let old = question(&env).unwrap();
    for i in 0..crate::macvm::ask_queue::GUARD_MAX {
        a.join(&host(&format!("queued-{i}.example")), who(), None, 101);
    }
    let notice = question(&env).unwrap();
    assert!(notice.connect.as_ref().unwrap().notice);
    assert!(broker::answer_connect(&env, &old.id, ConnectAnswer::AllowAlways).is_err());
    assert!(broker::answer_connect(&env, &notice.id, ConnectAnswer::AllowAlways).is_err());
    assert!(broker::answer_reply_cli(&env, &notice.id, "deny-always").is_err());
    assert!(broker::answer(&env, &notice.id, broker::Answer::Session).is_err());
    broker::answer_connect(&env, &notice.id, ConnectAnswer::AllowOnce).unwrap();
    assert!(a.poll(102).answers.is_empty());
    let shown = question(&env).unwrap();
    assert_ne!(old.id, shown.id);
    assert_eq!(shown.connect.as_ref().unwrap().host, "first.example");
    assert!(broker::answer_connect(&env, &old.id, ConnectAnswer::AllowAlways).is_err());
    broker::answer_connect(&env, &shown.id, ConnectAnswer::Deny).unwrap();
    let got = a.poll(103).answers;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].answer, ConnectAnswer::Deny);
}

#[test]
fn an_accepted_answer_survives_pause_and_notice_deny_all() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    a.join(&host("first.example"), who(), None, 100);
    let old = question(&env).unwrap();
    broker::answer_connect_cli(&env, &old.id, ConnectAnswer::DenyAlways).unwrap();
    for i in 0..crate::macvm::ask_queue::GUARD_MAX {
        a.join(&host(&format!("queued-{i}.example")), who(), None, 101);
    }
    let notice = question(&env).unwrap();
    assert!(notice.connect.as_ref().unwrap().notice);
    broker::answer_connect(&env, &notice.id, ConnectAnswer::Deny).unwrap();
    let got = a.poll(102).answers;
    let first = got
        .iter()
        .find(|x| x.triple.host == "first.example")
        .unwrap();
    assert_eq!(
        (first.answer, first.who),
        (ConnectAnswer::DenyAlways, "cli")
    );
    assert_eq!(
        got.iter()
            .filter(|x| x.triple.host == "first.example")
            .count(),
        1
    );
    assert_eq!(got.len(), crate::macvm::ask_queue::GUARD_MAX + 1);
    assert!(broker::answer_connect(&env, &old.id, ConnectAnswer::AllowAlways).is_err());
}

#[test]
fn an_accepted_answer_wins_wake_and_a_competing_rule_claim() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    let key = host("first.example");
    let q = match a.join(&key, who(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a queued question"),
    };
    let old = question(&env).unwrap();
    broker::answer_connect(&env, &old.id, ConnectAnswer::DenyAlways).unwrap();
    let got = a.poll(101).answers;
    assert_eq!(got.len(), 1);
    a.note_wake(110);
    assert!(!a.claim_decision(&key, &q).claimed);
    assert_eq!(got[0].answer, ConnectAnswer::DenyAlways);
    assert!(
        !a.claim_decision(&key, &q).claimed,
        "a claimed user answer cannot also be decided by a rule"
    );
    a.finish(&key, &q, settled(got[0].answer, "once"), 120);
    assert_eq!(q.result().unwrap().answer, ConnectAnswer::DenyAlways);
    assert!(question(&env).is_none());
}

#[test]
fn a_rule_that_claims_first_rejects_a_delayed_answer() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    let key = host("first.example");
    let q = match a.join(&key, who(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a queued question"),
    };
    let old = question(&env).unwrap();
    assert!(a.claim_decision(&key, &q).claimed);
    assert!(broker::answer_connect(&env, &old.id, ConnectAnswer::AllowAlways).is_err());
    assert!(!a.claim_decision(&key, &q).claimed);
    a.finish(
        &key,
        &q,
        Settled {
            answer: ConnectAnswer::Deny,
            allowed: false,
            who: BY_RULE,
            word: "rule".into(),
        },
        101,
    );
    assert_eq!(q.result().unwrap().who, BY_RULE);
}

#[test]
fn presentation_events_retire_old_ids_across_pause_and_wake() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let events = Rc::new(RefCell::new(Vec::<EventQuestion>::new()));
    let sink = events.clone();
    EVENT_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move |e| sink.borrow_mut().push(e))));

    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(60));
    let key = host("first.example");
    let q = match a.join(&key, who(), None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let old = question(&env).unwrap();
    for i in 0..crate::macvm::ask_queue::GUARD_MAX {
        a.join(&host(&format!("queued-{i}.example")), who(), None, 101);
    }
    let notice = question(&env).unwrap();
    broker::answer_connect_cli(&env, &notice.id, ConnectAnswer::AllowOnce).unwrap();
    a.poll(102);
    let new = question(&env).unwrap();
    broker::answer_connect_cli(&env, &new.id, ConnectAnswer::Deny).unwrap();
    let got = a.poll(103).answers;
    assert_eq!(got.len(), 1);
    a.finish(
        &key,
        &q,
        Settled {
            answer: got[0].answer,
            allowed: got[0].answer.allows(),
            who: got[0].who,
            word: "once".into(),
        },
        103,
    );

    let all = events.borrow();
    let pos = |state, id: &str| {
        all.iter()
            .position(|e| e.state == state && e.id == id)
            .unwrap()
    };
    let opened = pos(QuestionState::Open, &old.id);
    let withdrawn = pos(QuestionState::Withdrawn, &old.id);
    let reopened = pos(QuestionState::Open, &new.id);
    let paused = pos(QuestionState::Paused, &notice.id);
    let resumed = pos(QuestionState::Resumed, &notice.id);
    let answered = pos(QuestionState::Answered, &new.id);
    assert!(
        opened < withdrawn
            && withdrawn < paused
            && paused < resumed
            && resumed < reopened
            && reopened < answered
    );
    assert_eq!(all[answered].answer.as_deref(), Some("deny"));
    assert_eq!(all[answered].by.as_deref(), Some("cli"));
    drop(all);

    let t2 = tempfile::tempdir().unwrap();
    let (env2, a2) = asker(t2.path(), Duration::from_secs(60));
    let wake_key = host("wake.example");
    a2.join(&wake_key, who(), None, 200);
    let before = question(&env2).unwrap();
    a2.note_wake(210);
    assert!(matches!(
        a2.join(&wake_key, who(), None, 220),
        Join::Wait(_, false)
    ));
    let after = question(&env2).unwrap();
    let all = events.borrow();
    let retire = all
        .iter()
        .position(|e| e.state == QuestionState::Withdrawn && e.id == before.id)
        .unwrap();
    let reopen = all
        .iter()
        .position(|e| e.state == QuestionState::Open && e.id == after.id)
        .unwrap();
    assert_ne!(before.id, after.id);
    assert!(retire < reopen);
    drop(all);
    assert!(broker::answer_connect(&env2, &before.id, ConnectAnswer::Deny).is_err());
    EVENT_HOOK.with(|hook| *hook.borrow_mut() = None);
}

/// A question opens and is answered on the launcher event stream, with
/// the profile, the host, the process and the answer.
#[test]
fn questions_are_lines_on_the_event_stream() {
    use crate::eventstream::schema::{Event, QuestionState};
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(10));
    let (tx, rx) = std::sync::mpsc::channel();
    let fw = firewall(t.path(), a);
    let id = identity();
    let w = {
        let fw = fw.clone();
        std::thread::spawn(move || {
            EVENT_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |event| {
                    tx.send(event).unwrap();
                }))
            });
            ask(&fw, &triple(), Some(&id), 1000)
        })
    };
    let q = wait_for_question(&env);
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowAlways).unwrap();
    w.join().unwrap().unwrap();
    let mut got = Vec::new();
    while got.len() < 2 {
        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let line = crate::eventstream::Line::at(1, Event::Question(event)).to_text();
        let l: crate::eventstream::Line = serde_json::from_str(&line).unwrap();
        if let Event::Question(q) = l.event {
            got.push(q);
        }
    }
    assert_eq!(got[0].state, QuestionState::Open);
    assert_eq!(got[1].state, QuestionState::Answered);
    for e in &got {
        assert_eq!(e.id, q.id);
        assert_eq!(e.app, "sample-notes");
        assert_eq!((e.host.as_str(), e.port), ("telemetry.example.com", 443));
        assert_eq!(
            e.process.as_deref(),
            Some("/Applications/Sample Notes.app/Contents/MacOS/Sample Notes")
        );
    }
    assert_eq!(got[1].answer.as_deref(), Some("allow-always"));
    assert_eq!(got[1].by.as_deref(), Some("user"));
}

#[path = "approvals_tests/site.rs"]
mod site;
