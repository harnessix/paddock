//! The parent's choices before a session: the bundle, the memory, the
//! staging of the clone and the Dock face.

use super::*;
use crate::macvm::shares::Share;

fn env(home: &Path) -> HostEnv {
    HostEnv::from_parts(home.into(), "me".into(), 501, 20, Default::default())
}

/// A bundle with a version, under `root`.
fn bundle_at(root: &Path, name: &str, version: &str) -> PathBuf {
    let app = root.join(format!("{name}.app"));
    std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
    std::fs::write(app.join("Contents/Resources/app.icns"), b"icns").unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            "<plist><dict><key>CFBundleName</key><string>{name}</string><key>CFBundleShortVersionString</key><string>{version}</string><key>CFBundleVersion</key><string>7</string><key>CFBundleIconFile</key><string>app</string></dict></plist>"
        ),
    )
    .unwrap();
    app
}

#[test]
fn the_bundle_comes_from_the_user_configuration_first() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let e = env(&home);
    // The absolute bundle entries resolve under this root, never under
    // the /Applications of the machine that runs the test.
    let root = t.path().join("root");
    let apps = home.join("Applications");
    std::fs::create_dir_all(&apps).unwrap();
    let mut profile = crate::profile::builtin("vscode").unwrap();

    // Nothing installed: the refusal names every path it tried.
    let err = find_bundle(&root, &e, "vscode", Some(&profile), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not installed"), "{err}");
    assert!(
        err.contains("/Applications/Visual Studio Code.app"),
        "{err}"
    );

    // The second candidate, under the home, is found through `~/`.
    bundle_at(&apps, "Visual Studio Code", "1.96.0");
    let found = find_bundle(&root, &e, "vscode", Some(&profile), None).unwrap();
    assert_eq!(found, apps.join("Visual Studio Code.app"));
    // The first, absolute candidate wins once it is there, under the root.
    let system = root.join("Applications");
    std::fs::create_dir_all(&system).unwrap();
    let first = bundle_at(&system, "Visual Studio Code", "1.97.0");
    let found = find_bundle(&root, &e, "vscode", Some(&profile), None).unwrap();
    assert_eq!(found, first);

    // `[apps.NAME] bundle` wins, which is how the drag target adds an app.
    let dropped = bundle_at(t.path(), "TextEdit", "1.19");
    let cfg = AppCfg {
        bundle: Some(dropped.display().to_string()),
        ..Default::default()
    };
    assert_eq!(
        find_bundle(&root, &e, "textedit", Some(&profile), Some(&cfg)).unwrap(),
        dropped
    );
    // A named bundle that is not there is named in the refusal.
    let gone = AppCfg {
        bundle: Some(t.path().join("Gone.app").display().to_string()),
        ..Default::default()
    };
    let err = find_bundle(&root, &e, "x", Some(&profile), Some(&gone))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Gone.app") && err.contains("not installed"),
        "{err}"
    );

    // A profile with no macOS section names what to add.
    profile.app.as_mut().unwrap().macos = None;
    let err = find_bundle(&root, &e, "vscode", Some(&profile), None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("[app.macos]") && err.contains("bundles"),
        "{err}"
    );
}

#[test]
fn an_app_store_bundle_is_refused_before_anything_is_cloned() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let e = env(&home);
    // The absolute bundle entries resolve under this root, never under
    // the /Applications of the machine that runs the test.
    let root = t.path().join("root");
    let apps = home.join("Applications");
    std::fs::create_dir_all(&apps).unwrap();
    let app = bundle_at(&apps, "Visual Studio Code", "1.96.0");
    let profile = crate::profile::builtin("vscode").unwrap();
    let b = read_bundle(&root, &e, "vscode", Some(&profile), None).unwrap();
    assert_eq!(b.version, "1.96.0");
    assert_eq!(b.display_name, "Visual Studio Code");

    std::fs::create_dir_all(app.join("Contents/_MASReceipt")).unwrap();
    std::fs::write(app.join("Contents/_MASReceipt/receipt"), b"r").unwrap();
    let err = read_bundle(&root, &e, "vscode", Some(&profile), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("App Store"), "{err}");
    assert!(err.contains("vendor's website"), "{err}");
}

