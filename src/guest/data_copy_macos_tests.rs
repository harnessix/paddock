//! The copy plan against the real `ditto` of macOS, as the user (the
//! guest runs it with `sudo`; the runner here drops that prefix): a file
//! that cannot be read is a counted warning, the copy goes on, and a link
//! stays a link.

use super::*;
use crate::guest::configure::run_plan;
use std::os::unix::fs::PermissionsExt;

/// Run a plan without its `sudo -S -p '' --` prefix.
fn as_user(p: &Plan, limit: Duration) -> Result<Ran, Fail> {
    let plan = Plan {
        argv: p.argv[5..].to_vec(),
        password_on_stdin: false,
    };
    run_plan(&plan, None, limit).map_err(|e| (ErrorCode::Io, e.to_string()))
}

#[test]
fn ditto_on_this_mac_counts_an_unreadable_file_and_keeps_links() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    std::fs::create_dir_all(home.join("Music/a")).unwrap();
    std::fs::write(home.join("Music/a/song"), "la").unwrap();
    std::fs::write(home.join("Music/secret"), "no").unwrap();
    std::fs::set_permissions(
        home.join("Music/secret"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    std::os::unix::fs::symlink("/Volumes/My Shared Files/downloads", home.join("Downloads"))
        .unwrap();
    std::os::unix::fs::symlink("Music/a", home.join("tunes")).unwrap();
    let to = t.path().join("home.partial");
    let entries = read_entries(&home).unwrap();
    let (ops, _) = plan(&home, &to, 0o40750, &entries, None);
    let report = execute(
        &ops,
        &BTreeMap::new(),
        Duration::from_secs(60),
        &Watch::quiet(),
        as_user,
    )
    .unwrap();
    std::fs::set_permissions(
        home.join("Music/secret"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(report.denied, 1, "{report:?}");
    assert_eq!(
        std::fs::read_to_string(to.join("Music/a/song")).unwrap(),
        "la"
    );
    assert!(!to.join("Downloads").exists() && to.join("Downloads").symlink_metadata().is_err());
    assert_eq!(
        std::fs::read_link(to.join("tunes")).unwrap(),
        PathBuf::from("Music/a")
    );
}
