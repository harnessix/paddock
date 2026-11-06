//! The limits of the macOS guest route, each with its line.

use super::*;
use crate::macvm::crop::Rect;

fn mac(version: &str, arm: bool) -> HostFacts {
    HostFacts {
        macos: version.into(),
        apple_silicon: arm,
    }
}

fn session(app: &str, pid: u32) -> Session {
    Session {
        pid,
        app: app.into(),
        ..Default::default()
    }
}

#[test]
fn each_refusal_line() {
    // A third session names the two that run.
    let two = [session("chatgpt-desktop", 812), session("vscode", 913)];
    let line = third_session_refusal(&two, "claude-desktop").expect("refused");
    for part in [
        "two macOS VMs",
        "chatgpt-desktop (you, pid 812)",
        "vscode (you, pid 913)",
        "paddock app stop",
    ] {
        assert!(line.contains(part), "{line:?} lacks {part:?}");
    }
    // One running, or a second start of a running app: not this line.
    assert_eq!(third_session_refusal(&two[..1], "claude-desktop"), None);
    assert_eq!(third_session_refusal(&two, "vscode"), None);
    // A suspended session holds no slot, and a second instance of a
    // running app is a third session.
    let mut parked = two.clone();
    parked[0].suspended = true;
    assert_eq!(third_session_refusal(&parked, "claude-desktop"), None);
    assert!(third_session_refusal(&two, "vscode--2").is_some());

    // macOS 13 is refused, 14 and later run, an unknown version runs.
    let line = host_refusal(&mac("13.6.1", true)).expect("refused");
    assert!(
        line.contains("macOS 13.6.1") && line.contains("macOS 14 or later"),
        "{line}"
    );
    assert_eq!(host_refusal(&mac("14.0", true)), None);
    assert_eq!(host_refusal(&mac("27.0.1", true)), None);
    assert_eq!(host_refusal(&mac("", true)), None);

    // An Intel Mac is refused first, whatever its version.
    let line = host_refusal(&mac("15.2", false)).expect("refused");
    assert!(
        line.contains("Apple silicon") && line.contains("Intel"),
        "{line}"
    );

    // 32 GiB asked on a 16 GiB Mac: half of it at most.
    let line = memory_refusal("vscode", 32 << 10, Some(16)).expect("refused");
    assert!(
        line.contains("32 GiB") && line.contains("16 GiB") && line.contains("vm_memory = \"8G\""),
        "{line}"
    );
    assert_eq!(memory_refusal("vscode", 8 << 10, Some(16)), None);
    assert_eq!(memory_refusal("vscode", 32 << 10, None), None);
    // The memory of a profile goes through the same rule.
    let cfg = crate::manifest::AppCfg {
        vm_memory: Some("32G".into()),
        ..Default::default()
    };
    let e = crate::macvm::host_app::memory_mib("vscode", Some(&cfg), Some(16)).unwrap_err();
    assert!(format!("{e:#}").contains("16 GiB"));

    // Mixed scales: a note, not a refusal.
    let mixed = ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(1440.0, 0.0, 1920.0, 1080.0),
        ],
        main: 0,
        scales: vec![2.0, 1.0],
    };
    let note = scale_note(&mixed).expect("a note");
    assert!(
        note.contains("two scales") && note.contains("soft"),
        "{note}"
    );
    let same = ScreenSet {
        scales: vec![2.0, 2.0],
        ..mixed
    };
    assert_eq!(scale_note(&same), None);
}

#[test]
fn the_versions_parse() {
    assert_eq!(macos_major("15.2"), Some(15));
    assert_eq!(macos_major("13"), Some(13));
    assert_eq!(macos_major("x"), None);
}

#[test]
fn the_refusal_reads_the_live_records() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path();
    let write = |name: &str, pid: u32| {
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_vec(&session(name, pid)).unwrap(),
        )
        .unwrap();
    };
    // Two live records (this process) and one dead one.
    write("a", std::process::id());
    write("b", std::process::id());
    write("c", 2_000_000_000);
    assert!(refusal(None, dir, "z", false).is_some());
    assert!(refusal(None, dir, "a", false).is_none());
    std::fs::remove_file(dir.join("b.json")).unwrap();
    assert!(refusal(None, dir, "z", false).is_none());
    // The host comes first.
    assert!(refusal(Some(&mac("13.0", true)), dir, "z", false)
        .unwrap()
        .contains("macOS 13.0"));
}

#[test]
fn the_table_has_every_limit_row() {
    let whats: Vec<&str> = TABLE.iter().map(|r| r.what).collect();
    for w in [
        "Sessions at once",
        "RAM per session",
        "CPUs per session",
        "Golden image on disk",
        "Profile on disk",
        "First golden build",
        "First start of a profile",
        "Later start",
        "Quit",
        "Input to screen",
    ] {
        assert!(whats.contains(&w), "{w}");
    }
}

#[test]
fn auto_suspend_leaves_the_third_session_to_the_suspend() {
    let dir = tempfile::tempdir().unwrap();
    for (i, app) in ["a", "b"].iter().enumerate() {
        let s = Session {
            pid: std::process::id(),
            app: app.to_string(),
            started: i as u64,
            ..Default::default()
        };
        std::fs::write(
            dir.path().join(format!("{app}.json")),
            serde_json::to_vec(&s).unwrap(),
        )
        .unwrap();
    }
    assert!(refusal(None, dir.path(), "c", false).is_some());
    assert_eq!(refusal(None, dir.path(), "c", true), None);
    // The host refusal stays.
    assert!(refusal(Some(&mac("13.0", true)), dir.path(), "c", true).is_some());
}
