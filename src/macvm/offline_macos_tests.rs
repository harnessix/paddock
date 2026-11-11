//! The offline seed and trust on a real Mac, as a plain user: a small APFS
//! disk image from `hdiutil create`, the attach without owners of
//! `diskimage::Hdiutil`, the owner patch of `apfs.rs`, `fsck_apfs -n`, and
//! the code requirement of `tcc.rs` against `codesign` and `csreq`, and the
//! offline `~/Downloads` link of `offline_link.rs`. Only a
//! Mac runs these; they need no root and no VM.

use super::*;
use crate::macvm::diskimage::{self, DiskTool, Hdiutil};
use crate::macvm::tcc;
use std::os::unix::fs::MetadataExt;
use std::process::Command;

/// The volume of `hdiutil create -fs APFS` has no role.
const ROLE_NONE: u16 = 0;

fn image() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let img = t.path().join("test.dmg");
    let out = Command::new("/usr/bin/hdiutil")
        .args([
            "create", "-size", "64m", "-fs", "APFS", "-volname", "Data", "-layout", "GPTSPUD",
        ])
        .arg(&img)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    (t, img)
}

/// `hdiutil create` makes a UDIF image; the tool attaches it the same way
/// as the raw disk of a golden, so this checks the attach, the mount
/// without owners, the write, the detach, the owner patch and the check.
#[test]
fn a_plain_user_writes_files_and_makes_them_root_owned() {
    let (t, dmg) = image();
    // The golden disk is raw: convert, as the patch reads the image file.
    let raw = t.path().join("disk.img");
    let out = Command::new("/usr/bin/hdiutil")
        .args(["convert", "-format", "UDTO", "-o"])
        .arg(&raw)
        .arg(&dmg)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let cdr = raw.with_extension("img.cdr");
    if cdr.exists() {
        std::fs::rename(&cdr, &raw).unwrap();
    }
    let mut tool = Hdiutil;
    // macOS names /tmp as /private/tmp in a mount point.
    let mnt = t.path().canonicalize().unwrap().join("mnt");
    let a = tool.attach(&raw, &mnt).unwrap();
    assert!(a.disk.starts_with("/dev/disk"), "{a:?}");
    assert!(a.data.starts_with(&mnt), "{a:?}");
    let files = vec![
        GuestFile {
            path: format!("{STAGE}/firstboot.sh"),
            body: FIRSTBOOT.into(),
            mode: 0o755,
        },
        GuestFile {
            path: DAEMON.into(),
            body: DAEMON_PLIST.into(),
            mode: 0o644,
        },
    ];
    let made = write(&a.data, &files).unwrap();
    tool.detach(&a).unwrap();
    let paths: Vec<&str> = made.iter().map(String::as_str).collect();
    assert!(apfs::owners(&raw, ROLE_NONE, &paths)
        .unwrap()
        .iter()
        .all(|o| *o == (apfs::NOOWNERS_ID, apfs::NOOWNERS_ID)));
    assert_eq!(
        apfs::chown(&raw, ROLE_NONE, 0, 0, &paths).unwrap(),
        paths.len()
    );
    match tool.fsck(&raw).unwrap() {
        diskimage::Fsck::Clean => {}
        diskimage::Fsck::Skipped(why) => eprintln!("fsck skipped: {why}"), // not user text
    }
    // With owners on, the files belong to root.
    let on = t.path().join("on");
    std::fs::create_dir_all(&on).unwrap();
    let out = Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-imagekey",
            "diskimage-class=CRawDiskImage",
            "-nobrowse",
            "-owners",
            "on",
            "-mountpoint",
        ])
        .arg(&on)
        .arg(&raw)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let md = std::fs::metadata(on.join(DAEMON)).unwrap();
    let detach = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(&on)
        .arg("-force")
        .output();
    assert_eq!((md.uid(), md.gid()), (0, 0));
    assert!(detach.unwrap().status.success());
}

