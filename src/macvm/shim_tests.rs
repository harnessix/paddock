//! The shim bundle: its plist, its sign argv and the partial rule.

use super::*;

/// A fake `paddock-vm` and the folder that holds the bundles.
fn install(t: &Path) -> (PathBuf, PathBuf) {
    let bin = t.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let helper = bin.join("paddock-vm");
    std::fs::write(&helper, b"\xcf\xfa\xed\xfe helper").unwrap();
    let dir = t.join("shims");
    std::fs::create_dir_all(&dir).unwrap();
    (helper, dir)
}

#[test]
fn plist_and_sign_argv() {
    let icon = PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/app.icns");
    let face = Face::session("chatgpt-desktop", "ChatGPT", Some(icon));
    assert_eq!(face.display_name, "ChatGPT (paddock)");
    assert_eq!(face.bundle_id, "dev.paddock.app.chatgpt-desktop");
    let xml = info_plist(&face, "0.2.1");
    for part in [
        "<string>ChatGPT (paddock)</string>",
        "<key>CFBundleName</key>",
        "<key>CFBundleDisplayName</key>",
        "<string>dev.paddock.app.chatgpt-desktop</string>",
        "<key>CFBundleIconFile</key>",
        "<string>app.icns</string>",
        "<string>chatgpt-desktop</string>",
        "<string>0.2.1</string>",
        "<key>LSUIElement</key>\n\t<false/>",
        "<key>CFBundlePackageType</key>\n\t<string>APPL</string>",
    ] {
        assert!(xml.contains(part), "the plist lacks {part:?}:\n{xml}");
    }
    assert!(xml.starts_with("<?xml"));
    assert!(
        !xml.contains("CFBundleDocumentTypes"),
        "a session bundle takes no documents"
    );
    // No icon: no icon key, so the session shows the generic icon.
    let bare = Face::session("vscode", "Visual Studio Code", None);
    assert!(!info_plist(&bare, "0.2.1").contains("CFBundleIconFile"));

    // The drag target takes app bundles and has its own id.
    let drop = Face::drop_target();
    assert_eq!(drop.app, "Paddock Apps");
    assert_eq!(drop.display_name, "Paddock Apps");
    let xml = info_plist(&drop, "0.2.1");
    for part in [
        "<key>CFBundleDocumentTypes</key>",
        "<string>com.apple.application-bundle</string>",
        "<string>dev.paddock.app.drop</string>",
        "<string>Paddock_Apps</string>",
    ] {
        assert!(xml.contains(part), "the drop plist lacks {part:?}:\n{xml}");
    }
    // A name with XML characters stays valid XML.
    let odd = Face::session("x", "A & B <C>", None);
    assert!(info_plist(&odd, "1").contains("<string>A &amp; B &lt;C&gt; (paddock)</string>"));

    // The sign argv: ad hoc, with the entitlements file of the bundle.
    let app = Path::new("/d/ChatGPT.app");
    let ent = Path::new("/d/ChatGPT.app/Contents/Resources/paddock-vm.entitlements");
    let argv = sign_argv(app, ent);
    let at = argv.iter().position(|a| a == "--sign").unwrap();
    assert_eq!(argv[at + 1], "-", "an ad hoc signature is -s -");
    let at = argv.iter().position(|a| a == "--entitlements").unwrap();
    assert_eq!(argv[at + 1], ent.display().to_string());
    assert_eq!(argv.last().unwrap(), &app.display().to_string());
    assert!(argv.contains(&"--force".to_string()));
    // The entitlements that go into the bundle carry the one key the
    // Virtualization framework needs.
    assert!(ENTITLEMENTS.contains("com.apple.security.virtualization"));
}

