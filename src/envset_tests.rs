use super::*;
use crate::profile::Profile;

fn world(cfg: Option<&str>, vars: &[(&str, &str)]) -> (tempfile::TempDir, HostEnv, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let project = home.join("src/app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
    if let Some(c) = cfg {
        std::fs::write(home.join(".config/paddock/config.toml"), c).unwrap();
    }
    let mut v = BTreeMap::new();
    v.insert("PATH".to_string(), "/nonexistent-bin".to_string());
    for (k, val) in vars {
        v.insert(k.to_string(), val.to_string());
    }
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, v);
    let project = std::fs::canonicalize(project).unwrap();
    (t, env, project)
}

fn cfg_text(env: &HostEnv) -> String {
    std::fs::read_to_string(env.paddock_config().join("config.toml")).unwrap_or_default()
}

fn profile(name: &str, env_pass: &[&str], launch_env: &[(&str, &str)]) -> Profile {
    let mut p: Profile = toml::from_str(&format!("name = \"{name}\"\n")).unwrap();
    p.state.env_pass = env_pass.iter().map(|s| s.to_string()).collect();
    for (k, v) in launch_env {
        p.launch.env.insert(k.to_string(), v.to_string());
    }
    p
}

#[test]
fn the_never_list_names_each_kind_of_the_design() {
    for (name, why) in [
        ("SSH_AUTH_SOCK", Never::Socket),
        ("DBUS_SESSION_BUS_ADDRESS", Never::Socket),
        ("DYLD_INSERT_LIBRARIES", Never::Loader),
        ("BASH_ENV", Never::Loader),
        ("ENV", Never::Loader),
        ("PROMPT_COMMAND", Never::Loader),
        ("LD_PRELOAD", Never::Loader),
        ("BASH_FUNC_x%%", Never::Loader),
        ("HISTFILE", Never::History),
        ("ZDOTDIR", Never::History),
        ("HOME", Never::Sandbox),
        ("PADDOCK_ID", Never::Sandbox),
        ("MY_AGENT_SOCK", Never::Socket),
    ] {
        assert_eq!(policy::never_pass(name, Some("x")), Some(why), "{name}");
        // inherit mode drops them too: no switch.
        assert!(!policy::inheritable_var(name, "x"), "{name}");
    }
    for name in ["JAVA_HOME", "EDITOR", "CARGO_HOME", "GOFLAGS"] {
        assert_eq!(policy::never_pass(name, Some("/x")), None, "{name}");
    }
    // A value that is a socket makes any name a socket.
    let t = tempfile::tempdir().unwrap();
    let sock = t.path().join("s.sock");
    let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    assert_eq!(
        policy::never_pass("PLAIN", Some(sock.to_str().unwrap())),
        Some(Never::Socket)
    );
}

#[test]
fn a_refusal_names_the_reason_and_the_safe_way() {
    let r = refusal("SSH_AUTH_SOCK", None).unwrap();
    assert_eq!((r.rule, r.reason), ("never", "socket"));
    assert!(
        r.sentence
            .starts_with("SSH_AUTH_SOCK stays on this computer: "),
        "{}",
        r.sentence
    );
    assert!(r.sentence.contains("via = \"agent\""), "{}", r.sentence);
    let r = refusal("AWS_SECRET_ACCESS_KEY", None).unwrap();
    assert_eq!((r.rule, r.reason), ("key", "cloud"));
    assert!(r.sentence.contains("kind = \"aws\""), "{}", r.sentence);
    let r = refusal("MY_SERVICE_TOKEN", None).unwrap();
    assert_eq!((r.rule, r.reason), ("key", "secret"));
    assert!(
        r.sentence
            .contains("paddock credentials add MY_SERVICE_TOKEN"),
        "{}",
        r.sentence
    );
    let r = refusal("HISTFILE", None).unwrap();
    assert_eq!(r.reason, "history");
    assert!(refusal("JAVA_HOME", None).is_none());
}

#[test]
fn two_releases_force_the_new_default() {
    for (m, n, want) in [
        ("0.2.3", "0.2.3", false),
        ("0.2.3", "0.2.4", false),
        ("0.2.3", "0.2.5", true),
        ("0.2.3", "0.2.9", true),
        ("0.2.9", "0.3.0", false),
        ("0.2.9", "0.3.1", true),
        ("0.2.3", "0.4.0", true),
        ("0.2.3", "1.0.0", true),
        ("0.2.5", "0.2.3", false),
        ("0.2.3-rc.1", "0.2.5", true),
        ("garbage", "0.2.5", false),
    ] {
        assert_eq!(forced(m, n), want, "{m} -> {n}");
    }
    assert_eq!(forced_from("0.2.3"), "0.2.5");
}

