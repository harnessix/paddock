use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn sealed_layout(t: &Path) -> Layout {
    let l = Layout::at(t.join("macvm"));
    std::fs::create_dir_all(&l.golden.dir).unwrap();
    std::fs::write(&l.golden.disk, vec![1u8; 4096]).unwrap();
    std::fs::write(&l.golden.aux, vec![1u8; 4096]).unwrap();
    layout::seal_file(&l.golden.disk).unwrap();
    layout::seal_file(&l.golden.aux).unwrap();
    let mut m = Manifest::new("15.2", "24C101", "0.2.1");
    m.version = "15.2".into();
    m.build = "24C101".into();
    m.sealed = Some("2026-11-01T10:00:00Z".into());
    m.at(State::Sealed);
    l.golden.write_manifest(&m).unwrap();
    l
}

#[test]
fn views_of_missing_unsealed_sealed_and_unreadable() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    assert_eq!(GoldenView::read(&l), GoldenView::Missing);
    let lines = status_lines(&l, Some("15.2"), "u");
    assert!(
        lines[1].contains("none") && lines[1].contains("paddock macvm setup"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|x| x.contains("host     macOS 15.2")));

    let mut m = Manifest::new("15.2", "24C101", "0.2.1");
    m.version = "15.2".into();
    m.at(State::Guided);
    l.golden.write_manifest(&m).unwrap();
    assert!(matches!(GoldenView::read(&l), GoldenView::Unsealed(_)));
    let lines = status_lines(&l, None, "u");
    assert!(lines[1].contains("step 4 (guided)"), "{lines:?}");

    let l = sealed_layout(t.path());
    match GoldenView::read(&l) {
        GoldenView::Sealed { size, flags_ok, .. } => {
            assert!(size >= 8192);
            assert!(flags_ok);
        }
        v => panic!("{v:?}"),
    }
    let lines = status_lines(&l, Some("15.3"), "u");
    assert!(
        lines[1].contains("sealed 2026-11-01T10:00:00Z"),
        "{lines:?}"
    );
    assert!(
        lines[2].contains("older than the host") && lines[2].contains("paddock macvm update"),
        "{lines:?}"
    );
    let j = status_json(&l, Some("15.3"), "u");
    assert_eq!(j["golden"]["state"], "sealed");
    assert_eq!(j["golden"]["flags_ok"], true);

    // A lost seal. On a Mac, `chmod` of a `uchg` file fails with EPERM,
    // so the flag goes first, as it does when a user clears it.
    layout::unseal_file(&l.golden.disk).unwrap();
    std::fs::set_permissions(&l.golden.disk, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(status_lines(&l, None, "u")[1].contains("flags are missing"));

    std::fs::write(&l.golden.manifest, "schema = 7\n").unwrap();
    assert!(matches!(GoldenView::read(&l), GoldenView::Unreadable(_)));
    assert!(status_lines(&l, None, "u")[1].contains("paddock macvm remove"));
    // The temporary folder goes only without the flag.
    layout::unseal_file(&l.golden.aux).unwrap();
}

#[test]
fn sessions_are_read_from_records_with_a_live_pid() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    std::fs::create_dir_all(&l.sessions).unwrap();
    let me = std::process::id();
    let record = |pid: u32| {
        serde_json::to_string(&super::host::Session {
            pid,
            app: "vscode".into(),
            home: "default".into(),
            profile: PathBuf::from("vscode/default"),
            slot: None,
            input_source: String::new(),
            started: 1,
            control: PathBuf::from("/c.sock"),
            bench: false,
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
        })
        .unwrap()
    };
    std::fs::write(l.sessions.join("a1.json"), record(me)).unwrap();
    std::fs::write(l.sessions.join("dead.json"), record(999_999_999)).unwrap();
    std::fs::write(l.sessions.join("junk.json"), "not json").unwrap();
    std::fs::write(l.sessions.join("note.txt"), record(me)).unwrap();
    let s = running_sessions(&l);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, "a1");
    assert_eq!(s[0].1.pid, me);
    assert_eq!(s[0].1.profile, PathBuf::from("vscode/default"));
    let e = refuse_while_running(&l).unwrap_err().to_string();
    assert!(
        e.contains(&format!("pid {me}")) && e.contains("paddock macvm remove"),
        "{e}"
    );
    assert!(status_lines(&l, None, "u")
        .iter()
        .any(|x| x.contains("session  a1")));
    assert!(remove_golden(&l, None).is_err());
    assert!(remove_profiles(&l).is_err());
}

