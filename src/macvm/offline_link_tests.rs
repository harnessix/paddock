use super::*;
use crate::macvm::apfs_fixture::Entry;
use crate::macvm::offline_fake::{FakeDisk, Store};

fn entry(path: &str, dir: bool, uid: u32, gid: u32) -> Entry {
    Entry {
        path: path.into(),
        dir,
        uid,
        gid,
    }
}

/// A fake disk with the home of the first account and its fresh
/// `Downloads` folder (a `.localized` in it), as macOS makes them.
fn disk_with_home(image: &Path) -> FakeDisk {
    let mut d = FakeDisk::new(image);
    d.model.push(entry("Users/paddock", true, 501, 20));
    d.model
        .push(entry("Users/paddock/Downloads", true, 501, 20));
    d.model
        .push(entry("Users/paddock/Downloads/.localized", false, 501, 20));
    d.model
        .push(entry("Users/paddock/Documents", true, 501, 20));
    d.store(image);
    d
}

#[test]
fn the_paths_are_the_ones_of_the_configure_step() {
    assert_eq!(home(), "Users/paddock");
    assert_eq!(downloads(), "Users/paddock/Downloads");
    assert_eq!(DOWNLOADS_TARGET, "/Volumes/My Shared Files/downloads");
    let step = crate::macvm::configure::step("downloads-link").unwrap();
    assert_eq!(step.expect, DOWNLOADS_TARGET);
    assert_eq!(Linked::Made.word(), "made");
    assert_eq!(Linked::Already.word(), "already");
}

#[test]
fn a_fresh_folder_becomes_the_link_owned_by_the_account() {
    let t = tempfile::tempdir().unwrap();
    let image = t.path().join("disk.img");
    let mut d = disk_with_home(&image);
    let mnt = t.path().join("mnt");
    let mut tool = Store {
        d: &mut d,
        image: image.clone(),
    };
    assert_eq!(link_disk(&mut tool, &image, &mnt).unwrap(), Linked::Made);
    assert_eq!(d.calls, ["attach", "detach"]);
    // The link is a new inode of the mount without owners, patched to the
    // owner of the home; the other folders keep theirs.
    assert_eq!(
        FakeDisk::owner(&image, "Users/paddock/Downloads"),
        (501, 20)
    );
    assert_eq!(
        FakeDisk::owner(&image, "Users/paddock/Documents"),
        (501, 20)
    );
    assert_eq!(FakeDisk::owner(&image, "Users"), (0, 0));
    let e = d
        .model
        .iter()
        .find(|e| e.path == "Users/paddock/Downloads")
        .unwrap();
    assert!(!e.dir, "the folder went, the link is a file entry: {e:?}");
    assert!(!d.model.iter().any(|e| e.path.ends_with(".localized")));
    assert!(!mnt.exists(), "the mount root is gone");
}

#[test]
fn make_keeps_a_right_link_and_replaces_a_wrong_one() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path();
    std::fs::create_dir_all(data.join("Users/paddock/Downloads")).unwrap();
    std::fs::write(data.join("Users/paddock/Downloads/.DS_Store"), "x").unwrap();
    let link = data.join("Users/paddock/Downloads");
    assert_eq!(make(data).unwrap(), Linked::Made);
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new(DOWNLOADS_TARGET)
    );
    // A second run (a setup that runs again) keeps it.
    assert_eq!(make(data).unwrap(), Linked::Already);
    // A link to another place is replaced.
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("/tmp/elsewhere", &link).unwrap();
    assert_eq!(make(data).unwrap(), Linked::Made);
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new(DOWNLOADS_TARGET)
    );
    // No folder at all: the link is made.
    std::fs::remove_file(&link).unwrap();
    assert_eq!(make(data).unwrap(), Linked::Made);
}

#[test]
fn a_folder_with_files_is_left_alone_and_named() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path();
    let dl = data.join("Users/paddock/Downloads");
    std::fs::create_dir_all(dl.join("sub")).unwrap();
    std::fs::write(dl.join("report.pdf"), "pdf").unwrap();
    std::fs::write(dl.join(".localized"), "").unwrap();
    let e = format!("{:#}", make(data).unwrap_err());
    assert!(e.contains("report.pdf") && e.contains("sub"), "{e}");
    assert!(e.contains("Downloads"), "{e}");
    // Nothing went: not even the disposable file.
    assert!(dl.join("report.pdf").is_file() && dl.join(".localized").is_file());
    assert!(dl.join("sub").is_dir());
    // A folder named `.localized` is not the disposable file.
    std::fs::remove_dir_all(&dl).unwrap();
    std::fs::create_dir_all(dl.join(".localized")).unwrap();
    assert!(make(data).is_err());
    assert!(dl.join(".localized").is_dir());
}

