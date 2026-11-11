//! Interrupted end-marker recovery uses fake mailboxes and temporary state.
mod common;

use paddock::env::HostEnv;
use paddock::mail::{self, events, Dir, Message, RunInfo, Starter};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const RUN: &str = "fake-proj-1a2b3c";

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: HostEnv,
}

impl World {
    fn new() -> Self {
        let tmp = common::tempdir();
        let root = tmp.path().to_path_buf();
        let mut vars = BTreeMap::new();
        for (key, dir) in [
            ("XDG_CONFIG_HOME", "cfg"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "run"),
        ] {
            vars.insert(key.into(), root.join(dir).display().to_string());
        }
        vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
        let env = HostEnv::from_parts(
            root.join("home"),
            "test".into(),
            unsafe { libc::getuid() },
            unsafe { libc::getgid() },
            vars,
        );
        Self {
            _tmp: tmp,
            root,
            env,
        }
    }

    fn mailbox(&self, id: &str, pid: u32, pid_start: Option<u64>) {
        mail::create(&self.env, id, &Starter::host()).unwrap();
        mail::set_run_info(
            &self.env,
            &RunInfo {
                id: id.into(),
                harness: "fake".into(),
                project: self.root.join("project"),
                pid,
                pid_start,
                started: 1,
                detached: false,
                delivery: None,
                horse_session: None,
            },
        )
        .unwrap();
    }

    fn dead_identity(&self) -> (u32, Option<u64>) {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let pid = child.id();
        let start = events::proc_start(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        (pid, start)
    }

    fn marker(&self, id: &str, bytes: &[u8]) {
        std::fs::write(mail::dir(&self.env, id).unwrap().join("end"), bytes).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_paddock"));
        command
            .args(args)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env_remove("PADDOCK_CALLER")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn end_event(code: i32) -> Message {
    let mut m = Message::event("ended", "the run ended");
    m.code = Some(code);
    m
}

fn provisional(code: i32) -> Vec<u8> {
    serde_json::to_vec(&end_event(code)).unwrap()
}

#[test]
fn an_abandoned_marker_reports_unknown_without_changing_evidence() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    let marker = provisional(0);
    w.marker(RUN, &marker);
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    assert!(String::from_utf8_lossy(&waited.stdout).contains("outcome unknown"));
    let watched = w.run(&["mail", "watch", RUN, "--timeout", "3s"]);
    assert_eq!(watched.status.code(), Some(2), "{watched:?}");
    assert!(String::from_utf8_lossy(&watched.stderr).contains("outcome unknown"));
    let listed = w.run(&["mail", "ls", "--json"]);
    assert_eq!(listed.status.code(), Some(0), "{listed:?}");
    let rows: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(rows[0]["state"], "unknown");
    let stored = mail::stored(&w.env);
    assert_eq!(stored.iter().find(|m| m.id == RUN).unwrap().ended, None);
    assert_eq!(mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER), 0);
    assert_eq!(
        std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
        marker
    );
    assert!(mail::read(&w.env, RUN, Dir::Up).is_empty());
}

#[test]
fn a_proven_end_repairs_the_marker_without_a_second_event() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    w.marker(RUN, &provisional(0));
    let logged = mail::append(&w.env, RUN, Dir::Up, end_event(0)).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(0), "{waited:?}");
    assert_eq!(mail::end(&w.env, RUN), Some(logged.clone()));
    let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
    assert_eq!(watched.status.code(), Some(0), "{watched:?}");
    assert_eq!(
        String::from_utf8_lossy(&watched.stdout)
            .matches("event ended")
            .count(),
        1
    );
    let resumed = w.run(&["mail", "watch", RUN, "--timeout", "3s"]);
    assert_eq!(resumed.status.code(), Some(0), "{resumed:?}");
    assert_eq!(
        mail::read(&w.env, RUN, Dir::Up)
            .iter()
            .filter(|m| m.is_end())
            .count(),
        1
    );
}

#[test]
fn missing_markers_reconcile_but_malformed_markers_are_preserved() {
    for malformed in [true, false] {
        let w = World::new();
        let (pid, start) = w.dead_identity();
        w.mailbox(RUN, pid, start);
        if malformed {
            w.marker(RUN, b"{broken");
        }
        let mut event = Message::event("ended", "known failure");
        event.code = Some(3);
        let logged = mail::append(&w.env, RUN, Dir::Up, event).unwrap();
        let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
        if malformed {
            assert_eq!(waited.status.code(), Some(2), "{waited:?}");
            assert_eq!(
                std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
                b"{broken"
            );
            let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
            assert_eq!(watched.status.code(), Some(2), "{watched:?}");
            assert_eq!(mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER), 0);
        } else {
            assert_eq!(waited.status.code(), Some(1), "{waited:?}");
            assert_eq!(mail::end(&w.env, RUN), Some(logged));
        }
        assert_eq!(
            mail::read(&w.env, RUN, Dir::Up)
                .iter()
                .filter(|m| m.is_end())
                .count(),
            1
        );
    }
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    w.marker(RUN, b"{broken");
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    assert_eq!(
        std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
        b"{broken"
    );
    assert!(mail::read(&w.env, RUN, Dir::Up).is_empty());
}