#[test]
fn memory_from_the_profile_with_the_host_limit() {
    assert_eq!(memory_mib("x", None, Some(16)).unwrap(), 4096);
    let four = AppCfg {
        vm_memory: Some("4G".into()),
        ..Default::default()
    };
    assert_eq!(memory_mib("x", Some(&four), Some(16)).unwrap(), 4096);
    let six = AppCfg {
        vm_memory: Some("6144M".into()),
        ..Default::default()
    };
    assert_eq!(memory_mib("x", Some(&six), Some(16)).unwrap(), 6144);
    // More than half the Mac's memory is refused, with the line that
    // names the setting and the size to write.
    let twelve = AppCfg {
        vm_memory: Some("12G".into()),
        ..Default::default()
    };
    let err = memory_mib("chatgpt-desktop", Some(&twelve), Some(16))
        .unwrap_err()
        .to_string();
    assert!(err.contains("12 GiB") && err.contains("16 GiB"), "{err}");
    assert!(err.contains("vm_memory = \"8G\""), "{err}");
    assert!(err.contains("[apps.chatgpt-desktop]"), "{err}");
    // Without the host size paddock does not guess.
    assert_eq!(memory_mib("x", Some(&twelve), None).unwrap(), 12288);
    // Not a size, and out of the range the helper takes.
    let bad = AppCfg {
        vm_memory: Some("lots".into()),
        ..Default::default()
    };
    let err = memory_mib("x", Some(&bad), None).unwrap_err().to_string();
    assert!(err.contains("vm_memory") && err.contains("4G"), "{err}");
    let tiny = AppCfg {
        vm_memory: Some("64M".into()),
        ..Default::default()
    };
    let err = memory_mib("x", Some(&tiny), None).unwrap_err().to_string();
    assert!(err.contains("1024 to 262144"), "{err}");
}

#[test]
fn staging_clones_once_per_version_and_the_face_is_rebuilt() {
    let t = tempfile::tempdir().unwrap();
    let app = bundle_at(t.path(), "ChatGPT", "1.2026.300");
    let b = appcopy::read(&app).unwrap();
    let profile = Profile::new(t.path().join("profile"));

    // A fresh profile has no app stamp: the clone happens.
    assert!(stage_bundle_with(&profile, &b, &fake_archive).unwrap());
    let clone = profile.apps().join("ChatGPT.app");
    assert!(clone.join("Contents/Info.plist").is_file());

    // With the version in the stamp nothing is cloned again.
    let now = crate::macvm::session::Now {
        golden: "15.2 (24C101)".into(),
        host_build: "25A1".into(),
        vz: "1".into(),
        protocol: crate::macvm::proto::VERSION,
        paddock: "0.2.1".into(),
        agent: "0.2.1".into(),
        display: crate::macvm::session::Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    };
    let mut stamp = now.stamp("abc-0".into());
    stamp.app = Some(b.stamp());
    std::fs::create_dir_all(&profile.dir).unwrap();
    stamp.write(&profile.stamp()).unwrap();
    assert!(!stage_bundle_with(&profile, &b, &fake_archive).unwrap());

    // A new version of the app stages again.
    let app = bundle_at(t.path(), "ChatGPT", "1.2026.301");
    let next = appcopy::read(&app).unwrap();
    assert!(stage_bundle_with(&profile, &next, &fake_archive).unwrap());
    assert!(crate::macvm::session::partials(&profile).is_empty());

    // The Dock face: built once, then left alone, then rebuilt for a new
    // paddock version. `codesign` is not on Linux, so the test signs
    // through shim::make itself.
    let layout = Layout::at(t.path().join("macvm"));
    let helper = t.path().join("paddock-vm");
    std::fs::write(&helper, b"helper").unwrap();
    crate::privfs::dir(&layout.shims).unwrap();
    let face = Face::session("chatgpt-desktop", &next.display_name, next.icon.clone());
    let made = shim::make(&layout.shims, &face, &helper, "0.2.1", &|_| Ok(())).unwrap();
    assert_eq!(made, shim::bundle(&layout, "chatgpt-desktop"));
    assert!(!shim::needs_rebuild(&made, "0.2.1", &helper));
    assert!(shim::needs_rebuild(&made, "0.2.2", &helper));
    // A rebuilt helper of the same version makes a new shim.
    std::fs::write(&helper, b"helper, rebuilt").unwrap();
    assert!(shim::needs_rebuild(&made, "0.2.1", &helper));
    assert_eq!(
        shim::executable(&made, "chatgpt-desktop"),
        made.join("Contents/MacOS/chatgpt-desktop")
    );
}

