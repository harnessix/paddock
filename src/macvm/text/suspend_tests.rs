//! One test per function of text/suspend.rs, named as the function.

use super::super::suspend as t;
use std::time::Duration;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn instance_invalid() {
    has(&t::instance_invalid("x", 99), &["--instance", "99", "'x'"]);
}

#[test]
fn instance_and_new() {
    has(&t::instance_and_new(), &["--instance", "--new"]);
}

#[test]
fn instances_full() {
    has(&t::instances_full(99), &["99", "paddock app reset"]);
}

#[test]
fn busy_dialog() {
    has(&t::busy_dialog(), &["dialog"]);
}

#[test]
fn busy_copy() {
    has(&t::busy_copy(), &["copy"]);
}

#[test]
fn busy_data_prepare() {
    has(&t::busy_data_prepare(), &["data disk"]);
}

#[test]
fn suspend_while_quitting() {
    has(&t::suspend_while_quitting(), &["quitting", "suspended"]);
}

#[test]
fn suspend_frozen() {
    has(&t::suspend_frozen(), &["frozen", "paddock thaw"]);
}

#[test]
fn suspend_save_off() {
    has(&t::suspend_save_off(), &["save = false", "suspended"]);
}

#[test]
fn suspend_save_failed() {
    has(&t::suspend_save_failed(), &["save", "runs on"]);
}

#[test]
fn suspend_saved() {
    has(&t::suspend_saved(), &["suspended", "slot"]);
}

#[test]
fn suspend_reason_start() {
    has(&t::suspend_reason_start("notes 2"), &["slots", "notes 2"]);
}

#[test]
fn suspended_for() {
    has(
        &t::suspended_for("sample-notes 2", "sample-notes --instance 2", "notes"),
        &[
            "sample-notes 2",
            "notes",
            "paddock app run sample-notes --instance 2",
            "Dock",
        ],
    );
}

#[test]
fn suspend_timeout() {
    has(
        &t::suspend_timeout("sample-notes", "sample-notes", Duration::from_secs(30)),
        &["sample-notes", "30 s", "paddock app status sample-notes"],
    );
}

#[test]
fn suspend_refused_all() {
    let l = t::suspend_refused_all(
        "notes",
        &[("a".into(), "dialog".into()), ("b".into(), "copy".into())],
    );
    has(&l, &["notes", "a: dialog", "b: copy", "Quit one"]);
}

#[test]
fn suspended_line() {
    has(
        &t::suspended_line("sample-notes", "sample-notes"),
        &["suspended", "Dock", "paddock app run sample-notes"],
    );
}

#[test]
fn list_suspended() {
    has(&t::list_suspended(42), &["suspended", "pid 42", "Dock"]);
}

#[test]
fn dock_resume() {
    has(
        &t::dock_resume("Sample Notes"),
        &["Resume Sample Notes", "suspended"],
    );
}

#[test]
fn resume_reason_dock() {
    has(&t::resume_reason_dock(), &["Dock"]);
}

#[test]
fn resume_reason_run() {
    has(&t::resume_reason_run(), &["paddock app run"]);
}

#[test]
fn suspend_event() {
    has(&t::suspend_event("why"), &["suspend", "why"]);
}

#[test]
fn resume_event() {
    has(&t::resume_event("why"), &["resumed", "why"]);
}

#[test]
fn handover_timeout() {
    has(
        &t::handover_timeout("sample-notes", "sample-notes", 7),
        &["pid 7", "paddock app stop sample-notes"],
    );
}

#[test]
fn resume_spawn_failed() {
    has(
        &t::resume_spawn_failed("boom"),
        &["boom", "paddock app run"],
    );
}
