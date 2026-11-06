//! One test per function of text/files.rs, named as the function.

use super::super::files as t;
use super::super::BADGE_MAX;
use super::{ShareWhy, TransferWhy};
use std::path::Path;
use std::time::Duration;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

fn badge(text: &str) {
    assert!(
        text.chars().count() <= BADGE_MAX,
        "badge {text:?} is longer than {BADGE_MAX}"
    );
    assert!(!text.contains('\n'), "{text:?}");
}

#[test]
fn share_reason() {
    let all = [
        (ShareWhy::Root, "whole disk"),
        (ShareWhy::Home, "home folder"),
        (ShareWhy::ContainsHome, "contains your home"),
        (ShareWhy::AllHomes, "every user"),
        (ShareWhy::Volumes, "every disk"),
        (ShareWhy::SystemFiles, "system folder"),
        (ShareWhy::SystemApps, "system and its apps"),
        (ShareWhy::Protected, "keys, logins"),
        (ShareWhy::Relative, "absolute path"),
    ];
    for (why, word) in all {
        has(&t::share_reason(why), &[word]);
    }
    // The reason reads as the second half of share_refused.
    let line = super::super::share_refused(
        "vscode",
        Path::new("/Users/me"),
        &t::share_reason(ShareWhy::Home),
    );
    has(
        &line,
        &[
            "/Users/me",
            "it is your home folder",
            "paddock app run vscode",
        ],
    );
}

#[test]
fn env_file_visible() {
    has(
        &t::env_file_visible("vscode", Path::new("/Users/me/src/web/.env")),
        &["unprotected_workspaces", "vscode", "/Users/me/src/web/.env"],
    );
}

#[test]
fn downloads_folder() {
    has(
        &t::downloads_folder("chatgpt", Path::new("/p/downloads")),
        &["chatgpt", "/p/downloads", "lands there"],
    );
}

#[test]
fn drop_too_large() {
    let s = t::drop_too_large(3 << 30);
    badge(&s);
    has(&s, &["3 GiB", "max 2 GiB"]);
}

#[test]
fn drop_failed() {
    assert_eq!(t::drop_failed(), "drop failed");
}

#[test]
fn dropped_as_open() {
    assert_eq!(t::dropped_as_open(), "dropped as open");
}

#[test]
fn drop_staged() {
    has(
        &t::drop_staged(2, Path::new("/p/drop/1700000000-1")),
        &["2 item(s)", "/p/drop/1700000000-1"],
    );
}

#[test]
fn image_too_large() {
    let s = t::image_too_large();
    badge(&s);
    assert_eq!(s, "image too large for the clipboard (max 16 MiB)");
}

#[test]
fn clipboard_too_large() {
    let s = t::clipboard_too_large(20 << 20);
    badge(&s);
    has(&s, &["20 MiB", "max 16 MiB"]);
    badge(&t::clipboard_too_large(u64::MAX));
}

#[test]
fn transfer_refused() {
    let cases = [
        (
            TransferWhy::TooLarge { size: 17 << 20 },
            "17 MiB is more than the 16 MiB",
        ),
        (TransferWhy::TooMany, "64 transfers in a minute"),
        (
            TransferWhy::Idle(Duration::from_secs(10)),
            "no byte for 10 s",
        ),
        (TransferWhy::BadToken, "wrong token"),
        (TransferWhy::BadName, "not a plain file name"),
    ];
    for (why, word) in cases {
        let s = t::transfer_refused("clip-7.json", why);
        has(&s, &["guest-files", "clip-7.json", word]);
    }
}
