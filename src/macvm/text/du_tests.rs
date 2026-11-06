//! One test per function of text/du.rs, named as the function.

use super::super::du as t;
use std::path::Path;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn du_head() {
    has(
        &t::du_head("3 GiB", Path::new("/m")),
        &["macOS guest", "3 GiB", "/m"],
    );
}

#[test]
fn du_part() {
    has(
        &t::du_part("1 GiB", "golden image"),
        &["1 GiB", "golden image"],
    );
}

#[test]
fn du_days_ago() {
    assert_eq!(t::du_days_ago(0), "today");
    assert_eq!(t::du_days_ago(1), "1 day ago");
    has(&t::du_days_ago(9), &["9 days ago"]);
}

#[test]
fn du_profile() {
    has(
        &t::du_profile("5 GiB", "today", "chatgpt 2", "suspended"),
        &["5 GiB", "profile chatgpt 2", "[suspended]"],
    );
}

#[test]
fn du_profile_parts() {
    has(
        &t::du_profile_parts("1", "2", "3", "4"),
        &["disks 1", "saved state 2", "deleting loses", "login"],
    );
}

#[test]
fn gc_archive_what() {
    has(
        &t::gc_archive_what("x", Path::new("/a.cpio")),
        &["archive", "x", "/a.cpio"],
    );
}

#[test]
fn gc_archive_reason() {
    has(&t::gc_archive_reason(), &["copy"]);
}

#[test]
fn gc_state_what() {
    has(&t::gc_state_what("x 2"), &["saved state", "x 2"]);
}

#[test]
fn gc_state_reason() {
    has(&t::gc_state_reason(91), &["91 days", "cold boot"]);
}

#[test]
fn gc_profile_what() {
    has(&t::gc_profile_what("x"), &["profile x"]);
}

#[test]
fn gc_profile_reason() {
    has(&t::gc_profile_reason(91), &["not used for 91 days"]);
}

#[test]
fn gc_profile_warning() {
    has(&t::gc_profile_warning(), &["login", "will be lost"]);
}

#[test]
fn gc_profile_in_use() {
    has(&t::gc_profile_in_use("x"), &["x", "skipped"]);
}

#[test]
fn gc_auto_removed() {
    has(&t::gc_auto_removed("x", "1 GiB"), &["removed x", "1 GiB"]);
}

#[test]
fn gc_auto_failed() {
    has(
        &t::gc_auto_failed("x", "busy"),
        &["cannot remove x", "busy"],
    );
}

#[test]
fn low_space_cleanup() {
    has(&t::low_space_cleanup("3 GiB"), &["3 GiB free", "trash"]);
}