#[test]
fn conflicting_marker_payloads_are_preserved_and_not_watched() {
    for mismatch in ["type", "code", "text"] {
        let w = World::new();
        let (pid, start) = w.dead_identity();
        w.mailbox(RUN, pid, start);
        let marker = provisional(0);
        w.marker(RUN, &marker);
        let before = mail::append(
            &w.env,
            RUN,
            Dir::Up,
            Message::new("agent", mail::Kind::Status, "working"),
        )
        .unwrap();
        let mut event = end_event(0);
        match mismatch {
            "type" => event.event = Some("gone".into()),
            "code" => event.code = Some(3),
            "text" => event.text = "different result".into(),
            _ => unreachable!(),
        }
        mail::append(&w.env, RUN, Dir::Up, event).unwrap();
        let path = mail::dir(&w.env, RUN).unwrap();
        let log = std::fs::read(path.join("up.jsonl")).unwrap();
        let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
        assert_eq!(waited.status.code(), Some(2), "{mismatch}: {waited:?}");
        assert!(String::from_utf8_lossy(&waited.stdout).contains("conflicting end"));
        let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
        assert_eq!(watched.status.code(), Some(2), "{mismatch}: {watched:?}");
        let output = String::from_utf8_lossy(&watched.stdout);
        assert!(output.contains("working"), "{output}");
        assert!(
            !output.contains("event ended") && !output.contains("event gone"),
            "{output}"
        );
        assert_eq!(
            mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER),
            before.seq
        );
        let listed = w.run(&["mail", "ls", "--json"]);
        let rows: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
        assert_eq!(rows[0]["state"], "unknown");
        assert_eq!(
            mail::stored(&w.env)
                .iter()
                .find(|m| m.id == RUN)
                .unwrap()
                .ended,
            None
        );
        assert_eq!(std::fs::read(path.join("end")).unwrap(), marker);
        assert_eq!(std::fs::read(path.join("up.jsonl")).unwrap(), log);
    }
}

#[test]
fn multiple_logged_ends_are_not_reconciled_or_emitted() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    mail::append(&w.env, RUN, Dir::Up, end_event(0)).unwrap();
    mail::append(&w.env, RUN, Dir::Up, Message::event("gone", "other end")).unwrap();
    let d = mail::dir(&w.env, RUN).unwrap();
    let log = std::fs::read(d.join("up.jsonl")).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
    assert_eq!(watched.status.code(), Some(2), "{watched:?}");
    assert!(watched.stdout.is_empty(), "{watched:?}");
    assert_eq!(mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER), 0);
    assert!(!d.join("end").exists());
    assert_eq!(std::fs::read(d.join("up.jsonl")).unwrap(), log);
    assert_eq!(
        mail::stored(&w.env)
            .iter()
            .find(|m| m.id == RUN)
            .unwrap()
            .ended,
        None
    );

    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    events::ended(&w.env, RUN, Some(0)).unwrap();
    let d = mail::dir(&w.env, RUN).unwrap();
    let marker = std::fs::read(d.join("end")).unwrap();
    mail::append(&w.env, RUN, Dir::Up, Message::event("gone", "another end")).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
    assert_eq!(watched.status.code(), Some(2), "{watched:?}");
    assert!(watched.stdout.is_empty(), "{watched:?}");
    assert_eq!(mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER), 0);
    assert_eq!(std::fs::read(d.join("end")).unwrap(), marker);
}

#[test]
fn an_unterminated_end_is_not_committed_or_duplicated() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    w.marker(RUN, &provisional(0));
    let mut event = Message::event("ended", "not yet a complete line");
    event.code = Some(0);
    mail::append(&w.env, RUN, Dir::Up, event).unwrap();
    let path = mail::dir(&w.env, RUN).unwrap().join("up.jsonl");
    let mut bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.pop(), Some(b'\n'));
    std::fs::write(&path, &bytes).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    let later = mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, "working"),
    );
    assert!(later.is_err(), "an append must preserve the uncertain tail");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(mail::read(&w.env, RUN, Dir::Up).is_empty());
}

