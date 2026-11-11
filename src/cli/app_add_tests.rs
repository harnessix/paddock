use super::*;

const GIB: u64 = 1 << 30;

fn env(t: &Path) -> HostEnv {
    HostEnv::from_parts(
        t.join("home"),
        "demo".into(),
        501,
        20,
        [
            (
                "XDG_DATA_HOME".to_string(),
                t.join("data").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                t.join("config").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                t.join("state").display().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR".to_string(),
                t.join("run").display().to_string(),
            ),
            ("PADDOCK_MACVM_FAKE_FREE".to_string(), "100G".to_string()),
        ]
        .into_iter()
        .collect(),
    )
}

/// A bundle of `name` under `dir`, with a version and an icon.
fn bundle(dir: &Path, name: &str, mas: bool) -> PathBuf {
    let app = dir.join(format!("{name}.app"));
    let c = app.join("Contents");
    std::fs::create_dir_all(c.join("Resources")).unwrap();
    std::fs::write(
        c.join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundleShortVersionString</key><string>1.2.3</string>
<key>CFBundleVersion</key><string>123</string>
<key>CFBundleIdentifier</key><string>com.example.{name}</string>
<key>CFBundleIconFile</key><string>icon</string>
</dict></plist>"#
        ),
    )
    .unwrap();
    std::fs::write(c.join("Resources/icon.icns"), "icns").unwrap();
    std::fs::create_dir_all(c.join("MacOS")).unwrap();
    std::fs::write(c.join("MacOS").join(name), vec![0u8; 4096]).unwrap();
    if mas {
        std::fs::write(c.join("_MASReceipt"), "r").unwrap();
    }
    app
}

fn seal(layout: &Layout) {
    let g = &layout.golden;
    std::fs::create_dir_all(&g.dir).unwrap();
    std::fs::write(&g.disk, "disk").unwrap();
    let mut m = golden::Manifest::new("50.2", "99B102", "0.9.0");
    m.version = "50.1".into();
    m.build = "99A101".into();
    m.at(golden::State::Sealed);
    g.write_manifest(&m).unwrap();
}

fn args(path: &Path) -> AddArgs {
    AddArgs {
        path: path.to_path_buf(),
        name: None,
        memory: None,
        dry_run: true,
        json: true,
    }
}

fn host(free: u64) -> Host {
    Host {
        free,
        host_gib: Some(16),
        taken: vec!["notebook".into()],
        existing: None,
    }
}

fn codes(p: &Precheck) -> Vec<&'static str> {
    p.refusals.iter().map(|r| r.code).collect()
}

/// The precheck JSON as the launcher reads it: every field, through text.
#[test]
fn a_bundle_that_passes() {
    let t = tempfile::tempdir().unwrap();
    let layout = Layout::at(t.path().join("macvm"));
    seal(&layout);
    let app = bundle(t.path(), "Notebook", false);
    let p = precheck(&layout, &args(&app), &host(100 * GIB));
    assert!(p.ok, "{:?}", p.refusals);
    let j: serde_json::Value = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(j["v"], 1);
    assert_eq!(j["name"], "notebook-2", "notebook is taken");
    assert_eq!(j["display_name"], "Notebook");
    assert_eq!(j["version"], "1.2.3");
    assert_eq!(j["build"], "123");
    assert_eq!(j["bundle_id"], "com.example.Notebook");
    assert!(j["icon"].as_str().unwrap().ends_with("icon.icns"));
    assert!(j["bytes"].as_u64().unwrap() >= 4096);
    assert_eq!(j["app_store"], false);
    assert_eq!(j["golden"], "sealed");
    assert_eq!(j["space"]["free"], 100 * GIB);
    assert_eq!(
        j["space"]["need"].as_u64().unwrap(),
        8 * GIB + p.bytes.unwrap()
    );
    assert_eq!(j["space"]["ok"], true);
    assert_eq!(j["memory"]["mib"], 4096);
    assert_eq!(j["memory"]["max_mib"], 8192);
    assert_eq!(j["refusals"], serde_json::json!([]));
    assert_eq!(j["ok"], true);
    assert_eq!(j["dry_run"], true);
    assert_eq!(j["added"], false);
}

#[test]
fn every_refusal_is_a_coded_field() {
    let t = tempfile::tempdir().unwrap();
    let layout = Layout::at(t.path().join("macvm"));
    // No golden, an App Store app, little space, a taken name, too much
    // memory.
    let app = bundle(t.path(), "Sample Store", true);
    let mut a = args(&app);
    a.name = Some("notebook".into());
    a.memory = Some("12G".into());
    let p = precheck(&layout, &a, &host(GIB));
    assert!(!p.ok);
    assert!(p.app_store);
    assert_eq!(
        codes(&p),
        [
            "app_store",
            "name_taken",
            "bad_memory",
            "no_golden",
            "low_space"
        ]
    );
    assert!(p.refusals.iter().all(|r| !r.text.is_empty()));
    assert_eq!(p.golden, "missing");
    // Not a bundle, a bad name.
    let mut a = args(&t.path().join("nothing.app"));
    a.name = Some("../x".into());
    let p = precheck(&layout, &a, &host(100 * GIB));
    assert_eq!(codes(&p), ["not_a_bundle", "bad_name", "no_golden"]);
    assert_eq!(p.display_name, None);
}

