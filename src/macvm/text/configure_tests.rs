//! One test per function of text/configure.rs, named as the function.

use super::super::configure as t;
use crate::macvm::proto::StepResult;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn configure_step_line() {
    let mut r = StepResult {
        name: "no-sleep".into(),
        ok: true,
        code: Some(0),
        ms: 1250,
        ..Default::default()
    };
    has(
        &t::configure_step_line(&r, true, "0"),
        &["no-sleep: ok", "exit 0", "1.2 s"],
    );
    r.ok = false;
    r.code = Some(1);
    r.output = "line one\n\nline two\n".into();
    has(
        &t::configure_step_line(&r, false, "0"),
        &["no-sleep: FAILED", "exit 1", "line one / line two"],
    );
    r.timed_out = true;
    r.code = None;
    has(
        &t::configure_step_line(&r, false, "0"),
        &["timed out after 90 s", "killed"],
    );
    let r = StepResult {
        name: "dock-hidden".into(),
        ok: true,
        code: Some(0),
        verify: "0".into(),
        ..Default::default()
    };
    has(
        &t::configure_step_line(&r, false, "1"),
        &["FAILED", "the check printed \"0\" without \"1\""],
    );
    let long = StepResult {
        name: "x".into(),
        output: "y".repeat(5000),
        ..Default::default()
    };
    assert!(t::configure_step_line(&long, false, "").len() < 400);
    has(
        &t::configure_step_line(&long, false, ""),
        &["did not run", "..."],
    );
}

#[test]
fn configure_step_missing() {
    has(
        &t::configure_step_missing("proxy-http"),
        &["proxy-http", "FAILED", "no result"],
    );
}

#[test]
fn agent_step_not_run() {
    has(
        &t::agent_step_not_run("black-desktop"),
        &["black-desktop", "did not run"],
    );
}

#[test]
fn agent_step_unknown() {
    has(
        &t::agent_step_unknown(&["wallpaper".into(), "x".into()]),
        &["wallpaper x", "update paddock"],
    );
}

#[test]
fn shutdown_no_password() {
    has(
        &t::shutdown_no_password(),
        &["no guest password", "stops the VM"],
    );
}

#[test]
fn shutdown_failed() {
    has(
        &t::shutdown_failed(Some(1), "Sorry, try again.\n"),
        &["exit 1", "Sorry, try again.;", "stops the VM"],
    );
    has(
        &t::shutdown_failed(None, " "),
        &["exit none;", "stops the VM"],
    );
}

#[test]
fn shutdown_timed_out() {
    has(
        &t::shutdown_timed_out(15),
        &["15 s", "killed", "stops the VM"],
    );
}

#[test]
fn configure_quiet_line() {
    assert_eq!(
        t::configure_quiet_line("configure step a: ok", true),
        "configure step a: ok"
    );
    let l = t::configure_quiet_line("configure step a: FAILED", false);
    assert!(l.contains("the golden goes on without it"), "{l}");
    assert!(l.starts_with("configure step a: FAILED"), "{l}");
}

#[test]
fn machine_step_failed() {
    let s = t::machine_step_failed("no-screen-saver");
    assert!(s.contains("no-screen-saver") && s.contains("lock"), "{s}");
}
