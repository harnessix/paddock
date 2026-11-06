use super::*;

const ATTACH: &str = include_str!("testdata/hdiutil-attach.plist");
const APFS_LIST: &str = include_str!("testdata/diskutil-apfs-list.plist");

#[test]
fn the_attach_output_gives_the_disk_the_store_and_the_volumes() {
    let e = entities(ATTACH).unwrap();
    assert_eq!(e.disk.as_deref(), Some("/dev/disk4"));
    assert_eq!(e.store.as_deref(), Some("/dev/disk4s2"));
    assert_eq!(e.volumes.len(), 3);
    assert_eq!(e.volumes[2], ("/dev/disk5s3".to_string(), None));
}

#[test]
fn the_data_volume_is_found_by_its_role_then_by_its_name() {
    let e = entities(ATTACH).unwrap();
    let roles = data_devices(APFS_LIST).unwrap();
    assert_eq!(roles, ["disk3s5", "disk5s2"]);
    let m = data_mount(&e, &roles).unwrap();
    assert!(m.ends_with("golden/mnt/Data"), "{m:?}");
    // Without diskutil, the name of the installer.
    assert_eq!(data_mount(&e, &[]).unwrap(), m);
    // A role on an unmounted volume and no Data name: nothing.
    let mut e2 = e.clone();
    e2.volumes.retain(|(d, _)| d != "/dev/disk5s2");
    assert_eq!(data_mount(&e2, &["disk5s3".into()]), None);
    // The name form of an older installer.
    e2.volumes.push((
        "/dev/disk5s9".into(),
        Some(PathBuf::from("/x/Macintosh HD - Data")),
    ));
    assert_eq!(
        data_mount(&e2, &[]).unwrap(),
        PathBuf::from("/x/Macintosh HD - Data")
    );
}

#[test]
fn the_plist_reader_takes_every_form_and_refuses_garbage() {
    let p = parse_plist(
        "<?xml version=\"1.0\"?><!-- c --><plist version=\"1.0\"><dict><key>a&amp;b</key><string>x &lt;y&gt;</string>\
         <key>n</key><integer> -3 </integer><key>t</key><true/><key>f</key><false/>\
         <key>e</key><array/><key>d</key><dict/><key>s</key><string/><key>z</key><data>AAEC</data></dict></plist>",
    )
    .unwrap();
    assert_eq!(p.get("a&b").and_then(Plist::str), Some("x <y>"));
    assert_eq!(p.get("n"), Some(&Plist::Integer(-3)));
    assert_eq!(p.get("t"), Some(&Plist::Bool(true)));
    assert_eq!(p.get("f"), Some(&Plist::Bool(false)));
    assert_eq!(p.get("e").map(Plist::array), Some(&[][..]));
    assert_eq!(p.get("d"), Some(&Plist::Dict(vec![])));
    assert_eq!(p.get("s").and_then(Plist::str), Some(""));
    assert_eq!(p.get("z"), Some(&Plist::Other("AAEC".into())));
    for bad in [
        "",
        "hdiutil: attach failed - no such file",
        "<plist><dict><key>a</key>",
        "<plist><dict><string>x</string></dict></plist>",
        "<plist><integer>x</integer></plist>",
        "<plist><blob/></plist>",
        &format!("<plist>{}", "<array>".repeat(100)),
    ] {
        assert!(parse_plist(bad).is_err(), "{bad:?}");
    }
    assert!(entities("<plist><dict/></plist>").is_err());
}

