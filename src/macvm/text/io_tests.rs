//! One test per function of text/io.rs.

use super::super::io as t;
use std::path::Path;

fn p() -> &'static Path {
    Path::new("/Users/me/Library/Application Support/paddock/macvm/golden.noindex/disk.img")
}

#[test]
fn cannot_read() {
    assert_eq!(
        t::cannot_read(p()),
        format!("cannot read {}", p().display())
    );
}

#[test]
fn cannot_write() {
    assert!(t::cannot_write(p()).starts_with("cannot write /Users/me"));
}

#[test]
fn cannot_make() {
    assert!(t::cannot_make(p()).starts_with("cannot make /Users/me"));
}

#[test]
fn cannot_open() {
    assert!(t::cannot_open(p()).starts_with("cannot open /Users/me"));
}

#[test]
fn cannot_remove() {
    assert!(t::cannot_remove(p()).starts_with("cannot remove /Users/me"));
}

#[test]
fn cannot_copy() {
    let s = t::cannot_copy(Path::new("/a/data.img"), Path::new("/b/data.img.partial"));
    assert_eq!(s, "cannot copy /a/data.img to /b/data.img.partial");
}

#[test]
fn cannot_move() {
    let s = t::cannot_move(Path::new("/a/golden"), Path::new("/a/golden.prev"));
    assert_eq!(s, "cannot move /a/golden to /a/golden.prev");
}

#[test]
fn cannot_commit() {
    assert!(t::cannot_commit(p()).starts_with("cannot commit /Users/me"));
}

#[test]
fn cannot_size() {
    assert!(t::cannot_size(p()).starts_with("cannot size /Users/me"));
}

#[test]
fn cannot_unseal() {
    assert!(t::cannot_unseal(p()).starts_with("cannot unseal /Users/me"));
}

#[test]
fn damaged() {
    assert!(t::damaged(p()).ends_with("disk.img is damaged"));
}

#[test]
fn cannot_run() {
    assert_eq!(
        t::cannot_run("/usr/bin/hdiutil"),
        "cannot run /usr/bin/hdiutil"
    );
}

#[test]
fn free_space_unreadable() {
    assert!(t::free_space_unreadable(Path::new("/x")).contains("free space of /x"));
}

#[test]
fn no_folder_exists() {
    assert_eq!(
        t::no_folder_exists(Path::new("/x/y")),
        "no folder of /x/y exists"
    );
}

#[test]
fn not_utf8() {
    assert!(t::not_utf8(Path::new("/x")).contains("/x is not UTF-8"));
}

#[test]
fn not_partial_name() {
    assert_eq!(
        t::not_partial_name(Path::new("/x/disk.img")),
        "/x/disk.img is not a .partial name"
    );
}

#[test]
fn urandom_failed() {
    assert_eq!(t::urandom_failed("open"), "cannot open /dev/urandom");
    assert_eq!(t::urandom_failed("read"), "cannot read /dev/urandom");
}
