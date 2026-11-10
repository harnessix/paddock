//! A headless Claude Code run under live custody, with the fake claude of
//! `tests/fixtures/horse/claude` as a real process on pipes.
use super::*;
use crate::horse::claude;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Tests that arm the custody of the next sandbox: the hook is one per
/// process.
pub(super) static ARMS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/horse")
        .join(name)
}

/// Make `b` a spawned child in horse mode whose task `prompt` comes on
/// stdin: a headless run.
pub(super) fn headless(b: &mut Built, prompt: &str) {
    b.horse = true;
    b.stdin = Some(prompt.as_bytes().to_vec());
    b.spawn = Some(crate::spawn::ChildInfo {
        spawn_id: "s1".into(),
        parent: "parent".into(),
        chain: vec!["parent".into()],
        depth: 1,
        modes: BTreeMap::new(),
        role: Default::default(),
        hitch: None,
    });
}

fn child_of(b: &mut Built, prompt: &str) {
    headless(b, prompt);
    b.spec.id = "child".into();
    b.harness_exe = fixture("claude");
    b.spec.command = vec![
        fixture("claude").display().to_string(),
        "-p".into(),
        "--dangerously-skip-permissions".into(),
        "--model".into(),
        "claude-opus-5-5".into(),
    ];
}

/// Start the rewritten command of `b` as the sandbox would, with the
/// pipes that the armed custody takes.
pub(super) fn spawn_armed(b: &Built, env: &[(&str, &str)]) -> std::process::Child {
    let (hook, _) = crate::horse::live::take_hooks();
    let home = &b.spec.home_src;
    std::fs::create_dir_all(home).unwrap();
    let cmd = &b.spec.command;
    let cwd = &b.spec.workdir;
    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("PWD", cwd)
        .envs(env.iter().copied())
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    crate::horse::live::handoff(hook, &mut child);
    child
}

pub(super) fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_headless_claude_run_is_held_live_and_gets_entries_after_a_result() {
    let _armed = ARMS.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "claude", claude::VERSION);
    child_of(&mut b, "do the task");
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::Live);
    assert!(
        b.stdin.is_none(),
        "the task goes into the inbox, not on stdin"
    );
    let store = Store::open_host(&env).unwrap();
    // The session id is bound before the harness starts.
    let native = store
        .session(&l.run.session)
        .unwrap()
        .binding
        .native_id
        .expect("a bound native id");
    let cmd = &b.spec.command;
    let at = cmd.iter().position(|a| a == "--session-id").unwrap();
    assert_eq!(cmd[at + 1], native);
    assert!(cmd
        .windows(2)
        .any(|w| w == ["--output-format", "stream-json"]));
    assert!(cmd.windows(2).any(|w| w == ["--model", "claude-opus-5-5"]));
    assert_eq!(cmd.iter().filter(|a| *a == "--model").count(), 1);
    let inbox = store.inbox(&l.run.session).unwrap();
    assert_eq!(inbox.len(), 2);
    assert_eq!(inbox[1].role, "user");
    assert_eq!(inbox[1].text, "do the task");

    let mut child = spawn_armed(&b, &[]);
    let home = b.spec.home_src.clone();
    let cwd = b.spec.workdir.clone();
    // Turn 1 runs: its assistant row is in the transcript, no result yet.
    let file = claude::transcript_path(&home.join(".claude"), &cwd.display().to_string(), &native);
    wait_until("the first assistant row", || {
        std::fs::read_to_string(&file).is_ok_and(|s| s.lines().count() >= 2)
    });
    store
        .queue_append(&l.run.session, "user", "from the test")
        .unwrap();
    std::fs::write(home.join("go"), "").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "the fake failed: {status}");
    assert!(!home.join("early").exists(), "an entry came during a turn");
    let horse = l.run.horse.clone();
    let session = l.run.session.clone();
    let out = end(Some(l), 0, None).expect("the custody output");
    assert!(out.contains("claude: turn 1 done"), "{out}");
    assert!(out.contains("claude: turn 3 done"), "{out}");

    let s = store.session(&session).unwrap();
    let users: Vec<String> = store
        .session_nodes(&s)
        .unwrap()
        .iter()
        .filter_map(|n| {
            let v: serde_json::Value = serde_json::from_slice(&n.raw).ok()?;
            (v["type"] == "user").then(|| {
                v["message"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        })
        .collect();
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(users[0].starts_with("<paddock-append role=\"system\">"));
    assert_eq!(users[1], "do the task");
    assert_eq!(users[2], "from the test");
    assert!(store.inbox(&session).unwrap().is_empty());
    assert_eq!(store.horse(&horse).unwrap().sessions, vec![session.clone()]);
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["type"], "run.end");
    assert_eq!(e["custody"], "live");
    assert_eq!(e["added"], 0, "the custody ingested every turn: {e}");
    assert!(e.get("error").is_none(), "{e}");
    assert_eq!(e["nodes"], 6);
    assert_eq!(e["turns"], 3);
    assert_eq!(e["deliveries"], 3);
    assert_eq!(e["landed"], 3);
    assert_eq!(e["ok"], true);
    assert!(crate::horse::live::records(&store).unwrap().is_empty());
}

#[test]
fn an_interactive_claude_run_without_a_pty_table_stays_final_only() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "claude", claude::VERSION);
    b.horse = true;
    let before = b.spec.command.clone();
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::FinalOnly);
    assert_eq!(b.spec.command, before);
    let store = Store::open_host(&env).unwrap();
    assert!(store
        .session(&l.run.session)
        .unwrap()
        .binding
        .native_id
        .is_none());
    let horse = l.run.horse.clone();
    assert_eq!(end(Some(l), 0, None), None);
    let events = super::tests::run_events(&store, &horse).unwrap();
    assert_eq!(events.last().unwrap()["custody"], "final-only");
    assert!(events.last().unwrap().get("added").is_none());
}

