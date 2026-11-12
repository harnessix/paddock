use super::*;
use crate::broker;
use std::time::Duration;

fn firewall(t: &std::path::Path, mode: Mode, asker: Option<Asker>) -> Firewall {
    let mut rules = Rules::load_at(t.join("firewall").join("app.toml")).unwrap();
    rules.seed(&["notes.example.com".into()], 1);
    Firewall::new(mode, None, rules, asker, t.join("logs").join(connlog::FILE))
}

fn id() -> Identity {
    Identity {
        app: Some("com.example.notes".into()),
        team: Some("T1".into()),
        path: Some("/Applications/Sample Notes.app/Contents/MacOS/Sample Notes".into()),
        ..Default::default()
    }
}

#[test]
fn a_stale_rule_snapshot_cannot_finish_an_answer_already_claimed_by_poll() {
    use std::sync::mpsc;
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let asker = Asker::new(
        env.clone(),
        "app",
        "App",
        t.path().into(),
        std::process::id(),
    );
    let fw = Arc::new(firewall(t.path(), Mode::None, Some(asker)));
    let key = Triple {
        identity: None,
        app: "com.example.notes".into(),
        host: "race.example".into(),
        port: 443,
    };
    let who = Who {
        app: key.app.clone(),
        ..Default::default()
    };
    let q = match fw.asker().unwrap().join(&key, who, None, 100) {
        Join::Wait(q, false) => q,
        _ => panic!("expected a question"),
    };
    let id = broker::pending(&env).into_iter().next().unwrap().id;
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let fw2 = fw.clone();
    let rechecker = std::thread::spawn(move || {
        RECHECK_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready_tx.send(()).unwrap();
                go_rx.recv().unwrap();
            }))
        });
        fw2.recheck_current_rules(101);
    });
    ready_rx.recv().unwrap(); // recheck has a denying snapshot, but has not claimed it
    broker::answer_connect(&env, &id, ConnectAnswer::AllowAlways).unwrap();
    let answers = fw.asker().unwrap().poll(102).answers;
    assert_eq!(answers.len(), 1);
    go_tx.send(()).unwrap();
    rechecker.join().unwrap();
    assert!(
        q.result().is_none(),
        "the losing rule must not finish the claimed question"
    );
    assert!(fw.rules().file.is_empty());
    let a = &answers[0];
    let word = fw.apply_with_bus(a.answer, &a.question.who, &a.triple, BY_USER, 102, None);
    fw.asker().unwrap().finish(
        &key,
        &q,
        Settled {
            answer: a.answer,
            allowed: a.answer.allows() && word != RULE_STALE,
            who: a.who,
            word,
        },
        102,
    );
    assert_eq!(q.result().unwrap().answer, ConnectAnswer::AllowAlways);
    assert_eq!(fw.rules().file.len(), 1);
    assert_eq!(fw.rules().file[0].verdict, firewall::Verdict::Allow);
}

#[test]
fn build_reads_the_profile_and_starts_a_fresh_log() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let log = t.path().join("logs").join(connlog::FILE);
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, "old\n").unwrap();
    let fw = build(
        &env,
        "chatgpt-desktop",
        t.path(),
        log.clone(),
        "ask",
        &[],
        std::process::id(),
        AppSide::default(),
    );
    assert_eq!(fw.mode(), Mode::Ask);
    assert!(fw.asker().is_some());
    assert!(!fw.rules().seeds.is_empty(), "the seeds of the profile");
    assert!(!log.exists(), "a new session starts a new log");
    assert!(super::super::logs::numbered(&log, 1).exists());
    // Mode api with a list: the list before the rules. The asker waits
    // for a switch to ask (`paddock app network`).
    let fw = build(
        &env,
        "chatgpt-desktop",
        t.path(),
        log.clone(),
        "api",
        &["api.example.com".into()],
        1,
        AppSide::default(),
    );
    assert!(fw.asker().is_some());
    assert_eq!(
        fw.connection("evil.example", 443, Some(&id())).unwrap_err(),
        "mode"
    );
    fw.connection("api.example.com", 443, Some(&id()))
        .unwrap()
        .close(0, 0);
    assert_eq!(
        fw.connection("api.example.com", 80, Some(&id()))
            .unwrap_err(),
        "mode"
    );
    // An unknown mode word fails closed; a broken rules file is kept as it is.
    let path = Rules::path(&env, "vscode");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[[rule]]\nid = 1\n").unwrap();
    let fw = build(
        &env,
        "vscode",
        t.path(),
        log.clone(),
        "weird",
        &[],
        1,
        AppSide::default(),
    );
    assert_eq!(fw.mode(), Mode::None);
    assert!(
        fw.rules().save().is_err(),
        "a broken file is never overwritten"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[[rule]]\nid = 1\n"
    );
}

