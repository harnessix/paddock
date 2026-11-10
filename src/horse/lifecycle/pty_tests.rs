//! An interactive Codex run in horse mode: the pty custody when the
//! profile has a `[horse.pty]` table for the pinned version, the command
//! sets no `notify` and the caller has a terminal, the fallback line in
//! every other case. `notify` goes into the sandbox configuration only in
//! horse mode.
use super::*;
use crate::horse::live::tui_state::{self, SafePoint};

fn tui(t: &std::path::Path) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "codex", codex::VERSION);
    b.horse = true;
    b.profile = crate::profile::builtin("codex").unwrap();
    b.spec.command = ["codex", "--dangerously-bypass-approvals-and-sandbox"]
        .map(String::from)
        .to_vec();
    (env, b)
}

fn line(why: &str) -> Option<String> {
    Some(format!(
        "horse: no safe-point delivery for codex {} in the TUI ({why}); entries stay queued",
        codex::VERSION
    ))
}

fn notify_settings(b: &Built) -> usize {
    b.spec
        .home_settings
        .iter()
        .filter(|s| s.path == ["notify"] && s.chain)
        .count()
}

#[test]
fn an_interactive_codex_run_gets_the_pty_custody_or_one_line() {
    let t = tempfile::tempdir().unwrap();
    let v = codex::VERSION;
    let (_env, b) = tui(t.path());
    let choose = run::setup::choose_with;
    assert_eq!(choose(&b, v, true), (Custody::Pty, None));
    assert_eq!(
        choose(&b, v, false),
        (
            Custody::FinalOnly,
            line("the run has no terminal for paddock's relay")
        )
    );
    // The command sets notify itself.
    let (_env, mut c) = tui(t.path());
    c.spec
        .command
        .extend(["-c".into(), "notify=[\"x\"]".into()]);
    let (k, l) = choose(&c, v, true);
    assert_eq!(k, Custody::FinalOnly);
    assert!(l.unwrap().contains("sets notify"));
    // A profile without the table for this version.
    let (_env, d) = super::tests::setup(t.path(), "codex", v);
    assert_eq!(
        choose(&d, v, true),
        (
            Custody::FinalOnly,
            line(&format!("horse: no pty delivery pattern for codex {v}"))
        )
    );
}

#[test]
fn notify_goes_into_the_configuration_only_for_the_pty_custody() {
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = tui(t.path());
    let before = b.spec.command.clone();
    let p = run::setup::prepare(&mut b, Custody::Pty, codex::VERSION).unwrap();
    assert!(p.is_some());
    assert_eq!(b.spec.command, before, "the command line stays");
    assert_eq!(notify_settings(&b), 1);
    // A final-only Codex run gets nothing.
    let (_env, mut f) = tui(t.path());
    assert!(
        run::setup::prepare(&mut f, Custody::FinalOnly, codex::VERSION)
            .unwrap()
            .is_none()
    );
    assert_eq!(notify_settings(&f), 0);
    // Horse mode off: no store, no notify.
    let (_env, mut off) = tui(t.path());
    off.horse = false;
    assert!(start(&env, &mut off).unwrap().is_none());
    assert_eq!(notify_settings(&off), 0);
    assert!(!super::super::root(&env).exists());
}

#[test]
fn the_pty_custody_hears_the_stop_and_ends_with_the_run() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = tui(t.path());
    let p = run::setup::prepare(&mut b, Custody::Pty, codex::VERSION)
        .unwrap()
        .unwrap();
    let store = Store::open_host(&env).unwrap();
    let rh = run::begin(&store, "codex", codex::VERSION, None, None).unwrap();
    assert!(run::setup::arm(&env, &store, &rh, &mut b, p)
        .unwrap()
        .is_none());
    // The relay of the sandbox gets the tap; no stdio hook.
    assert!(live::take_hooks().0.is_none());
    let tap = live::take_pty_tap().expect("the relay tap");
    assert!(tap.screen().is_none(), "no relay yet");
    // A stop of a thread with no rollout binds nothing.
    let key = tui_state::run_key(&rh.horse, &rh.session);
    tui_state::runs().mark_thread(&key, SafePoint::Stop, Some("019a-none"));
    let (r, report) = run::setup::finish();
    r.unwrap();
    assert_eq!(report["stops"], 0, "{report}");
    assert_eq!(report["thread"], serde_json::Value::Null, "{report}");
    assert_eq!(report["deliveries"], 0, "{report}");
    assert!(tui_state::runs().state(&key).busy, "the state was dropped");
    assert!(live::take_pty_tap().is_none());
}

/// An interactive Claude Code run with the built-in profile, which has a
/// `[horse.pty]` table for the codec version.
fn claude_tui(t: &std::path::Path) -> (HostEnv, Built) {
    let (env, mut b) = super::tests::setup(t, "claude", claude::VERSION);
    b.horse = true;
    b.profile = crate::profile::builtin("claude").unwrap();
    b.spec.command = ["claude", "--model", "m"].map(String::from).to_vec();
    (env, b)
}

