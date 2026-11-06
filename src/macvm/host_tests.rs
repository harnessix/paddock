//! The parent of a session: the sweep of dead records, the golden check,
//! the names and the argv that the shim gets.

use super::*;

fn env(t: &Path) -> HostEnv {
    HostEnv::from_parts(t.into(), "u".into(), 501, 20, Default::default())
}

fn v_no_app(l: &Layout, args: &StartArgs) -> Vec<String> {
    command_args(l, args, Path::new("/c.sock"), Path::new("/r.json"))
}

fn record(dir: &Path, name: &str, pid: u32, profile: &Path, input: &str) -> PathBuf {
    let s = Session {
        pid,
        app: name.into(),
        home: DEFAULT_HOME.into(),
        profile: profile.to_path_buf(),
        slot: Some(0),
        input_source: input.into(),
        started: 1,
        bench: false,
        control: "/c.sock".into(),
        badge: None,
        health: None,
        start: None,
        app_info: None,
        network: None,
        network_mode: String::new(),
        shares: vec![],
        menu: None,
        ax: None,
        ..Default::default()
    };
    let p = dir.join(format!("{name}--default.json"));
    std::fs::write(&p, serde_json::to_vec(&s).unwrap()).unwrap();
    p
}

#[test]
fn sweep_removes_dead_record() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let l = Layout::new(&e);
    std::fs::create_dir_all(&l.sessions).unwrap();
    let live_profile = l.profile("live", DEFAULT_HOME);
    let dead_profile = l.profile("dead", DEFAULT_HOME);
    std::fs::create_dir_all(&dead_profile.dir).unwrap();
    let live = record(
        &l.sessions,
        "live",
        100,
        &live_profile.dir,
        "com.apple.keylayout.US",
    );
    let dead = record(
        &l.sessions,
        "dead",
        200,
        &dead_profile.dir,
        "com.apple.keylayout.German",
    );
    let mut restored = Vec::new();
    let gone = sweep(&l.sessions, &|pid| pid == 100, &mut |src| {
        restored.push(src.to_string())
    });
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].app, "dead");
    assert!(live.exists());
    assert!(!dead.exists());
    assert_eq!(restored, vec!["com.apple.keylayout.German"]);
    let events = std::fs::read_to_string(dead_profile.events_log()).unwrap();
    assert!(events.contains("\"kind\":\"crashed\""), "{events}");
    assert!(events.contains("session ended without a save"));
    assert_eq!(last_end(&dead_profile), SessionEnd::NeverStarted);
    // A second sweep finds nothing.
    assert!(sweep(&l.sessions, &|pid| pid == 100, &mut |_| {}).is_empty());
    assert_eq!(sessions(&l.sessions).len(), 1);
    // The list names the stopped profile with its end.
    assert_eq!(
        list_lines(&e),
        vec!["dead             macOS guest, stopped (never started)"]
    );
}

#[test]
fn sealed_golden_and_names() {
    let t = tempfile::tempdir().unwrap();
    let g = Golden::new(t.path().join("golden.noindex"));
    let e = sealed(&g, "ChatGPT").unwrap_err().to_string();
    assert!(e.contains("paddock macvm setup"), "{e}");
    let mut m = super::super::golden::Manifest::new("15.2", "24C101", "0.2.1");
    m.version = "15.2".into();
    m.build = "24C101".into();
    m.at(State::Guided);
    g.write_manifest(&m).unwrap();
    let e = sealed(&g, "ChatGPT").unwrap_err().to_string();
    assert!(
        e.contains("did not finish") && e.contains("paddock macvm setup"),
        "{e}"
    );
    m.at(State::Sealed);
    g.write_manifest(&m).unwrap();
    assert_eq!(sealed(&g, "ChatGPT").unwrap(), "15.2 (24C101)");
    assert!(valid_name("chatgpt-desktop"));
    assert!(!valid_name("../x") && !valid_name("") && !valid_name("a b"));
    let l = Layout::new(&env(t.path()));
    assert!(l.golden.dir.ends_with("macvm/golden.noindex"));
    assert!(l.profile("a", "b").dir.ends_with("profiles.noindex/a/b"));
    assert_eq!(l.record("a", "b").file_name().unwrap(), "a--b.json");
    let args = StartArgs {
        app: "a".into(),
        home: "b".into(),
        shares: vec![("src".into(), "/home/demo/src".into(), false)],
        display: "2560x1600@2".into(),
        cpus: 4,
        memory_mib: 4096,
        save: false,
        token: "t".into(),
        password: None,
        input_source: "com.apple.keylayout.US".into(),
        agent: None,
        app_bundle: None,
        app_unquarantine: false,
        app_args: vec![],
        network: "ask".into(),
        bench: true,
        allow: vec!["api.example".into()],
    };
    let v = command_args(&l, &args, Path::new("/c.sock"), Path::new("/r.json"));
    assert_eq!(v[0], "app");
    assert!(
        v.windows(2).any(|w| w == ["--allow", "api.example"]),
        "{v:?}"
    );
    assert!(v.contains(&"--no-save".to_string()));
    assert!(v.contains(&"src=/home/demo/src".to_string()));
    assert!(v.contains(&"com.apple.keylayout.US".to_string()));
    assert!(!v.contains(&"--agent".to_string()));
    // With the agent of the install, the share is agent/<version>/.
    let install = t.path().join("bin");
    std::fs::create_dir_all(&install).unwrap();
    assert_eq!(agent_share(&l, &install.join("paddock-vm")), None);
    std::fs::write(install.join("paddock-guest"), b"agent").unwrap();
    let dir = agent_share(&l, &install.join("paddock-vm")).unwrap();
    assert_eq!(dir, l.agent_dir(super::super::version::paddock_version()));
    assert!(dir.join("paddock-guest.sha256").is_file());
    let mut with = args.clone();
    with.agent = Some(dir.clone());
    with.app_bundle = Some(PathBuf::from("/p/apps/ChatGPT.app"));
    with.app_unquarantine = true;
    with.app_args = vec!["/Volumes/My Shared Files/ws-1".into()];
    let v = command_args(&l, &with, Path::new("/c.sock"), Path::new("/r.json"));
    let at = v.iter().position(|a| a == "--agent").unwrap();
    assert_eq!(v[at + 1], dir.display().to_string());
    // The app bundle, the quarantine decision, the app arguments and
    // the network mode reach the shim.
    let at = v.iter().position(|a| a == "--app-bundle").unwrap();
    assert_eq!(v[at + 1], "/p/apps/ChatGPT.app");
    assert!(v.contains(&"--app-unquarantine".to_string()));
    let at = v.iter().position(|a| a == "--app-arg").unwrap();
    assert_eq!(v[at + 1], "/Volumes/My Shared Files/ws-1");
    let at = v.iter().position(|a| a == "--network").unwrap();
    assert_eq!(v[at + 1], "ask");
    // Without a bundle the shim gets no app flags at all.
    assert!(!v_no_app(&l, &args).contains(&"--app-bundle".to_string()));
    assert!(!v_no_app(&l, &args).contains(&"--app-unquarantine".to_string()));
    // The shim parses what the parent makes.
    let mut argv = vec!["paddock-vm".to_string()];
    argv.extend(v);
    let cli = <crate::vm::helper::Cli as clap::Parser>::try_parse_from(&argv).unwrap();
    let crate::vm::helper::Cmd::App(a) = cli.cmd else {
        panic!()
    };
    assert!(!a.save);
    assert_eq!(a.token_fd, 3);
    assert_eq!(a.shares.len(), 1);
    assert_eq!(tail(Path::new("/nonexistent"), 3), Vec::<String>::new());
}
