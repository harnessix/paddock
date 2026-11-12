//! `paddock horse import`, `paddock horse adopt` and the hidden `paddock
//! horse hook`, as a user runs them: the real program, with a temporary
//! HOME and temporary XDG folders. The host sessions are fixtures that the
//! test writes; nothing reads or writes a real home.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ID: &str = "01934b20-1111-7777-8888-0000000000a1";

fn cmd(tmp: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.arg("horse")
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.join("home"))
        .env("XDG_DATA_HOME", tmp.join("data"))
        .env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CONFIG_HOME", tmp.join("config"))
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env("XDG_RUNTIME_DIR", tmp.join("run"));
    c
}

fn paddock(tmp: &Path, args: &[&str]) -> String {
    let out = cmd(tmp, args).output().unwrap();
    assert!(
        out.status.success(),
        "paddock horse {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `paddock horse hook HARNESS EVENT` with `input` on stdin: its stdout.
fn hook(tmp: &Path, event: &str, input: &serde_json::Value) -> String {
    let mut child = cmd(tmp, &["hook", "claude", event, "--mark", "paddock-horse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "a hook always exits 0");
    String::from_utf8(out.stdout).unwrap()
}

fn host_store(tmp: &Path) -> paddock::horse::Store {
    let vars = [("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state")]
        .into_iter()
        .map(|(key, path)| (key.to_string(), tmp.join(path).display().to_string()))
        .collect();
    let env = paddock::env::HostEnv::from_parts(tmp.join("home"), "test".into(), 0, 0, vars);
    paddock::horse::Store::open_host(&env).unwrap()
}

/// A Claude Code transcript of `n` rows in the project folder `project`.
fn write_claude(tmp: &Path, project: &str, n: usize) -> PathBuf {
    let rel = PathBuf::from(format!(".claude/projects/{project}/{ID}.jsonl"));
    let p = tmp.join("home").join(&rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut text = String::new();
    for i in 0..n {
        let parent = match i {
            0 => "null".to_string(),
            _ => format!("\"00000000-0000-4000-8000-{:012}\"", i - 1),
        };
        let (ty, role) = if i % 2 == 0 {
            ("user", "user")
        } else {
            ("assistant", "assistant")
        };
        text.push_str(&format!(
            "{{\"type\":\"{ty}\",\"uuid\":\"00000000-0000-4000-8000-{i:012}\",\"parentUuid\":{parent},\"sessionId\":\"{ID}\",\"version\":\"{}\",\"message\":{{\"role\":\"{role}\",\"content\":\"row {i}\"}}}}\n",
            paddock::horse::claude::VERSION
        ));
    }
    std::fs::write(&p, text).unwrap();
    rel
}

#[test]
fn import_list_and_fork_a_host_session() {
    let tmp = paddock::view::guard::tempdir();
    let t = tmp.path();
    let rel = write_claude(t, "-home-me-src-app", 3);
    let host_file = std::fs::read(t.join("home").join(&rel)).unwrap();
    assert_eq!(
        paddock(t, &["import", "claude", "--list"]).trim(),
        rel.display().to_string()
    );
    // The project folder is no session id (after `--`: it starts with `-`).
    let out = cmd(t, &["import", "claude", "--", "-home-me-src-app"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no claude session -home-me-src-app"));
    let line = paddock(t, &["import", "claude", ID]);
    let session = line.split('\t').next().unwrap().to_string();
    assert!(line.contains("3 row(s), new"), "{line}");
    let list = paddock(t, &["list"]);
    assert!(
        list.contains(&format!("host session (import), ~/{}", rel.display())),
        "{list}"
    );
    // A fork of the host session is a session of its own.
    let st = host_store(t);
    let nodes = st.session_nodes(&st.session(&session).unwrap()).unwrap();
    drop(st);
    let fork = paddock(t, &["fork", &session, "--at", nodes[1].hash.short()]);
    let st = host_store(t);
    let f = st.session(fork.trim()).unwrap();
    assert!(f.host.is_none());
    assert_eq!(f.fork_of.unwrap().session, session);
    assert_eq!(
        st.session_nodes(&st.session(&session).unwrap())
            .unwrap()
            .len(),
        3
    );
    // The host file never changed.
    assert_eq!(std::fs::read(t.join("home").join(&rel)).unwrap(), host_file);
}

#[test]
fn adopt_hook_and_off_keep_every_byte_of_the_settings() {
    let tmp = paddock::view::guard::tempdir();
    let t = tmp.path();
    let rel = write_claude(t, "p", 2);
    let settings = t.join("home/.claude/settings.json");
    let mine = "{\n    \"zeta\": true,\n    \"hooks\": {\n        \"Stop\": [ { \"hooks\": [ { \"type\": \"command\", \"command\": \"notify-send done\" } ] } ]\n    },\n    \"alpha\": 1\n}\n";
    std::fs::write(&settings, mine).unwrap();
    let out = paddock(t, &["adopt", "claude"]);
    assert!(out.contains("claude: 1 new session(s)"), "{out}");
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.starts_with("{\n    \"zeta\": true,\n    \"hooks\": {\n        \"Stop\": [ { \"hooks\": [ { \"type\": \"command\", \"command\": \"notify-send done\" } ] }"), "{text}");
    assert!(text.ends_with("    },\n    \"alpha\": 1\n}\n"), "{text}");
    let status = paddock(t, &["adopt", "--status"]);
    assert!(
        status.contains(&format!(
            "state: {}",
            t.join("state/paddock/horse/adoption.json").display()
        )),
        "{status}"
    );
    // A queued entry goes out at the end of the next turn of the host session.
    let st = host_store(t);
    let session = st.host_session("claude", ID).unwrap().unwrap().id;
    drop(st);
    paddock(
        t,
        &["append", &session, "--text", "The tests pass on main."],
    );
    write_claude(t, "p", 4);
    let input = serde_json::json!({"session_id": ID, "hook_event_name": "Stop",
        "transcript_path": t.join("home").join(&rel)});
    let out: serde_json::Value = serde_json::from_str(&hook(t, "Stop", &input)).unwrap();
    assert_eq!(out["decision"], "block");
    assert!(out["reason"]
        .as_str()
        .unwrap()
        .contains("The tests pass on main."));
    assert_eq!(hook(t, "Stop", &input), "");
    let st = host_store(t);
    let s = st.session(&session).unwrap();
    assert_eq!(st.session_nodes(&s).unwrap().len(), 4);
    assert!(st.inbox(&session).unwrap().is_empty());
    drop(st);
    let out = paddock(t, &["adopt", "--off", "claude"]);
    assert!(out.contains("back as it was"), "{out}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), mine);
}

#[test]
fn adopt_without_hooks_changes_no_configuration() {
    let tmp = paddock::view::guard::tempdir();
    let t = tmp.path();
    write_claude(t, "p", 1);
    paddock(t, &["adopt", "claude", "--no-hooks"]);
    assert!(!t.join("home/.claude/settings.json").exists());
    let status = paddock(t, &["adopt", "--status"]);
    assert!(status.contains("hooks: none"), "{status}");
}

#[test]
fn the_guide_names_the_state_folder_as_paddock_finds_it() {
    let guide = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/help/guide-horse.md"),
    )
    .unwrap();
    // paddock uses $XDG_STATE_HOME when it is set, on Linux and on macOS.
    assert!(guide.contains("`$XDG_STATE_HOME/paddock`"));
    assert!(
        !guide.contains("`~/.local/state/paddock/horse/"),
        "a fixed path of the state folder"
    );
}
