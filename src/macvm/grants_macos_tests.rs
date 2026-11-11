//! The share set of a live grant change, on a Mac: the framework takes a
//! `VZMultipleDirectoryShare` of the workspace names and paddock's own,
//! and refuses a name it cannot mount. The swap on a running VM needs a
//! VM: `packaging/macos/checks/macvm-grants.sh` checks it.

use super::multiple_share;
use crate::vm::helper::ShareArg;

#[test]
fn a_share_set_of_live_names_builds_and_a_bad_name_is_refused() {
    let t = tempfile::tempdir().unwrap();
    let arg = |name: &str, read_only| ShareArg {
        name: name.into(),
        path: t.path().to_path_buf(),
        read_only,
    };
    let set = [arg("ws-1", false), arg("ws-7", true), arg("drop", false)];
    let share = multiple_share(&set).expect("a share set");
    // SAFETY: a getter of the framework object just built.
    let dirs = unsafe { share.directories() };
    assert_eq!(dirs.count(), 3);
    let e = multiple_share(&[arg("a/b", false)])
        .unwrap_err()
        .to_string();
    assert!(e.contains("a/b"), "{e}");
}
