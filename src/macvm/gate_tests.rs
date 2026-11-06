use super::*;
use crate::broker;
use std::time::Duration;

fn firewall(t: &std::path::Path, mode: Mode, asker: Option<Asker>) -> Firewall {
    let mut rules = Rules::load_at(t.join("firewall").join("app.toml")).unwrap();
    rules.seed(&["chatgpt.com".into()], 1);
    Firewall::new(mode, None, rules, asker, t.join("logs").join(connlog::FILE))
}

fn id() -> Identity {
    Identity {
        app: Some("com.openai.chat".into()),
        team: Some("T1".into()),
        path: Some("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT".into()),
        ..Default::default()
    }
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
    assert_eq!(fw.mode, Mode::Ask);
    assert!(fw.asker().is_some());
    assert!(!fw.rules().seeds.is_empty(), "the seeds of the profile");
    assert!(!log.exists(), "a new session starts a new log");
    assert!(super::super::logs::numbered(&log, 1).exists());
    // Mode api with a list: no asker, the list before the rules.
    let fw = build(
        &env,
        "chatgpt-desktop",
        t.path(),
        log.clone(),
        "api",
        &["api.openai.com".into()],
        1,
        AppSide::default(),
    );
    assert!(fw.asker().is_none());
    assert_eq!(
        fw.connection("evil.example", 443, Some(&id())).unwrap_err(),
        "mode"
    );
    fw.connection("api.openai.com", 443, Some(&id()))
        .unwrap()
        .close(0, 0);
    assert_eq!(
        fw.connection("api.openai.com", 80, Some(&id()))
            .unwrap_err(),
        "mode"
    );
    // An unknown mode word is ask; a broken rules file is kept as it is.
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
    assert_eq!(fw.mode, Mode::Ask);
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
    let o = fw.connection("chatgpt.com", 443, Some(&id())).unwrap();
    o.close(10, 20);
    // No rule in egress: allowed, `egress`.
    fw.connection("other.example", 443, Some(&id()))
        .unwrap()
        .close(1, 1);
    // Mode none: denied before the rules, `mode`.
    let none = firewall(t.path(), Mode::None, None);
    assert_eq!(
        none.connection("chatgpt.com", 443, Some(&id()))
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
        ("com.openai.chat".into(), "allow".into(), "egress".into())
    );
    assert_eq!(
        words[2],
        ("com.openai.chat".into(), "deny".into(), "mode".into())
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
    .rate(100);
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
        q.detail.contains("/Applications/ChatGPT.app"),
        "{}",
        q.detail
    );
    assert_eq!(
        q.choices,
        [
            "allow-once",
            "allow-session",
            "allow-always",
            "deny",
            "deny-always"
        ]
    );
    let c = q.connect.clone().unwrap();
    assert_eq!(
        (c.app.as_str(), c.host.as_str(), c.port, c.count),
        ("com.openai.chat", "t.example", 443, 1)
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
        "com.openai.chat",
        &[std::path::Path::new(
            "/Users/paddock/Applications/ChatGPT.app",
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
    fw.connection("chatgpt.com", 443, Some(&id))
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
        app: Some("com.openai.chat.helper".into()),
        parent_app: Some("com.openai.chat".into()),
        path: Some(
            "/Users/paddock/Applications/ChatGPT.app/Contents/Frameworks/H.app/Contents/MacOS/H"
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
    let b = t.path().join("ChatGPT.app");
    std::fs::create_dir_all(b.join("Contents")).unwrap();
    std::fs::write(
        b.join("Contents/Info.plist"),
        "<plist><dict><key>CFBundleIdentifier</key><string>com.openai.chat</string><key>CFBundleShortVersionString</key><string>1.0</string></dict></plist>",
    )
    .unwrap();
    let bundle = crate::macvm::appcopy::read(&b).unwrap();
    let side = app_side(Some(&bundle), false);
    assert_eq!(side.owner.bundle_id(), "com.openai.chat");
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
