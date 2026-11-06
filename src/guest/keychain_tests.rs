use super::*;

/// The login keychain in the home on the data disk.
const MOVED: &str = concat!(
    "/Volumes/paddock/home",
    "/Library/Keychains/login.keychain-db"
);

#[test]
fn the_login_keychain_is_under_the_home() {
    assert_eq!(
        login_keychain(Path::new("/Volumes/paddock/home")),
        PathBuf::from(MOVED)
    );
}

#[test]
fn the_moved_home_wins_over_the_environment() {
    let moved = Path::new("/Volumes/paddock/home");
    assert_eq!(account_home(Some(moved), Some("/Users/paddock")), moved);
    assert_eq!(
        account_home(None, Some("/Users/other")),
        PathBuf::from("/Users/other")
    );
    assert_eq!(
        account_home(None, Some("")),
        PathBuf::from("/Users/paddock")
    );
    assert_eq!(account_home(None, None), PathBuf::from("/Users/paddock"));
}

#[test]
fn the_list_output_gives_the_quoted_paths() {
    let out = "    \"/Users/paddock/Library/Keychains/login.keychain-db\"\n    \"/Library/Keychains/System.keychain\"\n\n";
    assert_eq!(
        parse_list(out),
        vec![
            "/Users/paddock/Library/Keychains/login.keychain-db".to_string(),
            "/Library/Keychains/System.keychain".to_string()
        ]
    );
    assert!(parse_list("security: no default keychain\n").is_empty());
}

#[test]
fn only_login_keychains_are_login_keychains() {
    assert!(is_login_keychain(MOVED));
    assert!(is_login_keychain(
        "/Users/paddock/Library/Keychains/login.keychain"
    ));
    assert!(!is_login_keychain(
        "/Users/paddock/Library/Keychains/metadata.keychain-db"
    ));
    assert!(!is_login_keychain("/Library/Keychains/System.keychain"));
}

#[test]
fn the_search_list_gets_the_moved_keychain_first_and_keeps_the_others() {
    let have = vec![
        "/Users/paddock/Library/Keychains/login.keychain-db".to_string(),
        "/Users/paddock/Library/Keychains/work.keychain-db".to_string(),
    ];
    assert_eq!(
        search_list(&have, MOVED),
        Some(vec![
            MOVED.to_string(),
            "/Users/paddock/Library/Keychains/work.keychain-db".to_string()
        ])
    );
    // An empty list gets the keychain.
    assert_eq!(search_list(&[], MOVED), Some(vec![MOVED.to_string()]));
    // A list that is right already is left alone.
    assert_eq!(search_list(&[MOVED.to_string()], MOVED), None);
    // The keychain in a later place moves to the front.
    let later = vec!["/x/a.keychain-db".to_string(), MOVED.to_string()];
    assert_eq!(
        search_list(&later, MOVED),
        Some(vec![MOVED.to_string(), "/x/a.keychain-db".to_string()])
    );
}

#[test]
fn the_default_is_set_only_when_it_is_another_keychain() {
    assert!(default_needs_set(&[], MOVED));
    assert!(default_needs_set(
        &["/Users/paddock/Library/Keychains/login.keychain-db".to_string()],
        MOVED
    ));
    assert!(!default_needs_set(&[MOVED.to_string()], MOVED));
}

#[test]
fn no_argv_holds_a_password() {
    // The calls that go to a command line take only paths and words.
    let argv = security_argv(&["default-keychain", "-d", "user", "-s", MOVED]);
    assert_eq!(argv[0], "/usr/bin/security");
    assert_eq!(argv.len(), 6);
    assert!(!argv.iter().any(|a| a == "-p" || a.starts_with("unlock")));
}