#[test]
fn no_home_or_a_plain_file_is_an_error_with_the_path() {
    let t = tempfile::tempdir().unwrap();
    let e = format!("{:#}", make(t.path()).unwrap_err());
    assert!(e.contains("Users/paddock"), "{e}");
    std::fs::create_dir_all(t.path().join("Users/paddock")).unwrap();
    std::fs::write(t.path().join("Users/paddock/Downloads"), "x").unwrap();
    let e = format!("{:#}", make(t.path()).unwrap_err());
    assert!(e.contains("Downloads") && e.contains("not a folder"), "{e}");
    assert!(t.path().join("Users/paddock/Downloads").is_file());
}

#[test]
fn a_failed_make_still_detaches_and_changes_no_owner() {
    let t = tempfile::tempdir().unwrap();
    let image = t.path().join("disk.img");
    let mut d = FakeDisk::new(&image); // no home on the disk
    let mnt = t.path().join("mnt");
    let mut tool = Store {
        d: &mut d,
        image: image.clone(),
    };
    let e = format!("{:#}", link_disk(&mut tool, &image, &mnt).unwrap_err());
    assert!(e.contains("Users/paddock"), "{e}");
    assert_eq!(d.calls, ["attach", "detach"]);
    // A failed attach: nothing else runs.
    let mut d = disk_with_home(&image);
    d.fail_attach = true;
    let mut tool = Store {
        d: &mut d,
        image: image.clone(),
    };
    assert!(link_disk(&mut tool, &image, &mnt).is_err());
    assert_eq!(d.calls, ["attach"]);
    // A failed detach after a good make is an error, and no owner patch
    // runs on an image that may still be attached.
    let mut d = disk_with_home(&image);
    d.fail_detach = true;
    let mut tool = Store {
        d: &mut d,
        image: image.clone(),
    };
    assert!(link_disk(&mut tool, &image, &mnt).is_err());
    assert_eq!(
        FakeDisk::owner(&image, "Users/paddock/Downloads"),
        (501, 20)
    );
}

#[test]
fn the_owner_comes_from_the_home_and_must_be_an_account() {
    let t = tempfile::tempdir().unwrap();
    let image = t.path().join("disk.img");
    let mut d = FakeDisk::new(&image);
    d.model.push(entry("Users/paddock", true, 502, 20));
    d.model.push(entry(
        "Users/paddock/Downloads",
        false,
        apfs::NOOWNERS_ID,
        apfs::NOOWNERS_ID,
    ));
    d.store(&image);
    assert_eq!(own(&image).unwrap(), (502, 20));
    assert_eq!(
        FakeDisk::owner(&image, "Users/paddock/Downloads"),
        (502, 20)
    );
    // Again: the owner is already right, nothing to write.
    assert_eq!(own(&image).unwrap(), (502, 20));
    // A home of root, or of the mount without owners, is not an account.
    for uid in [0, apfs::NOOWNERS_ID] {
        let mut d = FakeDisk::new(&image);
        d.model.push(entry("Users/paddock", true, uid, 20));
        d.model
            .push(entry("Users/paddock/Downloads", false, 99, 99));
        d.store(&image);
        let e = format!("{:#}", own(&image).unwrap_err());
        assert!(e.contains("Users/paddock"), "{e}");
        assert_eq!(FakeDisk::owner(&image, "Users/paddock/Downloads"), (99, 99));
    }
    // A link with a foreign owner is not changed.
    let mut d = FakeDisk::new(&image);
    d.model.push(entry("Users/paddock", true, 501, 20));
    d.model.push(entry("Users/paddock/Downloads", false, 0, 0));
    d.store(&image);
    assert!(own(&image).is_err());
    assert_eq!(FakeDisk::owner(&image, "Users/paddock/Downloads"), (0, 0));
}
