//! An interactive Claude Code run in horse mode on a version with no
//! channel path, with the fake TUI of `tests/fixtures/horse/claude-tui-pty`
//! on a terminal that the test owns: an entry queued while the TUI
//! "thinks" is typed into it through paddock's relay only after the Stop
//! hook of that turn ran and the input row came back, and it lands.

use super::*;

#[test]
fn a_claude_tui_without_a_channel_gets_entries_between_turns_through_the_relay() {
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
        "claude",
        "claude-tui-pty",
        "[launch]\nargs = []\n[horse.pty]\nversion = \"2.1.317\"\nprompt = \"\u{276f}\"\nempty_input = true\nbusy = [\"esc to interrupt\"]\nquiet_ms = 500\ninterrupt_key = \"escape\"\nbracketed_paste = true\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[horse]\nenabled = true\n",
    )
    .unwrap();
    let (master, slave) = super::codex_tui::pty();
    let mut tui = Command::new(env!("CARGO_BIN_EXE_paddock"))
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
        .stdin(slave.try_clone().unwrap())
        .stdout(slave)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The test is the terminal: read what paddock shows, so it never
    // blocks on a full pty.
    let shown = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let shown = shown.clone();
        let mut m = master.try_clone().unwrap();
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = [0u8; 4096];
            while let Ok(n) = m.read(&mut buf) {
                if n == 0 {
                    break;
                }
                shown.lock().unwrap().extend_from_slice(&buf[..n]);
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
            .find(|s| s.binding.harness == "claude")
    };
    wait("no claude session", &mut tui, &mut || session().is_some());
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
    // The entry came only after the Stop hook of turn 1. The DNA entry of
    // the run goes first, so the test entry is the third turn.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let at = |l: &str| lines.iter().position(|x| *x == l);
    let stop1 = at("stop 1").unwrap_or_else(|| panic!("no stop 1:\n{text}"));
    let got = lines
        .iter()
        .position(|l| l.ends_with(" from the test"))
        .unwrap_or_else(|| panic!("the entry did not come:\n{text}"));
    assert!(stop1 < got, "{text}");
    assert!(
        lines[..got]
            .iter()
            .filter(|l| l.starts_with("stop "))
            .count()
            >= lines[..got]
                .iter()
                .filter(|l| l.starts_with("got "))
                .count(),
        "an entry came during a turn:\n{text}"
    );
    std::fs::write(root.join("project/done"), "").unwrap();
    let out = tui.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "claude gets queued entries between its turns: paddock types them into the TUI"
        ),
        "{stderr}"
    );
    let store = store_helpers::open(&store_path).unwrap();
    let s = store.session(&sid).unwrap();
    // paddock picked the session id and gave no channel.
    assert!(s.binding.native_id.is_some());
    assert!(!stderr.contains("development channel"), "{stderr}");
    let end = run::events(&store, &s.horse)
        .unwrap()
        .into_iter()
        .find(|e| e["type"] == "run.end")
        .expect("a run.end event");
    assert_eq!(end["custody"], "pty", "{end}");
    assert_eq!(end["landed"], 2, "{end}");
    assert_eq!(end["not_landed"], serde_json::json!([]), "{end}");
    assert_eq!(end["relay_lost"], false, "{end}");
    assert!(store.inbox(&sid).unwrap().is_empty());
    // The hooks went on the command line: the host settings have none.
    let cfg = std::fs::read_to_string(root.join("home/.claude/settings.json")).unwrap_or_default();
    assert!(!cfg.contains("safe-point"), "the host copy changed: {cfg}");
    drop(master);
}