#[test]
fn an_editor_gets_the_guest_paths_of_its_folders() {
    let ws = Share {
        name: "ws-1".into(),
        host: PathBuf::from("/Users/me/src/web"),
        read_only: false,
    };
    let own = Share {
        name: "downloads".into(),
        host: PathBuf::from("/p/downloads"),
        read_only: false,
    };
    let shares = vec![ws, own];
    let vscode = crate::profile::builtin("vscode").unwrap();
    assert_eq!(
        app_args(Some(&vscode), &shares),
        vec!["/Volumes/My Shared Files/ws-1".to_string()],
        "an editor opens its folders, and never paddock's own shares"
    );
    // A chat app gets no folder argument.
    let chat = crate::profile::builtin("chatgpt-desktop").unwrap();
    assert!(app_args(Some(&chat), &shares).is_empty());
}

#[test]
fn codesign_failure_names_the_bundle_and_the_reason() {
    // The real codesign is macOS only. The argv it gets is the one the
    // shim builds, and the failure text comes from text.rs.
    let argv = shim::sign_argv(
        Path::new("/d/ChatGPT.app.partial"),
        Path::new("/d/ChatGPT.app.partial/Contents/Resources/paddock-vm.entitlements"),
    );
    assert_eq!(argv.last().unwrap(), "/d/ChatGPT.app.partial");
    let line = text::sign_failed(
        Path::new("/d/ChatGPT.app.partial"),
        "errSecInternalComponent",
    );
    assert!(line.contains("ChatGPT.app.partial"), "{line}");
    assert!(line.contains("errSecInternalComponent"), "{line}");
    assert_eq!(CODESIGN, "/usr/bin/codesign");
}

/// A session name that no profile and no configuration names an app for
/// boots the guest and starts nothing. The Mac check scripts
/// `macvm-session.sh`, `macvm-windows.sh` and `macvm-network.sh` drive such
/// a session through the control socket.
#[test]
fn a_session_without_an_app_has_no_bundle() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let e = env(&home);
    // The absolute bundle entries resolve under this root, never under
    // the /Applications of the machine that runs the test.
    let root = t.path().join("root");
    assert_eq!(optional_bundle(&root, &e, "win", None, None).unwrap(), None);
    // A profile with no [app.macos] is the same.
    let mut bare = crate::profile::builtin("vscode").unwrap();
    bare.app.as_mut().unwrap().macos = None;
    assert_eq!(
        optional_bundle(&root, &e, "vscode", Some(&bare), None).unwrap(),
        None
    );
    // A profile with a bundles list that is not installed is an error,
    // never a session without an app.
    let vscode = crate::profile::builtin("vscode").unwrap();
    let err = optional_bundle(&root, &e, "vscode", Some(&vscode), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not installed"), "{err}");
    // A named bundle that is missing is an error too.
    let gone = AppCfg {
        bundle: Some(t.path().join("Gone.app").display().to_string()),
        ..Default::default()
    };
    assert!(optional_bundle(&root, &e, "win", None, Some(&gone)).is_err());
    // A named bundle that is there gives the app.
    let app = bundle_at(t.path(), "TextEdit", "1.19");
    let named = AppCfg {
        bundle: Some(app.display().to_string()),
        ..Default::default()
    };
    let b = optional_bundle(&root, &e, "textedit", None, Some(&named))
        .unwrap()
        .unwrap();
    assert_eq!(b.display_name, "TextEdit");
    // The Dock face of a session without an app uses the profile name.
    let layout = Layout::at(t.path().join("macvm"));
    crate::privfs::dir(&layout.shims).unwrap();
    let helper = t.path().join("paddock-vm");
    std::fs::write(&helper, b"helper").unwrap();
    let face = Face::session("win", "win", None);
    assert_eq!(face.display_name, "win (paddock)");
    let made = shim::make(&layout.shims, &face, &helper, "0.2.1", &|_| Ok(())).unwrap();
    assert_eq!(made, shim::bundle(&layout, "win"));
    // No app arguments without a profile.
    assert!(app_args(None, &[]).is_empty());
}

fn fake_archive(_src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(dst, b"archive")
}