#[test]
fn an_unterminated_end_without_a_marker_is_not_followed_by_a_new_end() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    let event = Message::event("ended", "not yet a complete line");
    mail::append(&w.env, RUN, Dir::Up, event).unwrap();
    let d = mail::dir(&w.env, RUN).unwrap();
    let path = d.join("up.jsonl");
    let mut bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.pop(), Some(b'\n'));
    std::fs::write(&path, &bytes).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!d.join("end").exists());
}

#[test]
fn a_live_process_with_a_provisional_marker_is_not_reported_as_finished() {
    let w = World::new();
    let pid = std::process::id();
    let start = events::proc_start(pid);
    w.mailbox(RUN, pid, start);
    let marker = provisional(0);
    w.marker(RUN, &marker);
    let waited = w.run(&["wait", RUN, "--timeout", "1s"]);
    let expected = if start.is_some() { 124 } else { 2 };
    assert_eq!(waited.status.code(), Some(expected), "{waited:?}");
    let watched = w.run(&["mail", "watch", RUN, "--timeout", "1s"]);
    assert_eq!(watched.status.code(), Some(expected), "{watched:?}");
    let listed = w.run(&["mail", "ls", "--json"]);
    let rows: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(
        rows[0]["state"],
        if start.is_some() {
            "running"
        } else {
            "unknown"
        }
    );
    assert_eq!(
        std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
        marker
    );
}

#[test]
fn a_held_mailbox_lock_does_not_defeat_wait_or_watch_timeouts() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    w.marker(RUN, &provisional(0));
    let guard = mail::lock(&mail::dir(&w.env, RUN).unwrap()).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "1s"]);
    assert_eq!(waited.status.code(), Some(124), "{waited:?}");
    let watched = w.run(&["mail", "watch", RUN, "--timeout", "1s"]);
    assert_eq!(watched.status.code(), Some(124), "{watched:?}");
    drop(guard);
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
}

#[test]
fn missing_process_identity_is_an_unknown_outcome() {
    let w = World::new();
    mail::create(&w.env, RUN, &Starter::host()).unwrap();
    let marker = provisional(0);
    w.marker(RUN, &marker);
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    assert!(String::from_utf8_lossy(&waited.stdout).contains("outcome unknown"));
    assert_eq!(
        std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
        marker
    );
    assert!(mail::read(&w.env, RUN, Dir::Up).is_empty());

    for start in [None, Some(u64::MAX)] {
        let w = World::new();
        w.mailbox(RUN, std::process::id(), start);
        let marker = provisional(0);
        w.marker(RUN, &marker);
        let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
        assert_eq!(waited.status.code(), Some(2), "{waited:?}");
        assert!(String::from_utf8_lossy(&waited.stdout).contains("outcome unknown"));
        assert_eq!(
            std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
            marker
        );
    }
}

#[test]
fn concurrent_readers_reconcile_one_proven_end() {
    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    w.marker(RUN, &provisional(0));
    let logged = mail::append(&w.env, RUN, Dir::Up, end_event(0)).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let env = w.env.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    let state = mail::inspect_end(&env, RUN);
                    if state != mail::EndState::Pending || std::time::Instant::now() >= deadline {
                        break state;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(
            thread.join().unwrap(),
            mail::EndState::Complete(Box::new(logged.clone()))
        );
    }
    assert_eq!(mail::end(&w.env, RUN), Some(logged));
    assert_eq!(
        mail::read(&w.env, RUN, Dir::Up)
            .iter()
            .filter(|m| m.is_end())
            .count(),
        1
    );
}

#[test]
fn recovery_does_not_replay_a_run_end_hook() {
    use std::os::unix::fs::PermissionsExt;

    let w = World::new();
    let (pid, start) = w.dead_identity();
    let parent = "fake-proj-7a8b9c";
    mail::create(&w.env, parent, &Starter::host()).unwrap();
    mail::create(&w.env, RUN, &Starter::sandbox(parent)).unwrap();
    w.mailbox(RUN, pid, start);
    let script = w.root.join("hook.sh");
    let called = w.root.join("hook-called");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf called > '{}'\n", called.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = w.root.join("cfg/paddock");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[hooks]\nrun_end = ['{}']\n", script.display()),
    )
    .unwrap();
    w.marker(RUN, &provisional(0));
    let uncertain = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(uncertain.status.code(), Some(2), "{uncertain:?}");
    mail::append(&w.env, RUN, Dir::Up, end_event(0)).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(0), "{waited:?}");
    let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
    assert_eq!(watched.status.code(), Some(0), "{watched:?}");
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(!called.exists(), "recovery must not rerun the hook");
    assert!(mail::read(&w.env, parent, Dir::Up).is_empty());
}