#[test]
fn a_new_installation_gets_listed_and_a_current_one_inherit_with_a_review() {
    let (_t, env, _) = world(None, &[]);
    let r = resolve(&env, &Manifest::load_user(&env).unwrap());
    assert_eq!((r.mode, r.origin), (Mode::Listed, "new installation"));
    assert!(r.review.is_none());

    // Earlier runs: a current installation.
    std::fs::create_dir_all(env.paddock_state()).unwrap();
    std::fs::write(env.paddock_state().join("recent.json"), "[]").unwrap();
    let r = resolve_at(&env, &Manifest::load_user(&env).unwrap(), "0.2.3");
    assert_eq!((r.mode, r.origin), (Mode::Inherit, "current installation"));
    assert_eq!(r.review.unwrap().forced_from, "0.2.5");

    // `[env] hide` is a choice of the inherit era.
    let (_t, env, _) = world(Some("[env]\nhide = []\n"), &[]);
    assert_eq!(
        resolve(&env, &Manifest::load_user(&env).unwrap()).mode,
        Mode::Inherit
    );

    // An explicit choice wins; `inherit = false` still reads.
    let (_t, env, _) = world(Some("[env]\ninherit = false\nhide = [\"X\"]\n"), &[]);
    assert_eq!(
        resolve(&env, &Manifest::load_user(&env).unwrap()).mode,
        Mode::Listed
    );
    let (_t, env, _) = world(Some("[env]\nmode = \"inherit\"\ninherit = false\n"), &[]);
    let r = resolve(&env, &Manifest::load_user(&env).unwrap());
    assert_eq!(
        (r.mode, r.origin, r.review),
        (Mode::Inherit, "configuration", None)
    );
}

#[test]
fn the_migrated_inherit_mode_is_forced_two_releases_later() {
    let (_t, env, _) = world(
        Some("[env]\nmode = \"inherit\"\nmigrated = \"0.2.3\"\n"),
        &[],
    );
    let m = Manifest::load_user(&env).unwrap();
    let r = resolve_at(&env, &m, "0.2.4");
    assert_eq!(r.mode, Mode::Inherit);
    assert_eq!(r.review.unwrap().migrated, "0.2.3");
    let r = resolve_at(&env, &m, "0.2.5");
    assert_eq!((r.mode, r.origin), (Mode::Listed, "forced"));
    // An explicit `paddock env mode inherit` is never forced.
    set_mode(&env, Mode::Inherit).unwrap();
    let m = Manifest::load_user(&env).unwrap();
    assert!(m.env.migrated.is_none());
    assert_eq!(resolve_at(&env, &m, "9.0.0").mode, Mode::Inherit);
}

#[test]
fn settle_writes_the_choice_once_and_keeps_the_file() {
    // A new installation: a marker, and the configuration stays as it is.
    let (_t, env, _) = world(Some("# my notes\n[network]\nmode = \"egress\"\n"), &[]);
    assert!(!settle(&env).unwrap());
    assert_eq!(cfg_text(&env), "# my notes\n[network]\nmode = \"egress\"\n");
    // Later runs do not make it a current installation.
    std::fs::write(env.paddock_state().join("recent.json"), "[]").unwrap();
    assert_eq!(
        resolve(&env, &Manifest::load_user(&env).unwrap()).mode,
        Mode::Listed
    );

    // A current installation: mode = "inherit" with the version.
    let (_t, env, _) = world(Some("# my notes\n[env]\nhide = [\"A\"]\n"), &[]);
    assert!(settle(&env).unwrap());
    let text = cfg_text(&env);
    assert!(text.starts_with("# my notes\n"), "{text}");
    assert!(text.contains("mode = \"inherit\""), "{text}");
    assert!(
        text.contains(&format!("migrated = \"{}\"", version())),
        "{text}"
    );
    // Once.
    assert!(!settle(&env).unwrap());
    assert_eq!(cfg_text(&env), text);
}