#[test]
fn remove_golden_unseals_and_removes_the_images_and_the_password() {
    let t = tempfile::tempdir().unwrap();
    let env = crate::privfs::tests::env_in(t.path());
    let l = sealed_layout(t.path());
    std::fs::create_dir_all(&l.ipsw).unwrap();
    std::fs::write(l.ipsw_file("15.2"), "x").unwrap();
    std::fs::create_dir_all(&l.golden_prev.dir).unwrap();
    std::fs::write(&l.golden_prev.disk, "old").unwrap();
    layout::seal_file(&l.golden_prev.disk).unwrap();
    std::fs::create_dir_all(l.profile("vscode", "default").dir).unwrap();
    let k = golden::Keyring::open(&env).unwrap();
    let pw = k.password().unwrap();
    let plan = RemovePlan::read(&l);
    assert_eq!(plan.golden.as_ref().unwrap().0, "15.2 (24C101)");
    assert!(plan.golden.as_ref().unwrap().1 >= 8192);
    remove_golden(&l, Some(&k)).unwrap();
    assert!(!l.golden.dir.exists() && !l.golden_prev.dir.exists() && !l.ipsw.exists());
    assert!(!k.guest_password.exists());
    assert_ne!(k.password().unwrap(), pw);
    assert!(
        l.profile("vscode", "default").dir.is_dir(),
        "the profiles stay"
    );
    assert_eq!(RemovePlan::read(&l).golden, None);
}

