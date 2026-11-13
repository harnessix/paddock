use super::*;
use crate::broker::{Approver, Pending};
use std::sync::Mutex;

/// A temporary home whose PATH has only the fake keychain tool, or no
/// tool at all. The real keychain is never reached.
fn world(cfg: &str, keychain: bool) -> (tempfile::TempDir, HostEnv, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let project = home.join("src/app");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
    std::fs::write(home.join(".config/paddock/config.toml"), cfg).unwrap();
    let bin = if keychain {
        crate::tack::store::tests::fake_secret_tool(&t.path().join("kc"), "ok")
            .parent()
            .unwrap()
            .to_path_buf()
    } else {
        let b = t.path().join("empty-bin");
        std::fs::create_dir_all(&b).unwrap();
        b
    };
    let mut v = BTreeMap::new();
    v.insert("PATH".to_string(), bin.display().to_string());
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, v);
    let project = std::fs::canonicalize(project).unwrap();
    (t, env, project)
}

fn cfg_text(env: &HostEnv) -> String {
    std::fs::read_to_string(env.paddock_config().join("config.toml")).unwrap()
}

fn spec(env: &HostEnv, harness: &str, project: &Path) -> crate::spec::SandboxSpec {
    crate::spec::SandboxSpec::new(
        format!("sb-{harness}"),
        harness,
        project,
        env.home.join("sb"),
        &env.home,
        1000,
        1000,
        "t",
    )
}

/// Answers the grant question from a list, and keeps the cards.
struct Pick(Mutex<Vec<Option<String>>>, Mutex<Vec<Pending>>);

impl Pick {
    fn new(answers: &[Option<&str>]) -> Self {
        Pick(
            Mutex::new(answers.iter().map(|a| a.map(str::to_string)).collect()),
            Mutex::new(Vec::new()),
        )
    }
    fn cards(&self) -> Vec<Pending> {
        self.1.lock().unwrap().clone()
    }
}

impl Approver for Pick {
    fn ask(
        &self,
        _: &HostEnv,
        _: &Pending,
        _: std::time::Duration,
    ) -> Option<crate::broker::Answer> {
        panic!("the grant question has fixed choices");
    }
    fn choose(&self, _: &HostEnv, p: &Pending, _: std::time::Duration) -> Option<String> {
        self.1.lock().unwrap().push(p.clone());
        self.0.lock().unwrap().remove(0)
    }
}

#[test]
fn presets_come_from_the_name_or_the_value() {
    assert_eq!(preset_for("ANTHROPIC_API_KEY", None), Some("anthropic"));
    assert_eq!(preset_for("OPENAI_API_KEY", None), Some("openai"));
    assert_eq!(preset_for("GH_TOKEN", None), Some("github"));
    assert_eq!(
        preset_for("MY_KEY", Some(b"sk-ant-api03-x")),
        Some("anthropic")
    );
    assert_eq!(preset_for("MY_KEY", Some(b"sk-proj-x")), Some("openai"));
    assert_eq!(preset_for("MY_KEY", Some(b"github_pat_x")), Some("github"));
    assert_eq!(preset_for("MY_KEY", Some(b"plain")), None);
}

#[cfg(target_os = "linux")]
#[test]
fn add_keeps_the_value_in_the_keychain_and_only_a_reference_in_the_file() {
    let (t, env, project) = world("# mine\n", true);
    let value = Secret::from("sk-proj-value-that-must-not-leak");
    let k = NewKey {
        name: "OPENAI_API_KEY".into(),
        preset: Some("openai".into()),
        agents: vec!["codex".into()],
        projects: vec![crate::envset::project_key(&env, &project)],
        ..Default::default()
    };
    assert!(add(&env, &k, &value).unwrap());
    let text = cfg_text(&env);
    assert!(text.contains("# mine\n"), "{text}");
    assert!(!text.contains("must-not-leak"), "{text}");
    let c = &super::super::load(&env).unwrap()["OPENAI_API_KEY"];
    assert_eq!((c.via, c.from), (Via::Gateway, Source::Store));
    assert_eq!(c.harnesses, ["codex"]);
    assert_eq!(c.projects, ["~/src/app"]);
    assert_eq!(c.hosts, ["api.openai.com"]);
    // The value went in on stdin, never in an argument.
    let calls = std::fs::read_to_string(t.path().join("kc/calls")).unwrap();
    assert!(!calls.contains("must-not-leak"), "{calls}");
    assert!(
        calls.contains("service paddock-credentials name OPENAI_API_KEY"),
        "{calls}"
    );
    assert_eq!(
        read_value(&env, "OPENAI_API_KEY").unwrap().as_bytes(),
        value.as_bytes()
    );
    // It is ready, and the run reads it from the keychain.
    assert!(super::super::source_ready(&env, "OPENAI_API_KEY", c));
    let plan = super::super::plan(&env, "codex", false).unwrap();
    let (values, notes) = super::super::read(&env, &plan);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(values[0].2.as_bytes(), value.as_bytes());

    // Again: a new value, the same grants.
    let v2 = Secret::from("sk-proj-second");
    assert!(!add(
        &env,
        &NewKey {
            name: "OPENAI_API_KEY".into(),
            ..Default::default()
        },
        &v2
    )
    .unwrap());
    assert_eq!(cfg_text(&env), text);
    assert_eq!(
        read_value(&env, "OPENAI_API_KEY").unwrap().as_bytes(),
        v2.as_bytes()
    );

    // Remove: the item, the table and the standing grants.
    let mut g = Grants::load(&env);
    g.add("OPENAI_API_KEY", "codex", &project);
    g.save(&env).unwrap();
    remove(&env, "OPENAI_API_KEY").unwrap();
    assert!(super::super::load(&env).unwrap().is_empty());
    assert_eq!(
        read_value(&env, "OPENAI_API_KEY").unwrap_err(),
        StoreError::Missing
    );
    assert!(Grants::load(&env).grants.is_empty());
    assert!(remove(&env, "OPENAI_API_KEY").is_err());
}

