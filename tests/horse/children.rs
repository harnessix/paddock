//! Claude Code and Codex children of a run in horse mode, and the Codex
//! swap, with the fake harnesses of `tests/fixtures/horse`.

use super::*;

/// A test folder whose claude harness is the fake claude: a git project
/// with a task, spawns allowed, horse mode on.
fn claude_setup() -> tempfile::TempDir {
    let tmp = test_folder();
    let root = tmp.path();
    install_harness(
        root,
        "claude",
        "claude",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"-p\", \"--dangerously-skip-permissions\"]\nprompt = \"stdin\"\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[host]\nspawn = \"allow\"\n[spawn]\nharnesses = [\"claude\"]\ntimeout = 120\n[horse]\nenabled = true\n",
    )
    .unwrap();
    task_project(root, "hi\n");
    tmp
}

#[test]
fn a_spawned_claude_child_in_horse_mode_is_held_live() {
    use paddock::horse::run;
    use std::process::Stdio;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = claude_setup();
    let root = tmp.path();
    let mut parent = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "claude", "--backend", "bwrap", "--network", "none"])
        .args(["--env", "clean", "-y"])
        .arg(root.join("project"))
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .current_dir(root.join("project"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let store_path = root.join("data/paddock/horse");
    // The child's session: the only claude session with a bound native id
    // (the parent runs final-only and binds none).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let child = loop {
        let found = store_helpers::open(&store_path).ok().and_then(|st| {
            st.sessions()
                .ok()?
                .into_iter()
                .find(|s| s.binding.harness == "claude" && s.binding.native_id.is_some())
        });
        if let Some(s) = found {
            break s;
        }
        if std::time::Instant::now() > deadline || parent.try_wait().unwrap().is_some() {
            let _ = parent.kill();
            let out = parent.wait_with_output().unwrap();
            let wait = std::fs::read_to_string(root.join("project/wait.txt")).unwrap_or_default();
            panic!("no child session: {out:?}\nwait.txt: {wait}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // The fake holds its first turn until it sees this entry in its inbox.
    paddock(root, &["append", &child.id, "--text", "from the test"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while parent.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = parent.kill();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = parent.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let store = store_helpers::open(&store_path).unwrap();
    let session = store.session(&child.id).unwrap();
    assert_eq!(
        store.horse(&session.horse).unwrap().sessions,
        [child.id.as_str()]
    );
    let users: Vec<String> = store
        .session_nodes(&session)
        .unwrap()
        .iter()
        .filter_map(|n| {
            let v: serde_json::Value = serde_json::from_slice(&n.raw).ok()?;
            (v["type"] == "user").then(|| v["message"]["content"][0]["text"].to_string())
        })
        .collect();
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(users[1].contains("hi"), "{users:?}");
    assert_eq!(users[2], "\"from the test\"");
    let events = run::events(&store, &session.horse).unwrap();
    let end = events
        .iter()
        .find(|e| e["type"] == "run.end")
        .expect("a run.end event");
    assert_eq!(end["custody"], "live", "{end}");
    assert_eq!(end["added"], 0, "{end}");
    assert_eq!(end["landed"], 3, "{end}");
}

/// A Codex rollout under a Codex home in the test folder: two turns.
fn codex_rollout(t: &Path, id: &str) -> std::path::PathBuf {
    let dir = t.join("codex-home/sessions/2026/11/06");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("rollout-2026-11-06T00-00-00-{id}.jsonl"));
    let row = |ty: &str, payload: serde_json::Value| {
        serde_json::json!({"timestamp": "2026-11-06T00:00:00.000Z", "type": ty, "payload": payload})
            .to_string()
    };
    let msg = |text: &str| {
        row(
            "response_item",
            serde_json::json!({"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": text}]}),
        )
    };
    let done = row("event_msg", serde_json::json!({"type": "task_complete"}));
    let rows = [
        row(
            "session_meta",
            serde_json::json!({"id": id, "cli_version": "0.166.0", "cwd": "/w"}),
        ),
        msg("first"),
        done.clone(),
        msg("second"),
        done,
    ];
    std::fs::write(&p, rows.join("\n") + "\n").unwrap();
    p
}

#[test]
fn swap_a_stopped_codex_session_on_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let id = "019a5f00-0000-7000-8000-0000000000dd";
    let p = codex_rollout(t, id);
    let before = std::fs::read(&p).unwrap();
    let out = paddock(
        t,
        &[
            "ingest-codex",
            p.to_str().unwrap(),
            "--codex-version",
            "0.166.0",
        ],
    );
    let sid = out.split(':').next().unwrap().to_string();
    let log = paddock(t, &["log", &sid]);
    let nodes: Vec<&str> = log
        .lines()
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(nodes.len(), 5, "{log}");
    // Without a run and without --codex-home there is nothing to swap.
    let o = cmd(t, &["swap", &sid, nodes[2]]).output().unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("--codex-home"),
        "{o:?}"
    );
    assert_eq!(std::fs::read(&p).unwrap(), before);
    // Back to the end of the first turn: the same thread, the prefix stays.
    let home = t.join("codex-home");
    let o = paddock(
        t,
        &[
            "swap",
            &sid,
            nodes[2],
            "--codex-home",
            home.to_str().unwrap(),
        ],
    );
    assert!(o.contains(&format!("under thread {id}")), "{o}");
    assert!(o.contains("the old head stays in session"), "{o}");
    let text = std::fs::read_to_string(&p).unwrap();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert!(!text.contains("second"));
    let show = paddock(t, &["show", &sid, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&show).unwrap();
    assert!(v["session"]["head"].as_str().unwrap().starts_with(nodes[2]));
    assert_eq!(v["session"]["binding"]["native_id"], id);
    // A branch of another thread is refused, and nothing changes.
    let other = "019a5f00-0000-7000-8000-0000000000ee";
    let q = codex_rollout(t, other);
    let out = paddock(
        t,
        &[
            "ingest-codex",
            q.to_str().unwrap(),
            "--codex-version",
            "0.166.0",
        ],
    );
    let other_sid = out.split(':').next().unwrap().to_string();
    let o = cmd(
        t,
        &[
            "swap",
            &sid,
            &other_sid,
            "--codex-home",
            home.to_str().unwrap(),
        ],
    )
    .output()
    .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("does not continue thread"),
        "{o:?}"
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
}

const FAKE_THREAD: &str = "019a5f00-0000-7000-8000-00000000e2e0";

/// Wait up to `secs` for `f`.
fn wait_for<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(
            std::time::Instant::now() < end,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The first node of a session whose row says `task_complete`.
fn first_turn_end(store: &paddock::horse::Store, sid: &str) -> Option<paddock::horse::store::Hash> {
    let s = store.session(sid).ok()?;
    store
        .session_nodes(&s)
        .ok()?
        .into_iter()
        .find(|n| String::from_utf8_lossy(&n.raw).contains("\"task_complete\""))
        .map(|n| n.hash)
}

fn rollout_of(store: &paddock::horse::Store, head: &paddock::horse::store::Hash) -> Vec<u8> {
    let mut b = Vec::new();
    for n in store.chain(Some(head)).unwrap() {
        b.extend_from_slice(&n.raw);
        b.push(b'\n');
    }
    b
}

#[test]
fn the_fake_app_server_swaps_between_turns_under_custody() {
    use paddock::horse::codex::swap::request;
    use paddock::horse::live::codex::{run, CustodyCtx, Policy};
    use paddock::horse::live::harness::{ChannelIo, Control, Ends};
    use paddock::horse::{codex, run as runs, Binding, Store};
    use std::process::Stdio;
    use std::time::Duration;

    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let home = t.join("home");
    let work = t.join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let store = Store::open(&t.join("store")).unwrap();
    let horse = store.new_default_horse().unwrap().id;
    let sid = store
        .new_session(
            &horse,
            Binding {
                harness: codex::HARNESS.into(),
                native_id: None,
                version: codex::VERSION.into(),
                container: codex::CONTAINER.into(),
            },
        )
        .unwrap()
        .id;
    store.queue_append(&sid, "user", "first").unwrap();
    store.queue_append(&sid, "user", "second").unwrap();
    let mut child = Command::new(fixture("codex-app-server"))
        .args(["app-server", "--listen", "stdio://"])
        .env("HOME", &home)
        .env("FAKE_CODEX_STEP", "1")
        .current_dir(&work)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (ctl, control) = std::sync::mpsc::channel();
    let io = ChannelIo {
        from: child.stdout.take().unwrap(),
        to: child.stdin.take().unwrap(),
        control,
    };
    let ctx = CustodyCtx {
        store: store.clone(),
        horse: horse.clone(),
        session: sid.clone(),
        cwd: work.display().to_string(),
        sessions_root: home.join(".codex/sessions"),
        run: "sbx-fixture".into(),
        model: None,
        effort: None,
        tick: Duration::from_millis(50),
        reply_timeout: Duration::from_secs(20),
        close_timeout: Duration::from_secs(10),
        out: Box::new(std::io::sink()),
    };
    let policy = Policy {
        approval: "never".into(),
        sandbox: "danger-full-access".into(),
    };
    let custody = std::thread::spawn(move || run(io, ctx, policy, Ends::ByRun));
    let to = wait_for("the first turn in the store", 30, || {
        first_turn_end(&store, &sid)
    });
    // Ask once the second turn moved the head on. A request that the
    // custody takes between the turns finds the session at `to` already,
    // and refuses it: there is nothing to swap.
    wait_for("the second turn in the store", 30, || {
        let head = store.session(&sid).ok()?.head;
        (head.is_some() && head.as_ref() != Some(&to)).then_some(())
    });
    let r = request::ask(&store, &sid, &to, false, Duration::from_secs(30)).unwrap();
    let a = wait_for("the answer of the custody", 40, || {
        request::answer(&store, &sid, &r.id).unwrap()
    });
    request::remove(&store, &sid, &r.id).unwrap();
    assert_eq!(a.state, request::State::Done, "{a:?}");
    assert_eq!(a.thread.as_deref(), Some(FAKE_THREAD));
    // The server read the swapped rollout when it loaded the thread again.
    assert_eq!(
        std::fs::read(work.join("resumed.txt")).unwrap(),
        rollout_of(&store, &to)
    );
    ctl.send(Control::Close).unwrap();
    let report = custody.join().unwrap().unwrap();
    assert_eq!(report.native_id.as_deref(), Some(FAKE_THREAD));
    assert!(child.wait().unwrap().success());
    let s = store.session(&sid).unwrap();
    assert!(store
        .session_nodes(&s)
        .unwrap()
        .iter()
        .any(|n| n.hash == to));
    let events = runs::events(&store, &horse).unwrap();
    let swap: Vec<_> = events.iter().filter(|e| e["type"] == "swap.live").collect();
    assert_eq!(swap.len(), 1, "{events:?}");
    assert_eq!(swap[0]["thread"], FAKE_THREAD);
    assert_eq!(swap[0]["to"], to.as_str());
}

/// A test folder whose dsh parent spawns the fake Codex app-server as a
/// headless child, in horse mode.
fn codex_spawn_setup() -> tempfile::TempDir {
    let tmp = sandbox_setup("dsh-spawn-codex");
    let root = tmp.path();
    install_harness(
        root,
        "codex",
        "codex-app-server",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"exec\", \"--dangerously-bypass-approvals-and-sandbox\", \"-\"]\nprompt = \"stdin\"\nprobe_args = [\"exec\", \"--help\"]\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[host]\nspawn = \"allow\"\n[spawn]\nharnesses = [\"codex\"]\ntimeout = 150\n",
    )
    .unwrap();
    task_project(root, "write the note\n");
    tmp
}

#[test]
fn a_headless_codex_child_runs_under_live_custody_and_swaps_between_turns() {
    use paddock::horse::{live, run as runs};

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = codex_spawn_setup();
    let root = tmp.path().to_path_buf();
    let parent = {
        let root = root.clone();
        std::thread::spawn(move || run_dsh_for(&root, 180))
    };
    let store = host_store(&root);
    // The child's custody writes a live record once its thread is open.
    let sid = wait_for("the live record of the codex child", 90, || {
        live::records(&store).ok()?.into_iter().find_map(|r| {
            let s = r.sessions.first()?;
            (s.native == FAKE_THREAD).then(|| s.session.clone())
        })
    });
    // A second turn keeps the run going while the swap waits.
    let out = paddock(&root, &["append", &sid, "--text", "second"]);
    assert!(!out.trim().is_empty());
    let to = wait_for("the first turn in the store", 60, || {
        first_turn_end(&store, &sid)
    });
    let swap = cmd(&root, &["swap", &sid, to.as_str()]).output().unwrap();
    let said = String::from_utf8_lossy(&swap.stdout);
    assert!(
        swap.status.success(),
        "{said}\n{}",
        String::from_utf8_lossy(&swap.stderr)
    );
    assert!(
        said.contains(&format!("under thread {FAKE_THREAD}")),
        "{said}"
    );
    let out = parent.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let s = store.session(&sid).unwrap();
    assert!(store
        .session_nodes(&s)
        .unwrap()
        .iter()
        .any(|n| n.hash == to));
    let kinds: Vec<String> = runs::events(&store, &s.horse)
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_string())
        .filter(|k| !k.starts_with("dna."))
        .collect();
    assert_eq!(kinds, ["run.begin", "swap.live", "run.end"], "{kinds:?}");
    let end = runs::events(&store, &s.horse)
        .unwrap()
        .into_iter()
        .find(|e| e["type"] == "run.end")
        .unwrap();
    assert_eq!(end["custody"], "live", "{end}");
}
