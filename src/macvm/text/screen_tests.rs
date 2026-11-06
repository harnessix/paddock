//! One test per function of text/screen.rs, named as the function.

use super::super::screen as t;
use std::path::Path;

#[test]
fn screen_lock_unsupported() {
    assert!(t::screen_lock_unsupported().contains("cannot lock"));
}

#[test]
fn screen_lock_missing() {
    assert!(t::screen_lock_missing("SACLockScreenImmediate").contains("SACLockScreenImmediate"));
}

#[test]
fn screen_locked() {
    let s = t::screen_locked("2026-11-06T10:00:00Z", Some(42), 3600);
    assert!(s.contains("2026-11-06T10:00:00Z"), "{s}");
    assert!(s.contains("42 s before"), "{s}");
    assert!(s.contains("up 3600 s"), "{s}");
    assert!(t::screen_locked("x", None, 1).contains("not known"));
}

#[test]
fn screen_typing() {
    assert_eq!(
        t::screen_typing(2, 3),
        "typing the guest password into the locked guest screen, try 2 of 3"
    );
}

#[test]
fn screen_unlocked() {
    assert!(t::screen_unlocked(1).contains("after 1 tries"));
    assert_eq!(t::screen_unlocked(0), "the guest screen is unlocked");
}

#[test]
fn screen_stays_locked() {
    let s = t::screen_stays_locked(3);
    assert!(s.contains("3 tries"), "{s}");
    assert!(s.contains("Click into the app window"), "{s}");
}

#[test]
fn screen_no_password() {
    assert!(t::screen_no_password().contains("no guest password"));
}

#[test]
fn screen_cannot_type() {
    assert!(t::screen_cannot_type().contains("US layout"));
}

#[test]
fn screen_no_keyboard() {
    assert!(t::screen_no_keyboard().contains("private keyboard"));
}

#[test]
fn screen_typing_stopped() {
    assert!(t::screen_typing_stopped().contains("no longer reported locked"));
}

#[test]
fn keychain_unsupported() {
    assert!(t::keychain_unsupported().contains("login keychain"));
}

#[test]
fn keychain_missing() {
    assert!(t::keychain_missing(Path::new("/h/k")).contains("/h/k"));
}

#[test]
fn keychain_call_failed() {
    let s = t::keychain_call_failed("SecKeychainUnlock", -25293);
    assert!(
        s.contains("SecKeychainUnlock") && s.contains("-25293"),
        "{s}"
    );
}

#[test]
fn keychain_still_locked() {
    assert!(t::keychain_still_locked(Path::new("/h/k")).contains("still locked"));
}

#[test]
fn keychain_done() {
    let s = t::keychain_done(Path::new("/h/k"), true, true, false);
    assert_eq!(
        s,
        "the login keychain /h/k is unlocked now, the search list is set to it"
    );
    let s = t::keychain_done(Path::new("/h/k"), false, false, true);
    assert_eq!(
        s,
        "the login keychain /h/k was unlocked, the default keychain is set to it"
    );
}

#[test]
fn keychain_failed() {
    let s = t::keychain_failed("x");
    assert!(s.contains("did not unlock: x"), "{s}");
    assert!(s.contains("\"login\" keychain"), "{s}");
}

#[test]
fn keychain_no_password() {
    assert!(t::keychain_no_password().contains("no guest password"));
}