#[test]
fn a_bundle_that_is_a_profile_already_keeps_its_name() {
    let t = tempfile::tempdir().unwrap();
    let layout = Layout::at(t.path().join("macvm"));
    seal(&layout);
    let app = bundle(t.path(), "Notebook", false);
    let mut h = host(100 * GIB);
    h.existing = Some("notebook".into());
    let p = precheck(&layout, &args(&app), &h);
    assert!(p.ok, "{:?}", p.refusals);
    assert_eq!(p.name.as_deref(), Some("notebook"));
    assert_eq!(p.existing.as_deref(), Some("notebook"));
}

#[test]
fn add_writes_the_profile_and_remove_takes_it_away() {
    let t = tempfile::tempdir().unwrap();
    let env = env(t.path());
    let layout = Layout::new(&env);
    seal(&layout);
    let app = bundle(t.path(), "Zed Notes", false);
    // The dry run changes nothing.
    let mut a = args(&app);
    assert_eq!(add(&env, a.clone()).unwrap(), 0);
    assert!(existing_bundle(&env, "zed-notes").is_none());
    a.dry_run = false;
    a.memory = Some("6G".into());
    assert_eq!(add(&env, a.clone()).unwrap(), 0);
    assert_eq!(existing_bundle(&env, "zed-notes"), Some(app.clone()));
    let cfg = std::fs::read_to_string(config_file(&env)).unwrap();
    assert!(
        cfg.contains("[apps.zed-notes]") && cfg.contains("vm_memory = \"6G\""),
        "{cfg}"
    );
    // A second add finds it.
    let p = precheck(&layout, &a, &host_of(&env, &app));
    assert_eq!(p.existing.as_deref(), Some("zed-notes"));
    // A profile folder after a session: state, app copy and the disks.
    let profile = layout.profile("zed-notes", host::DEFAULT_HOME);
    std::fs::create_dir_all(profile.apps()).unwrap();
    std::fs::write(profile.state(), vec![1u8; 64 << 10]).unwrap();
    std::fs::write(profile.apps().join("copy"), vec![1u8; 32 << 10]).unwrap();
    std::fs::write(profile.data(), vec![1u8; 16 << 10]).unwrap();
    let rm = RemoveArgs {
        name: "zed-notes".into(),
        delete_data: false,
        yes: true,
        json: true,
    };
    let r = remove_inner(&env, &rm).unwrap().unwrap();
    assert!(r.freed >= 96 << 10, "{r:?}");
    assert!(r.trash_bytes >= 16 << 10, "{r:?}");
    let trash = r.trash.clone().unwrap();
    assert!(
        trash.join("data.img").is_file(),
        "the data disk is in the trash"
    );
    assert!(!trash.join("state.vzs").exists());
    assert!(!profile.dir.exists());
    assert!(r.config);
    assert!(existing_bundle(&env, "zed-notes").is_none());
    let cfg = std::fs::read_to_string(config_file(&env)).unwrap();
    assert!(!cfg.contains("zed-notes"), "{cfg}");
    let j: serde_json::Value = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(j["v"], 1);
    assert_eq!(j["name"], "zed-notes");
    // Gone: a second remove says so.
    let e = remove_inner(&env, &rm).unwrap_err().to_string();
    assert!(e.contains("No app zed-notes"), "{e}");
    assert_eq!(remove(&env, rm).unwrap(), 1);
}

#[test]
fn remove_with_delete_data_frees_everything() {
    let t = tempfile::tempdir().unwrap();
    let env = env(t.path());
    let layout = Layout::new(&env);
    let profile = layout.profile("notes", host::DEFAULT_HOME);
    std::fs::create_dir_all(&profile.dir).unwrap();
    std::fs::write(profile.data(), vec![1u8; 64 << 10]).unwrap();
    let before = golden::used_bytes_in(&profile.dir);
    let (freed, trash, kept) = remove_profile(&env, &layout, "notes", true).unwrap();
    assert_eq!((freed, trash, kept), (before, None, 0));
    assert!(!profile.dir.exists());
    let left: Vec<_> = std::fs::read_dir(&layout.trash)
        .map(|r| r.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "nothing waits in the trash: {left:?}");
}

#[test]
fn add_json_returns_failure_when_configuration_cannot_be_written() {
    let t = tempfile::tempdir().unwrap();
    let env = env(t.path());
    seal(&Layout::new(&env));
    let app = bundle(t.path(), "Broken", false);
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(env.paddock_config().join("config.toml"), "apps = 1\n").unwrap();
    let mut a = args(&app);
    a.dry_run = false;
    assert_eq!(add(&env, a).unwrap(), 1);
}
