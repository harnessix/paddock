//! An interactive Codex run in horse mode, with the fake TUI of
//! `tests/fixtures/horse/codex-tui` on a terminal that the test owns: an
//! entry queued while the TUI "thinks" is typed into it only after
//! Codex's `notify` program ran and the input row came back, and it lands.

use super::*;

/// A pty pair: the master for the test, the slave for paddock.
pub(crate) fn pty() -> (std::fs::File, std::fs::File) {
    use std::os::fd::FromRawFd;
    let (mut m, mut s) = (0, 0);
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two new fds into the locals.
    let rc = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: both fds are new and owned here.
    unsafe { (std::fs::File::from_raw_fd(m), std::fs::File::from_raw_fd(s)) }
}

#[test]
fn a_codex_tui_in_horse_mode_gets_entries_between_turns_through_the_relay() {
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
        "codex",
        "codex-tui",
        "[launch]\nargs = []\n[horse.pty]\nversion = \"0.166.0\"\nprompt = \"\u{203a}\"\nempty_input = true\nbusy = [\"esc to interrupt\"]\nquiet_ms = 500\ninterrupt_key = \"escape\"\nbracketed_paste = true\n",
    );
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[horse]\nenabled = true\n",
    )
    .unwrap();
    let (master, slave) = pty();
    let mut tui = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["run", "codex", "--backend", "bwrap", "--network", "none"])
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
            .find(|s| s.binding.harness == "codex")
    };
    wait("no codex session", &mut tui, &mut || session().is_some());
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
    // The entry came only after the notify of turn 1. The DNA entry of
    // the run goes first, so the test entry is the third turn.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let at = |l: &str| lines.iter().position(|x| *x == l);
    let notify1 = at("notify 1").unwrap_or_else(|| panic!("no notify 1:\n{text}"));
    let got = lines
        .iter()
        .position(|l| l.ends_with(" from the test"))
        .unwrap_or_else(|| panic!("the entry did not come:\n{text}"));
    assert!(notify1 < got, "{text}");
    assert!(
        lines[..got]
            .iter()
            .filter(|l| l.starts_with("notify "))
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
        stderr.contains("paddock types them into the TUI"),
        "{stderr}"
    );
    let store = store_helpers::open(&store_path).unwrap();
    let s = store.session(&sid).unwrap();
    assert_eq!(
        s.binding.native_id.as_deref(),
        Some("019a5f00-0000-7000-8000-00000000c0de")
    );
    let end = run::events(&store, &s.horse)
        .unwrap()
        .into_iter()
        .find(|e| e["type"] == "run.end")
        .expect("a run.end event");
    assert_eq!(end["custody"], "pty", "{end}");
    assert_eq!(end["landed"], 2, "{end}");
    assert_eq!(end["not_landed"], serde_json::json!([]), "{end}");
    assert_eq!(end["other_stops"], 0, "{end}");
    assert!(store.inbox(&sid).unwrap().is_empty());
    // The notify of the sandbox copy is paddock's; the host has none.
    let cfg = std::fs::read_to_string(root.join("home/.codex/config.toml")).unwrap_or_default();
    assert!(!cfg.contains("safe-point"), "the host copy changed: {cfg}");
    drop(master);
}
