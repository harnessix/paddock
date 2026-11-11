//! agy in horse mode in a real sandbox, with the fakes of
//! `tests/fixtures/horse`: a spawned headless child under live
//! stream-json custody (`agy-stream`), and an interactive run whose
//! entries paddock types into the TUI between its turns (`agy-tui`). Both
//! skip without bwrap and FUSE (and without python3);
//! src/horse/lifecycle/agy_tests.rs and src/horse/live/pty/agy_tests.rs
//! prove the same paths without them.

use super::codex_tui::pty;
use super::*;

/// The `[hooks]` table of the built-in agy profile.
const HOOKS: &str = "[hooks]\nfiles = [\".gemini/config/hooks.json\"]\ninto = \".gemini/config/hooks.json\"\npath = []\nstyle = \"named\"\n";

fn python() -> bool {
    Command::new("python3")
        .args(["-c", "import sqlite3"])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_spawned_agy_child_in_horse_mode_is_held_live() {
    use paddock::horse::run;
    use std::process::Stdio;

    if !sandbox_works() || !python() {
        eprintln!("skipped: needs bwrap, FUSE and python3");
        return;
    }
    let tmp = test_folder();
    let root = tmp.path();
    install_harness(
        root,
        "agy",
        "agy-stream",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"--dangerously-skip-permissions\", \"--print\", \"Read the file {prompt_file} and do the task that it describes.\"]\nprompt = \"file\"\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[host]\nspawn = \"allow\"\n[spawn]\nharnesses = [\"agy\"]\ntimeout = 120\n[horse]\nenabled = true\n",
    )
    .unwrap();
    task_project(root, "hi\n");
    let mut parent = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "agy", "--backend", "bwrap", "--network", "none"])
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
    // The child's session: the only agy session with a bound native id
    // (the parent runs final-only and binds none before its end).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let child = loop {
        let found = store_helpers::open(&store_path).ok().and_then(|st| {
            st.sessions()
                .ok()?
                .into_iter()
                .find(|s| s.binding.harness == "agy" && s.binding.native_id.is_some())
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
    // The fake holds the task turn until it sees this entry in its inbox.
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
    // The user steps of the conversation: the DNA entry, the text of the
    // task file (task.md holds "hi"), and the entry of the test.
    let users: Vec<String> = store
        .session_nodes(&session)
        .unwrap()
        .iter()
        .filter_map(|n| paddock::horse::live::agy::user_text(&n.raw))
        .collect();
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(
        users[0].starts_with("<paddock-append role=\"system\">"),
        "{users:?}"
    );
    assert_eq!(users[1..], ["hi", "from the test"], "{users:?}");
    let events = run::events(&store, &session.horse).unwrap();
    let end = events
        .iter()
        .find(|e| e["type"] == "run.end")
        .expect("a run.end event");
    assert_eq!(end["custody"], "live", "{end}");
    assert_eq!(end["added"], 0, "{end}");
    assert_eq!(end["landed"], 3, "{end}");
}

#[test]
fn an_agy_tui_in_horse_mode_gets_entries_between_turns_through_the_relay() {
    use paddock::horse::run;
    use std::process::Stdio;

    if !sandbox_works() || !python() {
        eprintln!("skipped: needs bwrap, FUSE and python3");
        return;
    }
    let tmp = test_folder();
    let root = tmp.path();
    install_harness(
        root,
        "agy",
        "agy-tui",
        &format!("[launch]\nargs = []\n{HOOKS}[horse.pty]\nversion = \"1.4.12\"\nprompt = \">\"\nempty_input = true\nbusy = [\"esc to cancel\"]\nquiet_ms = 500\ninterrupt_key = \"escape\"\nbracketed_paste = true\n"),
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[horse]\nenabled = true\n",
    )
    .unwrap();
    let (master, slave) = pty();
    let mut tui = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "agy", "--backend", "bwrap", "--network", "none"])
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
        .stdin(slave.try_clone().unwrap())
        .stdout(slave)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The test is the terminal: read what paddock shows, so it never
    // blocks on a full pty.
    {
        let mut m = master.try_clone().unwrap();
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = [0u8; 4096];
            while let Ok(n) = m.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        });
    }
    let store_path = root.join("data/paddock/horse");
    let log = root.join("project/tui.log");
    let wait = |what: &str, tui: &mut std::process::Child, f: &mut dyn FnMut() -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while !f() {
            if std::time::Instant::now() > deadline || tui.try_wait().unwrap().is_some() {
                use std::io::Read as _;
                let _ = tui.kill();
                let mut err = String::new();
                if let Some(mut e) = tui.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                let log = std::fs::read_to_string(&log).unwrap_or_default();
                panic!("{what}: {:?}\n{err}\ntui.log:\n{log}", tui.wait());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    let session = || {
        store_helpers::open(&store_path)
            .ok()?
            .sessions()
            .ok()?
            .into_iter()
            .find(|s| s.binding.harness == "agy")
    };
    wait("no agy session", &mut tui, &mut || session().is_some());
    let sid = session().unwrap().id;
    // Turn 1 thinks for 3 s: the entry is queued while it runs.
    paddock(root, &["append", &sid, "--text", "from the test"]);
    let landed = || {
        store_helpers::open(&store_path)
            .and_then(|s| s.deliveries(&sid))
            .is_ok_and(|d| {
                d.iter()
                    .any(|d| d.entry.text == "from the test" && d.landed_seq.is_some())
            })
    };
    wait("no landed delivery", &mut tui, &mut || landed());
    // The entry came only after the Stop hook of turn 1, and every entry
    // came after the stop of the turn before it.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let stop1 = lines
        .iter()
        .position(|l| *l == "hook stop 1")
        .unwrap_or_else(|| panic!("no stop 1:\n{text}"));
    let got = lines
        .iter()
        .position(|l| l.ends_with(" from the test"))
        .unwrap_or_else(|| panic!("the entry did not come:\n{text}"));
    assert!(stop1 < got, "{text}");
    let count = |p: &str| lines[..got].iter().filter(|l| l.starts_with(p)).count();
    assert!(
        count("hook stop ") >= count("got "),
        "an entry came during a turn:\n{text}"
    );
    std::fs::write(root.join("project/done"), "").unwrap();
    let out = tui.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let store = store_helpers::open(&store_path).unwrap();
    let s = store.session(&sid).unwrap();
    assert_eq!(
        s.binding.native_id.as_deref(),
        Some("7c0e1f2a-3b4c-4d5e-8f60-0a1b2c3d4e5f")
    );
    let end = run::events(&store, &s.horse)
        .unwrap()
        .into_iter()
        .find(|e| e["type"] == "run.end")
        .expect("a run.end event");
    assert_eq!(end["custody"], "pty", "{end}");
    assert_eq!(end["landed"], 2, "{end}");
    assert_eq!(end["not_landed"], serde_json::json!([]), "{end}");
    assert!(store.inbox(&sid).unwrap().is_empty());
    // paddock's hooks are in the sandbox copy only; the host has none.
    let host =
        std::fs::read_to_string(root.join("home/.gemini/config/hooks.json")).unwrap_or_default();
    assert!(
        !host.contains("safe-point"),
        "the host copy changed: {host}"
    );
    drop(master);
}
