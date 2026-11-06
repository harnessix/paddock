use super::*;
use crate::broker;
use crate::macvm::connlog;
use crate::macvm::firewall::{Mode, Rules};
use crate::macvm::gate::Firewall;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::AtomicUsize;

fn asker(t: &std::path::Path, wait: Duration) -> (HostEnv, Asker) {
    let env = crate::privfs::tests::env_in(t);
    let a = Asker::new(
        env.clone(),
        "chatgpt-desktop",
        "ChatGPT",
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
        app: "com.openai.chat".into(),
        host: h.into(),
        port: 443,
    }
}

fn who() -> Who {
    Who {
        app: "com.openai.chat".into(),
        parent_app: None,
        team: Some("2DC432GLL2".into()),
    }
}

fn identity() -> Identity {
    Identity {
        pid: Some(612),
        app: Some("com.openai.chat".into()),
        team: Some("2DC432GLL2".into()),
        path: Some("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT".into()),
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
        who: "user",
        word: word.into(),
    }
}

#[test]
fn pending_map_shares_answer() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_secs(10));
    let fw = firewall(t.path(), a);
    let id = identity();
    // Two connections to the same triple: one question, both wait.
    let waiters: Vec<_> = (0..2)
        .map(|_| {
            let (fw, id) = (fw.clone(), id.clone());
            std::thread::spawn(move || ask(&fw, &triple(), Some(&id), 1000))
        })
        .collect();
    let q = wait_for_question(&env);
    for _ in 0..2000 {
        if question(&env).and_then(|q| q.connect).map(|c| c.count) == Some(2) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let q2 = question(&env).unwrap();
    assert_eq!(q2.id, q.id, "one question for both");
    let c = q2.connect.unwrap();
    assert_eq!(c.count, 2);
    assert_eq!(c.asked_at, 1000);
    assert_eq!(c.pid, std::process::id());
    assert_eq!(c.profile, "chatgpt-desktop");
    assert_eq!((c.label.as_str(), c.more, c.notice), ("ChatGPT", 0, false));
    assert_eq!(c.team.as_deref(), Some("2DC432GLL2"));
    assert_eq!(q2.sandbox, "macvm-chatgpt-desktop");
    assert_eq!(
        q2.summary,
        "ChatGPT wants to connect to telemetry.example.com port 443"
    );
    assert!(
        q2.detail.contains("2 connections wait on this answer"),
        "{}",
        q2.detail
    );
    assert!(
        q2.detail
            .contains("Why? The process /Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
        "{}",
        q2.detail
    );
    let a = fw.asker().unwrap();
    assert_eq!(a.waiting(), 1);
    // The card file is private.
    let file = broker::approvals_dir(&env).join(format!("{}.json", q.id));
    assert_eq!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // One answer, both get it, the files go, the map forgets.
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowSession).unwrap();
    for w in waiters {
        let s = w.join().unwrap().unwrap();
        assert_eq!((s.answer, s.who), (ConnectAnswer::AllowSession, "user"));
        assert!(s.word.starts_with("session-"), "{}", s.word);
    }
    assert!(question(&env).is_none());
    assert!(!file.exists());
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
    broker::answer_reply(&env, &q.id, "deny-always").unwrap();
    let s = w.join().unwrap().unwrap();
    assert_eq!(s.answer, ConnectAnswer::DenyAlways);
    // An answer file that holds nothing readable counts as deny.
    let f2 = fw.clone();
    let w = std::thread::spawn(move || ask(&f2, &host("d.example"), None, 1300));
    let q = wait_for_question(&env);
    std::fs::write(
        broker::approvals_dir(&env).join(format!("{}.answer", q.id)),
        "garbage",
    )
    .unwrap();
    assert_eq!(w.join().unwrap(), Ok(settled(ConnectAnswer::Deny, "once")));
}

