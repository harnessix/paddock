//! The app copy decisions, on every platform.

use super::*;
use crate::macvm::proto::Request;

/// A bundle on disk: `Contents/Info.plist`, the binary, and an icon when
/// `icon` is true.
fn bundle(dir: &Path, name: &str, version: &str, build: &str, icon: bool) -> PathBuf {
    let app = dir.join(format!("{name}.app"));
    let contents = app.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS")).unwrap();
    std::fs::create_dir_all(contents.join("Resources")).unwrap();
    std::fs::write(contents.join("MacOS").join(name), b"\xcf\xfa\xed\xfe").unwrap();
    if icon {
        std::fs::write(contents.join("Resources/app.icns").as_path(), b"icns....").unwrap();
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>{name}</string>
	<key>CFBundleIdentifier</key><string>com.vendor.{name}</string>
	<key>CFBundleExecutable</key><string>{name}</string>
	<key>CFBundleShortVersionString</key><string>{version}</string>
	<key>CFBundleVersion</key><string>{build}</string>
	<key>CFBundleIconFile</key><string>app</string>
</dict>
</plist>
"#
    );
    std::fs::write(contents.join("Info.plist"), plist).unwrap();
    app
}

#[test]
fn needs_copy_cases() {
    let t = tempfile::tempdir().unwrap();
    let app = bundle(t.path(), "ChatGPT", "1.2026.300", "1726", true);
    let b = read(&app).unwrap();
    assert_eq!(b.name, "ChatGPT");
    assert_eq!(b.display_name, "ChatGPT");
    assert_eq!(b.bundle_id, "com.vendor.ChatGPT");
    assert_eq!(b.version, "1.2026.300");
    assert_eq!(b.build, "1726");
    assert_eq!(b.icon, Some(app.join("Contents/Resources/app.icns")));
    assert!(!b.mas);
    assert!(b.bytes > 0);

    // A fresh profile has no app stamp: the guest needs the copy.
    assert!(needs_copy(None, &b.stamp()));
    // The same version and build: no copy.
    let same = AppStamp {
        version: "1.2026.300".into(),
        build: "1726".into(),
        copied: Some("2026-11-02".into()),
    };
    assert!(!needs_copy(Some(&same), &b.stamp()));
    // A new version, and a new build of the same version: a copy.
    let new_version = AppStamp {
        version: "1.2026.299".into(),
        build: "1726".into(),
        copied: None,
    };
    assert!(needs_copy(Some(&new_version), &b.stamp()));
    let new_build = AppStamp {
        version: "1.2026.300".into(),
        build: "1725".into(),
        copied: None,
    };
    assert!(needs_copy(Some(&new_build), &b.stamp()));

    // The guest paths and the estimate of the start line.
    let p = Profile::new(t.path().join("profile"));
    assert_eq!(b.clone_path(&p), p.apps().join("ChatGPT.app"));
    assert_eq!(
        b.guest_source(),
        Path::new("/Volumes/My Shared Files/apps/ChatGPT.app.cpio")
    );
    assert_eq!(b.archive_path(&p), p.apps().join("ChatGPT.app.cpio"));
    // The Applications folder of the guest home on the data disk.
    assert_eq!(
        b.guest_target(),
        Path::new(crate::guest::data::HOME)
            .join(GUEST_APPS)
            .join("ChatGPT.app")
    );
    assert!(b.guest_target().starts_with(crate::guest::data::MOUNT));
    assert!(b.copy_seconds() >= 2);
    let big = HostBundle {
        bytes: 500 << 20,
        ..b.clone()
    };
    assert_eq!(big.copy_seconds(), 20);

    // A bundle without Contents, and one without a version.
    let plain = t.path().join("notes.txt");
    std::fs::write(&plain, b"x").unwrap();
    let e = read(&plain).unwrap_err().to_string();
    assert!(e.contains("not an app bundle"), "{e}");
    let bare = t.path().join("Bare.app");
    std::fs::create_dir_all(bare.join("Contents")).unwrap();
    let e = read(&bare).unwrap_err().to_string();
    assert!(e.contains("CFBundleShortVersionString"), "{e}");
    // Without CFBundleName the folder name is the display name.
    std::fs::write(
        bare.join("Contents/Info.plist"),
        "<plist><dict><key>CFBundleShortVersionString</key><string>2</string></dict></plist>",
    )
    .unwrap();
    let b = read(&bare).unwrap();
    assert_eq!((b.display_name.as_str(), b.build.as_str()), ("Bare", ""));
    assert_eq!(b.icon, None);
}

