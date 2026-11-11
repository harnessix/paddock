use super::*;

const TOKEN: &str = "sk-test-Ab3dEf9hIj2kLm5nOp8qRs";
#[test]
fn the_guard_finds_a_token_in_the_answer() {
    let mut g = Guard::default();
    g.add_file(
        "the login file .x/auth.json of fake",
        &format!(r#"{{"tokens":{{"access_token":"{TOKEN}","expires_at":"2026-11-02T10:00:00Z"}},"email":"someone@example.org"}}"#),
    );
    assert!(!g.is_empty());
    assert_eq!(
        g.find(&format!("Here it is: {TOKEN}. Done.")),
        vec!["the login file .x/auth.json of fake".to_string()]
    );
    // A piece of 16 characters is enough.
    assert!(!g.find(&TOKEN[4..20]).is_empty());
    // Split by whitespace, or in other case.
    let split: String = TOKEN
        .chars()
        .enumerate()
        .flat_map(|(i, c)| if i % 3 == 0 { vec![' ', c] } else { vec![c] })
        .collect();
    assert!(!g.find(&split).is_empty(), "{split}");
    assert!(!g.find(&format!("{}\n", TOKEN.to_uppercase())).is_empty());
    assert!(!g.find(&TOKEN.replace("Ab3d", "Ab3\u{200b}d")).is_empty());
}

#[test]
fn short_words_and_names_never_match() {
    let mut g = Guard::default();
    g.add_file(
        "f",
        &format!(r#"{{"access_token":"{TOKEN}","refreshTokenExpiresAt":"x","path":"/home/demo/.config/tool","email":"someone@example.org","expires":"2026-11-02T10:00:00.000Z"}}"#),
    );
    for ok in [
        "sk-test",
        "access_token",
        "refreshTokenExpiresAt",
        "the answer is 42",
        "/home/demo/.config/tool",
        "someone@example.org",
        "2026-11-02T10:00:00.000Z",
        &TOKEN[..15],
    ] {
        assert!(g.find(ok).is_empty(), "{ok}");
    }
    // A secret shorter than the minimum has no marks.
    assert!(marks("short-secret-1").is_empty());
}

#[test]
fn delivered_marks_match_like_values() {
    let mut g = Guard::default();
    g.add_marks("the credential API of b", &marks("ghp_0123456789abcdefXYZ"));
    assert_eq!(
        g.find("token: GHP_0123456789ABCDEF xyz"),
        vec!["the credential API of b".to_string()]
    );
    assert!(g.find("ghp_0123").is_empty());
}

#[test]
fn plain_files_give_their_words() {
    let t = tokens("machine example.org\nlogin demo\npassword Zq7wX2pL9mNb4vC8kD\n");
    assert_eq!(t, vec!["Zq7wX2pL9mNb4vC8kD".to_string()]);
}

#[test]
fn the_tail_keeps_the_end_within_the_limit() {
    let big = format!("{}END", "x".repeat(MAX_ANSWER + 100));
    let (t, cut) = tail(big.as_bytes(), MAX_ANSWER);
    assert!(cut);
    assert_eq!(t.len(), MAX_ANSWER);
    assert!(t.ends_with("END"));
    let (t, cut) = tail("short".as_bytes(), MAX_ANSWER);
    assert_eq!((t.as_str(), cut), ("short", false));
    // A cut inside a character starts at the next one.
    let (t, cut) = tail("ééé".as_bytes(), 3);
    assert_eq!((t.as_str(), cut), ("é", true));
}

fn r(id: &str, harness: &str) -> Running {
    serde_json::from_value(serde_json::json!({
        "id": id, "harness": harness, "project": "/p", "backend": "bwrap",
        "network": "none", "gui": "off", "pid": 1, "started": 0,
    }))
    .unwrap()
}

#[test]
fn routes_by_harness_or_sandbox_id() {
    let all = vec![
        r("a1", "claude"),
        r("b1", "codex"),
        r("c1", "sh"),
        r("c2", "sh"),
    ];
    assert_eq!(route(&all, "a1", "codex").unwrap().id, "b1");
    assert_eq!(route(&all, "a1", "c2").unwrap().id, "c2");
    let (s, m) = route(&all, "a1", "sh").unwrap_err();
    assert_eq!(s, Status::Invalid);
    assert!(m.contains("c1, c2"), "{m}");
    let (s, m) = route(&all, "a1", "gemini").unwrap_err();
    assert_eq!(s, Status::NotFound);
    assert!(m.contains("codex (b1)"), "{m}");
    assert_eq!(route(&all, "a1", "a1").unwrap_err().0, Status::Invalid);
    // Its own harness names only the others.
    assert_eq!(route(&all, "c1", "sh").unwrap().id, "c2");
}

#[test]
fn the_host_checks_size_timeout_and_busy_before_it_asks() {
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().join("home"),
        "demo".into(),
        1000,
        1000,
        Default::default(),
    );
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        env.paddock_config().join("config.toml"),
        "[pastures.team]\nmembers = [\"sh\"]\nbridge = true\n",
    )
    .unwrap();
    let project = Path::new("/p");
    let mut b = r("b1", "codex");
    b.pid = std::process::id();
    b.kv_project = Some(project.into());
    b.pasture = Some("team".into());
    b.register(&env).unwrap();
    let base = dir(&env, project, "team");
    let rec = MemberRecord {
        oneshot: true,
        ..Default::default()
    };
    record_member(&base, "b1", &rec).unwrap();
    let id = Identity {
        sandbox: "a1".into(),
        harness: "sh".into(),
        project: project.into(),
        pasture: Some("team".into()),
        parent: None,
        chain: vec![],
        tree: None,
    };
    let req = |prompt: String, timeout: Option<u64>| Request {
        v: crate::kv::PROTOCOL,
        op: "ask".into(),
        key: Some("codex".into()),
        value: Some(prompt),
        timeout,
        ..Default::default()
    };
    let big = "x".repeat(MAX_PROMPT + 1);
    assert_eq!(ask(&env, &id, &req(big, None)).status, Status::TooLarge);
    // More than [spawn] timeout.
    let r = ask(&env, &id, &req("q".into(), Some(100_000)));
    assert_eq!(r.status, Status::Invalid, "{}", r.message);
    // Another question holds the target.
    let sp = spool(&base, "b1");
    crate::privfs::dir(&sp).unwrap();
    let held = try_lock(&sp.join("lock")).unwrap();
    let r = ask(&env, &id, &req("q".into(), Some(5)));
    assert_eq!(r.status, Status::Conflict, "{}", r.message);
    assert!(r.message.contains("another question"), "{}", r.message);
    drop(held);
    // The audit names the sizes, never the text.
    let audit = std::fs::read_to_string(crate::kv::audit_file(&env)).unwrap();
    assert!(audit.contains("\"result\":\"busy\""), "{audit}");
    assert!(audit.contains("\"target_harness\":\"codex\""), "{audit}");
    assert!(!audit.contains("xxxx"));
}