#[test]
fn add_and_remove_write_each_scope_and_refuse_held_names() {
    let (_t, env, project) = world(Some("# keep me\n"), &[("JAVA_HOME", "/jvm")]);
    add(&env, &Scope::User, "JAVA_HOME", None).unwrap();
    add(
        &env,
        &Scope::Agent("claude".into()),
        "ANTHROPIC_MODEL",
        None,
    )
    .unwrap();
    let key = project_key(&env, &project);
    assert_eq!(key, "~/src/app");
    add(&env, &Scope::Project(key.clone()), "APP_ENV", Some("test")).unwrap();
    let m = Manifest::load_user(&env).unwrap();
    assert_eq!(m.env.pass, ["JAVA_HOME"]);
    assert_eq!(m.env.agent["claude"].pass, ["ANTHROPIC_MODEL"]);
    assert_eq!(m.env.project[&key].set["APP_ENV"], "test");
    assert!(cfg_text(&env).contains("# keep me\n"), "{}", cfg_text(&env));
    // Twice is once; a fixed value replaces the name in pass.
    add(&env, &Scope::User, "JAVA_HOME", None).unwrap();
    assert_eq!(Manifest::load_user(&env).unwrap().env.pass, ["JAVA_HOME"]);
    add(&env, &Scope::User, "JAVA_HOME", Some("/other")).unwrap();
    let m = Manifest::load_user(&env).unwrap();
    assert!(m.env.pass.is_empty());
    assert_eq!(m.env.set["JAVA_HOME"], "/other");

    for (name, reason) in [
        ("SSH_AUTH_SOCK", "socket"),
        ("BASH_ENV", "loader"),
        ("HISTFILE", "history"),
        ("GITHUB_TOKEN", "cloud"),
        ("MY_API_KEY", "secret"),
    ] {
        let before = cfg_text(&env);
        let e = add(&env, &Scope::User, name, None).unwrap_err();
        let r = e.downcast_ref::<Refusal>().expect("a refusal");
        assert_eq!(r.reason, reason, "{name}");
        assert_eq!(cfg_text(&env), before, "{name} changed the file");
        // A fixed value is refused the same way.
        assert!(add(&env, &Scope::User, name, Some("v")).is_err());
    }
    assert!(add(&env, &Scope::User, "BAD-NAME", None).is_err());

    assert!(remove(&env, &Scope::Project(key.clone()), "APP_ENV").unwrap());
    assert!(!remove(&env, &Scope::Project(key), "APP_ENV").unwrap());
    assert!(remove(&env, &Scope::User, "JAVA_HOME").unwrap());
    assert!(Manifest::load_user(&env).unwrap().env.set.is_empty());
}

#[test]
fn a_key_name_is_not_an_env_variable() {
    let (_t, env, _) = world(
        Some("[credentials.OPENAI_API_KEY]\nvia = \"deliver\"\n\n[credentials.db]\nvia = \"deliver\"\nvar = \"DB_URL\"\n"),
        &[],
    );
    // A secret-shaped name is refused as a secret first.
    assert!(add(&env, &Scope::User, "OPENAI_API_KEY", None).is_err());
    let e = add(&env, &Scope::User, "DB_URL", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("is a key in [credentials]"), "{e}");
}

#[test]
fn a_project_file_cannot_set_scopes_or_the_migration() {
    for body in [
        "[env.agent.claude]\npass = [\"X\"]\n",
        "[env.project.\"/x\"]\npass = [\"X\"]\n",
        "[env]\nmigrated = \"0.1.0\"\n",
    ] {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("paddock.toml");
        std::fs::write(&f, body).unwrap();
        let e = Manifest::read(&f).unwrap_err().to_string();
        assert!(
            e.contains("only read from the user configuration"),
            "{body}: {e}"
        );
    }
    // mode = "inherit" in a project file asks for approval, as inherit = true.
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("paddock.toml");
    std::fs::write(&f, "[env]\nmode = \"inherit\"\n").unwrap();
    let m = Manifest::read(&f).unwrap();
    let base = crate::manifest::Base::of_user(&Manifest::default());
    assert!(m.widens(&base));
    assert_eq!(m.widening(&base)["env.inherit"], true);
    std::fs::write(&f, "[env]\nmode = \"listed\"\n").unwrap();
    assert!(!Manifest::read(&f).unwrap().widens(&base));
}