#[test]
fn an_unpinned_claude_aborts_before_the_store_with_the_codec_message() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "claude", "2.1.999 (Claude Code)");
    child_of(&mut b, "do the task");
    // The version script of `setup`, not the fake.
    b.harness_exe = t.path().join("fake-version");
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("Claude Code 2.1.999 is not supported"), "{e}");
    assert!(!crate::horse::root(&env).exists());
    assert!(b.stdin.is_some());
}

#[test]
fn a_bad_oneshot_command_fails_before_the_store() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "claude", claude::VERSION);
    child_of(&mut b, "do the task");
    b.spec.command.push("--output-format".into());
    b.spec.command.push("json".into());
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("must not set --output-format"), "{e}");
    assert!(!crate::horse::root(&env).exists());
}

#[test]
fn a_headless_codex_run_is_held_live_and_ends_when_its_inbox_is_empty() {
    use crate::horse::codex;
    let _armed = ARMS.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "codex", codex::VERSION);
    headless(&mut b, "write the note\n");
    b.spec.id = "child".into();
    b.harness_exe = fixture("codex-app-server");
    b.profile.oneshot = Some(
        toml::from_str(
            "args = [\"exec\", \"--dangerously-bypass-approvals-and-sandbox\", \"-\"]\nprompt = \"stdin\"\n",
        )
        .unwrap(),
    );
    b.spec.command = vec![
        fixture("codex-app-server").display().to_string(),
        "exec".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        "-".into(),
    ];
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::Live);
    assert_eq!(b.spec.command[1], "app-server");
    let store = Store::open_host(&env).unwrap();
    let session = l.run.session.clone();
    let horse = l.run.horse.clone();
    let mut child = spawn_armed(&b, &[("FAKE_CODEX_STEP", "0")]);
    // The custody closes the input once the task is done and the inbox is
    // empty; the fake then reads the end of its input and exits.
    let status = child.wait().unwrap();
    assert!(status.success(), "the fake failed: {status}");
    let out = end(Some(l), 0, None).expect("the custody output");
    assert!(
        out.contains("paddock: codex thread 019a5f00-0000-7000-8000-00000000e2e0 in custody"),
        "{out}"
    );
    assert!(store.inbox(&session).unwrap().is_empty());
    let s = store.session(&session).unwrap();
    let rows: Vec<String> = store
        .session_nodes(&s)
        .unwrap()
        .iter()
        .map(|n| String::from_utf8_lossy(&n.raw).into_owned())
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("done turn-1")),
        "the task turn is in the store: {rows:?}"
    );
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["type"], "run.end");
    assert_eq!(e["custody"], "live");
    assert!(e.get("error").is_none(), "{e}");
    assert_eq!(e["added"], 0, "the custody ingested every turn: {e}");
    // The DNA entry goes in as an injected item, the task as the turn.
    assert_eq!(e["deliveries"], 2, "{e}");
    assert_eq!(e["turns"], 1, "{e}");
    assert!(crate::horse::live::records(&store).unwrap().is_empty());
}

#[test]
fn an_empty_task_fails_before_the_store() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "claude", claude::VERSION);
    child_of(&mut b, " \n");
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(e.contains("the task of the run is empty"), "{e}");
    assert!(!crate::horse::root(&env).exists());
}

#[test]
fn a_custody_that_did_not_start_says_so_in_the_run_end() {
    let _armed = ARMS.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "claude", claude::VERSION);
    child_of(&mut b, "do the task");
    let l = start(&env, &mut b).unwrap().unwrap();
    let store = Store::open_host(&env).unwrap();
    let horse = l.run.horse.clone();
    // The sandbox never starts: nobody takes the hook.
    assert_eq!(end(Some(l), 1, None).as_deref(), Some(""));
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["custody"], "live");
    assert_eq!(e["error"], "the custody did not start");
    assert_eq!(e["ok"], false);
    assert!(crate::horse::live::take_hook().is_none(), "no hook is left");
}

#[test]
fn a_headless_codex_run_without_a_oneshot_command_fails_before_the_store() {
    use crate::horse::codex;
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = super::tests::setup(t.path(), "codex", codex::VERSION);
    headless(&mut b, "write the note\n");
    let before = b.spec.command.clone();
    let e = format!("{:#}", start(&env, &mut b).err().unwrap());
    assert!(
        e.contains("the codex profile has no [oneshot] command"),
        "{e}"
    );
    assert!(!crate::horse::root(&env).exists());
    assert_eq!(b.spec.command, before);
    assert!(b.stdin.is_some(), "the task stays on stdin");
}
