//! The grants file: defaults, the round trip, the atomic and locked
//! change, a damaged or newer file, the refusals of a folder, the merge
//! into a start and the event line of a change.

use super::*;
use crate::macvm::shares::Root;

fn profile(t: &Path) -> Profile {
    Profile::new(t.join("profile"))
}

fn change(t: u64) -> Change {
    Change::new(
        t,
        What::Folder,
        "add",
        "/h/me/src",
        "ro",
        Applies::Live,
        Source::You,
    )
}

#[test]
fn a_missing_file_is_the_defaults_and_a_change_round_trips() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let g = load(&p).unwrap();
    assert_eq!(g, Grants::default());
    assert!(g.clipboard.to_app && g.clipboard.from_app);
    assert_eq!(g.network, None);
    update(&p, |g| {
        g.network = Some("none".into());
        g.clipboard.from_app = false;
        g.shares.push(FolderGrant {
            path: "/h/me/src".into(),
            read_only: true,
            unprotected: false,
            added: 5,
        });
        g.changes.push(change(5));
        Ok(())
    })
    .unwrap();
    let back = load(&p).unwrap();
    assert_eq!(back.network.as_deref(), Some("none"));
    assert!(!back.clipboard.from_app && back.clipboard.to_app);
    assert_eq!(back.shares.len(), 1);
    assert_eq!(back.changes, [change(5)]);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path(&p)).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // The JSON words of the launcher.
    let text = std::fs::read_to_string(path(&p)).unwrap();
    for w in [
        "\"applies\": \"live\"",
        "\"source\": \"you\"",
        "\"what\": \"folder\"",
    ] {
        assert!(text.contains(w), "{w} in {text}");
    }
}

#[test]
fn a_failed_change_leaves_the_file_and_no_temporary_file() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    update(&p, |g| {
        g.network = Some("ask".into());
        Ok(())
    })
    .unwrap();
    let before = std::fs::read(path(&p)).unwrap();
    let r: Result<()> = update(&p, |g| {
        g.network = Some("none".into());
        bail!("the shim refused")
    });
    assert_eq!(r.unwrap_err().to_string(), "the shim refused");
    assert_eq!(std::fs::read(path(&p)).unwrap(), before);
    let names: Vec<String> = std::fs::read_dir(&p.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.contains("paddock-tmp")),
        "{names:?}"
    );
}