#[test]
fn the_view_has_four_groups_in_order_with_held_names_and_no_key_value() {
    let (_t, env, project) = world(
        Some(
            r#"
[env]
pass = ["JAVA_HOME", "NOT_ON_HOST", "SSH_AUTH_SOCK"]
set = { APP_ENV = "dev" }

[env.agent.claude]
pass = ["ANTHROPIC_MODEL"]

[env.agent.codex]
pass = ["CODEX_ONLY"]

[env.project."~/src/app"]
set = { APP_ENV = "test" }

[credentials.OPENAI_API_KEY]
via = "gateway"
preset = "openai"
harnesses = ["codex"]

[credentials.GH_TOKEN]
via = "deliver"
"#,
        ),
        &[
            ("TERM", "xterm"),
            ("LANG", "C.UTF-8"),
            ("JAVA_HOME", "/jvm"),
            ("GOFLAGS", "-mod=mod"),
            ("ANTHROPIC_MODEL", "opus"),
            ("ANTHROPIC_API_KEY", "sk-ant-value-that-must-not-leak"),
            ("OPENAI_API_KEY", "sk-value-that-must-not-leak"),
            ("SERVICE_TOKEN", "t"),
            ("AWS_SECRET_ACCESS_KEY", "s"),
            ("SSH_AUTH_SOCK", "/nowhere"),
            ("HISTFILE", "/h"),
            ("RANDOM_THING", "1"),
            ("CODEX_ONLY", "1"),
        ],
    );
    std::fs::write(env.paddock_state().join("env-default.json"), "{}").ok();
    let p = profile(
        "claude",
        &["ANTHROPIC_API_KEY"],
        &[("DISABLE_AUTOUPDATER", "1")],
    );
    let v = view(&Input {
        env: &env,
        project: &project,
        profile: Some(&p),
        cli: None,
        values: false,
    })
    .unwrap();
    assert_eq!(v.api_version, "paddock.env/v1");
    assert_eq!(v.mode, Mode::Listed);
    let ids: Vec<&str> = v.groups.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(ids, ["agent", "yours", "keys", "host"]);

    let agent = &v.groups[0];
    let origin = |n: &str| {
        agent
            .items
            .iter()
            .find(|i| i.name == n)
            .map(|i| i.origin.as_str())
    };
    assert_eq!(origin("TERM"), Some("basics"));
    assert_eq!(origin("DISABLE_AUTOUPDATER"), Some("profile"));
    let login = agent
        .items
        .iter()
        .find(|i| i.name == "ANTHROPIC_API_KEY")
        .unwrap();
    assert_eq!(login.origin, "login");
    assert!(login.handed_over && login.passes);
    assert_eq!(login.action.as_deref(), Some("keep_on_host"));

    let yours = &v.groups[1];
    let item = |n: &str| yours.items.iter().find(|i| i.name == n).unwrap();
    assert!(item("JAVA_HOME").passes);
    assert_eq!(item("NOT_ON_HOST").reason.as_deref(), Some("not_set"));
    assert_eq!(item("SSH_AUTH_SOCK").reason.as_deref(), Some("held"));
    assert!(!item("SSH_AUTH_SOCK").passes);
    assert_eq!(item("ANTHROPIC_MODEL").scope.as_deref(), Some("agent"));
    // The project's fixed value replaces the user's.
    assert_eq!(item("APP_ENV").scope.as_deref(), Some("project"));
    assert!(yours.items.iter().all(|i| i.name != "CODEX_ONLY"));

    let keys = &v.groups[2].keys;
    let openai = keys.iter().find(|k| k.name == "OPENAI_API_KEY").unwrap();
    assert_eq!(
        (openai.state.as_str(), openai.grant.as_str()),
        ("kept_on_host", "agent")
    );
    assert!(!openai.granted);
    let gh = keys.iter().find(|k| k.name == "GH_TOKEN").unwrap();
    assert_eq!(
        (gh.state.as_str(), gh.grant.as_str(), gh.unit.as_str()),
        ("handed_over", "ask", "starts")
    );

    let host = &v.groups[3];
    assert_eq!(host.passes, Some(false));
    assert_eq!(host.suggested, ["GOFLAGS"]);
    assert!(host.names.contains(&"RANDOM_THING".to_string()));
    assert!(!host.names.contains(&"OPENAI_API_KEY".to_string()));

    let held: Vec<(&str, &str)> = v
        .held
        .iter()
        .map(|h| (h.name.as_str(), h.reason.as_str()))
        .collect();
    for want in [
        ("AWS_SECRET_ACCESS_KEY", "cloud"),
        ("HISTFILE", "history"),
        ("SERVICE_TOKEN", "secret"),
        ("SSH_AUTH_SOCK", "socket"),
    ] {
        assert!(held.contains(&want), "{want:?} in {held:?}");
    }
    assert!(v
        .held
        .iter()
        .all(|h| h.text.contains("stays on this computer")));

    // The summary counts what passes.
    let passing = agent.items.iter().filter(|i| i.passes).count()
        + yours.items.iter().filter(|i| i.passes).count();
    assert_eq!(v.counts.variables, passing);
    assert_eq!(v.counts.keys, 0);
    assert_eq!(v.counts.stay, v.held.len() + host.count.unwrap());
    assert!(v
        .summary
        .starts_with(&format!("claude gets {passing} variables and 0 keys. ")));
    assert_eq!(v.note, "Applies at the next start.");

    // No value without --values, and never the value of a key.
    let json = serde_json::to_string(&v).unwrap();
    assert!(
        !json.contains("/jvm") && !json.contains("must-not-leak"),
        "{json}"
    );
    let v = view(&Input {
        env: &env,
        project: &project,
        profile: Some(&p),
        cli: None,
        values: true,
    })
    .unwrap();
    let json = serde_json::to_string(&v).unwrap();
    assert!(json.contains("/jvm"), "{json}");
    assert!(!json.contains("must-not-leak"), "{json}");
}