#[test]
fn the_sleep_watch_sees_a_jump_of_the_wall_clock() {
    assert!(!SleepWatch::slept(10, 10));
    assert!(!SleepWatch::slept(40, 11));
    assert!(SleepWatch::slept(41, 10));
    let mut w = SleepWatch::new(1000);
    assert_eq!(w.check(1001), None);
    assert_eq!(
        w.check(1500),
        Some(1500),
        "the wall clock jumped 499 s in no time"
    );
    assert_eq!(w.check(1501), None);
}

#[test]
fn decisions_are_logged_with_their_rule_word() {
    let t = tempfile::tempdir().unwrap();
    let fw = firewall(t.path(), Mode::Egress, None);
    // A seed, logged with its id at the close, with the bytes.
    let o = fw
        .connection("notes.example.com", 443, Some(&id()))
        .unwrap();
    o.close(10, 20);
    // No rule in egress: allowed, `egress`.
    fw.connection("other.example", 443, Some(&id()))
        .unwrap()
        .close(1, 1);
    // Mode none: denied before the rules, `mode`.
    let none = firewall(t.path(), Mode::None, None);
    assert_eq!(
        none.connection("notes.example.com", 443, Some(&id()))
            .unwrap_err(),
        "mode"
    );
    // Ask with no asker: denied, `ask`.
    let ask = firewall(t.path(), Mode::Ask, None);
    assert_eq!(
        ask.connection("other.example", 443, None).unwrap_err(),
        "ask"
    );
    let lines = connlog::read_at(&fw.log);
    let words: Vec<(String, String, String)> = lines
        .iter()
        .map(|l| (l.app.clone(), l.verdict.clone(), l.rule.clone()))
        .collect();
    assert!(words[0].2.starts_with("seed-"), "{words:?}");
    assert_eq!((lines[0].up, lines[0].down), (10, 20));
    assert_eq!(lines[0].team.as_deref(), Some("T1"));
    assert_eq!(
        words[1],
        ("com.example.notes".into(), "allow".into(), "egress".into())
    );
    assert_eq!(
        words[2],
        ("com.example.notes".into(), "deny".into(), "mode".into())
    );
    assert_eq!(words[3], ("unknown".into(), "deny".into(), "ask".into()));
    // The gate of the proxy carries the same.
    let gate = Arc::new(firewall(t.path(), Mode::Egress, None)).gate();
    let o = (gate.0)("x.example", 80, None).unwrap();
    o.close(5, 6);
    let last = connlog::read_at(&fw.log).pop().unwrap();
    assert_eq!(
        (last.host.as_str(), last.port, last.up, last.down),
        ("x.example", 80, 5, 6)
    );
}

