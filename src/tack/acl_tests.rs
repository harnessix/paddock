use super::*;
#[cfg(target_os = "linux")]
use crate::tack::protect::Grant;
#[cfg(target_os = "linux")]
use std::path::Path;

fn e(tag: u16, id: u32, perm: u16) -> Entry {
    Entry { tag, id, perm }
}

#[test]
fn an_acl_round_trips_in_the_kernel_format() {
    let mut a = Acl::from_mode(0o750);
    assert_eq!(
        a.0,
        [
            e(USER_OBJ, NO_ID, 7),
            e(GROUP_OBJ, NO_ID, 5),
            e(OTHER, NO_ID, 0)
        ]
    );
    assert!(a.add_user(981, R | X));
    let b = a.encode();
    assert_eq!(b.len(), 4 + 5 * 8);
    assert_eq!(&b[..4], &[2, 0, 0, 0], "version 2, little-endian");
    // user_obj, user 981, group_obj, mask, other: the kernel's order.
    let tags: Vec<u16> = a.0.iter().map(|x| x.tag).collect();
    assert_eq!(tags, [USER_OBJ, USER, GROUP_OBJ, MASK, OTHER]);
    assert_eq!(&b[12..20], &[2, 0, 5, 0, 0xd5, 0x03, 0, 0], "USER r-x 981");
    assert_eq!(Acl::decode(&b).unwrap(), a);
    assert!(Acl::decode(&b[..7]).is_err());
    let mut v3 = b.clone();
    v3[0] = 3;
    assert!(Acl::decode(&v3).unwrap_err().contains("version 3"));
}

#[test]
fn adding_keeps_old_permissions_and_recomputes_the_mask() {
    let mut a = Acl::from_mode(0o640);
    a.add_user(981, R);
    assert_eq!(a.get(MASK, NO_ID), Some(R), "group r-- and user r--");
    assert!(!a.add_user(981, R), "no change the second time");
    a.add_user(981, W);
    assert_eq!(a.user(981), Some(R | W));
    assert_eq!(a.get(MASK, NO_ID), Some(R | W));
    assert!(a.grants(981, R | W) && !a.grants(981, X));
    a.add_user(1000, R | W | X);
    let users: Vec<u32> = a.0.iter().filter(|x| x.tag == USER).map(|x| x.id).collect();
    assert_eq!(users, [981, 1000], "named users in uid order");
}

#[test]
fn the_mask_limits_what_an_entry_grants() {
    let mut a = Acl::from_mode(0o700);
    a.0.push(e(USER, 981, 7));
    a.0.push(e(MASK, NO_ID, R));
    a.sort();
    assert!(a.grants(981, R) && !a.grants(981, W));
}

#[test]
fn removing_the_last_named_entry_leaves_the_mode_only() {
    let mut a = Acl::from_mode(0o750);
    a.add_user(981, R | X);
    a.add_user(1000, 7);
    assert!(a.remove_user(981));
    assert!(a.named() && a.get(MASK, NO_ID).is_some());
    assert!(a.remove_user(1000));
    assert!(!a.named());
    assert_eq!(
        a,
        Acl::from_mode(0o750),
        "no mask: the kernel turns it into the mode"
    );
    assert!(!a.remove_user(981));
}

#[test]
fn x_only_for_folders_and_programs() {
    assert_eq!(perm_for(true, 0o700, false), R | X);
    assert_eq!(perm_for(false, 0o644, false), R);
    assert_eq!(perm_for(false, 0o744, true), R | W | X);
    assert_eq!(perm_for(false, 0o600, true), R | W);
}

#[test]
fn a_grant_report_reads_as_one_line() {
    let r = GrantReport {
        changed: 12,
        not_owned: 2,
        errors: vec!["x".into()],
        ..Default::default()
    };
    assert_eq!(
        r.summary(),
        "ACL added to 12 files, 2 files of other users left as they are, 1 errors"
    );
    assert_eq!(
        GrantReport {
            already: true,
            ..Default::default()
        }
        .summary(),
        "ACL of the folder already in place"
    );
}