#[test]
fn inherit_mode_shows_the_host_group_as_passing() {
    let (_t, env, project) = world(
        Some("[env]\nmode = \"inherit\"\nmigrated = \"0.0.1\"\n"),
        &[("RANDOM_THING", "1"), ("GOFLAGS", "x")],
    );
    let m = Manifest::load_user(&env).unwrap();
    // 0.0.1 is more than two releases ago: forced.
    assert_eq!(resolve(&env, &m).mode, Mode::Listed);
    let (_t, env, project2) = world(
        Some("[env]\nmode = \"inherit\"\n"),
        &[("RANDOM_THING", "1"), ("GOFLAGS", "x")],
    );
    let _ = project;
    let v = view(&Input {
        env: &env,
        project: &project2,
        profile: None,
        cli: None,
        values: false,
    })
    .unwrap();
    let host = &v.groups[3];
    assert_eq!(host.passes, Some(true));
    assert_eq!(host.count, Some(2));
    assert!(host.suggested.is_empty());
    assert_eq!(v.counts.variables, 2);
    assert!(
        v.summary
            .starts_with("Each agent gets 2 variables and 0 keys."),
        "{}",
        v.summary
    );
}

#[test]
fn plain_lines_fit_in_80_columns_with_long_names() {
    let long = "A_VERY_LONG_VARIABLE_NAME_THAT_GOES_ON_AND_ON_FOR_EVER";
    let (_t, env, project) = world(
        Some(&format!(
            "[env]\npass = [\"{long}\"]\nset = {{ {long}_2 = \"{}\" }}\n\n[credentials.{long}_KEY]\nvia = \"deliver\"\n",
            "v".repeat(100)
        )),
        &[(long, "x"), ("SSH_AUTH_SOCK", "/s"), ("HISTFILE", "/h")],
    );
    let p = profile("a-rather-long-agent-name", &[], &[]);
    for values in [false, true] {
        let v = view(&Input {
            env: &env,
            project: &project,
            profile: Some(&p),
            cli: None,
            values,
        })
        .unwrap();
        let lines = plain_lines(&v);
        for l in &lines {
            assert!(
                l.chars().count() <= COLUMNS,
                "{} > 80: {l}",
                l.chars().count()
            );
        }
        let text = lines.join("\n");
        assert!(text.contains('…'), "{text}");
        for t in [
            "Needed by",
            "Your variables",
            "Keys",
            host_title(),
            "Held (2)",
            "Applies at the next start.",
        ] {
            assert!(text.contains(t), "{t} in\n{text}");
        }
    }
    assert_eq!(cut("abcdef", 4), "abc…");
    assert_eq!(cut("abc", 4), "abc");
}