#[test]
fn conflicting_evidence_does_not_replay_a_hook_or_parent_notification() {
    use std::os::unix::fs::PermissionsExt;

    let w = World::new();
    let (pid, start) = w.dead_identity();
    let parent = "fake-proj-7a8b9c";
    mail::create(&w.env, parent, &Starter::host()).unwrap();
    mail::create(&w.env, RUN, &Starter::sandbox(parent)).unwrap();
    w.mailbox(RUN, pid, start);
    let script = w.root.join("hook.sh");
    let called = w.root.join("hook-called");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf called > '{}'\n", called.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = w.root.join("cfg/paddock");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[hooks]\nrun_end = ['{}']\n", script.display()),
    )
    .unwrap();
    let marker = provisional(0);
    w.marker(RUN, &marker);
    mail::append(&w.env, RUN, Dir::Up, end_event(3)).unwrap();
    let waited = w.run(&["wait", RUN, "--timeout", "3s"]);
    assert_eq!(waited.status.code(), Some(2), "{waited:?}");
    let watched = w.run(&["mail", "watch", RUN, "--all", "--timeout", "3s"]);
    assert_eq!(watched.status.code(), Some(2), "{watched:?}");
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(!called.exists());
    assert!(mail::read(&w.env, parent, Dir::Up).is_empty());
    assert_eq!(
        std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
        marker
    );
}

#[test]
fn watch_mine_reports_one_uncertain_run_and_keeps_following_others() {
    use std::io::{BufRead, BufReader};

    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    w.marker(RUN, &provisional(0));
    let other = "fake-proj-4d5e6f";
    w.mailbox(
        other,
        std::process::id(),
        events::proc_start(std::process::id()),
    );
    let mut child = w
        .command(&["mail", "watch", "--mine", "--timeout", "3s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let diagnostic = rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    assert!(diagnostic.contains("incomplete end event"), "{diagnostic}");
    mail::append(
        &w.env,
        other,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, "working"),
    )
    .unwrap();
    let watched = child.wait_with_output().unwrap();
    reader.join().unwrap();
    assert_eq!(watched.status.code(), Some(124), "{watched:?}");
    assert!(String::from_utf8_lossy(&watched.stdout).contains("working"));
    assert_eq!(
        rx.try_iter()
            .filter(|line| line.contains("incomplete end event"))
            .count(),
        0,
        "the first diagnostic was the only one"
    );
    assert_eq!(mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER), 0);
}

#[test]
fn watch_mine_does_not_pass_a_conflicting_end() {
    use std::io::{BufRead, BufReader};

    let w = World::new();
    let (pid, start) = w.dead_identity();
    w.mailbox(RUN, pid, start);
    let marker = provisional(0);
    w.marker(RUN, &marker);
    let before = mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Say, "before conflict"),
    )
    .unwrap();
    let mut event = end_event(0);
    event.code = Some(3);
    mail::append(&w.env, RUN, Dir::Up, event).unwrap();
    mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Say, "after conflict"),
    )
    .unwrap();
    let other = "fake-proj-4d5e6f";
    w.mailbox(
        other,
        std::process::id(),
        events::proc_start(std::process::id()),
    );
    let mut child = w
        .command(&["mail", "watch", "--mine", "--timeout", "3s"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let diagnostic = rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    assert!(
        diagnostic.contains("conflicting end evidence"),
        "{diagnostic}"
    );
    mail::append(
        &w.env,
        other,
        Dir::Up,
        Message::new("agent", mail::Kind::Say, "other run continues"),
    )
    .unwrap();
    let watched = child.wait_with_output().unwrap();
    reader.join().unwrap();
    assert_eq!(watched.status.code(), Some(124), "{watched:?}");
    let output = String::from_utf8_lossy(&watched.stdout);
    assert!(output.contains("before conflict"), "{output}");
    assert!(output.contains("other run continues"), "{output}");
    assert!(
        !output.contains("event ended") && !output.contains("after conflict"),
        "{output}"
    );
    assert_eq!(
        rx.try_iter()
            .filter(|line| line.contains("conflicting end evidence"))
            .count(),
        0
    );
    assert_eq!(
        mail::cursor(&w.env, RUN, Dir::Up, mail::STARTER),
        before.seq
    );
    assert_eq!(
        std::fs::read(mail::dir(&w.env, RUN).unwrap().join("end")).unwrap(),
        marker
    );
}