/// A folder on a file system with ACLs, or None (the test says so). The
/// entries name the test's own uid: in a user namespace (a sandbox) an
/// ACL can only name a uid that the namespace maps.
#[cfg(target_os = "linux")]
fn acl_dir() -> Option<tempfile::TempDir> {
    let t = tempfile::tempdir().unwrap();
    match probe(t.path(), me()) {
        Ok(()) => Some(t),
        Err(e) => {
            eprintln!("SKIP: no POSIX ACLs in {} ({e})", t.path().display());
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn me() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(target_os = "linux")]
#[test]
fn a_grant_covers_the_tree_and_a_revoke_takes_it_back() {
    use std::os::unix::fs::PermissionsExt;
    let Some(t) = acl_dir() else { return };
    let top = t.path().join("proj");
    std::fs::create_dir_all(top.join("src/deep")).unwrap();
    std::fs::write(top.join("src/a.rs"), "a").unwrap();
    std::fs::write(top.join("run.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(top.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", top.join("link")).unwrap();
    // The helper is the test's own uid (see acl_dir): the entries are
    // real, and a revoke gives "the helper's" files back to the same uid.
    let helper = me();
    let g = Grant {
        folder: top.clone(),
        owner: me(),
        helper,
        write: true,
    };
    if me() == 0 {
        eprintln!("SKIP: a grant refuses root, and this test runs as root");
        return;
    }
    let rep = grant(&g, me(), MAX_FILES).unwrap();
    assert!(!rep.already);
    assert_eq!(rep.changed, 5, "{rep:?}"); // proj, src, deep, a.rs, run.sh
    assert_eq!(rep.skipped, 1, "the link: {rep:?}");
    let a = read(&top.join("src/a.rs"), false).unwrap().unwrap();
    assert!(a.grants(helper, R | W) && !a.grants(helper, X));
    assert!(read(&top.join("run.sh"), false)
        .unwrap()
        .unwrap()
        .grants(helper, R | W | X));
    let d = read(&top.join("src"), true).unwrap().unwrap();
    assert!(
        d.grants(helper, R | W | X) && d.user(me()) == Some(7),
        "{d:?}"
    );
    // A new file inherits the entries.
    std::fs::write(top.join("src/new.txt"), "n").unwrap();
    assert!(read(&top.join("src/new.txt"), false)
        .unwrap()
        .unwrap()
        .user(helper)
        .is_some());
    // The link's target is untouched.
    assert!(read(Path::new("/etc/passwd"), false)
        .ok()
        .flatten()
        .is_none_or(|a| a.user(helper).is_none()));
    // The second time: nothing to walk.
    assert!(grant(&g, me(), MAX_FILES).unwrap().already);
    // A read grant on a folder that has write already changes nothing.
    let r = grant(
        &Grant {
            write: false,
            ..g.clone()
        },
        me(),
        MAX_FILES,
    )
    .unwrap();
    assert!(r.already, "{r:?}");
    // Revoke: no entry for the helper anywhere, the mode as before.
    let rv = revoke(&g, me(), MAX_FILES);
    assert!(rv.errors.is_empty(), "{rv:?}");
    for f in ["", "src", "src/deep", "src/a.rs", "run.sh", "src/new.txt"] {
        let p = top.join(f);
        let acc = read(&p, false).unwrap();
        assert!(
            acc.as_ref().is_none_or(|a| a.user(helper).is_none()),
            "{f}: {acc:?}"
        );
        let def = read(&p, true).unwrap();
        assert!(def.is_none(), "{f}: {def:?}");
    }
    assert_eq!(
        std::fs::metadata(top.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_grant_stops_at_its_limit_and_at_files_it_cannot_change() {
    let Some(t) = acl_dir() else { return };
    if me() == 0 {
        eprintln!("SKIP: a grant refuses root, and this test runs as root");
        return;
    }
    let top = t.path().join("big");
    std::fs::create_dir(&top).unwrap();
    for i in 0..10 {
        std::fs::write(top.join(format!("f{i}")), "x").unwrap();
    }
    let g = Grant {
        folder: top.clone(),
        owner: me(),
        helper: me(),
        write: false,
    };
    let r = grant(&g, me(), 4).unwrap();
    assert!(r.limit && r.summary().contains("stopped at"), "{r:?}");
    // Someone else's grant cannot run as this user.
    let other = Grant {
        owner: me() + 1,
        ..g.clone()
    };
    assert!(grant(&other, me(), 10).unwrap_err().contains("runs as uid"));
    assert!(grant(&g, 0, 10).is_err(), "never as root");
}

#[cfg(target_os = "linux")]
#[test]
fn the_ledger_keeps_each_grant_once_and_skips_a_cut_line() {
    let t = tempfile::tempdir().unwrap();
    let l = Ledger::new(t.path());
    assert!(l.all().is_empty());
    let g = Grant {
        folder: "/home/a/p".into(),
        owner: 1000,
        helper: 981,
        write: false,
    };
    l.add(&g).unwrap();
    l.add(&g).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(l.path())
        .unwrap();
    std::io::Write::write_all(&mut f, b"{\"folder\": \"/x").unwrap();
    let g2 = Grant {
        write: true,
        ..g.clone()
    };
    l.add(&g2).unwrap();
    assert_eq!(l.all(), [g.clone(), g2.clone()]);
    l.retain(&|x| x.write).unwrap();
    assert_eq!(l.all(), [g2]);
    l.retain(&|_| false).unwrap();
    assert!(!l.path().exists());
}