/// The raw image of `image()`, as a golden disk is.
fn raw_image() -> (tempfile::TempDir, PathBuf) {
    let (t, dmg) = image();
    let raw = t.path().join("disk.img");
    let out = Command::new("/usr/bin/hdiutil")
        .args(["convert", "-format", "UDTO", "-o"])
        .arg(&raw)
        .arg(&dmg)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let cdr = raw.with_extension("img.cdr");
    if cdr.exists() {
        std::fs::rename(&cdr, &raw).unwrap();
    }
    (t, raw)
}

/// The offline `~/Downloads` link on a real mount without owners: the
/// folder that macOS makes carries the ACL `everyone deny delete`, and
/// the mount without owners must still let the link replace it. Then the
/// owner patch sets the owner of the symlink inode, and a mount with
/// owners shows the link as 501:20 with its target.
#[test]
fn the_downloads_link_replaces_a_folder_with_the_deny_delete_acl() {
    use crate::macvm::configure::DOWNLOADS_TARGET;
    use crate::macvm::offline_link::{self, Linked};
    let (t, raw) = raw_image();
    let mut tool = Hdiutil;
    let mnt = t.path().canonicalize().unwrap().join("mnt");
    let a = tool.attach(&raw, &mnt).unwrap();
    let dl = a.data.join(offline_link::downloads());
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join(".localized"), "").unwrap();
    let acl = Command::new("/bin/chmod")
        .args(["+a", "everyone deny delete"])
        .arg(&dl)
        .output()
        .unwrap();
    assert!(acl.status.success(), "{acl:?}");
    tool.detach(&a).unwrap();
    // A fresh attach, as the trust does.
    let a = tool.attach(&raw, &mnt).unwrap();
    let made = offline_link::make(&a.data);
    tool.detach(&a).unwrap();
    assert_eq!(made.unwrap(), Linked::Made);
    let link = offline_link::downloads();
    assert_eq!(
        apfs::owners(&raw, ROLE_NONE, &[link.as_str()]).unwrap(),
        [(apfs::NOOWNERS_ID, apfs::NOOWNERS_ID)]
    );
    assert_eq!(
        apfs::chown(&raw, ROLE_NONE, 501, 20, &[link.as_str()]).unwrap(),
        1
    );
    match tool.fsck(&raw).unwrap() {
        diskimage::Fsck::Clean => {}
        diskimage::Fsck::Skipped(why) => eprintln!("fsck skipped: {why}"), // not user text
    }
    let on = t.path().join("on");
    std::fs::create_dir_all(&on).unwrap();
    let out = Command::new("/usr/bin/hdiutil")
        .args([
            "attach",
            "-imagekey",
            "diskimage-class=CRawDiskImage",
            "-nobrowse",
            "-readonly",
            "-owners",
            "on",
            "-mountpoint",
        ])
        .arg(&on)
        .arg(&raw)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let md = std::fs::symlink_metadata(on.join(&link));
    let target = std::fs::read_link(on.join(&link));
    let detach = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(&on)
        .arg("-force")
        .output();
    let md = md.unwrap();
    assert!(md.file_type().is_symlink());
    assert_eq!((md.uid(), md.gid()), (501, 20));
    assert_eq!(target.unwrap(), Path::new(DOWNLOADS_TARGET));
    assert!(detach.unwrap().status.success());
}

#[test]
fn the_clone_of_a_disk_costs_no_copy_and_matches() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("a");
    std::fs::write(&a, vec![7u8; 1 << 20]).unwrap();
    let b = t.path().join("b");
    diskimage::clone_file(&a, &b).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

