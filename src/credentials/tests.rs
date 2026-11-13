use super::*;

#[test]
fn a_process_left_behind_does_not_hang_a_value_command() {
    let start = std::time::Instant::now();
    let cmd = ["sh", "-c", "echo v; sleep 30 &"].map(String::from);
    let e = run_command(&cmd, &|_| {}).unwrap_err();
    assert!(format!("{e:#}").contains("left its output open"), "{e:#}");
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn parses_the_documented_forms() {
    let all = parse(
        r#"
[credentials.GITHUB_TOKEN]
via = "gateway"
hosts = ["api.github.com"]
paths = ["/repos/", "/user"]
base_url_env = "GH_API_URL"
harnesses = ["claude", "codex"]

[credentials.DB_PASSWORD]
via = "deliver"
from = "command"
command = ["secret-tool", "lookup", "service", "mydb"]

[credentials.ssh]
via = "agent"
keys = ["~/.ssh/id_ed25519.pub"]
confirm = "each"
"#,
    );
    // ssh has no variable name of its own for deliver, but it is an agent.
    let all = all.unwrap();
    assert_eq!(all["GITHUB_TOKEN"].via, Via::Gateway);
    assert!(all["GITHUB_TOKEN"].applies_to("codex"));
    assert!(!all["GITHUB_TOKEN"].applies_to("gemini"));
    assert_eq!(all["DB_PASSWORD"].from, Source::Command);
    assert_eq!(all["DB_PASSWORD"].sandbox_var("DB_PASSWORD"), "DB_PASSWORD");
    assert_eq!(all["ssh"].confirm, Confirm::Each);
    assert!(Via::Deliver.available() && Via::Gateway.available());
    assert!(all["ssh"].available() && !Via::Intercept.available());
    let bare = parse("[credentials.git]\nvia = \"agent\"\n").unwrap();
    assert!(!bare["git"].available());
    assert!(parse("[credentials.x]\nvia = \"deliver\"\nkeys = [\"k.pub\"]\n").is_err());
    assert!(parse("").unwrap().is_empty());
}

#[test]
fn deliver_as_file_parses_and_needs_no_variable_name() {
    let all = parse(
        "[credentials.my-token]\nvia = \"deliver\"\nas = \"file\"\n\n[credentials.DB]\nvia = \"deliver\"\nas = \"env\"\n\n[credentials.PLAIN]\nvia = \"deliver\"\n",
    )
    .unwrap();
    assert_eq!(all["my-token"].as_, DeliverAs::File);
    assert_eq!(all["DB"].as_, DeliverAs::Env);
    assert_eq!(all["PLAIN"].as_, DeliverAs::Env);
    assert_eq!(secret::fifo::path_on(false, "T"), "/dev/paddock/secret/T");
    assert!(describe("my-token", &all["my-token"]).contains(&secret_path("my-token")));
    for bad in [
        "[credentials.x]\nvia = \"deliver\"\nas = \"disk\"\n",
        "[credentials.x]\nvia = \"deliver\"\nas = \"file\"\nenv = \"X\"\n",
        "[credentials.x]\nvia = \"gateway\"\npreset = \"github\"\nas = \"file\"\n",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn secret_file_reads_count_as_uses() {
    let (_t, env) = world(
        "[credentials.GH]\nvia = \"deliver\"\nas = \"file\"\n",
        &[("GH", "v")],
    );
    let s = spec(&env);
    audit(&env, &s, "GH", "delivered");
    audit_read(&env, &s, "GH");
    audit_read(&env, &s, "GH");
    let r = rows(&env).unwrap();
    assert_eq!((r[0].delivered, r[0].used), (1, 2));
    assert_eq!(r[0].deliver_as, Some("file"));
}

#[test]
fn rejects_unclear_entries() {
    for bad in [
        "[credentials.x]\nvia = \"teleport\"\n",
        "[credentials.x]\nvia = \"deliver\"\nfrom = \"command\"\n",
        "[credentials.x]\nvia = \"deliver\"\ncommand = [\"pass\"]\n",
        "[credentials.x]\nvia = \"gateway\"\n",
        "[credentials.x]\nvia = \"deliver\"\nenv = \"A-B\"\n",
        "[credentials.my-db]\nvia = \"deliver\"\n",
        "[credentials.x]\nvia = \"deliver\"\nother = 1\n",
        "[credentials.\"1x\"]\nvia = \"deliver\"\n",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    parse("[credentials.my-db]\nvia = \"deliver\"\nenv = \"DB\"\n").unwrap();
    for bad in [
        "[credentials.x]\nvia = \"gateway\"\nhosts = [\"a.example\"]\n",
        "[credentials.x]\nvia = \"gateway\"\npreset = \"nope\"\n",
        "[credentials.x]\nvia = \"gateway\"\nhosts = [\"a.example/x\"]\nbase_url_env = \"B\"\n",
        "[credentials.x]\nvia = \"gateway\"\nhosts = [\"a.example:http\"]\nbase_url_env = \"B\"\n",
        "[credentials.x]\nvia = \"gateway\"\npreset = \"github\"\npaths = [\"repos\"]\n",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
    let all = parse("[credentials.ANTHROPIC_API_KEY]\nvia = \"gateway\"\npreset = \"anthropic\"\n\n[credentials.OPENAI_API_KEY]\nvia = \"gateway\"\npreset = \"openai\"\nhosts = [\"proxy.example:8443\"]\n").unwrap();
    let a = &all["ANTHROPIC_API_KEY"];
    assert_eq!(a.hosts, ["api.anthropic.com"]);
    assert_eq!(a.header.as_deref(), Some("x-api-key"));
    assert_eq!(a.base_url_env.as_deref(), Some("ANTHROPIC_BASE_URL"));
    // Fields set by the user win over the preset.
    let o = &all["OPENAI_API_KEY"];
    assert_eq!(o.hosts, ["proxy.example:8443"]);
    assert_eq!(o.base_path.as_deref(), Some("/v1"));
}

#[test]
fn the_github_git_preset_binds_git_smart_http_with_basic_auth() {
    let all = parse("[credentials.github_git]\nvia = \"gateway\"\npreset = \"github-git\"\nfrom = \"command\"\ncommand = [\"gh\", \"auth\", \"token\"]\n").unwrap();
    let c = &all["github_git"];
    assert!(c.is_git());
    assert_eq!(c.hosts, ["github.com"]);
    assert_eq!(c.paths, ["/*/*/info/refs", "/*/*/git-upload-pack"]);
    assert_eq!(c.header.as_deref(), Some("Authorization"));
    assert_eq!(c.prefix.as_deref(), Some("Basic "));
    assert_eq!(c.basic_user.as_deref(), Some("x-access-token"));
    assert_eq!(c.base_url_env, None);
    let r = gateway::Route::new(c, "ghp_tok".into());
    // base64("x-access-token:ghp_tok")
    assert_eq!(r.value.as_str(), "eC1hY2Nlc3MtdG9rZW46Z2hwX3Rvaw==");
    assert_eq!(r.prefix, "Basic ");
    assert!(parse(
        "[credentials.g]\nvia = \"gateway\"\npreset = \"github-git\"\nbasic_user = \"a:b\"\n"
    )
    .is_err());
}

#[test]
fn the_git_rewrite_lives_in_the_environment() {
    let env = git_env(
        3130,
        &[
            ("github_git".into(), "github.com".into()),
            ("ghe".into(), "git.example:8443".into()),
        ],
    );
    let want: Vec<(String, String)> = [
        ("GIT_CONFIG_COUNT", "2"),
        (
            "GIT_CONFIG_KEY_0",
            "url.http://127.0.0.1:3130/s/github_git/.insteadOf",
        ),
        ("GIT_CONFIG_VALUE_0", "https://github.com/"),
        (
            "GIT_CONFIG_KEY_1",
            "url.http://127.0.0.1:3130/s/ghe/.insteadOf",
        ),
        ("GIT_CONFIG_VALUE_1", "https://git.example:8443/"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(env, want);
}

struct Answers(
    std::sync::Mutex<Vec<Option<crate::broker::Answer>>>,
    std::sync::Mutex<usize>,
);
impl crate::broker::Approver for Answers {
    fn ask(
        &self,
        _: &HostEnv,
        p: &crate::broker::Pending,
        _: std::time::Duration,
    ) -> Option<crate::broker::Answer> {
        assert_eq!(p.capability, CAPABILITY);
        assert!(!p.session_allowed);
        *self.1.lock().unwrap() += 1;
        self.0.lock().unwrap().remove(0)
    }
}

fn world(cfg: &str, vars: &[(&str, &str)]) -> (tempfile::TempDir, HostEnv) {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    std::fs::create_dir_all(home.join(".config/paddock")).unwrap();
    std::fs::write(home.join(".config/paddock/config.toml"), cfg).unwrap();
    let mut v = BTreeMap::new();
    v.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    for (k, val) in vars {
        v.insert(k.to_string(), val.to_string());
    }
    let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, v);
    (t, env)
}

fn spec(env: &HostEnv) -> crate::spec::SandboxSpec {
    crate::spec::SandboxSpec::new(
        "sb-1".into(),
        "claude",
        std::path::Path::new("/home/demo/app"),
        env.home.join("sb"),
        &env.home,
        1000,
        1000,
        "t",
    )
}

const CFG: &str = r#"
[credentials.DB_PASSWORD]
via = "deliver"
var = "HOST_DB"

[credentials.FROM_CMD]
via = "deliver"
from = "command"
command = ["sh", "-c", "printf 'line1\nline2\n'"]

[credentials.ONLY_CODEX]
via = "deliver"
harnesses = ["codex"]

[credentials.LATER]
via = "intercept"
hosts = ["api.example.com"]

[credentials.MISSING]
via = "deliver"
"#;

#[test]
fn plan_reads_only_what_is_there_and_names_the_rest() {
    let (_t, env) = world(CFG, &[("HOST_DB", "s3cret")]);
    let p = plan(&env, "claude", false).unwrap();
    let names: Vec<&str> = p.deliver.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["DB_PASSWORD", "FROM_CMD"]);
    let notes = p.notices.join("\n");
    assert!(
        notes.contains("LATER") && notes.contains("not available"),
        "{notes}"
    );
    assert!(
        notes.contains("MISSING") && notes.contains("not set"),
        "{notes}"
    );
    assert!(!notes.contains("ONLY_CODEX"));
    let (values, notes) = read(&env, &p);
    assert!(notes.is_empty(), "{notes:?}");
    let values: Vec<(&str, &str, &str)> = values
        .iter()
        .map(|(n, v, s)| (n.as_str(), v.as_str(), s.as_str()))
        .collect();
    assert_eq!(
        values,
        [
            ("DB_PASSWORD", "DB_PASSWORD", "s3cret"),
            ("FROM_CMD", "FROM_CMD", "line1\nline2"),
        ]
    );
    // A spawned sandbox gets none.
    assert!(plan(&env, "claude", true).unwrap().deliver.is_empty());
}

#[test]
fn a_failing_command_delivers_nothing() {
    let (_t, env) = world(
        "[credentials.X]\nvia = \"deliver\"\nfrom = \"command\"\ncommand = [\"sh\", \"-c\", \"exit 3\"]\n",
        &[],
    );
    let p = plan(&env, "claude", false).unwrap();
    let (values, notes) = read(&env, &p);
    assert!(values.is_empty());
    assert!(notes[0].contains("failed"), "{notes:?}");
}

#[test]
fn only_two_yes_answers_deliver_and_the_log_has_no_value() {
    use crate::broker::Answer::*;
    let (_t, env) = world(CFG, &[("HOST_DB", "s3cret")]);
    let p = plan(&env, "claude", false).unwrap();
    let s = spec(&env);
    let wait = std::time::Duration::from_secs(1);
    for (answers, delivered, asked) in [
        (vec![Some(Once), Some(Once)], true, 2),
        (vec![Some(Once), Some(Deny)], false, 2),
        (vec![Some(Deny)], false, 1),
        (vec![None], false, 1),
    ] {
        let a = Answers(std::sync::Mutex::new(answers), std::sync::Mutex::new(0));
        assert_eq!(ask(&env, &s, &a, wait, &p), delivered);
        assert_eq!(*a.1.lock().unwrap(), asked);
    }
    let log = std::fs::read_to_string(crate::broker::audit_file(&env)).unwrap();
    assert!(
        log.contains("\"confirmed\"") && log.contains("\"denied\"") && log.contains("timed out")
    );
    assert!(!log.contains("s3cret"));
}

#[test]
fn an_agent_credential_without_a_host_agent_gets_a_notice_and_counts_signatures() {
    let (_t, env) = world(
        "[credentials.ssh]\nvia = \"agent\"\nkeys = [\"~/.ssh/id_test.pub\"]\n",
        &[],
    );
    // The key file is missing.
    let p = plan(&env, "claude", false).unwrap();
    assert!(p.deliver.is_empty());
    assert!(
        p.notices[0].contains("cannot read the key"),
        "{:?}",
        p.notices
    );
    // The key is there, the host agent is not.
    use base64::Engine as _;
    let mut blob = Vec::new();
    for part in [&b"ssh-ed25519"[..], &[1u8; 32][..]] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    std::fs::create_dir_all(env.home.join(".ssh")).unwrap();
    std::fs::write(
        env.home.join(".ssh/id_test.pub"),
        format!(
            "ssh-ed25519 {} test\n",
            base64::engine::general_purpose::STANDARD.encode(&blob)
        ),
    )
    .unwrap();
    let p = plan(&env, "claude", false).unwrap();
    assert!(p.deliver.is_empty());
    assert!(
        p.notices[0].contains("SSH_AUTH_SOCK")
            && p.notices[0].contains("without the ssh agent socket"),
        "{:?}",
        p.notices
    );
    // Allowed signatures count as uses.
    let s = spec(&env);
    let k = sshagent::Key {
        name: "ssh".into(),
        fingerprint: sshagent::fingerprint(&blob),
        blob,
        comment: "test".into(),
        confirm: Confirm::Each,
    };
    sshagent::audit(&env, &s, &k, "allowed");
    sshagent::audit(&env, &s, &k, "denied by the user");
    let r = rows(&env).unwrap();
    assert_eq!(
        (r[0].via, r[0].available, r[0].ready, r[0].used),
        ("agent", true, true, 1)
    );
    assert!(r[0].source.contains("ssh agent"));
    assert!(
        host_vars(&parse("[credentials.ssh]\nvia = \"agent\"\nkeys = [\"k.pub\"]\n").unwrap())
            .is_empty()
    );
}

#[test]
fn rows_count_deliveries_from_the_log() {
    let (_t, env) = world(CFG, &[("HOST_DB", "s3cret")]);
    let s = spec(&env);
    audit(&env, &s, "DB_PASSWORD", "delivered");
    audit(&env, &s, "DB_PASSWORD", "delivered");
    let rows = rows(&env).unwrap();
    let db = rows.iter().find(|r| r.name == "DB_PASSWORD").unwrap();
    assert_eq!((db.delivered, db.ready, db.available), (2, true, true));
    assert_eq!(db.source, "env HOST_DB");
    let later = rows.iter().find(|r| r.name == "LATER").unwrap();
    assert!(!later.available);
    assert!(!serde_json::to_string(&rows).unwrap().contains("s3cret"));
}