#[test]
fn a_claude_tui_with_no_channel_path_gets_the_pty_custody_or_one_line() {
    let t = tempfile::tempdir().unwrap();
    let v = claude::VERSION;
    assert!(claude::channel_pin(v).is_none());
    let choose = run::setup::choose_with;
    let (_env, b) = claude_tui(t.path());
    assert_eq!(choose(&b, v, true), (Custody::Pty, None));
    let (k, l) = choose(&b, v, false);
    assert_eq!(k, Custody::FinalOnly);
    assert!(l
        .unwrap()
        .contains("the run has no terminal for paddock's relay"));
    // A command that picks its own session falls back and says why.
    let (_env, mut r) = claude_tui(t.path());
    r.spec.command.push("--resume".into());
    let (k, l) = choose(&r, v, true);
    assert_eq!(k, Custody::FinalOnly);
    assert!(l.unwrap().contains("sets --resume"));
    // The channel flags are the user's own in a run with no channel.
    let (_env, mut m) = claude_tui(t.path());
    m.spec.command.push("--mcp-config=x.json".into());
    assert_eq!(choose(&m, v, true), (Custody::Pty, None));
}

#[test]
fn prepare_gives_a_claude_tui_its_session_and_hooks_and_no_channel() {
    let t = tempfile::tempdir().unwrap();
    let (_env, mut b) = claude_tui(t.path());
    let p = run::setup::prepare(&mut b, Custody::Pty, claude::VERSION).unwrap();
    assert!(p.is_some());
    let c = &b.spec.command;
    assert_eq!(c[0], "claude");
    assert!(c.iter().any(|a| a.starts_with("--session-id=")), "{c:?}");
    assert!(
        !c.iter()
            .any(|a| a.starts_with("--channels") || a.starts_with("--mcp-config")),
        "{c:?}"
    );
    let at = c
        .iter()
        .position(|a| a == "--settings")
        .expect("--settings");
    let settings: serde_json::Value = serde_json::from_str(&c[at + 1]).unwrap();
    for (event, kind) in [("Stop", "stop"), ("UserPromptSubmit", "prompt")] {
        let cmd = &settings["hooks"][event][0]["hooks"][0]["command"];
        assert_eq!(
            cmd.as_str(),
            Some(format!("paddock-host horse safe-point {kind} || true").as_str())
        );
    }
    // A profile without the table refuses before the store.
    let (_env, mut n) = super::tests::setup(t.path(), "claude", claude::VERSION);
    n.horse = true;
    let e = run::setup::prepare(&mut n, Custody::Pty, claude::VERSION)
        .err()
        .unwrap();
    assert_eq!(
        format!("{e:#}"),
        format!(
            "horse: no pty delivery pattern for claude {}",
            claude::VERSION
        )
    );
}

#[test]
fn the_claude_pty_custody_binds_its_session_hears_the_stop_and_ends_with_the_run() {
    let _armed = super::live_tests::ARMS
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let (env, mut b) = claude_tui(t.path());
    let p = run::setup::prepare(&mut b, Custody::Pty, claude::VERSION)
        .unwrap()
        .unwrap();
    let native = b
        .spec
        .command
        .iter()
        .find_map(|a| a.strip_prefix("--session-id="))
        .unwrap()
        .to_string();
    let store = Store::open_host(&env).unwrap();
    let rh = run::begin(&store, "claude", claude::VERSION, None, None).unwrap();
    assert!(run::setup::arm(&env, &store, &rh, &mut b, p)
        .unwrap()
        .is_none());
    assert_eq!(
        store.session(&rh.session).unwrap().binding.native_id,
        Some(native)
    );
    assert!(live::take_hooks().0.is_none());
    let tap = live::take_pty_tap().expect("the relay tap");
    assert!(tap.screen().is_none(), "no relay yet");
    let key = tui_state::run_key(&rh.horse, &rh.session);
    assert!(
        tui_state::runs().state(&key).typed,
        "no channel server gets an entry"
    );
    tui_state::runs().mark(&key, SafePoint::Stop);
    // The custody heard the Stop when it holds the state idle and the
    // live record shows the session idle.
    let idle = || {
        live::records(&store)
            .unwrap_or_default()
            .iter()
            .any(|r| r.sessions.iter().any(|s| s.session == rh.session && s.idle))
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !idle() {
        assert!(
            std::time::Instant::now() < deadline,
            "the custody did not hear the stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (r, report) = run::setup::finish();
    r.unwrap();
    assert_eq!(report["stops"], 1, "{report}");
    assert_eq!(report["deliveries"], 0, "{report}");
    assert_eq!(report["relay_lost"], false, "{report}");
    assert!(tui_state::runs().state(&key).busy, "the state was dropped");
    assert!(live::take_pty_tap().is_none());
}
