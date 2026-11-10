//! An interactive Claude Code run in horse mode, with the fake TUI of
//! `tests/fixtures/horse/claude-tui`: queued entries go out between its
//! turns through the channel server, and `paddock horse status` shows
//! them landed. A Claude Code version without a checked channel path and
//! without a `[horse.pty]` table in its profile falls back, says so in one
//! line, and keeps the entry queued.

use super::*;

#[test]
fn a_claude_tui_in_horse_mode_gets_entries_between_turns() {
    use paddock::horse::{claude, run};
    use std::process::Stdio;

    if !sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = test_folder();
    let root = tmp.path();
    install_harness(root, "claude", "claude-tui", "[launch]\nargs = []\n");
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[horse]\nenabled = true\n",
    )
    .unwrap();
    let channel = claude::channel_pin(claude::VERSION).is_some();
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let store_path = root.join("data/paddock/horse");
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
                panic!("{what}: {:?}\n{err}", tui.wait());
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
    paddock(root, &["append", &sid, "--text", "from the test"]);
    if channel {
        // The fake's first turn ends with Stop; then the entry goes out,
        // becomes the user row of its second turn, and lands.
        wait("no landed delivery", &mut tui, &mut || {
            let out = cmd(root, &["status", "--json"]).output().unwrap();
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            v.as_array()
                .into_iter()
                .flatten()
                .any(|r| r["session"] == sid.as_str() && r["deliveries"][0]["landed"].is_u64())
        });
        let text = paddock(root, &["status"]);
        assert!(text.contains("landed at row"), "{text}");
    }
    std::fs::write(root.join("project/done"), "").unwrap();
    let out = tui.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let store = store_helpers::open(&store_path).unwrap();
    let s = store.session(&sid).unwrap();
    let end = run::events(&store, &s.horse)
        .unwrap()
        .into_iter()
        .find(|e| e["type"] == "run.end")
        .expect("a run.end event");
    if channel {
        assert_eq!(end["custody"], "channel", "{end}");
        assert_eq!(end["landed"], 1, "{end}");
        assert_eq!(end["not_landed"], serde_json::json!([]), "{end}");
        assert!(store.inbox(&sid).unwrap().is_empty());
    } else {
        // The codec version has no channel path, and the profile of the
        // test has no [horse.pty] table: final-only, one line, and the
        // entry waits in the inbox.
        assert_eq!(end["custody"], "final-only", "{end}");
        assert!(
            stderr.contains(&format!(
                "horse: no safe-point delivery for claude {v} in the TUI (horse: no pty delivery pattern for claude {v}); entries stay queued",
                v = claude::VERSION
            )),
            "{stderr}"
        );
        let inbox = store.inbox(&sid).unwrap();
        assert!(inbox.iter().any(|e| e.text == "from the test"), "{inbox:?}");
    }
}
