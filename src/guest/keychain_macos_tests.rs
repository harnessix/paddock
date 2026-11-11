//! The keychain calls against the Security framework of this Mac, on a
//! throwaway keychain in a temporary folder: never the login keychain of
//! the person who runs the tests.

use super::mac::Keychain;
use super::*;
use std::process::Command;

fn security(args: &[&str]) -> bool {
    Command::new("/usr/bin/security")
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn a_locked_keychain_unlocks_with_its_password_and_not_with_another() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("check.keychain-db");
    let p = path.to_str().unwrap();
    // The test keychain's own throwaway password; it is not added to the
    // search list.
    assert!(security(&["create-keychain", "-p", "pw-of-the-check", p]));
    assert!(security(&["lock-keychain", p]));
    let kc = Keychain::open(&path).unwrap();
    assert!(!kc.unlocked().unwrap(), "the keychain is locked");
    let wrong = kc.unlock("not-the-password");
    assert!(
        matches!(&wrong, Err((_, t)) if t.contains("SecKeychainUnlock")),
        "{wrong:?}"
    );
    assert!(!kc.unlocked().unwrap());
    kc.unlock("pw-of-the-check").unwrap();
    assert!(kc.unlocked().unwrap(), "the keychain is unlocked");
    drop(kc);
    let _ = security(&["delete-keychain", p]);
}

#[test]
fn the_search_list_of_this_mac_parses() {
    let out = Command::new("/usr/bin/security")
        .args(["list-keychains", "-d", "user"])
        .output()
        .unwrap();
    let list = parse_list(&String::from_utf8_lossy(&out.stdout));
    assert!(list.iter().all(|p| p.contains("/Keychains/")), "{list:?}");
}
