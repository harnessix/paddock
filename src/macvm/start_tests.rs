//! The start lines of `paddock app run`, for every start kind.

use super::*;
use std::path::PathBuf;

fn plan<'a>(start: Start, web: &'a Path, downloads: &'a Path) -> Plan<'a> {
    Plan {
        app: "chatgpt-desktop",
        golden: "15.2 (24C101)",
        memory_mib: 4096,
        display: Display {
            w: 3024,
            h: 1964,
            scale: 2,
        },
        mixed_scale: false,
        shares: vec![Share {
            path: web,
            writable: true,
        }],
        downloads,
        network: "ask",
        start,
        copy: None,
    }
}

/// Every start kind.
/// `app::tests::start_lines_per_start_kind` is the acceptance test.
#[test]
fn every_start_kind_has_its_lines_and_seconds() {
    let web = PathBuf::from("/Users/me/src/web");
    let dl = PathBuf::from("/Users/me/Library/.../chatgpt-desktop/default/downloads");
    let cases = [
        (Start::Fresh, 60, "the first start of this profile"),
        (Start::Repair, 60, "the profile was rebuilt"),
        (
            Start::Stale(Reason::GoldenChanged),
            60,
            "the guest image changed",
        ),
        (Start::Stale(Reason::StateDamaged), 40, "no saved state"),
        (Start::Cold, 40, "no saved state"),
    ];
    for (start, secs, reason) in cases {
        assert_eq!(seconds(start), secs, "{start:?}");
        assert_eq!(kind(start), StartKind::Cold, "{start:?}");
        let out = lines(&plan(start, &web, &dl));
        let joined = out.join("\n");
        for part in [
            "backend   macvm",
            "15.2 (24C101)",
            "memory    4 GiB",
            "display   3024 x 1964 points at 2x",
            "/Users/me/src/web (rw)",
            "downloads /Users/me/Library",
            "network   ask",
            "cold start, about ",
            reason,
            "the session continues after this terminal closes",
        ] {
            assert!(joined.contains(part), "{start:?} lacks {part:?}:\n{joined}");
        }
        assert!(
            joined.contains(&format!("about {secs} s")),
            "{start:?} does not say {secs} s:\n{joined}"
        );
        // Without a copy the block is nine lines, each one line long.
        assert_eq!(out.len(), 8, "{out:?}");
        assert!(out.iter().all(|l| !l.contains('\n')));
        assert!(!joined.contains("copy      "), "{joined}");
    }

    // Warm: six seconds and no reason.
    assert_eq!(seconds(Start::Warm), 6);
    assert_eq!(kind(Start::Warm), StartKind::Warm);
    assert_eq!(why(Start::Warm), None);
    let warm = lines(&plan(Start::Warm, &web, &dl)).join("\n");
    let start_line = lines(&plan(Start::Warm, &web, &dl))
        .into_iter()
        .find(|l| l.starts_with("start "))
        .unwrap();
    assert_eq!(start_line, "start     warm start, about 6 s");
    assert!(warm.contains("warm start, about 6 s"), "{warm}");

    // A start that also copies the app names the copy and its seconds.
    let mut p = plan(Start::Fresh, &web, &dl);
    p.copy = Some(("ChatGPT", 20));
    let out = lines(&p);
    assert_eq!(out.len(), 9);
    let joined = out.join("\n");
    assert!(
        joined.contains("copy      then the copy of ChatGPT into the guest, about 20 s"),
        "{joined}"
    );
    assert!(joined.contains("once per version"), "{joined}");
    // The copy line comes after the start line and before the last one.
    let at = out.iter().position(|l| l.starts_with("copy ")).unwrap();
    assert!(out[at - 1].starts_with("start "));
    assert_eq!(at, out.len() - 2);

    // No workspace folder: the line says how to name one.
    let mut p = plan(Start::Warm, &web, &dl);
    p.shares.clear();
    p.network = "egress";
    p.mixed_scale = true;
    let joined = lines(&p).join("\n");
    assert!(joined.contains("shares    0 folders"), "{joined}");
    assert!(
        joined.contains("paddock app run chatgpt-desktop ~/src/web"),
        "{joined}"
    );
    assert!(joined.contains("every host is allowed"), "{joined}");
    assert!(joined.contains("two scales"), "{joined}");
}

#[test]
fn copy_part_follows_needs_copy() {
    let t = tempfile::tempdir().unwrap();
    let app = t.path().join("ChatGPT.app");
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        "<plist><dict><key>CFBundleShortVersionString</key><string>1</string><key>CFBundleName</key><string>ChatGPT</string></dict></plist>",
    )
    .unwrap();
    let b = super::super::appcopy::read(&app).unwrap();
    assert_eq!(copy_part(&b, false), None);
    let (name, secs) = copy_part(&b, true).unwrap();
    assert_eq!(name, "ChatGPT");
    assert_eq!(secs, b.copy_seconds());
}

