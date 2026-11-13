//! Grok Build in horse mode in a real sandbox, with the fakes of
//! `tests/fixtures/horse`: a spawned headless child under live ACP custody
//! (`grok-acp`), and an interactive run whose entries paddock types into
//! the TUI between its turns (`grok-tui`). Both skip without bwrap and
//! FUSE; src/horse/lifecycle/grok_tests.rs and
//! src/horse/live/pty/grok_tests.rs prove the same paths without them.

use super::codex_tui::pty;
use super::*;

/// The `[hooks]` table of the built-in grok profile.
const HOOKS: &str = "[hooks]\nfiles = [\".grok/hooks/*.json\"]\ninto = \".grok/hooks/paddock-compaction.json\"\n[hooks.folder_trust]\nfile = \".grok/trusted_folders.toml\"\ndrop_hooks = [\".grok/config.toml\"]\n";

#[test]
fn a_spawned_grok_child_in_horse_mode_is_held_live() {
    use paddock::horse::run;
    use std::process::Stdio;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = test_folder();
    let root = tmp.path();
    install_harness(
        root,
        "grok",
        "grok-acp",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"--always-approve\", \"--prompt-file\", \"{prompt_file}\"]\nprompt = \"file\"\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[host]\nspawn = \"allow\"\n[spawn]\nharnesses = [\"grok\"]\ntimeout = 120\n[horse]\nenabled = true\n",
    )
    .unwrap();
    task_project(root, "hi\n");
    let mut parent = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "grok", "--backend", "bwrap", "--network", "none"])
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
    // The child's session: the only grok session with a bound native id
    // (the parent runs final-only and binds none before its end).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let child = loop {
        let found = store_helpers::open(&store_path).ok().and_then(|st| {
            st.sessions()
                .ok()?
                .into_iter()
                .find(|s| s.binding.harness == "grok" && s.binding.native_id.is_some())
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
    let users: Vec<String> = store
        .session_nodes(&session)
        .unwrap()
        .iter()
        .filter_map(|n| {
            let v: serde_json::Value = serde_json::from_slice(&n.raw).ok()?;
            (v["type"] == "user").then(|| v["content"][0]["text"].to_string())
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

#[test]
fn a_grok_tui_in_horse_mode_gets_entries_between_turns_through_the_relay() {
    use paddock::horse::run;
    use std::process::Stdio;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = test_folder();
    let root = tmp.path();
    install_harness(
        root,
        "grok",
        "grok-tui",
        &format!("[launch]\nargs = []\n{HOOKS}[horse.pty]\nversion = \"1.0.70\"\nprompt = \"\u{276f}\"\nempty_input = true\nbusy = [\"[stop]\"]\nquiet_ms = 500\ninterrupt_key = \"ctrl-c\"\nbracketed_paste = true\n"),
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[horse]\nenabled = true\n",
    )
    .unwrap();
    let (master, slave) = pty();
    let mut tui = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "grok", "--backend", "bwrap", "--network", "none"])
        // The fake TUI logs in the project folder and waits for its flag there.
        .args(["--env", "clean", "-y", "--direct"])
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
            .find(|s| s.binding.harness == "grok")
    };
    wait("no grok session", &mut tui, &mut || session().is_some());
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
        Some("019a5f00-2222-7000-8000-00000000c0de")
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
    let host = std::fs::read_to_string(root.join("home/.grok/hooks/paddock-compaction.json"))
        .unwrap_or_default();
    assert!(
        !host.contains("safe-point"),
        "the host copy changed: {host}"
    );
    drop(master);
}