/// The helper runs in the target sandbox. An error it sends is checked
/// like the answer text. A clean error still reaches the asker.
#[test]
fn the_guard_checks_a_secret_in_the_error() {
    if crate::storage::package_cache::isolated_test(
        "pasture::bridge::tests::the_guard_checks_a_secret_in_the_error",
    ) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        tmp.path().join("home"),
        "demo".into(),
        1000,
        1000,
        Default::default(),
    );
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        env.paddock_config().join("config.toml"),
        "[pastures.team]\nmembers = [\"sh\"]\nbridge = true\n",
    )
    .unwrap();
    let project = Path::new("/p");
    let mut b = r("b1", "codex");
    b.pid = std::process::id();
    b.kv_project = Some(project.into());
    b.pasture = Some("team".into());
    b.register(&env).unwrap();
    let home = tmp.path().join("b-home");
    std::fs::create_dir_all(home.join(".fake")).unwrap();
    std::fs::write(
        home.join(".fake/auth.json"),
        format!(r#"{{"token":"{TOKEN}"}}"#),
    )
    .unwrap();
    let base = dir(&env, project, "team");
    record_member(
        &base,
        "b1",
        &MemberRecord {
            oneshot: true,
            home,
            login: vec![".fake/auth.json".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let id = Identity {
        sandbox: "a1".into(),
        harness: "sh".into(),
        project: project.into(),
        pasture: Some("team".into()),
        parent: None,
        chain: vec![],
        tree: None,
    };
    let sp = spool(&base, "b1");
    let reply = |error: Option<String>| {
        let sp = sp.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            let q = loop {
                if let Ok(b) = std::fs::read(sp.join("question.json")) {
                    if let Ok(q) = serde_json::from_slice::<Question>(&b) {
                        break q;
                    }
                }
                assert!(
                    start.elapsed() < Duration::from_secs(3),
                    "the question was not written"
                );
                std::thread::sleep(Duration::from_millis(20));
            };
            let ans = Answer {
                id: q.id,
                error,
                ..Default::default()
            };
            std::fs::write(sp.join("answer.json"), serde_json::to_vec(&ans).unwrap()).unwrap();
        })
    };
    let req = |prompt: &str| Request {
        v: crate::kv::PROTOCOL,
        op: "ask".into(),
        key: Some("codex".into()),
        value: Some(prompt.into()),
        timeout: Some(8),
        ..Default::default()
    };
    let writer = reply(Some(format!("failed: {TOKEN}")));
    let r = ask(&env, &id, &req("hello"));
    writer.join().unwrap();
    assert_eq!(r.status, Status::Denied, "{}", r.message);
    assert!(!r.message.contains(TOKEN), "{}", r.message);
    assert!(!r.result.to_string().contains(TOKEN), "{}", r.result);
    let audit = std::fs::read_to_string(crate::kv::audit_file(&env)).unwrap();
    assert!(audit.contains("intercepted"), "{audit}");
    assert!(!audit.contains(TOKEN), "{audit}");

    let clean = "cannot start the harness";
    let writer = reply(Some(clean.into()));
    let r = ask(&env, &id, &req("again"));
    writer.join().unwrap();
    assert_eq!(r.status, Status::Failed, "{}", r.message);
    assert!(r.message.contains(clean), "{}", r.message);
    assert!(!r.message.contains(TOKEN), "{}", r.message);

    // A long error keeps its end and stays within the cap.
    let qid = "cap-1";
    crate::privfs::dir(&sp).unwrap();
    std::fs::write(
        sp.join("taken.json"),
        serde_json::to_vec(&Question {
            id: qid.into(),
            from: "a1".into(),
            from_harness: "sh".into(),
            prompt: "q".into(),
            timeout: 5,
        })
        .unwrap(),
    )
    .unwrap();
    let err = format!("{}END", "e".repeat(MAX_ERROR + 40));
    let body = serde_json::to_string(&Answer {
        id: qid.into(),
        error: Some(err),
        ..Default::default()
    })
    .unwrap();
    let target = Identity {
        sandbox: "b1".into(),
        harness: "codex".into(),
        project: project.into(),
        pasture: Some("team".into()),
        parent: None,
        chain: vec![],
        tree: None,
    };
    let stored = answer(
        &env,
        &target,
        &Request {
            v: crate::kv::PROTOCOL,
            op: "ask".into(),
            value: Some(body),
            ..Default::default()
        },
    );
    assert_eq!(stored.status, Status::Done, "{}", stored.message);
    let got: Answer =
        serde_json::from_slice(&std::fs::read(sp.join("answer.json")).unwrap()).unwrap();
    let e = got.error.unwrap();
    assert!(e.len() <= MAX_ERROR, "{}", e.len());
    assert!(e.ends_with("END"), "{e}");
}

#[test]
fn one_run_answers_on_stdin_and_through_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let q = Question {
        id: "q1".into(),
        from: "a1".into(),
        from_harness: "sh".into(),
        prompt: "what is two and two".into(),
        timeout: 10,
    };
    let stdin = Command {
        argv: vec![
            "sh".into(),
            "-c".into(),
            "printf 'log\\n'; read l; echo \"answer: $l\"".into(),
        ],
        via: PromptVia::Stdin,
        prompt_file: String::new(),
    };
    let a = run(&stdin, &q);
    assert_eq!(a.exit_code, Some(0), "{a:?}");
    assert!(a.text.ends_with("answer: what is two and two\n"), "{a:?}");
    let file = tmp.path().join("h/.paddock-bridge/prompt.md");
    let o: crate::profile::OneShot = toml::from_str(
        "args = [\"-c\", \"cat \\\"$0\\\"\", \"{prompt_file}\"]\nprompt = \"file\"\n",
    )
    .unwrap();
    let c = Command::of(Path::new("sh"), vec![], &o, &tmp.path().join("h"));
    assert_eq!(c.prompt_file, file.display().to_string());
    let a = run(&c, &q);
    assert_eq!(a.text, "what is two and two", "{a:?}");
    assert!(!file.exists());
    // A run that takes too long ends with an error.
    let slow = Command {
        argv: vec!["sh".into(), "-c".into(), "sleep 30".into()],
        via: PromptVia::Stdin,
        prompt_file: String::new(),
    };
    let a = run(&slow, &Question { timeout: 1, ..q });
    assert!(a.error.unwrap().contains("did not finish"));
}
