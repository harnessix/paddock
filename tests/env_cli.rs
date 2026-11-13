//! `paddock env` and `paddock credentials` of ENV1, through the binary,
//! in a temporary home. The keychain is a fake `secret-tool` on PATH; the
//! real keychain and the real configuration are never touched.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct World {
    _t: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    project: PathBuf,
}

impl World {
    fn new(keychain: bool) -> World {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        let project = root.join("home/src/app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(root.join("config/paddock")).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        if keychain {
            fake_secret_tool(&root, &bin);
        }
        World {
            _t: t,
            project: std::fs::canonicalize(&project).unwrap(),
            root,
            bin,
        }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config/paddock/config.toml")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.args(args)
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("JAVA_HOME", "/usr/lib/jvm/java-21")
            .env("SSH_AUTH_SOCK", "/nowhere/agent.sock")
            .env("SERVICE_TOKEN", "service-token-value")
            .env("GOFLAGS", "-mod=mod");
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn run_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .cmd(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(
            o.status.success(),
            "paddock {args:?}: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

/// A `secret-tool` that keeps items as files. Only names reach its
/// arguments; the value comes on stdin.
fn fake_secret_tool(root: &Path, bin: &Path) {
    let items = root.join("items");
    std::fs::create_dir_all(&items).unwrap();
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{root}/calls'
cmd=$1; shift
case "$cmd" in
  store) shift; cat > '{items}'/"$2-$4" ;;
  lookup) f='{items}'/"$2-$4"; [ -f "$f" ] || exit 1; cat "$f" ;;
  clear) rm -f '{items}'/"$2-$4" ;;
  *) exit 2 ;;
esac
"#,
        root = root.display(),
        items = items.display()
    );
    let p = bin.join("secret-tool");
    std::fs::write(&p, script).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn env_add_refuses_a_held_name_with_exit_2_and_the_sentence() {
    let w = World::new(false);
    for (name, words) in [
        ("SSH_AUTH_SOCK", "via = \"agent\""),
        ("HISTFILE", "shell history"),
        ("DYLD_INSERT_LIBRARIES", "load code"),
        (
            "AWS_SECRET_ACCESS_KEY",
            "paddock credentials add AWS_SECRET_ACCESS_KEY",
        ),
        ("SERVICE_TOKEN", "paddock credentials add SERVICE_TOKEN"),
    ] {
        let o = w.run(&["env", "add", name]);
        assert_eq!(o.status.code(), Some(2), "{name}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains(&format!("{name} stays on this computer: ")) && err.contains(words),
            "{name}: {err}"
        );
        assert!(
            !w.config().exists() || !std::fs::read_to_string(w.config()).unwrap().contains(name)
        );
    }
}

#[test]
fn env_show_json_is_the_contract_of_the_screens() {
    let w = World::new(false);
    let out = w.ok(&["env", "add", "JAVA_HOME"]);
    assert!(out.contains("Applies at the next start."), "{out}");
    w.ok(&["env", "add", "APP_ENV=dev", "--agent", "claude"]);
    w.ok(&["env", "add", "APP_ENV=test", "--project", "."]);
    let cfg = std::fs::read_to_string(w.config()).unwrap();
    assert!(cfg.contains("[env.project.\"~/src/app\"]"), "{cfg}");

    let v = w.json(&["env", "show", "--agent", "claude", "--json"]);
    assert_eq!(v["api_version"], "paddock.env/v1");
    assert_eq!(v["agent"], "claude");
    assert_eq!(v["mode"], "listed");
    assert_eq!(v["mode_origin"], "new installation");
    assert_eq!(v["note"], "Applies at the next start.");
    let ids: Vec<&str> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["agent", "yours", "keys", "host"]);
    let yours = &v["groups"][1]["items"];
    let java = yours
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "JAVA_HOME")
        .unwrap();
    assert_eq!(
        (
            java["kind"].as_str(),
            java["scope"].as_str(),
            java["passes"].as_bool()
        ),
        (Some("pass"), Some("user"), Some(true))
    );
    let app = yours
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "APP_ENV")
        .unwrap();
    assert_eq!(app["scope"], "project");
    assert!(v["groups"][3]["suggested"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s == "GOFLAGS"));
    let held: Vec<&str> = v["held"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["name"].as_str().unwrap())
        .collect();
    assert!(
        held.contains(&"SSH_AUTH_SOCK") && held.contains(&"SERVICE_TOKEN"),
        "{held:?}"
    );
    let text = v.to_string();
    assert!(
        !text.contains("/usr/lib/jvm") && !text.contains("service-token-value"),
        "{text}"
    );
    assert!(v["summary"].as_str().unwrap().starts_with("claude gets "));

    // --values adds plain values, never a secret.
    let v = w.json(&["env", "show", "--agent", "claude", "--json", "--values"]);
    let text = v.to_string();
    assert!(text.contains("/usr/lib/jvm/java-21"), "{text}");
    assert!(!text.contains("service-token-value"), "{text}");

    // The plain form fits in 80 columns.
    let plain = w.ok(&["env", "show", "--agent", "claude"]);
    assert!(plain.lines().all(|l| l.chars().count() <= 80), "{plain}");
    assert!(
        plain.contains("Your variables") && plain.contains("Held ("),
        "{plain}"
    );

    // remove, then an unknown agent.
    w.ok(&["env", "remove", "JAVA_HOME"]);
    assert!(!w.run(&["env", "remove", "JAVA_HOME"]).status.success());
    assert!(!w
        .run(&["env", "show", "--agent", "no-such-agent"])
        .status
        .success());
}

