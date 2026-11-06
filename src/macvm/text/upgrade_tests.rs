//! One test per function of text/upgrade.rs, named as the function.

use super::super::tests::has;
use super::super::upgrade as t;
use std::path::Path;

#[test]
fn update_needs_golden() {
    has(
        &t::update_needs_golden(),
        &["no sealed golden", "paddock macvm setup"],
    );
}

#[test]
fn update_plan() {
    has(
        &t::update_plan("15.2", 2),
        &[
            "macOS 15.2",
            "golden.prev",
            "paddock macvm rollback",
            "2 profile(s)",
            "keep their own system disk",
            "paddock app reset NAME --data",
        ],
    );
}

#[test]
fn update_removes_stale_new() {
    has(
        &t::update_removes_stale_new(Path::new("/g.new")),
        &["/g.new", "rollback set aside", "builds anew"],
    );
}

#[test]
fn update_same_version() {
    has(
        &t::update_same_version("15.2"),
        &["already runs macOS 15.2", "paddock macvm update --force"],
    );
}

#[test]
fn new_golden_unsealed() {
    has(
        &t::new_golden_unsealed(Path::new("/g.new")),
        &["/g.new", "not sealed", "paddock macvm update"],
    );
}

#[test]
fn update_done() {
    has(
        &t::update_done("15.2", "15.3", 1),
        &[
            "macOS 15.3 now",
            "macOS 15.2",
            "golden.prev",
            "1 profile(s)",
            "backup",
        ],
    );
}

#[test]
fn rollback_question() {
    has(
        &t::rollback_question("15.3", "15.2"),
        &[
            "replaces the golden (macOS 15.3)",
            "macOS 15.2",
            "No profile changes",
            "own system disk",
        ],
    );
}

#[test]
fn rollback_done() {
    has(
        &t::rollback_done("15.2", Path::new("/g.new")),
        &[
            "macOS 15.2 again",
            "/g.new",
            "paddock macvm update",
            "paddock macvm remove",
        ],
    );
}

#[test]
fn recovered_update() {
    has(
        &t::recovered_update(),
        &[
            "paddock macvm update",
            "two folder renames",
            "new golden is in place",
        ],
    );
}

#[test]
fn recovered_rollback() {
    has(
        &t::recovered_rollback(),
        &[
            "paddock macvm rollback",
            "two folder renames",
            "previous golden is in place",
        ],
    );
}

#[test]
fn status_prev() {
    let kept = t::status_prev("15.2", false);
    has(
        &kept,
        &[
            "  prev     macOS 15.2",
            "paddock macvm rollback",
            "until a profile",
        ],
    );
    assert!(!kept.contains("paddock gc"));
    has(
        &t::status_prev("15.2", true),
        &[
            "  prev     macOS 15.2",
            "paddock macvm rollback",
            "paddock gc",
        ],
    );
}

#[test]
fn gc_prev_what() {
    has(
        &t::gc_prev_what("15.2"),
        &["previous", "golden", "macOS 15.2"],
    );
}

#[test]
fn gc_prev_reason() {
    has(&t::gc_prev_reason(), &["completed a session", "rollback"]);
}

#[test]
fn gc_agent_what() {
    has(&t::gc_agent_what("0.8.0"), &["guest agent share", "0.8.0"]);
}

#[test]
fn gc_not_macvm() {
    has(
        &t::gc_not_macvm(Path::new("/x")),
        &[
            "/x is not the previous golden",
            "paddock gc does not remove it",
        ],
    );
}

#[test]
fn gc_agent_reason() {
    has(
        &t::gc_agent_reason("0.9.0"),
        &["this paddock is 0.9.0", "own version"],
    );
}

#[test]
fn agent_hash_mismatch() {
    has(
        &t::agent_hash_mismatch(Path::new("/b/paddock-guest"), "aa", "bb"),
        &[
            "/b/paddock-guest",
            "file says aa",
            "binary is bb",
            "build-guest.sh",
        ],
    );
}

#[test]
fn agent_share_failed() {
    has(
        &t::agent_share_failed("read-only"),
        &[
            "not made: read-only",
            "agent of the golden",
            "cannot update",
        ],
    );
}

#[test]
fn agent_rolled_back() {
    has(
        &t::agent_rolled_back("0.9.1", "0.9.0"),
        &[
            "did not update to 0.9.1",
            "runs 0.9.0 again",
            "paddock macvm update",
        ],
    );
}

#[test]
fn event_agent_update() {
    assert_eq!(
        t::event_agent_update("0.9.0", "0.9.1", false),
        "agent update from 0.9.0 to 0.9.1"
    );
    has(
        &t::event_agent_update("0.9.1", "0.9.0", true),
        &["agent update from 0.9.1 to 0.9.0", "downgrade"],
    );
}

#[test]
fn event_agent_updated() {
    assert_eq!(
        t::event_agent_updated("0.9.1", 3.17),
        "agent update done: 0.9.1 dialed in 3.2 s after the request"
    );
}

#[test]
fn event_agent_rollback() {
    assert_eq!(
        t::event_agent_rollback("0.9.0"),
        "agent rollback: the guest runs 0.9.0 again"
    );
}
