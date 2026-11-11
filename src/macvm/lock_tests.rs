use super::*;
use crate::macvm::text::Holder;
use std::os::unix::fs::PermissionsExt;

#[test]
fn profile_lock_second_fails_with_pid() {
    let t = tempfile::tempdir().unwrap();
    let p = Profile::new(t.path().join("app").join("home"));
    let first = ProfileLock::take_as(&p, 812).unwrap();
    assert_eq!(std::fs::read_to_string(p.lock()).unwrap(), "812\n");
    assert_eq!(
        std::fs::metadata(&p.dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let e = ProfileLock::take_as(&p, 813).unwrap_err();
    assert!(matches!(e, LockError::Held { pid: Some(812) }), "{e:?}");
    assert_eq!(e.to_string(), "the profile already runs (pid 812)");
    // The text is not trusted beyond display.
    drop(first);
    std::fs::write(p.lock(), "not a pid").unwrap();
    let again = ProfileLock::take_as(&p, 814).unwrap();
    assert_eq!(std::fs::read_to_string(p.lock()).unwrap(), "814\n");
    // A link in place of the lock file is refused.
    drop(again);
    std::fs::remove_file(p.lock()).unwrap();
    std::os::unix::fs::symlink(t.path().join("elsewhere"), p.lock()).unwrap();
    assert!(matches!(
        ProfileLock::take_as(&p, 815),
        Err(LockError::Io(_))
    ));
}

#[test]
fn slots_third_fails_with_holders() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("slots");
    let a = Slot::take_in(&dir, "ana", "sample-notes", 812).unwrap();
    assert_eq!(a.index, 0);
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777,
        0o1777
    );
    assert_eq!(
        std::fs::metadata(&a.path).unwrap().permissions().mode() & 0o777,
        0o666
    );
    assert_eq!(
        std::fs::read_to_string(&a.path).unwrap(),
        "ana sample-notes 812\n"
    );
    let b = Slot::take_in(&dir, "bo", "vscode", 900).unwrap();
    assert_eq!(b.index, 1);
    assert_eq!(
        holders(&dir, "ana"),
        vec![
            Some(Holder::Mine {
                app: "sample-notes".into(),
                pid: 812
            }),
            Some(Holder::User { name: "bo".into() })
        ]
    );
    let e = Slot::take_in(&dir, "cy", "textedit", 1000).unwrap_err();
    let SlotError::Full(full) = e else {
        panic!("{e}");
    };
    assert_eq!(
        full.holders,
        vec![
            Holder::User { name: "ana".into() },
            Holder::User { name: "bo".into() }
        ]
    );
    assert_eq!(
        full.to_string(),
        "two macOS VMs already run on this Mac: a VM of user ana and a VM of user bo. Stop one, then try again"
    );
    // The holder of this user is named with the app, the pid and the
    // command that stops it.
    let e = Slot::take_in(&dir, "ana", "textedit", 1000).unwrap_err();
    let SlotError::Full(full) = e else {
        panic!("{e}");
    };
    assert_eq!(
        full.to_string(),
        "two macOS VMs already run on this Mac: sample-notes (you, pid 812) and a VM of user bo. Stop one (paddock app stop sample-notes), then try again"
    );
    // No third file appeared.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    // A released slot is free again, and the holder text is rewritten.
    drop(a);
    assert_eq!(holders(&dir, "ana")[0], None);
    let c = Slot::take_in(&dir, "cy", "textedit", 1000).unwrap();
    assert_eq!(c.index, 0);
    assert_eq!(
        std::fs::read_to_string(&c.path).unwrap(),
        "cy textedit 1000\n"
    );
}

#[test]
fn holder_text_is_cut_and_one_line() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("slots");
    let _a = Slot::take_in(&dir, "ana", "sample-notes", 812).unwrap();
    let _b = Slot::take_in(&dir, "bo", "vscode", 900).unwrap();
    // Another program wrote junk into the slot files.
    std::fs::write(dir.join("slot-0"), "first line\nsecond line\n").unwrap();
    std::fs::write(dir.join("slot-1"), "x".repeat(10 * 1024)).unwrap();
    let e = Slot::take_in(&dir, "cy", "textedit", 1000).unwrap_err();
    let SlotError::Full(full) = e else {
        panic!("{e}");
    };
    let name = |h: &Holder| match h {
        Holder::User { name } => name.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(name(&full.holders[0]), "first line");
    assert_eq!(name(&full.holders[1]).chars().count(), 80);
    assert!(full.holders.iter().all(|h| !name(h).contains('\n')));
    assert_eq!(holder_line(""), "unknown");
    assert_eq!(holder_line("a\tb\r\n"), "a b");
    assert_eq!(
        describe_holder("ana app 12", "ana"),
        Holder::Mine {
            app: "app".into(),
            pid: 12
        }
    );
    assert_eq!(
        describe_holder("ana app 12", "bo"),
        Holder::User { name: "ana".into() }
    );
    assert_eq!(
        describe_holder("ana app twelve", "ana"),
        Holder::User {
            name: "ana app twelve".into()
        }
    );
    assert_eq!(holder_text("a b", "x\ty", 1), "a_b x_y 1\n");
    assert_eq!(holder_text("", "", 1), "unknown unknown 1\n");
}