#[test]
fn an_answer_makes_its_rule_and_the_log_names_it() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let asker = Asker::new(
        env.clone(),
        "app",
        "App",
        t.path().into(),
        std::process::id(),
    )
    .timing(Duration::from_secs(5), Duration::from_millis(20))
    .rate(100)
    .wake_fenced_for_test(); // Models a platform wake fence.
    let fw = Arc::new(firewall(t.path(), Mode::Ask, Some(asker)));
    // allow-always: the file gets the rule, the connection passes, the
    // next one is silent.
    let f2 = fw.clone();
    let th = std::thread::spawn(move || f2.connection("t.example", 443, Some(&id())));
    let q = loop {
        let p = broker::pending(&env);
        if let Some(q) = p.into_iter().find(|p| p.capability == "connect") {
            break q;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(q.summary, "App wants to connect to t.example port 443");
    assert!(
        q.detail.contains("/Applications/Sample Notes.app"),
        "{}",
        q.detail
    );
    assert_eq!(
        q.choices,
        [
            "allow-once",
            "allow-site",
            "allow-everything",
            "allow-session",
            "allow-always",
            "deny",
            "deny-always"
        ]
    );
    let c = q.connect.clone().unwrap();
    assert_eq!(
        (c.app.as_str(), c.host.as_str(), c.port, c.count),
        ("com.example.notes", "t.example", 443, 1)
    );
    broker::answer_connect(&env, &q.id, ConnectAnswer::AllowAlways).unwrap();
    th.join().unwrap().unwrap().close(1, 2);
    let rules = fw.rules();
    assert_eq!(rules.file.len(), 1);
    let rid = rules.file[0].id.clone();
    drop(rules);
    assert!(fw.rules().path.exists());
    assert!(broker::pending(&env).is_empty(), "the question is gone");
    fw.connection("t.example", 443, Some(&id()))
        .unwrap()
        .close(0, 0);
    let lines = connlog::read_at(&fw.log);
    let words: Vec<(String, String)> = lines
        .iter()
        .map(|l| (l.verdict.clone(), l.rule.clone()))
        .collect();
    assert_eq!(words[0], ("ask".into(), "ask".into()));
    assert_eq!(words[1], ("allow".into(), rid.clone()));
    assert_eq!(lines[1].count, 2, "the second allow merged into the first");
    // deny: once, nothing written; deny-always: a deny rule.
    let f2 = fw.clone();
    let th = std::thread::spawn(move || f2.connection("d.example", 443, Some(&id())));
    let q = loop {
        if let Some(q) = broker::pending(&env)
            .into_iter()
            .find(|p| p.capability == "connect")
        {
            break q;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    broker::answer(&env, &q.id, broker::Answer::Deny).unwrap();
    assert_eq!(th.join().unwrap().unwrap_err(), "once");
    assert_eq!(fw.rules().file.len(), 1);
    let f2 = fw.clone();
    let th = std::thread::spawn(move || f2.connection("d.example", 443, Some(&id())));
    let q = loop {
        if let Some(q) = broker::pending(&env)
            .into_iter()
            .find(|p| p.capability == "connect")
        {
            break q;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    broker::answer_reply(&env, &q.id, "deny-always").unwrap();
    let word = th.join().unwrap().unwrap_err();
    assert!(word.starts_with("rule-"), "{word}");
    assert_eq!(fw.rules().file.len(), 2);
    assert_eq!(
        fw.connection("d.example", 443, Some(&id())).unwrap_err(),
        word
    );
    // allow-session: a session rule, not in the file.
    let f2 = fw.clone();
    let th = std::thread::spawn(move || f2.connection("s.example", 443, Some(&id())));
    let q = loop {
        if let Some(q) = broker::pending(&env)
            .into_iter()
            .find(|p| p.capability == "connect")
        {
            break q;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    broker::answer(&env, &q.id, broker::Answer::Session).unwrap();
    th.join().unwrap().unwrap().close(0, 0);
    assert_eq!(fw.rules().session.len(), 1);
    assert_eq!(fw.rules().file.len(), 2);
    let last = connlog::read_at(&fw.log).pop().unwrap();
    assert!(last.rule.starts_with("session-"), "{}", last.rule);
    assert_eq!(answer_word(ConnectAnswer::AllowSession), "allow-session");
}

fn os_id(bundle: &str, path: &str) -> Identity {
    Identity {
        pid: Some(301),
        app: Some(bundle.into()),
        path: Some(path.into()),
        ..Default::default()
    }
}

#[test]
fn the_guest_system_never_asks_and_is_counted_per_host() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let asker = Asker::new(
        env.clone(),
        "app",
        "App",
        t.path().into(),
        std::process::id(),
    )
    .timing(Duration::from_millis(100), Duration::from_millis(10));
    let owner = AppOwner::new(
        "com.example.notes",
        &[std::path::Path::new(
            "/Users/paddock/Applications/Sample Notes.app",
        )],
    );
    let fw = firewall(t.path(), Mode::Ask, Some(asker)).with_owner(owner);
    // Fifty connections of the guest's own services: no question at all.
    let services = [
        ("com.apple.weather.widget", "weather-edge.apple.com"),
        ("com.apple.stocks.widget", "stocks-edge.apple.com"),
        ("com.apple.news.widget", "news-edge.apple.com"),
        ("com.apple.apsd", "init.push.apple.com"),
        ("com.apple.softwareupdated", "swscan.apple.com"),
    ];
    for i in 0..50 {
        let (b, h) = services[i % services.len()];
        let id = os_id(b, "/System/Library/PrivateFrameworks/X.framework/x");
        assert_eq!(fw.connection(h, 443, Some(&id)).unwrap_err(), RULE_SYSTEM);
    }
    // An unknown process is no app either.
    assert_eq!(
        fw.connection("gdmf.apple.com", 443, None).unwrap_err(),
        RULE_SYSTEM
    );
    assert!(broker::pending(&env).is_empty(), "nothing of the OS asks");
    assert_eq!(fw.asker().unwrap().waiting(), 0);
    // The log names the host and the process, merged per minute.
    let lines = connlog::read_at(&fw.log);
    let weather = lines
        .iter()
        .find(|l| l.host == "weather-edge.apple.com")
        .unwrap();
    assert_eq!(
        (
            weather.app.as_str(),
            weather.verdict.as_str(),
            weather.rule.as_str(),
            weather.count
        ),
        ("com.apple.weather.widget", "deny", "system", 10)
    );
    // The status file counts per host.
    let s = ask_status::read(&fw.status_path());
    assert_eq!(s.system_denied(), 51);
    assert_eq!(
        s.system
            .iter()
            .find(|h| h.host == "init.push.apple.com")
            .unwrap()
            .count,
        10
    );
    assert_eq!(fw.system_denied().len(), 6);
    // A seed rule (every process) still lets the guest system reach a
    // host the app needs, and a rule for an Apple service works as well.
    let id = os_id("com.apple.trustd", "/usr/libexec/trustd");
    fw.connection("notes.example.com", 443, Some(&id))
        .unwrap()
        .close(0, 0);
    // The app itself asks.
    let f = Arc::new(fw);
    let f2 = f.clone();
    let th = std::thread::spawn(move || f2.connection("api.example.com", 443, Some(&id_app())));
    let th = th.join().unwrap();
    assert_eq!(th.unwrap_err(), RULE_TIMEOUT, "it asked and timed out");
    let q = broker::pending(&env);
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].connect.as_ref().unwrap().host, "api.example.com");
    let s = ask_status::read(&f.status_path());
    assert_eq!(s.open.len(), 1);
    assert!(s.open[0].shown);
}

fn id_app() -> Identity {
    Identity {
        pid: Some(612),
        app: Some("com.example.notes.helper".into()),
        parent_app: Some("com.example.notes".into()),
        path: Some(
            "/Users/paddock/Applications/Sample Notes.app/Contents/Frameworks/H.app/Contents/MacOS/H"
                .into(),
        ),
        ..Default::default()
    }
}

#[test]
fn a_rule_added_in_the_file_decides_the_open_questions() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let asker = Asker::new(
        env.clone(),
        "app",
        "App",
        t.path().into(),
        std::process::id(),
    )
    .timing(Duration::from_millis(80), Duration::from_millis(10));
    let fw = firewall(t.path(), Mode::Ask, Some(asker));
    for h in ["q1.example", "q2.example"] {
        assert_eq!(
            fw.connection(h, 443, Some(&id())).unwrap_err(),
            RULE_TIMEOUT
        );
    }
    assert_eq!(fw.asker().unwrap().waiting(), 2);
    // The user adds a rule from a terminal (`paddock app rules NAME add`).
    let path = fw.rules().path.clone();
    let mut other = Rules::load_at(path.clone()).unwrap();
    let r = firewall::new_rule(
        "*",
        None,
        "q2.example",
        firewall::Port::Num(443),
        firewall::Verdict::Allow,
        Scope::Always,
        BY_USER,
        10,
    );
    other.add(r, false).unwrap();
    other.save().unwrap();
    fw.settle(crate::state::now());
    let a = fw.asker().unwrap();
    assert_eq!(a.waiting(), 1, "q2 is decided by the rule, q1 still asks");
    assert_eq!(a.open()[0].0.host, "q1.example");
    fw.connection("q2.example", 443, Some(&id()))
        .unwrap()
        .close(0, 0);
}

#[test]
fn the_app_side_comes_from_the_host_bundle() {
    let t = tempfile::tempdir().unwrap();
    let b = t.path().join("Sample Notes.app");
    std::fs::create_dir_all(b.join("Contents")).unwrap();
    std::fs::write(
        b.join("Contents/Info.plist"),
        "<plist><dict><key>CFBundleIdentifier</key><string>com.example.notes</string><key>CFBundleShortVersionString</key><string>1.0</string></dict></plist>",
    )
    .unwrap();
    let bundle = crate::macvm::appcopy::read(&b).unwrap();
    let side = app_side(Some(&bundle), false);
    assert_eq!(side.owner.bundle_id(), "com.example.notes");
    let in_copy = Identity {
        path: Some(format!(
            "{}/Contents/MacOS/X",
            bundle.guest_target().display()
        )),
        ..Default::default()
    };
    assert!(side.owner.owns(Some(&in_copy)));
    let curl = Identity {
        app: Some("com.apple.curl".into()),
        ..Default::default()
    };
    assert!(!side.owner.owns(Some(&curl)));
    assert!(app_side(Some(&bundle), true).owner.owns(Some(&curl)));
    assert_eq!(app_side(None, false).owner, AppOwner::nobody());
}

#[test]
fn a_switch_to_none_shuts_the_open_connections_and_denies_new_ones() {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    let t = tempfile::tempdir().unwrap();
    let fw = Arc::new(firewall(t.path(), Mode::Egress, None));
    let gate = fw.clone().gate();
    let (client, mut peer) = UnixStream::pair().unwrap();
    let mut o = (gate.0)("x.example", 443, Some(&id())).ok().unwrap();
    o.hold(&client).unwrap();
    assert_eq!(fw.open_count(), 1);
    // Another mode leaves the connection open.
    assert_eq!(fw.set_mode(Mode::Ask), 0);
    assert_eq!(fw.open_count(), 1);
    assert_eq!(fw.set_mode(Mode::None), 1);
    assert_eq!(fw.mode(), Mode::None);
    let mut buf = [0u8; 1];
    assert_eq!(peer.read(&mut buf).unwrap(), 0, "the app sees the end");
    assert_eq!(
        (gate.0)("x.example", 443, Some(&id())).err().unwrap(),
        "mode"
    );
    o.close(1, 2);
    assert_eq!(fw.open_count(), 0);
    // Back to egress: new connections pass, and each close forgets its stream.
    fw.set_mode(Mode::Egress);
    let (c2, _p2) = UnixStream::pair().unwrap();
    let mut o2 = (gate.0)("x.example", 443, Some(&id())).ok().unwrap();
    o2.hold(&c2).unwrap();
    assert_eq!(fw.open_count(), 1);
    o2.close(0, 0);
    assert_eq!(fw.open_count(), 0);
}

#[test]
fn a_connection_held_after_a_switch_to_none_is_shut_at_once() {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    let t = tempfile::tempdir().unwrap();
    let fw = Arc::new(firewall(t.path(), Mode::Egress, None));
    let gate = fw.clone().gate();
    let mut o = (gate.0)("x.example", 443, Some(&id())).ok().unwrap();
    // The switch comes between the decision and the hold.
    fw.set_mode(Mode::None);
    let (client, mut peer) = UnixStream::pair().unwrap();
    assert!(o.hold(&client).is_err());
    let mut buf = [0u8; 1];
    assert_eq!(peer.read(&mut buf).unwrap(), 0);
}

#[test]
fn a_rule_of_an_answer_goes_to_the_grant_changes() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let profile = t.path().join("profile");
    let fw = firewall(t.path(), Mode::Ask, None).with_changes(profile.clone(), home);
    let who = Who {
        app: "com.example.notes".into(),
        parent_app: None,
        team: None,
    };
    let triple = Triple {
        identity: None,
        app: "com.example.notes".into(),
        host: "api.example.com".into(),
        port: 443,
    };
    let word = fw.apply(ConnectAnswer::AllowAlways, &who, &triple, BY_USER, 7);
    assert!(word.starts_with("rule-"), "{word}");
    let fw_once = fw.apply(ConnectAnswer::AllowOnce, &who, &triple, BY_USER, 8);
    assert_eq!(fw_once, RULE_ONCE);
    fw.apply(ConnectAnswer::DenyAlways, &who, &triple, BY_APPROVER, 9);
    let g =
        super::super::grants::load(&super::super::session::Profile::new(profile.clone())).unwrap();
    let got: Vec<(String, String, super::super::grants::Source)> = g
        .history_changes
        .iter()
        .map(|c| (c.target.clone(), c.value.clone(), c.source))
        .collect();
    assert_eq!(
        got,
        [
            (
                "api.example.com:443".to_string(),
                "allow always (historical; order unknown)".to_string(),
                super::super::grants::Source::QuestionPanel
            ),
            (
                "api.example.com:443".to_string(),
                "allow always (historical; order unknown)".to_string(),
                super::super::grants::Source::Approver
            ),
            (
                "api.example.com:443".to_string(),
                "deny always (historical; order unknown)".to_string(),
                super::super::grants::Source::Approver
            ),
        ]
    );
    let events = std::fs::read_to_string(profile.join("logs").join("events.jsonl")).unwrap();
    assert_eq!(events.lines().count(), 3);
    let first: serde_json::Value = serde_json::from_str(events.lines().next().unwrap()).unwrap();
    assert_eq!(first["kind"], "grant_history");
    assert_eq!(first["grant_history"]["what"], "rule");
    assert_eq!(first["grant_history"]["source"], "question-panel");
    assert_eq!(
        first["text"],
        "grant rule add api.example.com:443 allow always (historical; order unknown) unknown"
    );
}

#[test]
fn switching_to_allowlist_closes_only_unlisted_connections() {
    use std::os::unix::net::UnixStream;
    let t = tempfile::tempdir().unwrap();
    let fw = Arc::new(Firewall::new(
        Mode::Egress,
        Some(Allowlist::parse(&["allowed.example:443"]).0),
        Rules::load_at(t.path().join("rules")).unwrap(),
        None,
        t.path().join("log"),
    ));
    let gate = fw.clone().gate();
    let (a, _ap) = UnixStream::pair().unwrap();
    let (b, _bp) = UnixStream::pair().unwrap();
    let mut allowed = (gate.0)("allowed.example", 443, None).ok().unwrap();
    let mut denied = (gate.0)("other.example", 443, None).ok().unwrap();
    allowed.hold(&a).unwrap();
    denied.hold(&b).unwrap();
    assert_eq!(fw.set_mode(Mode::Allowlist), 1);
    assert_eq!(fw.open_count(), 1);
}

#[test]
fn rejected_rule_is_not_recorded() {
    let t = tempfile::tempdir().unwrap();
    let profile = t.path().join("profile");
    let fw = firewall(t.path(), Mode::Ask, None).with_changes(profile.clone(), t.path().into());
    let who = Who {
        app: "app".into(),
        parent_app: None,
        team: None,
    };
    let triple = Triple {
        identity: None,
        app: "app".into(),
        host: "bad host".into(),
        port: 443,
    };
    fw.apply(ConnectAnswer::AllowAlways, &who, &triple, BY_USER, 1);
    assert!(
        super::super::grants::load(&super::super::session::Profile::new(profile))
            .unwrap()
            .history_changes
            .is_empty()
    );
}

#[test]
fn grant_note_does_not_block_on_another_writer() {
    let t = tempfile::tempdir().unwrap();
    let profile = super::super::session::Profile::new(t.path().join("profile"));
    let _lock = super::super::grants::Lock::take(&profile).unwrap();
    let fw = firewall(t.path(), Mode::Ask, None).with_changes(profile.dir.clone(), t.path().into());
    let who = Who {
        app: "app".into(),
        parent_app: None,
        team: None,
    };
    let triple = Triple {
        identity: None,
        app: "app".into(),
        host: "example.com".into(),
        port: 443,
    };
    let start = Instant::now();
    fw.apply(ConnectAnswer::AllowSession, &who, &triple, BY_USER, 1);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_rejected_rule_does_not_publish_a_grant() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    GRANT_EVENT_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |event| sink.borrow_mut().push(event)))
    });
    let asker = Asker::new(
        env,
        "grant-rejection-test",
        "App",
        t.path().into(),
        std::process::id(),
    );
    let profile = super::super::session::Profile::new(t.path().join("profile"));
    let fw = firewall(t.path(), Mode::Ask, Some(asker))
        .with_changes(profile.dir.clone(), t.path().into());
    let who = Who {
        app: "app".into(),
        ..Default::default()
    };
    let mut triple = Triple {
        identity: None,
        app: "app".into(),
        host: "valid.example".into(),
        port: 443,
    };
    fw.apply_with_bus(
        ConnectAnswer::AllowAlways,
        &who,
        &triple,
        BY_APPROVER,
        1,
        None,
    );
    assert_eq!(events.borrow().len(), 1);
    assert!(matches!(
        &events.borrow()[0],
        crate::eventstream::Event::Stream(_)
    ));
    let history = || {
        super::super::grants::load(&profile)
            .unwrap()
            .history_changes
    };
    assert_eq!(history().len(), 1);
    triple.host = "bad host".into();
    fw.apply_with_bus(
        ConnectAnswer::AllowAlways,
        &who,
        &triple,
        BY_APPROVER,
        2,
        None,
    );
    assert_eq!(events.borrow().len(), 1);
    assert_eq!(history().len(), 1);
    GRANT_EVENT_HOOK.with(|hook| *hook.borrow_mut() = None);
}

#[test]
fn an_app_that_showed_a_window_may_ask() {
    let t = tempfile::tempdir().unwrap();
    let mut fw = firewall(t.path(), Mode::Ask, None);
    fw.owner = Some(AppOwner::new("com.example.tool", &[]));
    let safari = Identity {
        pid: Some(7),
        app: Some("com.apple.Safari".into()),
        ..Identity::default()
    };
    assert!(!fw.windowed(Some(&safari)));
    fw.saw_window_of("system");
    assert!(!fw.windowed(Some(&safari)));
    fw.saw_window_of("com.apple.Safari");
    assert!(fw.windowed(Some(&safari)));
    assert!(!fw.windowed(None));
}
