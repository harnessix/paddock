use super::*;
use crate::broker::ConnectAnswer;
use std::os::unix::fs::PermissionsExt;

fn rule(app: &str, host: &str, port: Port, verdict: Verdict, scope: Scope, by: &str) -> Rule {
    new_rule(app, None, host, port, verdict, scope, by, 1_762_250_000)
}

fn allow(app: &str, host: &str) -> Rule {
    rule(
        app,
        host,
        Port::Num(443),
        Verdict::Allow,
        Scope::Always,
        BY_USER,
    )
}

fn deny(app: &str, host: &str) -> Rule {
    rule(
        app,
        host,
        Port::Num(443),
        Verdict::Deny,
        Scope::Always,
        BY_USER,
    )
}

#[test]
fn glob_cases() {
    // Exact, case and a trailing dot aside.
    assert!(host_matches("chat.example", "chat.example"));
    assert!(host_matches("Chat.Example", "chat.example."));
    assert!(!host_matches("chat.example", "ab.chat.example"));
    assert!(!host_matches("chat.example", "evilchat.example"));
    // `*.x.y`: every name under x.y, not x.y itself.
    assert!(host_matches("*.service.example", "auth.service.example"));
    assert!(host_matches("*.service.example", "a.b.service.example"));
    assert!(!host_matches("*.service.example", "service.example"));
    assert!(!host_matches("*.service.example", "evilservice.example"));
    // `*`: everything, an IP too.
    assert!(host_matches("*", "anything.example"));
    assert!(host_matches("*", "10.0.0.5"));
    // An IP: the same address in any spelling, never a name.
    assert!(host_matches("10.0.0.5", "10.0.0.5"));
    assert!(host_matches("::1", "0:0:0:0:0:0:0:1"));
    assert!(host_matches("[::1]", "::1"));
    assert!(!host_matches("10.0.0.5", "10.0.0.50"));
    assert!(!host_matches("*.0.0.5", "10.0.0.5"));
    // The glob check of `add`.
    for ok in [
        "*",
        "*.service.example",
        "chat.example",
        "10.0.0.5",
        "::1",
        "x_y.example",
    ] {
        assert!(check_host_glob(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "a b",
        "*.",
        "**",
        "chat.example:443",
        "-",
        "a..b",
        "http://x",
    ] {
        assert!(check_host_glob(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn reload_checks_content_even_when_the_recorded_timestamp_matches() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("rules.toml");
    let mut writer = Rules::load_at(path.clone()).unwrap();
    writer.add(allow("*", "removed.example"), false).unwrap();
    writer.save().unwrap();
    let mut live = Rules::load_at(path).unwrap();
    writer.file.clear();
    writer.save().unwrap();
    live.modified = writer.modified;
    assert!(live.reload_if_changed().unwrap());
    assert!(live.file.is_empty());
}

#[test]
fn a_rule_writer_and_a_decision_use_the_same_lock_inode() {
    use std::os::unix::io::AsRawFd;
    let t = tempfile::tempdir().unwrap();
    let rules = Rules::load_at(t.path().join("rules.toml")).unwrap();
    let _guard = rules.decision_lock().unwrap();
    let probe = std::fs::OpenOptions::new()
        .write(true)
        .open(t.path().join("rules.toml.lock"))
        .unwrap();
    let rc = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_ne!(rc, 0);
    assert_eq!(
        std::io::Error::last_os_error().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn concurrent_stale_writer_cannot_resurrect_a_removed_rule() {
    use std::sync::mpsc;
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("rules.toml");
    let mut seed = Rules::load_at(path.clone()).unwrap();
    let original = allow("*", "removed.example");
    seed.add(original.clone(), false).unwrap();
    seed.save().unwrap();

    let (loaded_tx, loaded_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let stale_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut stale = Rules::load_at(stale_path).unwrap();
        stale.add(allow("*", "new.example"), false).unwrap();
        loaded_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        stale.save()
    });
    loaded_rx.recv().unwrap();
    let mut remover = Rules::load_at(path.clone()).unwrap();
    remover.remove(&original.id, "app").unwrap();
    remover.save().unwrap();
    go_tx.send(()).unwrap();
    assert!(writer.join().unwrap().is_err());
    let saved = Rules::load_at(path).unwrap();
    assert!(saved.file.is_empty(), "the removed allow must not return");
}

#[test]
fn specificity_order() {
    let exact = allow("com.example.notes", "chat.example");
    let any_port = rule(
        "com.example.notes",
        "chat.example",
        Port::Any,
        Verdict::Allow,
        Scope::Always,
        BY_USER,
    );
    let sub = allow("com.example.notes", "*.chat.example");
    let deep_sub = allow("com.example.notes", "*.a.chat.example");
    let star_host = allow("com.example.notes", "*");
    let any_app = allow("*", "chat.example");
    assert!(specificity(&exact) > specificity(&any_port));
    assert!(
        specificity(&any_port) > specificity(&sub),
        "a wildcard port costs less than a wildcard host"
    );
    assert!(specificity(&deep_sub) > specificity(&sub));
    assert!(specificity(&sub) > specificity(&star_host));
    assert!(
        specificity(&star_host) > specificity(&any_app),
        "a wildcard app costs the most"
    );
    // The winner is the most specific match: a deny for the exact host
    // under an allow for the domain, and the other way round.
    let conn = Conn::new("com.example.notes", "ab.chat.example", 443);
    let rules = vec![
        allow("*", "*.chat.example"),
        deny("com.example.notes", "ab.chat.example"),
    ];
    assert_eq!(winner(&rules, &conn).unwrap().id, rules[1].id);
    let rules = vec![deny("*", "*"), allow("com.example.notes", "*.chat.example")];
    assert_eq!(winner(&rules, &conn).unwrap().id, rules[1].id);
    // A helper matches the rule of its outermost app.
    let id = Identity {
        app: Some("com.example.notes.helper".into()),
        parent_app: Some("com.example.notes".into()),
        team: Some("T1".into()),
        ..Default::default()
    };
    let name = id.app_name();
    let conn = Conn::of(&id, &name, "chat.example", 443);
    assert!(matches(&allow("com.example.notes", "chat.example"), &conn));
    assert!(matches(
        &allow("com.example.notes.helper", "chat.example"),
        &conn
    ));
    assert!(!matches(&allow("com.other", "chat.example"), &conn));
    // A team in the rule must match.
    let mut teamed = allow("com.example.notes", "chat.example");
    teamed.team = Some("T2".into());
    assert!(!matches(&teamed, &conn));
    teamed.team = Some("T1".into());
    assert!(matches(&teamed, &conn));
    // The port.
    assert!(!matches(
        &allow("*", "chat.example"),
        &Conn::new("x", "chat.example", 80)
    ));
    assert!(matches(
        &rule(
            "*",
            "chat.example",
            Port::Any,
            Verdict::Allow,
            Scope::Always,
            BY_USER
        ),
        &Conn::new("x", "chat.example", 80)
    ));
}

#[test]
fn deny_wins_ties() {
    let conn = Conn::new("com.example.notes", "chat.example", 443);
    let a = allow("com.example.notes", "chat.example");
    let d = deny("com.example.notes", "chat.example");
    assert_eq!(winner(&[a.clone(), d.clone()], &conn).unwrap().id, d.id);
    assert_eq!(winner(&[d.clone(), a.clone()], &conn).unwrap().id, d.id);
    // A user rule beats a seed of the same specificity, even an allow
    // over a seed deny would (seeds are allows, so the case is a deny).
    let seed = seed_rule("chat.example", 1);
    let mut user_deny = deny("*", "chat.example");
    user_deny.port = Port::Num(443);
    assert_eq!(
        winner(&[seed.clone(), user_deny.clone()], &conn)
            .unwrap()
            .id,
        user_deny.id
    );
    assert_eq!(
        decide(Mode::Ask, None, &[seed], &conn),
        Decision::Allow(Some(format!("seed-{}", hash6(&["chat.example"]))))
    );
}

#[test]
fn session_beats_always() {
    let conn = Conn::new("com.example.notes", "chat.example", 443);
    let always_allow = allow("com.example.notes", "chat.example");
    let session_deny = rule(
        "com.example.notes",
        "chat.example",
        Port::Num(443),
        Verdict::Deny,
        Scope::Session,
        BY_USER,
    );
    assert!(session_deny.id.starts_with("session-"));
    assert!(always_allow.id.starts_with("rule-"));
    let rules = [always_allow.clone(), session_deny.clone()];
    assert_eq!(winner(&rules, &conn).unwrap().id, session_deny.id);
    // And a session allow over an always deny: a one-session exception.
    let always_deny = deny("com.example.notes", "chat.example");
    let session_allow = rule(
        "com.example.notes",
        "chat.example",
        Port::Num(443),
        Verdict::Allow,
        Scope::Session,
        BY_USER,
    );
    assert_eq!(
        decide(
            Mode::Ask,
            None,
            &[always_deny, session_allow.clone()],
            &conn
        ),
        Decision::Allow(Some(session_allow.id))
    );
}

#[test]
fn api_list_before_rules() {
    // Checklist item 6: a forged identity with an allow rule for a host
    // outside the list of mode api is denied before any rule.
    let (list, _) = Allowlist::parse(&["api.service.example"]);
    let evil = Conn::new("com.example.notes", "evil.example", 443);
    let rules = [allow("com.example.notes", "evil.example"), allow("*", "*")];
    assert_eq!(
        decide(Mode::Api, Some(&list), &rules, &evil),
        Decision::Deny(None)
    );
    assert_eq!(
        decide(Mode::Allowlist, Some(&list), &rules, &evil),
        Decision::Deny(None)
    );
    assert_eq!(
        decide(Mode::Api, None, &rules, &evil),
        Decision::Deny(None),
        "no list: nothing passes"
    );
    // A host on the list passes, and a deny rule still denies it.
    let ok = Conn::new("com.example.notes", "api.service.example", 443);
    assert_eq!(
        decide(Mode::Api, Some(&list), &[], &ok),
        Decision::Allow(None)
    );
    let d = deny("*", "api.service.example");
    assert_eq!(
        decide(Mode::Api, Some(&list), std::slice::from_ref(&d), &ok),
        Decision::Deny(Some(d.id))
    );
    // egress with no rule allows, ask with no rule asks, none denies first.
    assert_eq!(
        decide(Mode::Egress, None, &[], &evil),
        Decision::Allow(None)
    );
    assert_eq!(decide(Mode::Ask, None, &[], &evil), Decision::Ask);
    assert_eq!(
        decide(Mode::None, None, &[allow("*", "*")], &evil),
        Decision::Deny(None)
    );
    // Modes parse and print.
    for m in [
        Mode::None,
        Mode::Api,
        Mode::Allowlist,
        Mode::Egress,
        Mode::Ask,
    ] {
        assert_eq!(Mode::parse(m.word()), Some(m));
    }
    assert_eq!(Mode::parse("host"), None);
}

#[test]
fn answers_write_rules() {
    let id = Identity {
        app: Some("com.example.notes.helper".into()),
        parent_app: Some("com.example.notes".into()),
        team: Some("EXAMPLE123".into()),
        ..Default::default()
    };
    let name = id.app_name();
    let conn = Conn::of(&id, &name, "Telemetry.Example.com", 443);
    let now = 1_762_250_000;
    assert_eq!(
        rule_of_answer(ConnectAnswer::AllowOnce, &conn, BY_USER, now),
        None
    );
    assert_eq!(
        rule_of_answer(ConnectAnswer::Deny, &conn, BY_USER, now),
        None
    );
    let s = rule_of_answer(ConnectAnswer::AllowSession, &conn, BY_USER, now).unwrap();
    assert_eq!((s.verdict, s.scope), (Verdict::Allow, Scope::Session));
    assert_eq!(
        s.app, "com.example.notes",
        "the rule is for the outermost app"
    );
    assert_eq!(s.team.as_deref(), Some("EXAMPLE123"));
    assert_eq!(s.host, "telemetry.example.com");
    assert_eq!(s.port, Port::Num(443));
    assert_eq!(s.made, "2025-11-04T09:53:20Z");
    assert_eq!(s.by, "user");
    let a = rule_of_answer(ConnectAnswer::AllowAlways, &conn, BY_APPROVER, now).unwrap();
    assert_eq!(
        (a.verdict, a.scope, a.by.as_str()),
        (Verdict::Allow, Scope::Always, "approver")
    );
    let d = rule_of_answer(ConnectAnswer::DenyAlways, &conn, BY_USER, now).unwrap();
    assert_eq!((d.verdict, d.scope), (Verdict::Deny, Scope::Always));
    // Into the rules: a session rule to the session, the others to the file.
    let t = tempfile::tempdir().unwrap();
    let mut rules = Rules::load_at(t.path().join("firewall").join("sample-notes.toml")).unwrap();
    rules.add(s.clone(), false).unwrap();
    rules.add(a.clone(), false).unwrap();
    rules.add(d.clone(), false).unwrap();
    assert_eq!(rules.session.len(), 1);
    assert_eq!(
        rules.file.len(),
        1,
        "the deny replaced the allow of the same target"
    );
    assert_eq!(rules.file[0].id, d.id);
    rules.save().unwrap();
    let back = Rules::load_at(rules.path.clone()).unwrap();
    assert_eq!(back.file, rules.file);
    assert!(
        back.session.is_empty(),
        "session rules never reach the file"
    );
    // The session allow decides over the file deny of the same target.
    assert_eq!(
        decide(Mode::Ask, None, &rules.all(), &conn),
        Decision::Allow(Some(s.id.clone()))
    );
}

#[test]
fn session_rule_beats_file_rule_of_same_target() {
    // The case the test above ends on, spelled out: a session allow and a
    // file deny for the same target, the session rule wins.
    let conn = Conn::new("com.example.notes", "t.example", 443);
    let now = 1;
    let s = rule_of_answer(ConnectAnswer::AllowSession, &conn, BY_USER, now).unwrap();
    let d = rule_of_answer(ConnectAnswer::DenyAlways, &conn, BY_USER, now).unwrap();
    assert_eq!(
        decide(Mode::Ask, None, &[d, s.clone()], &conn),
        Decision::Allow(Some(s.id))
    );
}

#[test]
fn all_hosts_allow_needs_flag() {
    let t = tempfile::tempdir().unwrap();
    let mut rules = Rules::load_at(t.path().join("a.toml")).unwrap();
    let every = allow("*", "*");
    assert!(all_hosts_allow(&every));
    let e = rules.add(every.clone(), false).unwrap_err().to_string();
    assert!(e.contains("--yes-all-hosts"), "{e}");
    assert!(rules.file.is_empty());
    rules.add(every.clone(), true).unwrap();
    assert_eq!(rules.file.len(), 1);
    // A deny of every host needs no flag, and a wildcard under a domain
    // neither.
    rules.add(deny("*", "*"), false).unwrap();
    rules.add(allow("*", "*.example.com"), false).unwrap();
    // A rule with a bad host is refused.
    let mut bad = allow("*", "a b");
    bad.host = "a b".into();
    assert!(rules.add(bad, false).is_err());
    // `remove`: by id; a seed cannot go; an unknown id says so.
    rules.seed(&["chat.example".into(), "not a host".into()], 1);
    assert_eq!(rules.seeds.len(), 1, "a seed that is no glob is skipped");
    let seed_id = rules.seeds[0].id.clone();
    let e = rules
        .remove(&seed_id, "sample-notes")
        .unwrap_err()
        .to_string();
    assert!(e.contains("seed rule of the profile sample-notes"), "{e}");
    let e = rules
        .remove("rule-nope", "sample-notes")
        .unwrap_err()
        .to_string();
    assert!(e.contains("no rule rule-nope"), "{e}");
    let id = rules.file[0].id.clone();
    assert_eq!(rules.remove(&id, "x").unwrap().id, id);
    assert!(rules.find(&id).is_none());
    assert!(rules.find(&seed_id).is_some());
    assert_eq!(rules.all().len(), rules.file.len() + rules.seeds.len());
    assert!(rule_line(&rules.seeds[0]).contains("by profile"));
}

#[test]
fn save_mode_0600_atomic() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("firewall").join("app.toml");
    let mut rules = Rules::load_at(path.clone()).unwrap();
    assert!(!path.exists(), "a load makes no file");
    rules
        .add(allow("com.example.notes", "*.service.example"), false)
        .unwrap();
    rules.save().unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    // Whole and renamed: no temporary file stays, and the file parses.
    let mut names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["app.toml", "app.toml.lock"]);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# The firewall rules"), "{text}");
    assert!(text.contains("[[rule]]"));
    assert!(text.contains("host = \"*.service.example\""));
    assert!(text.contains("port = 443"));
    // A loosened mode is tightened at the next save.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    rules.add(deny("*", "*"), false).unwrap();
    rules.save().unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // `port = "*"` round trips, and a bad file is one clear line.
    let back = Rules::load_at(path.clone()).unwrap();
    assert_eq!(back.file, rules.file);
    let saved_text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, "[[rule]]\nid = 1\n").unwrap();
    let e = Rules::load_at(path.clone()).unwrap_err().to_string();
    assert!(e.contains("does not parse"), "{e}");
    std::fs::write(&path, "[[rule]]\nid = \"r\"\napp = \"*\"\nhost = \"*\"\nport = 0\nverdict = \"allow\"\nscope = \"always\"\nmade = \"\"\nby = \"user\"\n").unwrap();
    let e = Rules::load_at(path.clone()).unwrap_err().to_string();
    assert!(e.contains("port \"0\""), "{e}");
    // `reload_if_changed` sees a write of another process.
    let mut live = Rules::load_at(path.clone())
        .unwrap_or_else(|_| Rules::load_at(t.path().join("none.toml")).unwrap());
    live.path = path.clone();
    assert!(
        rules.save().is_err(),
        "a stale writer cannot overwrite the invalid edit"
    );
    std::fs::write(&path, saved_text).unwrap();
    rules.save().unwrap();
    assert!(live.reload_if_changed().unwrap());
    assert_eq!(live.file, rules.file);
    assert!(!live.reload_if_changed().unwrap());
    std::fs::remove_file(&path).unwrap();
    assert!(live.reload_if_changed().unwrap());
    assert!(live.file.is_empty());
}

#[test]
fn seed_covers_chatgpt_login_hosts() {
    #[derive(serde::Deserialize)]
    struct Fixture {
        hosts: Vec<Host>,
    }
    #[derive(serde::Deserialize)]
    struct Host {
        host: String,
        port: u16,
    }
    let f: Fixture = toml::from_str(include_str!("testdata/chatgpt-first-login.toml")).unwrap();
    assert!(f.hosts.len() >= 5);
    let profile = crate::profile::builtin("chatgpt-desktop").unwrap();
    let hosts = seed_hosts(Some(&profile));
    for h in [
        "chatgpt.com",
        "*.chatgpt.com",
        "auth.openai.com",
        "*.openai.com",
        "*.oaistatic.com",
        "*.oaiusercontent.com",
        "api.apple-cloudkit.com",
        "ocsp2.apple.com",
    ] {
        assert!(hosts.iter().any(|s| s == h), "the seed list names {h}");
    }
    let mut rules =
        Rules::load_at(std::env::temp_dir().join("paddock-never-written.toml")).unwrap();
    rules.seed(&hosts, 1);
    assert_eq!(rules.seeds.len(), hosts.len());
    for h in &f.hosts {
        let conn = Conn::new("com.openai.chat", &h.host, h.port);
        match decide(Mode::Ask, None, &rules.all(), &conn) {
            Decision::Allow(Some(id)) => assert!(id.starts_with("seed-"), "{id}"),
            other => panic!(
                "{}:{} of a first login would {other:?}: the first run would ask",
                h.host, h.port
            ),
        }
    }
    // The seed never covers a host outside the list.
    let outside = Conn::new("com.openai.chat", "telemetry.example.com", 443);
    assert_eq!(
        decide(Mode::Ask, None, &rules.all(), &outside),
        Decision::Ask
    );
    assert_eq!(
        decide(
            Mode::Ask,
            None,
            &rules.all(),
            &Conn::new("x", "evilopenai.com", 443)
        ),
        Decision::Ask
    );
    assert_eq!(
        decide(
            Mode::Ask,
            None,
            &rules.all(),
            &Conn::new("x", "chatgpt.com", 80)
        ),
        Decision::Ask
    );
    assert!(seed_hosts(None).is_empty());
}