#[test]
fn the_commands_of_the_tool() {
    let a = attach_argv(Path::new("/g/disk.img"), Path::new("/g/mnt"));
    assert_eq!(
        a,
        [
            "/usr/bin/hdiutil",
            "attach",
            "-imagekey",
            "diskimage-class=CRawDiskImage",
            "-nobrowse",
            "-owners",
            "off",
            "-mountRoot",
            "/g/mnt",
            "-plist",
            "/g/disk.img"
        ]
    );
    let n = attach_nomount_argv(Path::new("/g/disk.img"));
    assert!(n.contains(&"-nomount".to_string()) && !n.contains(&"-owners".to_string()));
    assert_eq!(
        detach_argv("/dev/disk4", false),
        ["/usr/bin/hdiutil", "detach", "/dev/disk4"]
    );
    assert_eq!(detach_argv("/dev/disk4", true).last().unwrap(), "-force");
    assert_eq!(
        fsck_argv("/dev/disk4s2"),
        ["/sbin/fsck_apfs", "-n", "/dev/disk4s2"]
    );
    assert_eq!(fsck_argv("disk4s2")[2], "/dev/disk4s2");
    assert!(fsck_denied("fsck_apfs: /dev/rdisk4s2: Permission denied"));
    assert!(fsck_denied("error: must be run as root"));
    assert!(!fsck_denied("error: the root tree is invalid"));
    assert!(busy("hdiutil: detach failed - Resource busy"));
    assert!(busy(
        "hdiutil: attach failed - Resource temporarily unavailable"
    ));
    assert!(!busy("hdiutil: attach failed - no such file"));
}

#[test]
fn run_gives_the_streams_and_names_a_missing_program() {
    let (ok, out, err) = run(&[
        "/bin/sh".into(),
        "-c".into(),
        "echo a; echo b >&2; exit 2".into(),
    ])
    .unwrap();
    assert!(!ok);
    assert_eq!((out.as_str(), err.as_str()), ("a\n", "b\n"));
    let e = run(&["/nonexistent/hdiutil".into()]).unwrap_err();
    assert!(format!("{e:#}").contains("/nonexistent/hdiutil"));
    assert!(run(&[]).is_err());
}

#[test]
fn clone_file_copies_and_replaces() {
    let t = tempfile::tempdir().unwrap();
    let (a, b) = (t.path().join("a"), t.path().join("b"));
    std::fs::write(&a, "one").unwrap();
    std::fs::write(&b, "old").unwrap();
    clone_file(&a, &b).unwrap();
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "one");
    assert!(clone_file(&t.path().join("none"), &b).is_err());
}

#[test]
fn the_mount_root_goes_only_when_it_holds_nothing_but_empty_folders() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("mnt");
    std::fs::create_dir_all(root.join("Data")).unwrap();
    std::fs::create_dir_all(root.join("Macintosh HD")).unwrap();
    remove_mount_root(&root);
    assert!(!root.exists());
    // A folder with files (a volume that did not detach) keeps every file.
    std::fs::create_dir_all(root.join("Data/Library")).unwrap();
    std::fs::write(root.join("Data/Library/keep"), "x").unwrap();
    remove_mount_root(&root);
    assert!(root.join("Data/Library/keep").is_file());
    // No mount point under a plain folder.
    assert!(mounts_under(&root).is_empty());
    assert!(mounts_under(&t.path().join("none")).is_empty());
}

#[test]
fn an_attach_waits_until_the_image_is_unlocked() {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    let t = tempfile::tempdir().unwrap();
    let img = t.path().join("disk.img");
    std::fs::write(&img, b"x").unwrap();
    let holder = std::fs::File::open(&img).unwrap();
    // SAFETY: flock on an fd that `holder` owns.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let t0 = Instant::now();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        drop(holder);
    });
    wait_unlocked(&img, Duration::from_secs(10));
    let waited = t0.elapsed();
    release.join().unwrap();
    assert!(waited >= Duration::from_millis(600), "{waited:?}");
    assert!(waited < Duration::from_secs(5), "{waited:?}");
    // A lock that stays: the wait ends at its limit.
    let holder = std::fs::File::open(&img).unwrap();
    // SAFETY: as above.
    assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
    let t0 = Instant::now();
    wait_unlocked(&img, Duration::from_millis(800));
    assert!(t0.elapsed() < Duration::from_secs(3));
    // A missing image does not wait.
    wait_unlocked(&t.path().join("none"), Duration::from_secs(10));
}
