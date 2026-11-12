//! An MCP server gets the closure of its command only: the helper programs
//! of a harness (profile/closure.rs) are not for it.

use super::*;
use std::os::unix::fs::PermissionsExt;

fn exe(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "\x7fELF").unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_server_in_a_home_bin_dir_gets_no_helpers() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let bin = home.join(".local/bin");
    exe(&bin.join("srv"));
    exe(&bin.join("srv-helper"));
    // A link out of the bin folder: a harness would mount its target.
    exe(&home.join("dotfiles/srv-x"));
    std::os::unix::fs::symlink(home.join("dotfiles/srv-x"), bin.join("srv-link")).unwrap();
    let e = HostEnv::from_parts(home.clone(), "tester".into(), 1, 1, Default::default());
    let s = Server {
        name: "s".into(),
        source: "test".into(),
        command: Some(bin.join("srv").display().to_string()),
        args: vec![],
        url: None,
        trusted: true,
        definition: serde_json::Value::Null,
    };
    match assess(&e, &s, &Manifest::default(), &project) {
        Status::Ready { mounts, path, .. } => {
            assert_eq!(mounts, vec![bin.join("srv")]);
            assert!(path.is_empty(), "{path:?}");
        }
        other => panic!("{other:?}"),
    }
}
