//! An interactive Claude Code run in horse mode: the channel custody on a
//! version with a checked channel path. Any other version goes to the pty
//! custody (pty_tests.rs), and falls back with one line when the profile
//! has no `[horse.pty]` table for it.
use super::*;
use crate::horse::live::tui_state::{self, SafePoint};

fn tui(t: &std::path::Path) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "claude", claude::VERSION);
    b.horse = true;
    b.spec.command = ["claude", "--model", "m"].map(String::from).to_vec();
    (env, b)
}

fn value<'a>(command: &'a [String], flag: &str) -> &'a str {
    let eq = format!("{flag}=");
    command
        .iter()
        .find_map(|a| a.strip_prefix(&eq))
        .unwrap_or_else(|| panic!("no {flag} in {command:?}"))
}

#[test]
fn an_interactive_claude_run_on_a_channel_version_gets_the_channel() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = tui(t.path());
    claude::test_channel_pin(claude::VERSION, &claude::CHANNEL_VERSIONS[0]);
    assert_eq!(
        run::setup::choose(&b, claude::VERSION),
        (Custody::Channel, None)
    );
    let l = start(&env, &mut b).unwrap().unwrap();
    assert_eq!(l.custody, Custody::Channel);
    // The TUI keeps its terminal: no stdio hook.
    assert!(live::take_hooks().0.is_none());
    let c = &b.spec.command;
    assert_eq!(c[c.len() - 2..], ["--model".to_string(), "m".to_string()]);
    assert_eq!(value(c, "--channels"), "server:paddock");
    assert!(value(c, "--mcp-config").contains("paddock-host"));
    let at = c.iter().position(|a| a == "--settings").unwrap();
    let settings: serde_json::Value = serde_json::from_str(&c[at + 1]).unwrap();
    let stop = settings["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(
        stop.last().unwrap()["hooks"][0]["command"],
        live::channel::STOP_HOOK
    );
    let store = Store::open_host(&env).unwrap();
    let native = store.session(&l.run.session).unwrap().binding.native_id;
    assert_eq!(native.as_deref(), Some(value(c, "--session-id")));
    // A hook through the broker reaches the custody of this run.
    let key = tui_state::run_key(&l.run.horse, &l.run.session);
    tui_state::runs().mark(&key, SafePoint::Stop);
    // The custody heard it: the live record shows the session idle.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !live::records(&store).unwrap().iter().any(|r| {
        r.sessions
            .iter()
            .any(|s| s.session == l.run.session && s.idle)
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "the custody did not hear the stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let horse = l.run.horse.clone();
    assert_eq!(end(Some(l), 0, None), None);
    let events = super::tests::run_events(&store, &horse).unwrap();
    let e = events.last().unwrap();
    assert_eq!(e["type"], "run.end");
    assert_eq!(e["custody"], "channel", "{e}");
    assert_eq!(e["stops"], 1, "{e}");
    assert_eq!(e["deliveries"], 0, "{e}");
    assert!(e.get("error").is_none(), "{e}");
    assert!(live::records(&store).unwrap().is_empty());
}

#[test]
fn a_claude_tui_without_a_channel_or_a_pty_table_falls_back_with_one_line() {
    let t = tempfile::tempdir().unwrap();
    let (_env, b) = tui(t.path());
    // The codec version has no checked channel path, and the profile of
    // the test has no [horse.pty] table.
    assert!(claude::channel_pin(claude::VERSION).is_none());
    assert_eq!(
        run::setup::choose_with(&b, claude::VERSION, true),
        (
            Custody::FinalOnly,
            Some(format!(
                "horse: no safe-point delivery for claude {v} in the TUI (horse: no pty delivery pattern for claude {v}); entries stay queued",
                v = claude::VERSION
            ))
        )
    );
    // A command that picks its own session falls back too, and says why.
    let (_env, mut b) = tui(t.path());
    b.spec.command.push("--resume".into());
    claude::test_channel_pin(claude::VERSION, &claude::CHANNEL_VERSIONS[0]);
    let (c, line) = run::setup::choose(&b, claude::VERSION);
    assert_eq!(c, Custody::FinalOnly);
    let line = line.unwrap();
    assert!(
        line.contains("sets --resume") && line.ends_with("entries stay queued"),
        "{line}"
    );
    // A run that is not a TUI says nothing.
    let (_env, b) = super::tests::setup(t.path(), "dsh", crate::horse::dsh::VERSION);
    let v = crate::horse::dsh::VERSION;
    assert_eq!(run::setup::choose(&b, v), (Custody::Live, None));
}
