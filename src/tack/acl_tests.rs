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

#[cfg(target_os = "linux")]
fn safe_test_fsuid(status: &str) -> Result<u32, String> {
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .ok_or_else(|| format!("missing {name} in thread status"))
    };
    let uids = field("Uid:")?
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("invalid thread uids: {e}"))?;
    if uids.len() != 4 || uids[0] == 0 || uids.iter().any(|uid| *uid != uids[0]) {
        return Err(format!(
            "test needs one non-root real, effective, saved and filesystem uid: {uids:?}"
        ));
    }
    // With no permitted or effective capabilities, the thread cannot regain
    // CAP_FOWNER or use another capability to change an unowned ACL.
    for name in ["CapPrm:", "CapEff:"] {
        let value = field(name)?;
        if value.is_empty() || !value.bytes().all(|c| c == b'0') {
            return Err(format!("test needs no {name} capabilities: {value}"));
        }
    }
    Ok(uids[0])
}

#[cfg(target_os = "linux")]
fn check_external_ancestor_metadata(ancestor: &Path, fsuid: u32) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata =
        std::fs::symlink_metadata(ancestor).map_err(|e| format!("{}: {e}", ancestor.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", ancestor.display()));
    }
    if metadata.uid() == fsuid {
        return Err(format!(
            "{} is owned by the test uid {fsuid}; its ACL could be changed",
            ancestor.display()
        ));
    }
    if metadata.permissions().mode() & 0o001 == 0 {
        return Err(format!(
            "{} is not searchable by others",
            ancestor.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn check_external_ancestor_permissions() {
    // ACL writes depend on the calling thread's filesystem uid and Linux
    // capabilities, not just on getuid() or the directory's mode. Read the
    // current thread: tests may run concurrently in separate threads.
    let status = std::fs::read_to_string("/proc/thread-self/status")
        .expect("cannot check ACL test thread credentials");
    let fsuid = safe_test_fsuid(&status).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fsuid, me(), "thread status disagrees with getuid");
    for ancestor in Path::new("/tmp").ancestors() {
        check_external_ancestor_metadata(ancestor, fsuid).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[cfg(target_os = "linux")]
fn check_external_ancestors() {
    check_external_ancestor_permissions();

    // The permission check prevents changes even if an entry appears after
    // this check. Still require grant and revoke to have no work there.
    for ancestor in Path::new("/tmp").ancestors() {
        for default in [false, true] {
            let a = read(ancestor, default).unwrap();
            assert!(
                a.as_ref().is_none_or(|a| a.user(me()).is_none()),
                "{} has an existing {} ACL entry for the test uid: {a:?}",
                ancestor.display(),
                if default { "default" } else { "access" }
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn the_external_ancestor_guard_rejects_unsafe_credentials() {
    let safe = "Uid:\t1000 1000 1000 1000\nCapPrm:\t00000000\nCapEff:\t00000000\n";
    assert_eq!(safe_test_fsuid(safe).unwrap(), 1000);
    for status in [
        "Uid:\t0 0 0 0\nCapPrm:\t0\nCapEff:\t0\n",
        "Uid:\t1000 0 1000 1000\nCapPrm:\t0\nCapEff:\t0\n",
        "Uid:\t1000 1000 0 1000\nCapPrm:\t0\nCapEff:\t0\n",
        "Uid:\t1000 1000 1000 0\nCapPrm:\t0\nCapEff:\t0\n",
        "Uid:\t1000 1000 1000 1000\nCapPrm:\t8\nCapEff:\t0\n",
        "Uid:\t1000 1000 1000 1000\nCapPrm:\t0\nCapEff:\t8\n",
        "Uid:\t1000 1000 1000 1000\nCapPrm:\t0\n",
        "Uid:\t1000 1000 1000 bad\nCapPrm:\t0\nCapEff:\t0\n",
        "Uid:\t1000 1000 1000 1000\nCapPrm:\t0\nCapEff:\tz\n",
    ] {
        assert!(safe_test_fsuid(status).is_err(), "{status:?}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn the_external_ancestor_guard_rejects_a_test_owned_directory() {
    use std::os::unix::fs::PermissionsExt;

    let t = tempfile::tempdir().unwrap();
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o701)).unwrap();
    let error = check_external_ancestor_metadata(t.path(), me()).unwrap_err();
    assert!(error.contains("owned by the test uid"), "{error}");
    assert!(check_external_ancestor_metadata(t.path(), me().wrapping_add(1)).is_ok());
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let error = check_external_ancestor_metadata(t.path(), me().wrapping_add(1)).unwrap_err();
    assert!(error.contains("not searchable by others"), "{error}");
}

#[cfg(target_os = "linux")]
fn acl_probe_is_unsupported(error: &str) -> bool {
    error.contains("(this file system has no POSIX ACLs:")
}

#[cfg(target_os = "linux")]
#[test]
fn only_an_unsupported_acl_probe_is_skipped() {
    assert!(acl_probe_is_unsupported(
        "Operation not supported (os error 95) (this file system has no POSIX ACLs: use protect = \"refs\")"
    ));
    assert!(!acl_probe_is_unsupported("Permission denied (os error 13)"));
    assert!(!acl_probe_is_unsupported("the entry did not stay"));
}

/// A test-owned folder on a file system with ACLs, or None if unsupported.
/// Use /tmp instead of TMPDIR: grant and revoke walk every ancestor, and an
/// externally supplied TMPDIR may be private or contain real data.
/// The entries name the test's own uid: in a user namespace (a sandbox) an
/// ACL can only name a uid that the namespace maps.
#[cfg(target_os = "linux")]
fn acl_dir() -> Option<tempfile::TempDir> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    // Do not read external ACLs until the test-owned probe has established
    // that this filesystem supports them.
    check_external_ancestor_permissions();
    let t = tempfile::Builder::new()
        .prefix("paddock-acl-")
        .tempdir_in("/tmp")
        .unwrap();
    if let Err(e) = probe(t.path(), me()) {
        // probe appends this explanation only for EOPNOTSUPP. Other errors
        // are unsafe or unexpected, and must not silently skip the tests.
        if acl_probe_is_unsupported(&e) {
            eprintln!("SKIP: no POSIX ACLs in {} ({e})", t.path().display());
            return None;
        }
        panic!("cannot probe POSIX ACLs in {}: {e}", t.path().display());
    }
    check_external_ancestors();

    // An external default ACL may have been inherited by this new folder.
    // Clear it only on the folder the test owns, before making any children.
    let path = CString::new(t.path().as_os_str().as_bytes()).unwrap();
    for attr in [ACCESS, DEFAULT] {
        let name = CString::new(attr).unwrap();
        // SAFETY: both strings are NUL-terminated, and path is test-owned.
        if unsafe { libc::removexattr(path.as_ptr(), name.as_ptr()) } != 0 {
            let e = std::io::Error::last_os_error();
            assert_eq!(e.raw_os_error(), Some(libc::ENODATA), "{attr}: {e}");
        }
    }
    std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o701)).unwrap();
    Some(t)
}

#[cfg(target_os = "linux")]
fn me() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(target_os = "linux")]
#[test]
fn a_grant_covers_the_tree_and_a_revoke_takes_it_back() {
    if me() == 0 {
        eprintln!("SKIP: a grant refuses root, and this test runs as root");
        return;
    }
    let Some(t) = acl_dir() else { return };
    for (name, parent_mode, ancestor_changes) in [("private", 0o700, 1), ("searchable", 0o701, 0)] {
        check_grant_and_revoke(t.path(), name, parent_mode, ancestor_changes);
    }
}

#[cfg(target_os = "linux")]
fn check_grant_and_revoke(root: &Path, name: &str, parent_mode: u32, ancestor_changes: usize) {
    use std::os::unix::fs::PermissionsExt;
    let parent = root.join(name);
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(parent_mode)).unwrap();
    assert!(read(&parent, false).unwrap().is_none());
    assert!(read(&parent, true).unwrap().is_none());
    let top = parent.join("proj");
    std::fs::create_dir_all(top.join("src/deep")).unwrap();
    std::fs::write(top.join("src/a.rs"), "a").unwrap();
    std::fs::write(top.join("run.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(top.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let target = root.join(format!("{name}-link-target"));
    std::fs::write(&target, "outside the granted tree").unwrap();
    let target_acl = read(&target, false).unwrap();
    assert!(target_acl.as_ref().is_none_or(|a| a.user(me()).is_none()));
    std::os::unix::fs::symlink(&target, top.join("link")).unwrap();
    // The helper is the test's own uid (see acl_dir): the entries are
    // real, and a revoke gives "the helper's" files back to the same uid.
    let helper = me();
    let g = Grant {
        folder: top.clone(),
        owner: me(),
        helper,
        write: true,
    };
    check_external_ancestors();
    let rep = grant(&g, me(), MAX_FILES).unwrap();
    assert!(!rep.already);
    assert_eq!(rep.changed, 5 + ancestor_changes, "{name}: {rep:?}");
    assert_eq!(rep.skipped, 1, "the link: {rep:?}");
    assert!(rep.errors.is_empty() && !rep.limit, "{name}: {rep:?}");
    assert_eq!(rep.not_owned, 0, "{name}: {rep:?}");
    assert!(read(root, false).unwrap().is_none());
    assert!(read(root, true).unwrap().is_none());
    let parent_acl = read(&parent, false).unwrap();
    if ancestor_changes == 1 {
        let a = parent_acl.unwrap();
        assert_eq!(a.user(helper), Some(X), "{name}: {a:?}");
        assert!(a.grants(helper, X) && !a.grants(helper, R | W));
    } else {
        assert!(parent_acl.is_none(), "{name}: {parent_acl:?}");
    }
    assert!(read(&parent, true).unwrap().is_none());
    for dir in [&top, &top.join("src"), &top.join("src/deep")] {
        let access = read(dir, false).unwrap().unwrap();
        let default = read(dir, true).unwrap().unwrap();
        assert!(access.grants(helper, R | W | X), "{name}: {access:?}");
        assert!(default.grants(helper, R | W | X), "{name}: {default:?}");
        assert_eq!(default.user(me()), Some(7), "{name}: {default:?}");
    }
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
    assert_eq!(read(&target, false).unwrap(), target_acl);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "outside the granted tree"
    );
    // The second time: nothing to walk.
    check_external_ancestors();
    assert!(grant(&g, me(), MAX_FILES).unwrap().already);
    // A read grant on a folder that has write already changes nothing.
    check_external_ancestors();
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
    check_external_ancestors();
    // SAFETY: getgid cannot fail.
    let rv = revoke(&g, unsafe { libc::getgid() }, MAX_FILES);
    assert!(rv.errors.is_empty(), "{rv:?}");
    assert_eq!(rv.changed, 6 + ancestor_changes, "{name}: {rv:?}");
    assert_eq!(rv.given_back, 6, "{name}: {rv:?}");
    assert_eq!(read(&target, false).unwrap(), target_acl);
    assert!(read(&parent, false).unwrap().is_none());
    assert!(read(&parent, true).unwrap().is_none());
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        parent_mode
    );
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
    if me() == 0 {
        eprintln!("SKIP: a grant refuses root, and this test runs as root");
        return;
    }
    let Some(t) = acl_dir() else { return };
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
    check_external_ancestors();
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
        folder: "/home/demo/p".into(),
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
