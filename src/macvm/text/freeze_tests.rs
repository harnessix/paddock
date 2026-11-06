//! One test per function of text/freeze.rs, named as the function.

use super::super::freeze as t;
use super::super::BADGE_MAX;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn pause_while_quitting() {
    has(
        &t::pause_while_quitting(),
        &["quitting", "cannot be frozen"],
    );
}

#[test]
fn pause_while_resuming() {
    has(&t::pause_while_resuming(), &["resuming", "freeze it again"]);
}

#[test]
fn resume_while_pausing() {
    has(&t::resume_while_pausing(), &["pausing", "thaw it again"]);
}

#[test]
fn pause_failed_freeze() {
    has(&t::pause_failed_freeze("E1"), &["(E1)", "keeps running"]);
}

#[test]
fn resume_failed_freeze() {
    has(
        &t::resume_failed_freeze("E2"),
        &["(E2)", "paddock thaw", "paddock app stop"],
    );
}

#[test]
fn vm_state_refused() {
    has(
        &t::vm_state_refused(),
        &["cannot", "starting, saving or stopping"],
    );
}

#[test]
fn frozen_badge() {
    let b = t::frozen_badge();
    has(&b, &["frozen", "paddock thaw"]);
    assert!(b.chars().count() <= BADGE_MAX);
}

#[test]
fn frozen_line() {
    has(&t::frozen_line(), &["paused", "paddock freeze"]);
}

#[test]
fn thawed_line() {
    has(&t::thawed_line(42), &["42 s", "paddock thaw"]);
}

#[test]
fn thaw_before_quit() {
    has(
        &t::thaw_before_quit("chatgpt-desktop"),
        &["chatgpt-desktop is frozen", "thaws it first"],
    );
}

#[test]
fn screens_changed() {
    has(&t::screens_changed(1, 2), &["(1 now)", "2 windows moved"]);
}

#[test]
fn mixed_scales() {
    has(
        &t::mixed_scales(),
        &["two scales", "main screen's scale", "soft"],
    );
}