#[test]
fn env_mode_and_the_review_of_current_users() {
    let w = World::new(false);
    // A current installation: earlier runs, no [env] choice.
    std::fs::create_dir_all(w.root.join("state/paddock")).unwrap();
    std::fs::write(w.root.join("state/paddock/recent.json"), "[]").unwrap();
    let v = w.json(&["env", "show", "--json"]);
    assert_eq!(
        (v["mode"].as_str(), v["mode_origin"].as_str()),
        (Some("inherit"), Some("current installation"))
    );
    assert!(v["review"]["forced_from"].is_string());

    let r = w.json(&["env", "migrate", "--json"]);
    assert_eq!(r["api_version"], "paddock.env.review/v1");
    let items = r["items"].as_array().unwrap();
    let java = items.iter().find(|i| i["name"] == "JAVA_HOME").unwrap();
    assert_eq!(java["suggested"], true);
    assert!(items
        .iter()
        .all(|i| i["name"] != "SSH_AUTH_SOCK" && i["name"] != "SERVICE_TOKEN"));

    let out = w.ok(&["env", "migrate", "--yes"]);
    assert!(out.contains("JAVA_HOME"), "{out}");
    let cfg = std::fs::read_to_string(w.config()).unwrap();
    assert!(cfg.contains("mode = \"listed\""), "{cfg}");
    assert!(w.json(&["env", "show", "--json"])["review"].is_null());

    w.ok(&["env", "mode", "inherit"]);
    let v = w.json(&["env", "show", "--json"]);
    assert_eq!(
        (v["mode"].as_str(), v["mode_origin"].as_str()),
        (Some("inherit"), Some("configuration"))
    );
    assert!(v["review"].is_null());
    assert!(!w.run(&["env", "mode", "everything"]).status.success());
}

#[cfg(target_os = "linux")]
#[test]
fn credentials_add_reads_the_value_from_stdin_and_never_writes_it() {
    let w = World::new(true);
    let secret = "sk-proj-value-that-must-not-leak";
    let o = w.run_stdin(
        &[
            "credentials",
            "add",
            "OPENAI_API_KEY",
            "--agent",
            "codex",
            "--project",
            ".",
        ],
        &format!("{secret}\n"),
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("stays on this computer"), "{out}");
    let cfg = std::fs::read_to_string(w.config()).unwrap();
    assert!(!cfg.contains(secret), "{cfg}");
    assert!(
        cfg.contains("from = \"store\"") && cfg.contains("preset = \"openai\""),
        "{cfg}"
    );
    let calls = std::fs::read_to_string(w.root.join("calls")).unwrap();
    assert!(!calls.contains(secret), "{calls}");
    let stored =
        std::fs::read_to_string(w.root.join("items/paddock-credentials-OPENAI_API_KEY")).unwrap();
    assert_eq!(stored, secret, "the final newline is not part of the value");

    let v = w.json(&["env", "show", "--agent", "codex", "--json"]);
    let k = &v["groups"][2]["keys"][0];
    assert_eq!(k["name"], "OPENAI_API_KEY");
    assert_eq!(
        (
            k["state"].as_str(),
            k["grant"].as_str(),
            k["granted"].as_bool()
        ),
        (Some("kept_on_host"), Some("project"), Some(true))
    );
    assert_eq!(k["store"], "keychain");
    assert_eq!(k["unit"], "requests");
    assert_eq!(v["counts"]["keys"], 1);
    let v = w.json(&["env", "show", "--agent", "claude", "--json"]);
    assert_eq!(v["groups"][2]["keys"][0]["grant"], "agent");
    for args in [
        &["env", "show", "--agent", "codex", "--json", "--values"][..],
        &["credentials", "list", "--json"][..],
        &["credentials", "log", "--json"][..],
    ] {
        assert!(!w.ok(args).contains(secret), "{args:?}");
    }
    let log = w.json(&["credentials", "log", "--json"]);
    assert_eq!(log["api_version"], "paddock.credentials.log/v1");

    w.ok(&[
        "credentials",
        "grant",
        "OPENAI_API_KEY",
        "--agent",
        "claude",
    ]);
    w.ok(&["credentials", "remove", "OPENAI_API_KEY"]);
    assert!(!w
        .root
        .join("items/paddock-credentials-OPENAI_API_KEY")
        .exists());
    assert!(!std::fs::read_to_string(w.config())
        .unwrap()
        .contains("OPENAI_API_KEY"));
}

#[test]
fn credentials_add_without_a_keychain_refuses_and_names_the_command_backend() {
    let w = World::new(false);
    // PATH has /usr/bin: a real secret-tool there must not be reached.
    let mut c = w.cmd(&["credentials", "add", "ANTHROPIC_API_KEY"]);
    c.env("PATH", w.bin.display().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sk-ant-x\n")
        .unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("backend = \"command\""), "{err}");
    assert!(
        !w.config().exists()
            || !std::fs::read_to_string(w.config())
                .unwrap()
                .contains("ANTHROPIC")
    );
}