#[test]
fn predict_reads_the_profile_folder_only() {
    let t = tempfile::tempdir().unwrap();
    let p = super::super::session::Profile::new(t.path().join("profiles/chatgpt/default"));
    let display = Display {
        w: 2560,
        h: 1600,
        scale: 2,
    };
    // No folder yet.
    assert_eq!(predict(&p, "15.2 (24C101)", display), Ok(Start::Fresh));

    // The files of a profile that quit with a save.
    std::fs::create_dir_all(&p.dir).unwrap();
    for f in [p.system(), p.aux(), p.data(), p.machine_id()] {
        std::fs::write(f, b"x").unwrap();
    }
    std::fs::write(p.state(), vec![0u8; 64]).unwrap();
    let now = super::super::session::Now {
        golden: "15.2 (24C101)".into(),
        host_build: "25A1".into(),
        vz: "1".into(),
        protocol: super::super::proto::VERSION,
        paddock: "0.2.1".into(),
        agent: "0.2.1".into(),
        display,
    };
    let mut stamp = now.stamp("abc-1".into());
    stamp.state_id = Some("abc-1".into());
    stamp.state_len = Some(64);
    stamp.clean = true;
    stamp.write(&p.stamp()).unwrap();
    assert_eq!(predict(&p, "15.2 (24C101)", display), Ok(Start::Warm));

    // A new golden, another display, and a state that does not match.
    assert_eq!(
        predict(&p, "15.3 (24D70)", display),
        Ok(Start::Stale(Reason::GoldenChanged))
    );
    let other = Display {
        w: 3024,
        h: 1964,
        scale: 2,
    };
    assert_eq!(
        predict(&p, "15.2 (24C101)", other),
        Ok(Start::Stale(Reason::DisplayChanged))
    );
    let mut dirty = stamp.clone();
    dirty.clean = false;
    dirty.write(&p.stamp()).unwrap();
    assert_eq!(
        predict(&p, "15.2 (24C101)", display),
        Ok(Start::Stale(Reason::StateDamaged))
    );
    stamp.write(&p.stamp()).unwrap();

    // No saved state: a cold boot. A partial anywhere: a repair.
    std::fs::remove_file(p.state()).unwrap();
    assert_eq!(predict(&p, "15.2 (24C101)", display), Ok(Start::Cold));
    std::fs::write(p.dir.join("system.img.partial"), b"half").unwrap();
    assert_eq!(predict(&p, "15.2 (24C101)", display), Ok(Start::Repair));
    std::fs::remove_file(p.dir.join("system.img.partial")).unwrap();

    // A stamp that paddock cannot read, next to the disks: a repair.
    std::fs::write(p.stamp(), b"{").unwrap();
    assert_eq!(predict(&p, "15.2 (24C101)", display), Ok(Start::Repair));
    // A stamp of another schema: the whole profile is foreign.
    std::fs::write(p.stamp(), b"{\"schema\":99}").unwrap();
    assert_eq!(
        predict(&p, "15.2 (24C101)", display),
        Ok(Start::Stale(Reason::Unreadable))
    );
}

/// The Mac script of ChatGPT greps the terminal and the control socket for
/// fixed text. Each part must be text that this code really prints, so a
/// change here fails on Linux and not on the Mac.
#[test]
fn the_mac_script_matches_the_lines_it_greps() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script =
        std::fs::read_to_string(root.join("packaging/macos/checks/macvm-chatgpt.sh")).unwrap();
    let web = PathBuf::from("/Users/me/src/web");
    let dl = PathBuf::from("/d");
    let mut p = plan(Start::Fresh, &web, &dl);
    p.copy = Some(("ChatGPT", 20));
    let printed = lines(&p).join("\n");
    for part in [
        "copy      then the copy of ChatGPT",
        "start     cold start",
        "the first start of this profile",
    ] {
        assert!(script.contains(part), "the script does not check {part:?}");
        assert!(
            printed.contains(part),
            "the script wants {part:?}, which the start lines do not print:\n{printed}"
        );
    }
    // The Dock menu entry that the relaunch check presses.
    let again = super::super::text::open_again("ChatGPT");
    assert_eq!(again, "Open ChatGPT again");
    assert!(
        script.contains(&again),
        "the script does not check {again:?}"
    );
    // The Dock name of the session.
    let dock = super::super::shim::paddock_name("ChatGPT");
    assert!(script.contains(&dock), "the script does not check {dock:?}");
    // Every check of the ChatGPT session is a case of the script.
    for check in [
        "first",
        "login-kept",
        "chat",
        "zero-direct",
        "dock",
        "copy-partial",
        "relaunch",
        "gatekeeper",
        "drop-target",
        "vscode",
    ] {
        assert!(
            script.contains(&format!("  {check})")),
            "the script has no case for {check}"
        );
    }
    // The event kinds that the script waits for are the ones the session
    // records.
    for kind in [
        "\"kind\":\"copy\"",
        "\"kind\":\"launch\"",
        "\"kind\":\"start\"",
    ] {
        assert!(script.contains(kind), "the script does not wait for {kind}");
    }
}