#[test]
fn a_run_record_has_names_and_origins_never_values() {
    let (_t, env, project) = world(None, &[("SSH_AUTH_SOCK", "/s"), ("TERM", "xterm")]);
    let mut spec = crate::spec::SandboxSpec::new(
        "sb-abc123".into(),
        "claude",
        &project,
        env.home.join("sb"),
        &env.home,
        1000,
        1000,
        "t",
    );
    spec.set_env("DISABLE_AUTOUPDATER", "1", "profile claude");
    spec.set_secret_env(
        "ANTHROPIC_API_KEY",
        "secret-login-value",
        "profile claude env_pass",
    );
    spec.set_secret_env("JAVA_HOME", "/jvm-value", "manifest env.pass");
    spec.set_env("APP_ENV", "dev", "manifest env.agent.claude set");
    spec.set_env(
        "OPENAI_API_KEY",
        "standin-value",
        "credentials: stand-in for OPENAI_API_KEY",
    );
    spec.set_env("EDITOR", "nvim-value", "host env (inherited)");
    spec.deny(
        crate::boundary::ENV,
        "SERVICE_TOKEN",
        "secret-looking name; [env] pass hands it over",
    );
    spec.deny(
        crate::boundary::ENV,
        "AWS_SECRET_ACCESS_KEY",
        "cloud credential; [env] pass hands it over",
    );
    let r = record(&env, &spec, Mode::Inherit);
    let names = |id: &str| -> Vec<String> {
        r.groups
            .iter()
            .find(|g| g.id == id)
            .unwrap()
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect()
    };
    assert_eq!(
        names("agent"),
        ["TERM", "ANTHROPIC_API_KEY", "DISABLE_AUTOUPDATER"]
    );
    assert_eq!(names("yours"), ["APP_ENV", "JAVA_HOME"]);
    assert_eq!(names("keys"), ["OPENAI_API_KEY"]);
    assert_eq!(names("host"), ["EDITOR"]);
    let held: Vec<(&str, &str, &str)> = r
        .held
        .iter()
        .map(|h| (h.name.as_str(), h.rule.as_str(), h.reason.as_str()))
        .collect();
    assert_eq!(
        held,
        [
            ("AWS_SECRET_ACCESS_KEY", "key", "cloud"),
            ("SERVICE_TOKEN", "key", "secret"),
            ("SSH_AUTH_SOCK", "never", "socket"),
        ]
    );
    let path = save(&env, &r).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for v in [
        "secret-login-value",
        "/jvm-value",
        "standin-value",
        "nvim-value",
        "\"dev\"",
    ] {
        assert!(!text.contains(v), "{v} in the record");
    }
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(load_record(&env, "sb-abc").unwrap(), r);
    assert_eq!(load_record(&env, "sb-abc123").unwrap().api_version, RUN_API);
    assert!(load_record(&env, "nope").is_err());
    let lines = record_lines(&r);
    assert!(lines[0].starts_with("What claude got at "), "{lines:?}");
    assert!(lines.iter().all(|l| l.chars().count() <= COLUMNS));
    // A bad id never leaves the folder.
    let mut bad = r.clone();
    bad.id = "../x".into();
    assert!(save(&env, &bad).is_err());
}

#[test]
fn old_records_are_pruned() {
    let (_t, env, project) = world(None, &[]);
    let spec = crate::spec::SandboxSpec::new(
        "x".into(),
        "claude",
        &project,
        env.home.join("sb"),
        &env.home,
        1000,
        1000,
        "t",
    );
    let mut r = record(&env, &spec, Mode::Listed);
    for i in 0..(KEEP_RECORDS + 5) {
        r.id = format!("run-{i:04}");
        save(&env, &r).unwrap();
    }
    let n = std::fs::read_dir(env.paddock_state().join("env-runs"))
        .unwrap()
        .count();
    assert_eq!(n, KEEP_RECORDS);
}

#[test]
fn the_review_lists_what_passes_unseen_and_finishing_it_writes_listed() {
    let (_t, env, project) = world(
        Some("[env]\nmode = \"inherit\"\nmigrated = \"0.2.3\"\npass = [\"EDITOR\"]\n"),
        &[
            ("EDITOR", "nvim"),
            ("JAVA_HOME", "/jvm"),
            ("RANDOM_THING", "1"),
            ("SERVICE_TOKEN", "t"),
        ],
    );
    let r = review(&env, &project).unwrap();
    assert_eq!(r.api_version, REVIEW_API);
    let names: Vec<(&str, bool)> = r
        .items
        .iter()
        .map(|i| (i.name.as_str(), i.suggested))
        .collect();
    // EDITOR is in the user's list already, so it is not "unseen".
    assert_eq!(names, [("JAVA_HOME", true), ("RANDOM_THING", false)]);
    assert!(
        r.summary.starts_with("Until now, 2 variables of "),
        "{}",
        r.summary
    );
    assert!(finish_review(&env, &["SSH_AUTH_SOCK".into()]).is_err());
    finish_review(&env, &["JAVA_HOME".into()]).unwrap();
    let m = Manifest::load_user(&env).unwrap();
    assert_eq!(m.env.mode, Some(Mode::Listed));
    assert!(m.env.migrated.is_none());
    assert_eq!(m.env.pass, ["EDITOR", "JAVA_HOME"]);
}
