//! APFS clones on a Mac: the bytes and the mode come along, the owner is
//! the user, and the probe finds clones on the volume of the temp folder.

use super::*;

fn dir(p: &Path) -> Dir {
    Root::open(p).unwrap().dir(Path::new(""), false).unwrap()
}

#[test]
fn a_clone_has_the_same_bytes_and_mode() {
    let t = tempfile::tempdir().unwrap();
    let (a, b) = (t.path().join("a"), t.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("f"), b"clone me\n").unwrap();
    std::fs::set_permissions(a.join("f"), std::fs::Permissions::from_mode(0o640)).unwrap();
    clone_at(&dir(&a), OsStr::new("f"), &dir(&b), OsStr::new("g"), 0o640).unwrap();
    assert_eq!(std::fs::read(b.join("g")).unwrap(), b"clone me\n");
    assert_eq!(
        std::fs::metadata(b.join("g")).unwrap().mode() & 0o7777,
        0o640
    );
    // A clone never replaces a file.
    assert!(clone_at(&dir(&a), OsStr::new("f"), &dir(&b), OsStr::new("g"), 0o640).is_err());
    // The copy of a snapshot is a clone too, with the source's mode.
    let n = copy_file(&dir(&a), OsStr::new("f"), &dir(&b), OsStr::new("h"), true).unwrap();
    assert_eq!(n, Some(9));
    assert_eq!(std::fs::read(b.join("h")).unwrap(), b"clone me\n");
    assert_eq!(
        std::fs::metadata(b.join("h")).unwrap().mode() & 0o7777,
        0o640
    );
    // A symlink at the source is not followed.
    std::os::unix::fs::symlink("f", a.join("l")).unwrap();
    assert!(
        clone_at(&dir(&a), OsStr::new("l"), &dir(&b), OsStr::new("m"), 0o600)
            .map(|_| std::fs::symlink_metadata(b.join("m"))
                .unwrap()
                .file_type()
                .is_symlink())
            .unwrap_or(true)
    );
}

#[test]
fn the_probe_finds_clones_on_apfs() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let run = t.path().join("run");
    std::fs::create_dir_all(home.join(".fake")).unwrap();
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(home.join(".fake/conv.jsonl"), "x").unwrap();
    let root = Root::open(&home).unwrap();
    assert!(probe_reflink(&root, &run, Path::new(".fake/conv.jsonl")));
    // The probe file is gone.
    assert_eq!(std::fs::read_dir(&run).unwrap().count(), 0);
}

#[test]
fn a_clone_of_a_file_of_another_owner_belongs_to_the_user() {
    // /etc/hosts belongs to root and is on the data volume, as the temp
    // folder is.
    let uid = unsafe { libc::getuid() };
    let etc = Path::new("/private/etc");
    if uid == 0 || std::fs::metadata(etc.join("hosts")).map(|m| m.uid()).ok() != Some(0) {
        eprintln!("skipped: needs a root-owned /etc/hosts and a user that is not root");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    match clone_at(
        &dir(etc),
        OsStr::new("hosts"),
        &dir(t.path()),
        OsStr::new("h"),
        0o644,
    ) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            eprintln!("skipped: /etc is on another volume than the temp folder");
            return;
        }
        Err(e) => panic!("{e}"),
    }
    assert_eq!(std::fs::metadata(t.path().join("h")).unwrap().uid(), uid);
}