#[test]
fn concurrent_changes_lose_nothing() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let p = p.clone();
            std::thread::spawn(move || {
                for j in 0..5 {
                    update(&p, |g| {
                        g.changes.push(change(i * 100 + j));
                        Ok(())
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for th in threads {
        th.join().unwrap();
    }
    assert_eq!(load(&p).unwrap().changes.len(), 40);
}

#[test]
fn the_file_keeps_the_last_changes_and_status_shows_twenty_newest_first() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    update(&p, |g| {
        for i in 0..70 {
            g.changes.push(change(i));
        }
        Ok(())
    })
    .unwrap();
    let g = load(&p).unwrap();
    assert_eq!(g.changes.len(), CHANGES_KEPT);
    assert_eq!(g.changes[0].t, 20);
    let r = recent(&g);
    assert_eq!(r.len(), CHANGES_SHOWN);
    assert_eq!(r[0].t, 69);
    assert_eq!(r[19].t, 50);
}

#[test]
fn a_damaged_or_newer_file_is_never_written_over() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    std::fs::create_dir_all(&p.dir).unwrap();
    std::fs::write(path(&p), "{not json").unwrap();
    let e = load(&p).unwrap_err().to_string();
    assert!(e.contains("is damaged"), "{e}");
    assert!(update(&p, |_| Ok(())).is_err());
    assert_eq!(std::fs::read_to_string(path(&p)).unwrap(), "{not json");
    // A start goes on with the defaults and one line.
    let (g, line) = load_for_start(&p);
    assert_eq!(
        g.clipboard,
        Clipboard {
            to_app: false,
            from_app: false
        }
    );
    assert!(g.shares.is_empty());
    assert!(line
        .unwrap()
        .contains("This start uses none of its folders"));
    std::fs::write(path(&p), r#"{"v": 9, "network": "none"}"#).unwrap();
    let e = load(&p).unwrap_err().to_string();
    assert!(e.contains("version 9"), "{e}");
    assert!(update(&p, |_| Ok(())).is_err());
    // A file of this version with unknown fields of a later minor loads.
    std::fs::write(path(&p), r#"{"v": 1, "network": "none", "later": true}"#).unwrap();
    assert_eq!(load(&p).unwrap().network.as_deref(), Some("none"));
}

#[test]
fn the_folders_of_paddock_the_home_and_the_system_are_refused() {
    let home = Path::new("/Users/me");
    let paddock = [
        PathBuf::from("/Users/me/Library/Application Support/paddock"),
        PathBuf::from("/Users/me/.config/paddock"),
    ];
    let why = |p: &str| refusal(Path::new(p), home, &paddock);
    assert_eq!(why("/Users/me"), Some(ShareWhy::Home));
    assert_eq!(why("/"), Some(ShareWhy::Root));
    assert_eq!(why("/System/Library"), Some(ShareWhy::SystemApps));
    assert_eq!(why("/Users/me/Library"), Some(ShareWhy::Protected));
    assert_eq!(why("/Users/me/.ssh"), Some(ShareWhy::Protected));
    assert_eq!(why("/Users/me/.config"), Some(ShareWhy::PaddockData));
    assert_eq!(
        why("/Users/me/.config/paddock/firewall"),
        Some(ShareWhy::PaddockData)
    );
    assert_eq!(why("/Users/me/src/web"), None);
    assert_eq!(why("/Users/me/.config-other"), None);
}

#[test]
fn check_folder_resolves_refuses_and_finds_secrets() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    let data = home.join(".local/share/paddock");
    let web = home.join("src/web");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let paddock = [data.clone()];
    let (real, warnings) = check_folder(
        "sample-notes",
        Path::new("src/web"),
        &home,
        &home,
        &paddock,
        false,
    )
    .unwrap();
    assert_eq!(real, std::fs::canonicalize(&web).unwrap());
    assert!(warnings.is_empty());
    let e = check_folder("sample-notes", &home, &home, &home, &paddock, false)
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "paddock does not share ~ with sample-notes: it is your home folder, with every file of yours. Choose a project folder"
    );
    let e = check_folder(
        "sample-notes",
        &home.join(".local"),
        &home,
        &home,
        &paddock,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains("~/.local with sample-notes: it holds paddock's own data"),
        "{e}"
    );
    let e = check_folder(
        "sample-notes",
        &home.join("nope"),
        &home,
        &home,
        &paddock,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("no folder of"), "{e}");
    // A link to a refused folder is refused by its real path.
    std::os::unix::fs::symlink(home.join(".local"), home.join("src/link")).unwrap();
    let e = check_folder(
        "sample-notes",
        &home.join("src/link"),
        &home,
        &home,
        &paddock,
        false,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("paddock's own data"), "{e}");
    // A .env file: refused, with the way to share it anyway.
    std::fs::write(web.join(".env"), "KEY=1").unwrap();
    let e = check_folder("sample-notes", &web, &home, &home, &paddock, false)
        .unwrap_err()
        .to_string();
    assert!(e.contains("looks like a secrets file"), "{e}");
    assert!(
        e.contains("paddock app share sample-notes add '~/src/web' --unprotected"),
        "{e}"
    );
    let (_, warnings) = check_folder("sample-notes", &web, &home, &home, &paddock, true).unwrap();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn the_grants_join_the_roots_of_a_start() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let roots = vec![Root {
        path: "/h/me/a".into(),
        read_only: false,
        unprotected: false,
    }];
    update(&p, |g| {
        g.network = Some("allowlist".into());
        for (path, ro, unprotected) in [("/h/me/a", true, false), ("/h/me/b", false, true)] {
            g.shares.push(FolderGrant {
                path: path.into(),
                read_only: ro,
                unprotected,
                added: 1,
            });
        }
        Ok(())
    })
    .unwrap();
    let (merged, network) = for_start(&p, &roots);
    assert_eq!(network.as_deref(), Some("allowlist"));
    assert_eq!(
        merged,
        [
            Root {
                path: "/h/me/a".into(),
                read_only: true,
                unprotected: false,
            },
            Root {
                path: "/h/me/b".into(),
                read_only: false,
                unprotected: true,
            },
        ]
    );
}

#[test]
fn a_change_has_its_line_and_its_event() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let home = Path::new("/h/me");
    let c = change(1_762_400_000);
    assert_eq!(c.line(home), "grant folder add ~/src ro live");
    let n = Change::new(
        1,
        What::Network,
        "set",
        "none",
        "none",
        Applies::NextStart,
        Source::You,
    );
    assert_eq!(n.line(home), "grant network set none none next-start");
    note(&p, c.clone(), home);
    assert_eq!(load(&p).unwrap().changes, [c]);
    let events = std::fs::read_to_string(p.events_log()).unwrap();
    let v: serde_json::Value = serde_json::from_str(events.trim()).unwrap();
    assert_eq!(v["kind"], "grant");
    assert_eq!(v["text"], "grant folder add ~/src ro live");
    assert_eq!(
        v["grant"],
        serde_json::json!({
            "t": 1_762_400_000u64,
            "what": "folder",
            "action": "add",
            "target": "/h/me/src",
            "value": "ro",
            "applies": "live",
            "source": "you",
        })
    );
    // The reader of the event log still reads the line.
    let read = crate::macvm::events::read(&p.events_log());
    assert_eq!(
        read.last().unwrap().kind(),
        crate::macvm::events::Kind::Grant
    );
    assert_eq!(tilde(Path::new("/h/me"), home), "~");
    assert_eq!(tilde(Path::new("/elsewhere"), home), "/elsewhere");
}

#[test]
fn saved_writable_grant_does_not_weaken_explicit_read_only_root() {
    let root = shares::Root {
        path: PathBuf::from("/tmp/example"),
        read_only: true,
        unprotected: false,
    };
    let mut g = Grants::default();
    g.shares.push(FolderGrant {
        path: root.path.clone(),
        read_only: false,
        unprotected: false,
        added: 0,
    });
    assert!(merge_roots(&[root], &g)[0].read_only);
}

#[test]
fn save_syncs_the_parent_directory_and_reports_failure() {
    let t = tempfile::tempdir().unwrap();
    let p = Profile::new(t.path().to_path_buf());
    let error = save_with_sync(&p, &Grants::default(), |dir| {
        assert_eq!(dir, t.path());
        assert!(dir.join(FILE).is_file());
        Err(std::io::Error::other("directory sync failed"))
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("directory sync failed"));
}