#[test]
fn without_a_keychain_paddock_refuses_and_names_the_command_backend() {
    let (_t, env, _) = world("", false);
    let k = NewKey {
        name: "ANTHROPIC_API_KEY".into(),
        preset: Some("anthropic".into()),
        ..Default::default()
    };
    let e = format!("{:#}", add(&env, &k, &Secret::from("v")).unwrap_err());
    assert!(e.contains("backend = \"command\""), "{e}");
    assert!(e.contains("does not keep keys in a file"), "{e}");
    assert_eq!(cfg_text(&env), "", "nothing configured");
    assert!(!keychain_tool(&env));
}

#[test]
fn a_new_key_table_is_checked_before_anything_is_stored() {
    let k = |name: &str, deliver: bool, hosts: &[&str]| NewKey {
        name: name.into(),
        deliver,
        hosts: hosts.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    // Kept on the computer without a preset: it needs a host.
    let e = table(&k("MY_SERVICE_KEY", false, &[]))
        .unwrap_err()
        .to_string();
    assert!(e.contains("--host"), "{e}");
    let t = table(&k("MY_SERVICE_KEY", false, &["api.example.com"])).unwrap();
    assert_eq!(t["base_url_env"].as_str(), Some("MY_SERVICE_BASE_URL"));
    assert!(table(&k("MY_SERVICE_KEY", true, &[])).is_ok());
    assert!(table(&k("bad name", true, &[])).is_err());
    assert!(table(&k("X", false, &["not a host/"])).is_err());
}

#[test]
fn grants_come_from_projects_or_a_standing_answer() {
    let (_t, env, project) = world("", false);
    let parse = |s: &str| {
        super::super::parse(&format!("[credentials.K]\nvia = \"deliver\"\n{s}")).unwrap()["K"]
            .clone()
    };
    let other = env.home.join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    assert_eq!(
        grant(
            &env,
            "K",
            &parse("projects = [\"*\"]\n"),
            "claude",
            &project
        ),
        Grant::Any
    );
    assert_eq!(
        grant(
            &env,
            "K",
            &parse("projects = [\"~/src/app\"]\n"),
            "claude",
            &project
        ),
        Grant::Project
    );
    assert_eq!(
        grant(
            &env,
            "K",
            &parse("projects = [\"~/src/app\"]\n"),
            "claude",
            &other
        ),
        Grant::Ask
    );
    let c = parse("");
    assert_eq!(grant(&env, "K", &c, "claude", &project), Grant::Ask);
    let mut g = Grants::load(&env);
    g.add("K", "claude", &project);
    g.save(&env).unwrap();
    assert_eq!(grant(&env, "K", &c, "claude", &project), Grant::Standing);
    // Another agent, another project: still a question.
    assert_eq!(grant(&env, "K", &c, "codex", &project), Grant::Ask);
    assert_eq!(grant(&env, "K", &c, "claude", &other), Grant::Ask);
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(env.paddock_state().join("credential-grants.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

const CFG: &str = r#"
[credentials.DB_PASSWORD]
via = "deliver"
var = "HOST_DB"

[credentials.ANY]
via = "deliver"
var = "HOST_ANY"
projects = ["*"]
"#;

#[test]
fn one_question_for_each_agent_and_project_then_a_standing_grant() {
    let (_t, mut env, project) = world(CFG, false);
    env.vars.insert("HOST_DB".into(), "s3cret-value".into());
    env.vars.insert("HOST_ANY".into(), "any-value".into());
    let plan = super::super::plan(&env, "claude", false).unwrap();
    assert_eq!(plan.deliver.len(), 2);
    let s = spec(&env, "claude", &project);
    let wait = std::time::Duration::from_secs(1);

    // No: only the key with a grant passes. One card, three answers.
    let a = Pick::new(&[None]);
    let r = ask_once(&env, &s, &a, wait, &plan);
    assert_eq!((r.allowed.clone(), r.cards), (vec!["ANY".to_string()], 1));
    let card = &a.cards()[0];
    assert_eq!(card.choices, [THIS_RUN, ALWAYS_HERE]);
    assert_eq!(card.capability, CAPABILITY);
    assert!(
        card.summary.contains("DB_PASSWORD") && !card.summary.contains("ANY"),
        "{}",
        card.summary
    );

    // This run: allowed, and asked again next time.
    let a = Pick::new(&[Some(THIS_RUN)]);
    assert_eq!(ask_once(&env, &s, &a, wait, &plan).allowed.len(), 2);
    let a = Pick::new(&[Some(ALWAYS_HERE)]);
    let r = ask_once(&env, &s, &a, wait, &plan);
    assert_eq!((r.allowed.len(), r.cards), (2, 1));

    // Always in this project: no card any more for this agent here.
    let a = Pick::new(&[]);
    let r = ask_once(&env, &s, &a, wait, &plan);
    assert_eq!((r.allowed.len(), r.cards), (2, 0));
    // Another agent asks once for itself.
    let a = Pick::new(&[None]);
    let r = ask_once(&env, &spec(&env, "codex", &project), &a, wait, &plan);
    assert_eq!((r.allowed, r.cards), (vec!["ANY".to_string()], 1));

    let log = std::fs::read_to_string(crate::broker::audit_file(&env)).unwrap();
    for d in [
        "granted (any)",
        "granted (standing)",
        "allowed for this run",
        "allowed always in this project",
        "not allowed",
    ] {
        assert!(log.contains(d), "{d} in\n{log}");
    }
    assert!(!log.contains("s3cret-value") && !log.contains("any-value"));
}

#[test]
fn the_cli_grants_and_takes_back() {
    let (_t, env, project) = world(
        "[credentials.K]\nvia = \"deliver\"\nharnesses = [\"claude\"]\n\n[credentials.ALL]\nvia = \"deliver\"\n",
        false,
    );
    let said = grant_cmd(&env, "K", "codex", None).unwrap();
    assert!(
        said.contains("codex can use K") && said.ends_with("Applies at the next start."),
        "{said}"
    );
    assert_eq!(
        super::super::load(&env).unwrap()["K"].harnesses,
        ["claude", "codex"]
    );
    let said = grant_cmd(&env, "K", "codex", Some(&project)).unwrap();
    assert!(said.contains("without a question"), "{said}");
    assert!(Grants::load(&env).has("K", "codex", &project));
    ungrant_cmd(&env, "K", "codex", Some(&project)).unwrap();
    assert!(!Grants::load(&env).has("K", "codex", &project));
    assert!(ungrant_cmd(&env, "K", "codex", Some(&project)).is_err());
    ungrant_cmd(&env, "K", "codex", None).unwrap();
    assert_eq!(super::super::load(&env).unwrap()["K"].harnesses, ["claude"]);
    let e = ungrant_cmd(&env, "K", "claude", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("paddock credentials remove K"), "{e}");
    let e = ungrant_cmd(&env, "ALL", "claude", None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("every agent"), "{e}");
    assert!(grant_cmd(&env, "NOPE", "claude", None).is_err());
}

#[test]
fn the_log_counts_uses_by_agent_and_project_without_values() {
    let (_t, env, project) = world(CFG, false);
    let s = spec(&env, "claude", &project);
    let other = PathBuf::from("/elsewhere");
    let s2 = spec(&env, "codex", &other);
    for _ in 0..3 {
        super::super::audit_use(
            &env,
            &s,
            "GW",
            "POST",
            "api.openai.com",
            "/v1/x",
            "forwarded 200",
        );
    }
    super::super::audit_use(
        &env,
        &s,
        "GW",
        "POST",
        "api.openai.com",
        "/v1/x",
        "refused: revoked",
    );
    super::super::audit(&env, &s2, "DB_PASSWORD", "delivered");
    super::super::audit(&env, &s2, "DB_PASSWORD", "allowed for this run");
    let u = uses(&env);
    assert_eq!(count(&u, "GW", Some("claude"), Some(&project)).0, 3);
    assert_eq!(count(&u, "GW", Some("codex"), None).0, 0);
    assert_eq!(count(&u, "DB_PASSWORD", None, None).0, 1);
    let rows = log(&env, None);
    let gw = rows.iter().find(|r| r.name == "GW").unwrap();
    assert_eq!(
        (gw.count, gw.unit, gw.runs, gw.agent.as_str()),
        (3, "requests", 1, "claude")
    );
    let db = rows.iter().find(|r| r.name == "DB_PASSWORD").unwrap();
    assert_eq!(
        (db.count, db.unit, db.project.clone()),
        (1, "starts", other)
    );
    assert_eq!(log(&env, Some("GW")).len(), 1);
}