#[test]
fn remove_profiles_moves_each_profile_into_the_trash() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    let p = l.profile("chatgpt-desktop", "default").dir;
    std::fs::create_dir_all(&p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(p.join("data.img"), "the login keychain").unwrap();
    std::fs::write(p.join("system.img"), "clone").unwrap();
    layout::seal_file(&p.join("system.img")).unwrap();
    let q = l.profile("vscode", "work").dir;
    std::fs::create_dir_all(&q).unwrap();
    let plan = RemovePlan::read(&l);
    assert_eq!(plan.profiles.len(), 2);
    assert!(plan.profiles_bytes() > 0);
    let made = remove_profiles(&l).unwrap();
    assert_eq!(made.len(), 2, "every profile folder moves, whole");
    assert!(made[0].starts_with(&l.trash));
    assert!(made[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("chatgpt-desktop-default-"));
    // The pair stays together in the trash: the system disk with its data.
    assert_eq!(
        std::fs::read_to_string(made[0].join("data.img")).unwrap(),
        "the login keychain"
    );
    assert_eq!(
        std::fs::read_to_string(made[0].join("system.img")).unwrap(),
        "clone"
    );
    assert_eq!(
        std::fs::metadata(&made[0]).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(layout::partials_in(&made[0]).is_empty());
    assert!(!p.exists() && !q.exists() && !l.profiles.exists());
    // `paddock gc` lists a trash entry after 7 days and removes it, with
    // its sealed file; another path is refused.
    assert!(gc_items_at(&l, 0).is_empty());
    let entries = trash_entries(&l);
    assert_eq!(entries.len(), 2);
    let items = gc_items_at(&l, 0);
    assert!(items.is_empty(), "the folders are new");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
    let f = std::fs::File::open(&made[0]).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    let items = gc_items_at(&l, 0);
    assert_eq!(items.len(), 1);
    assert!(items[0].what.contains("chatgpt-desktop-default-"));
    assert!(gc_remove_in(&l, &t.path().join("elsewhere")).is_err());
    gc_remove_in(&l, &made[0]).unwrap();
    assert!(!made[0].exists() && made[1].exists());
}

#[test]
fn gc_lists_the_backups_above_the_retention() {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    let p = l.profile("chatgpt-desktop", "default");
    std::fs::create_dir_all(p.backups()).unwrap();
    for (i, name) in [
        "pre-repair-1",
        "pre-repair-2",
        "pre-repair-3",
        "pre-repair-4",
        "manual-1",
    ]
    .iter()
    .enumerate()
    {
        let d = p.backups().join(name);
        std::fs::create_dir_all(&d).unwrap();
        let meta = super::super::backup::Meta {
            reason: name.split('-').next().unwrap().into(),
            time: 1000 + i as u64,
            manual: name.starts_with("manual"),
            bytes: 7,
        };
        std::fs::write(d.join("backup.json"), serde_json::to_string(&meta).unwrap()).unwrap();
    }
    let items = gc_items_at(&l, 2000);
    assert_eq!(items.len(), 1);
    assert!(items[0].what.contains("pre-repair-1") && items[0].what.contains("chatgpt-desktop"));
    let dir = match &items[0].action {
        crate::storage::Action::Macvm { dir } => dir.clone(),
        other => panic!("{other:?}"),
    };
    gc_remove_in(&l, &dir).unwrap();
    assert!(!dir.exists() && p.backups().join("manual-1").exists());
    assert!(gc_remove_in(&l, &p.backups().join("..")).is_err());
}

#[test]
fn clone_or_copy_makes_a_private_copy() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("a");
    let b = t.path().join("b");
    std::fs::write(&a, "abc").unwrap();
    std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o644)).unwrap();
    clone_or_copy(&a, &b).unwrap();
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "abc");
    assert_eq!(
        std::fs::metadata(&b).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn status_names_prev_and_gc_lists_it_once_a_profile_ran_clean_on_the_new_golden() {
    use super::super::golden::{prev_state, Prev};
    use super::super::proto::Version;
    use super::super::session::{Display, Now};
    let t = tempfile::tempdir().unwrap();
    let l = sealed_layout(t.path());
    let env = crate::privfs::tests::env_in(t.path());
    // The layout of the env is elsewhere: point it at ours.
    let env_layout = Layout::new(&env);
    std::fs::create_dir_all(env_layout.root.parent().unwrap()).unwrap();
    std::fs::rename(&l.root, &env_layout.root).unwrap();
    let l = env_layout;
    assert!(!status_lines(&l, None, "u")
        .iter()
        .any(|x| x.contains("prev")));
    assert!(gc_items(&env, &Default::default()).is_empty());
    // A previous golden, kept: the status says so, gc lists nothing.
    std::fs::create_dir_all(&l.golden_prev.dir).unwrap();
    std::fs::write(&l.golden_prev.disk, vec![2u8; 8192]).unwrap();
    layout::seal_file(&l.golden_prev.disk).unwrap();
    let mut m = Manifest::new("15.1", "24B1", "0.2.0");
    m.version = "15.1".into();
    m.build = "24B1".into();
    m.at(State::Sealed);
    l.golden_prev.write_manifest(&m).unwrap();
    let lines = status_lines(&l, None, "u");
    let prev = lines.iter().find(|x| x.starts_with("  prev")).unwrap();
    assert!(
        prev.contains("macOS 15.1 (24B1)") && prev.contains("until a profile"),
        "{prev}"
    );
    assert_eq!(status_json(&l, None, "u")["prev"]["removable"], false);
    assert!(gc_items(&env, &Default::default()).is_empty());
    // A clean session on the new golden: removable, and gc lists it.
    let now = Now {
        golden: "15.2 (24C101)".into(),
        host_build: "24C101".into(),
        vz: "vz".into(),
        protocol: Version { major: 1, minor: 0 },
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    };
    let p = l.profile("chatgpt", "default");
    std::fs::create_dir_all(&p.dir).unwrap();
    now.stamp("x-0".into()).write(&p.stamp()).unwrap();
    assert!(matches!(prev_state(&l), Prev::Removable { .. }));
    let lines = status_lines(&l, None, "u");
    assert!(
        lines
            .iter()
            .any(|x| x.starts_with("  prev") && x.contains("paddock gc")),
        "{lines:?}"
    );
    assert_eq!(status_json(&l, None, "u")["prev"]["removable"], true);
    // An old agent share is an item too; this version's is not.
    let current = super::super::version::paddock_version();
    std::fs::create_dir_all(l.agent_dir(current)).unwrap();
    std::fs::create_dir_all(l.agent_dir("0.1.0")).unwrap();
    std::fs::write(l.agent_dir("0.1.0").join("paddock-guest"), b"old").unwrap();
    let items = gc_items(&env, &Default::default());
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(items[0].what.contains("15.1 (24B1)") && items[0].bytes > 0);
    assert!(items[1].what.contains("0.1.0") && items[1].reason.contains(current));
    assert!(items.iter().all(|i| i.selected()));
    // gc removes exactly those folders, and refuses any other path.
    for i in &items {
        let crate::storage::Action::Macvm { dir } = &i.action else {
            panic!("{:?}", i.action)
        };
        gc_remove(&env, dir).unwrap();
        assert!(!dir.exists());
    }
    assert!(l.golden.disk.exists() && l.agent_dir(current).exists());
    let e = gc_remove(&env, &l.golden.dir).unwrap_err().to_string();
    assert!(e.contains("does not remove"), "{e}");
    let e = gc_remove(&env, &l.agent.join("../golden.noindex"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("does not remove"), "{e}");
    assert!(l.golden.disk.exists());
    assert_eq!(prev_state(&l), Prev::None);
}