#[test]
fn timeout_denies_once_keeps_question_and_a_late_answer_counts() {
    let t = tempfile::tempdir().unwrap();
    let (env, a) = asker(t.path(), Duration::from_millis(150));
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
    // The next connection joins the same question, and the answer ends it.
    let f2 = fw.clone();
    let w = std::thread::spawn(move || ask(&f2, &triple(), Some(&identity()), 2001));
    for _ in 0..2000 {
        if question(&env).and_then(|q| q.connect).map(|c| c.count) == Some(1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let q2 = question(&env).unwrap();
    assert_eq!(q2.id, q.id, "the same question, not a second one");
    assert_eq!(
        q2.connect.unwrap().asked_at,
        2000,
        "the time of the first ask stays"
    );
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowAlways).unwrap();
    let s = w.join().unwrap().unwrap();
    assert_eq!(s.answer, ConnectAnswer::AllowAlways);
    assert!(question(&env).is_none());
    assert_eq!(a.waiting(), 0);
    // An answer that comes when nobody waits: the tick applies it, and
    // the next connection of the triple gets it.
    assert_eq!(
        ask(&fw, &host("late.example"), None, 2100),
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
        ask(&fw, &host("late.example"), None, 2102),
        Ok(settled(ConnectAnswer::AllowOnce, "once"))
    );
    assert_eq!(a.waiting(), 0, "allow-once is used up");
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
    let (env, a) = asker(t.path(), Duration::from_millis(150));
    let fw = firewall(t.path(), a);
    let a = fw.asker().unwrap();
    // A question asked at 3000, left unanswered.
    assert_eq!(ask(&fw, &triple(), None, 3000), Err("timeout".into()));
    let q = question(&env).unwrap();
    // The host wakes at 3500: the next connection is denied once and the
    // question is asked again with the new time.
    a.note_wake(3500);
    assert_eq!(a.last_wake(), 3500);
    assert_eq!(ask(&fw, &triple(), None, 3600), Err("stale".into()));
    let q2 = question(&env).unwrap();
    assert_eq!(q2.id, q.id);
    assert_eq!(q2.connect.as_ref().unwrap().asked_at, 3600);
    assert_eq!(q2.connect.as_ref().unwrap().count, 0);
    assert_eq!(q2.created, 3600);
    // Then it waits as usual, and an answer ends it.
    let f2 = fw.clone();
    let w = std::thread::spawn(move || ask(&f2, &triple(), None, 3601));
    for _ in 0..2000 {
        if question(&env).and_then(|q| q.connect).map(|c| c.count) == Some(1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    broker::answer_connect(&env, &q.id, ConnectAnswer::Deny).unwrap();
    assert_eq!(w.join().unwrap(), Ok(settled(ConnectAnswer::Deny, "once")));
    // A wake before the question changes nothing; an older wake never
    // lowers the mark.
    assert_eq!(ask(&fw, &triple(), None, 4000), Err("timeout".into()));
    a.note_wake(3000);
    assert_eq!(a.last_wake(), 3500);
    assert_eq!(ask(&fw, &triple(), None, 4001), Err("timeout".into()));
}

#[test]
fn a_dead_helper_takes_its_question_along() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    // A question of a pid that is not alive: `pending` removes it.
    let dead = Asker::new(env.clone(), "app", "App", t.path().into(), 2_000_000_000)
        .timing(Duration::from_millis(50), Duration::from_millis(10));
    let fw = firewall(t.path(), dead);
    assert_eq!(ask(&fw, &triple(), None, 1), Err("timeout".into()));
    let dir = broker::approvals_dir(&env);
    let files = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(files, 1);
    assert!(question(&env).is_none());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
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
    let got = a.poll(now);
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
    assert!(a.poll(150).is_empty());
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
    assert!(qs[0].detail.contains("paddock app status chatgpt-desktop"));
    assert_eq!(qs[0].choices, ["allow-once", "deny"]);
    assert!(a.paused());
    // More questions while paused: still the one notice, with the count.
    a.join(&host("more.example"), who(), None, 101);
    let qs = questions(&env);
    assert_eq!(qs.len(), 1);
    assert_eq!(qs[0].connect.clone().unwrap().more, n as u64 + 1);
    // "Ask one by one": the first question comes up again.
    broker::answer_connect(&env, &qs[0].id, ConnectAnswer::AllowOnce).unwrap();
    assert!(a.poll(102).is_empty());
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
    let got = a.poll(104);
    assert_eq!(got.len(), n + 1 + crate::macvm::ask_queue::GUARD_MAX + 1);
    assert!(got.iter().all(|x| x.answer == ConnectAnswer::Deny));
    for x in &got {
        a.finish(&x.triple, &x.question, settled(x.answer, "once"), 104);
    }
    assert!(question(&env).is_none());
    assert_eq!(a.waiting(), 0);
    assert!(!a.paused());
}
