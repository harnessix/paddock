//! The paths, the names and the configuration write of the macOS
//! launchers. The bundles themselves need `codesign`, which only a Mac
//! has; `macvm::shim::tests` covers the build with a fake signer.

use super::*;

fn env(home: &Path) -> HostEnv {
    HostEnv::from_parts(home.into(), "me".into(), 501, 20, Default::default())
}

fn bundle_at(root: &Path, name: &str) -> PathBuf {
    let app = root.join(format!("{name}.app"));
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            "<plist><dict><key>CFBundleName</key><string>{name}</string><key>CFBundleShortVersionString</key><string>1.0</string></dict></plist>"
        ),
    )
    .unwrap();
    app
}

#[test]
fn launcher_paths_carry_the_dock_name() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    assert_eq!(applications_dir(&e), t.path().join("Applications"));
    assert_eq!(
        launcher_path(&e, "Sample Notes"),
        t.path().join("Applications/Sample Notes (paddock).app")
    );
    assert_eq!(
        launcher_path(&e, "Sample Mail"),
        t.path().join("Applications/Sample Mail (paddock).app")
    );
    assert_eq!(
        drop_path(&e),
        t.path().join("Applications/Paddock Apps.app")
    );
    // Removing one that is not there is no error.
    assert!(!uninstall(&e, "Sample Notes").unwrap());
    assert!(!uninstall_drop(&e).unwrap());
    // A bundle that is there goes, with any partial of it.
    let p = launcher_path(&e, "Sample Notes");
    std::fs::create_dir_all(p.join("Contents")).unwrap();
    assert!(uninstall(&e, "Sample Notes").unwrap());
    assert!(!p.exists());
}

#[test]
fn a_dropped_bundle_becomes_a_profile_in_the_user_configuration() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let app = bundle_at(t.path(), "Notebook");

    set_bundle(&e, "notebook", &app).unwrap();
    let file = e.paddock_config().join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("[apps.notebook]"), "{text}");
    assert!(
        text.contains(&format!("bundle = \"{}\"", app.display())),
        "{text}"
    );

    // A second app keeps the first entry.
    let other = bundle_at(t.path(), "Sketch");
    set_bundle(&e, "sketch", &other).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("[apps.notebook]") && text.contains("[apps.sketch]"),
        "{text}"
    );

    // The configuration is read back as the bundle of that profile.
    let m = crate::manifest::Manifest::load(&e, &e.home.join(".paddock-no-project")).unwrap();
    assert_eq!(
        m.merged.apps.get("notebook").and_then(|a| a.bundle.clone()),
        Some(app.display().to_string())
    );
    // The same path is found again, so a second drop reuses the profile.
    assert_eq!(existing_for(&e, &app).as_deref(), Some("notebook"));
    assert_eq!(existing_for(&e, &t.path().join("None.app")), None);

    // The slug avoids the built-in profiles and the entries of the user.
    let taken = taken_names(&e);
    assert!(taken.contains(&"notebook".to_string()), "{taken:?}");
    assert!(taken.contains(&"chatgpt-desktop".to_string()), "{taken:?}");
    assert!(taken.contains(&"vscode".to_string()), "{taken:?}");
    assert_eq!(
        crate::macvm::appcopy::slug("Notebook", &taken),
        "notebook-2"
    );
    assert_eq!(crate::macvm::appcopy::slug("Claude", &taken), "claude-2");

    // Something that is not a bundle is refused by name.
    let note = t.path().join("notes.txt");
    std::fs::write(&note, b"x").unwrap();
    let err = add_dropped(&e, &note).unwrap_err().to_string();
    assert!(err.contains("notes.txt") && err.contains(".app"), "{err}");

    // An App Store bundle is refused before anything is written.
    let mas = bundle_at(t.path(), "Sample Store");
    std::fs::create_dir_all(mas.join("Contents/_MASReceipt")).unwrap();
    std::fs::write(mas.join("Contents/_MASReceipt/receipt"), b"r").unwrap();
    let err = add_dropped(&e, &mas).unwrap_err().to_string();
    assert!(err.contains("App Store"), "{err}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(!text.contains("[apps.sample-store]"), "{text}");

    // A drop of a bundle that is already an app keeps its profile and
    // writes nothing new.
    assert_eq!(add_dropped(&e, &app).unwrap(), "notebook");
    let again = std::fs::read_to_string(&file).unwrap();
    assert_eq!(again.matches("[apps.notebook]").count(), 1, "{again}");
}

#[test]
fn a_broken_configuration_is_one_line_not_a_panic() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let file = e.paddock_config().join("config.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"apps = 3\n").unwrap();
    let err = set_bundle(&e, "notebook", Path::new("/Applications/Notebook.app"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("[apps]") && err.contains("not a table"),
        "{err}"
    );
}