#[test]
fn mas_refusal_text() {
    let t = tempfile::tempdir().unwrap();
    let app = bundle(t.path(), "Pages", "14.0", "7036", false);
    let b = read(&app).unwrap();
    check(&b).expect("a bundle from the vendor runs");
    std::fs::create_dir_all(app.join("Contents/_MASReceipt")).unwrap();
    std::fs::write(app.join("Contents/_MASReceipt/receipt"), b"r").unwrap();
    let b = read(&app).unwrap();
    assert!(b.mas);
    let e = check(&b).unwrap_err().to_string();
    assert!(e.contains("Pages.app"), "the refusal names the app: {e}");
    assert!(e.contains("App Store"), "{e}");
    assert!(
        e.contains("Install the app from the vendor's website"),
        "the refusal says to download the DMG from the vendor: {e}"
    );
    assert!(!e.contains('\n'), "{e}");
}

#[test]
fn failure_text_keeps_old() {
    let t = tempfile::tempdir().unwrap();
    let app = bundle(t.path(), "ChatGPT", "1.0", "1", true);
    let b = read(&app).unwrap();
    let p = Profile::new(t.path().join("profile"));

    // The clone makes the final name only, never a partial.
    let made = clone_into_profile(&p, &b, &crate::macvm::drop::clone_into).unwrap();
    assert_eq!(made, p.apps().join("ChatGPT.app"));
    assert!(made.join("Contents/Info.plist").is_file());
    assert!(super::super::session::partials(&p).is_empty());

    // A clone over an old copy and a leftover partial removes both.
    let stale = super::super::layout::partial(&made);
    std::fs::create_dir_all(stale.join("Contents")).unwrap();
    std::fs::write(made.join("Contents/old-marker"), b"x").unwrap();
    let again = clone_into_profile(&p, &b, &crate::macvm::drop::clone_into).unwrap();
    assert!(!again.join("Contents/old-marker").exists());
    assert!(!stale.exists());

    // The archive of the clone: made as a partial, then the final name;
    // a failed one leaves nothing.
    let arch = archive_into_profile(&p, &b, &|src, dst| {
        assert_eq!(src, b.clone_path(&p));
        assert!(
            dst.to_string_lossy().ends_with(".cpio.partial"),
            "{}",
            dst.display()
        );
        std::fs::write(dst, b"archive")
    })
    .unwrap();
    assert_eq!(arch, b.archive_path(&p));
    assert_eq!(std::fs::read(&arch).unwrap(), b"archive");
    let e = archive_into_profile(&p, &b, &|_, dst| {
        std::fs::write(dst, b"half")?;
        Err(std::io::Error::other("No space left on device"))
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("No space left on device"), "{e}");
    assert!(!arch.exists() && !super::super::layout::partial(&arch).exists());

    // A clone that fails names the reason and says the old copy stays.
    let e = clone_into_profile(&p, &b, &|_, _| {
        Err(std::io::Error::other("No space left on device"))
    })
    .unwrap_err()
    .to_string();
    assert!(e.contains("No space left on device"), "{e}");
    assert!(e.contains("The old copy stays"), "{e}");
    // The copy failure the agent reports reads the same way.
    let line = crate::macvm::text::copy_failed("ChatGPT", "the ditto failed");
    assert!(
        line.contains("the ditto failed") && line.contains("The old copy stays"),
        "{line}"
    );

    // The request carries the guest paths, the version and the
    // quarantine decision. Nothing on disk here carries the attribute,
    // so the guest may remove it from the copy.
    assert!(!quarantined_unapproved(&app));
    let Request::AppCopy {
        from,
        to,
        version,
        unquarantine,
    } = request(&b)
    else {
        panic!("not an app.copy")
    };
    assert_eq!(from, b.guest_source());
    assert_eq!(to, b.guest_target());
    assert_eq!(version, "1.0");
    assert!(unquarantine);
    // The flags field decides: bit 0x40 is the user's approval.
    assert!(super::approved_flags("0041;68f0;Safari;"));
    assert!(!super::approved_flags("0001;68f0;Safari;"));
    assert!(!super::approved_flags("nothex;x"));
}

#[test]
fn size_stops_at_the_budget() {
    let t = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(t.path().join("a/b")).unwrap();
    std::fs::write(t.path().join("a/one"), vec![0u8; 100]).unwrap();
    std::fs::write(t.path().join("a/b/two"), vec![0u8; 50]).unwrap();
    std::os::unix::fs::symlink("one", t.path().join("a/link")).unwrap();
    assert_eq!(size(&t.path().join("a")), 150);
    assert_eq!(size(&t.path().join("none")), 0);
    assert_eq!(COPY_HEADROOM, 500 << 20);
}