#[test]
fn make_no_partial() {
    let t = tempfile::tempdir().unwrap();
    let (helper, dir) = install(t.path());
    let icon = t.path().join("app.icns");
    std::fs::write(&icon, b"icns-bytes").unwrap();
    let signed = std::cell::RefCell::new(Vec::<Vec<String>>::new());
    let sign = |argv: &[String]| {
        signed.borrow_mut().push(argv.to_vec());
        Ok(())
    };
    let face = Face::session("chatgpt-desktop", "ChatGPT", Some(icon));
    let made = make(&dir, &face, &helper, "0.2.1", &sign).unwrap();

    assert_eq!(made, dir.join("chatgpt-desktop.app"));
    let exe = executable(&made, "chatgpt-desktop");
    assert!(exe.is_file(), "the bundle holds a copy of paddock-vm");
    assert_eq!(
        std::fs::read(&exe).unwrap(),
        std::fs::read(&helper).unwrap()
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the executable is executable");
    }
    assert_eq!(
        std::fs::read(made.join("Contents/Resources/app.icns")).unwrap(),
        b"icns-bytes"
    );
    assert!(built_by(&made).as_deref() == Some("0.2.1"));
    assert!(!needs_rebuild(&made, "0.2.1", &helper));
    assert!(
        needs_rebuild(&made, "0.3.0", &helper),
        "a bundle of another paddock version is built again"
    );
    assert!(needs_rebuild(&dir.join("none.app"), "0.2.1", &helper));
    // The same version from another helper binary is built again; a
    // helper that cannot be read leaves the version to decide.
    std::fs::write(&helper, b"\xcf\xfa\xed\xfe helper, rebuilt").unwrap();
    assert!(needs_rebuild(&made, "0.2.1", &helper));
    assert!(!needs_rebuild(&made, "0.2.1", &dir.join("no-helper")));
    // The session bundle has no target file: paddock starts it with an argv.
    assert!(!target_file(&made).exists());
    assert!(no_partials(&dir), "{:?}", std::fs::read_dir(&dir));

    // One codesign call, for the partial bundle, with the entitlements
    // that the bundle itself carries.
    let calls = signed.borrow().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].last().unwrap().ends_with(".app.partial"));
    assert!(calls[0]
        .iter()
        .any(|a| a.ends_with("paddock-vm.entitlements")));

    // A failed signature leaves nothing: no partial and no bundle.
    let fail = |_: &[String]| anyhow::bail!("errSecInternalComponent");
    let e = make(
        &dir,
        &Face::session("claude-desktop", "Claude", None),
        &helper,
        "0.2.1",
        &fail,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("errSecInternalComponent"), "{e}");
    assert!(!dir.join("claude-desktop.app").exists());
    assert!(no_partials(&dir));
    // The bundle of the app that was made stayed.
    assert!(made.is_dir());

    // A second make over the first replaces it and leaves no partial.
    std::fs::write(made.join("Contents/stale"), b"x").unwrap();
    let again = make(&dir, &face, &helper, "0.3.0", &sign).unwrap();
    assert_eq!(again, made);
    assert!(!made.join("Contents/stale").exists());
    assert_eq!(built_by(&made).as_deref(), Some("0.3.0"));
    assert!(no_partials(&dir));

    // A missing helper is a refusal that names the install command.
    let e = make(&dir, &face, &t.path().join("gone"), "0.2.1", &sign)
        .unwrap_err()
        .to_string();
    assert!(e.contains("brew reinstall"), "{e}");
    assert!(no_partials(&dir));

    // remove takes the bundle and any partial of it.
    std::fs::create_dir_all(partial(&made)).unwrap();
    assert!(remove(&made).unwrap());
    assert!(!made.exists() && !partial(&made).exists());
    assert!(!remove(&made).unwrap(), "removing it twice is no error");
}

fn no_partials(dir: &Path) -> bool {
    super::super::layout::partials_in(dir).is_empty()
}

#[test]
fn launcher_and_drop_target_carry_their_argv() {
    let t = tempfile::tempdir().unwrap();
    let (helper, dir) = install(t.path());
    let sign = |_: &[String]| Ok(());
    let face = Face::launcher("chatgpt-desktop", "ChatGPT", None);
    let made = make(&dir, &face, &helper, "0.2.1", &sign).unwrap();
    let exe = executable(&made, "chatgpt-desktop");
    assert_eq!(
        argv_from_bundle(&exe, &[]),
        Some(vec![
            "install".to_string(),
            "--app".into(),
            "chatgpt-desktop".into()
        ])
    );
    // The process serial number that older Finder passes is not an argv.
    assert!(argv_from_bundle(&exe, &["-psn_0_1234".into()]).is_some());
    // A real argv wins: `paddock app run` starts the session this way.
    assert_eq!(argv_from_bundle(&exe, &["app".into()]), None);

    let drop = make(&dir, &Face::drop_target(), &helper, "0.2.1", &sign).unwrap();
    let exe = executable(&drop, "Paddock Apps");
    assert_eq!(
        argv_from_bundle(&exe, &[]),
        Some(vec!["install".to_string(), "--drop-target".into()])
    );
    // The session bundle has no target, so its own argv must say.
    let plain = make(
        &dir,
        &Face::session("vscode", "Visual Studio Code", None),
        &helper,
        "0.2.1",
        &sign,
    )
    .unwrap();
    assert_eq!(argv_from_bundle(&executable(&plain, "vscode"), &[]), None);
    // A path that is not inside a bundle answers nothing.
    assert_eq!(argv_from_bundle(Path::new("/usr/bin/true"), &[]), None);
}

#[test]
fn bundle_path_is_under_the_shims_folder() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    assert_eq!(
        bundle(&l, "chatgpt-desktop"),
        l.shims.join("chatgpt-desktop.app")
    );
    // A name with a separator cannot leave the folder.
    assert_eq!(bundle(&l, "../evil"), l.shims.join("_evil.app"));
    assert_eq!(bundle_id("a/b"), "dev.paddock.app.a_b");
    assert_eq!(paddock_name("Claude"), "Claude (paddock)");
}