#[test]
fn the_identifier_requirement_is_what_csreq_compiles() {
    let t = tempfile::tempdir().unwrap();
    let out = t.path().join("req.bin");
    let src = t.path().join("req.txt");
    std::fs::write(&src, "identifier \"dev.paddock.guest\"").unwrap();
    let r = Command::new("/usr/bin/csreq")
        .arg("-r")
        .arg(&src)
        .arg("-b")
        .arg(&out)
        .output()
        .unwrap();
    assert!(r.status.success(), "{r:?}");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        tcc::identifier_requirement(tcc::IDENTIFIER)
    );
}

/// The signature of `build-guest.sh` on a copy of a binary: its
/// designated requirement is the identifier, and the requirement of the
/// TCC row is the identifier blob.
#[test]
fn an_agent_signed_as_build_guest_signs_it_has_the_identifier_requirement() {
    let t = tempfile::tempdir().unwrap();
    let bin = t.path().join("paddock-guest");
    std::fs::copy("/usr/bin/true", &bin).unwrap();
    let r = Command::new("/usr/bin/codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--identifier",
            "dev.paddock.guest",
        ])
        .arg("-r=designated => identifier \"dev.paddock.guest\"")
        .arg(&bin)
        .output()
        .unwrap();
    assert!(r.status.success(), "{r:?}");
    let d = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-"])
        .arg(&bin)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&d.stdout),
        String::from_utf8_lossy(&d.stderr)
    ); // not user text
    assert_eq!(
        tcc::designated(&text).as_deref(),
        Some("identifier \"dev.paddock.guest\""),
        "{text}"
    );
    assert_eq!(
        tcc::agent_requirement(Some(&bin)),
        tcc::identifier_requirement(tcc::IDENTIFIER)
    );
    // A plain ad-hoc signature pins the hash: the requirement is compiled
    // by csreq and differs.
    let r = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&bin)
        .output()
        .unwrap();
    assert!(r.status.success(), "{r:?}");
    let req = tcc::agent_requirement(Some(&bin));
    assert_ne!(req, tcc::identifier_requirement(tcc::IDENTIFIER));
    assert_eq!(&req[..4], &[0xfa, 0xde, 0x0c, 0x00]);
}

/// The default browser of the guest: a binary LaunchServices file as
/// macOS writes it goes through `plutil -convert xml1`, keeps its other
/// handlers, names the link helper for http and https, and still lints as
/// a property list.
#[test]
fn the_browser_file_merges_a_binary_plist() {
    use crate::macvm::launch_services as ls;
    let t = tempfile::tempdir().unwrap();
    let data = t.path();
    let dir = data.join("Users/paddock").join(ls::FOLDER);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(ls::FILE);
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>LSHandlers</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>LSHandlerRoleAll</key>\n\t\t\t<string>com.apple.mail</string>\n\t\t\t<key>LSHandlerURLScheme</key>\n\t\t\t<string>mailto</string>\n\t\t</dict>\n\t\t<dict>\n\t\t\t<key>LSHandlerRoleAll</key>\n\t\t\t<string>com.apple.safari</string>\n\t\t\t<key>LSHandlerURLScheme</key>\n\t\t\t<string>https</string>\n\t\t</dict>\n\t</array>\n</dict>\n</plist>\n";
    std::fs::write(&file, xml).unwrap();
    let c = Command::new("/usr/bin/plutil")
        .args(["-convert", "binary1"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(c.status.success(), "{c:?}");
    assert!(ls::is_binary(&std::fs::read(&file).unwrap()));
    let w = ls::write(data, crate::guest::link::BUNDLE_ID, &ls::plutil_xml).unwrap();
    assert!(w.merged);
    let lint = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(&file)
        .output()
        .unwrap();
    assert!(lint.status.success(), "{lint:?}");
    let text = ls::plutil_xml(&file).unwrap();
    assert!(text.contains("<string>com.apple.mail</string>"), "{text}");
    assert!(!text.contains("com.apple.safari"), "{text}");
    assert_eq!(
        text.matches("<string>dev.paddock.link</string>").count(),
        2,
        "{text}"
    );
}
