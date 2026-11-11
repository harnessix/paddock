use super::*;
use crate::macvm::host::Session;
use std::time::{Duration, SystemTime};

const NOW: u64 = 1_800_000_000;

fn old(path: &Path, days: u64) {
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(NOW - days * DAY);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// A profile instance with a stamp, a disk, a saved state and an archive,
/// last used `days` ago.
fn profile(l: &Layout, app: &str, n: u32, days: u64) -> Profile {
    let p = l.profile(app, &crate::macvm::instance::folder(n));
    std::fs::create_dir_all(p.apps()).unwrap();
    std::fs::create_dir_all(p.backups()).unwrap();
    for f in [p.data(), p.system(), p.state()] {
        std::fs::write(&f, vec![1u8; 8192]).unwrap();
    }
    std::fs::write(p.apps().join("Note Pad.app.cpio"), vec![2u8; 4096]).unwrap();
    std::fs::create_dir_all(p.apps().join("Note Pad.app/Contents")).unwrap();
    let mut s = crate::macvm::session::tests::now().stamp("ab-1".into());
    s.state_id = Some("ab-1".into());
    s.state_len = Some(8192);
    s.write(&p.stamp()).unwrap();
    old(&p.stamp(), days);
    old(&p.state(), days);
    p
}

fn live(l: &Layout, app: &str, n: u32, suspended: bool) {
    std::fs::create_dir_all(&l.sessions).unwrap();
    let home = crate::macvm::instance::folder(n);
    let s = Session {
        pid: std::process::id(),
        app: app.into(),
        home: home.clone(),
        instance: n,
        suspended,
        ..Default::default()
    };
    std::fs::write(l.record(app, &home), serde_json::to_vec(&s).unwrap()).unwrap();
}

fn alive(pid: u32) -> bool {
    pid == std::process::id()
}

fn setup() -> (tempfile::TempDir, Layout) {
    let t = tempfile::tempdir().unwrap();
    let l = Layout::at(t.path().join("macvm"));
    std::fs::create_dir_all(&l.golden.dir).unwrap();
    std::fs::write(l.golden.dir.join("disk.img"), vec![0u8; 4096]).unwrap();
    profile(&l, "notes", 1, 200);
    profile(&l, "notes", 2, 1);
    profile(&l, "chat", 1, 300);
    live(&l, "chat", 1, true);
    (t, l)
}

#[test]
fn du_lists_every_instance_with_its_parts_and_state() {
    let (_t, l) = setup();
    let d = scan(&l, &alive);
    assert_eq!(d.profiles.len(), 3);
    let notes2 = d
        .profiles
        .iter()
        .find(|p| p.app == "notes" && p.instance == 2)
        .unwrap();
    assert_eq!(notes2.state, Use::Stopped);
    assert!(
        notes2.disks >= 16384 && notes2.saved_state >= 8192,
        "{notes2:?}"
    );
    assert!(notes2.archives >= 4096 && notes2.apps >= notes2.archives);
    let chat = d.profiles.iter().find(|p| p.app == "chat").unwrap();
    assert_eq!(chat.state, Use::Suspended);
    assert!(d.golden > 0 && d.bytes >= d.golden);
    let text = render(&d, NOW);
    for part in [
        "macOS guest (macvm)",
        "golden image",
        "profile notes 2 [stopped]",
        "profile chat [suspended]",
        "200 days ago",
        "deleting loses",
    ] {
        assert!(text.contains(part), "{text}");
    }
    let json = serde_json::to_value(&d).unwrap();
    assert_eq!(json["profiles"][0]["state"], "suspended");
    // No macvm folder: no section.
    let none = scan(&Layout::at(_t.path().join("none")), &alive);
    assert!(render(&none, NOW).is_empty());
}

#[test]
fn gc_lists_archives_old_states_and_never_takes_a_profile_without_the_flags() {
    let (_t, l) = setup();
    let o = GcOptions {
        older_than_days: 90,
        ..Default::default()
    };
    let items = gc_items(&l, &o, NOW, &alive);
    let whats: Vec<&str> = items.iter().map(|i| i.what.as_str()).collect();
    // The suspended instance gives nothing.
    assert!(
        !whats.iter().any(|w| w.contains("profile chat")),
        "{whats:?}"
    );
    // Both stopped instances: their archives, automatic.
    let archives: Vec<&Item> = items
        .iter()
        .filter(|i| i.what.contains("archive"))
        .collect();
    assert_eq!(archives.len(), 2);
    assert!(archives
        .iter()
        .all(|i| i.tier == Tier::Auto && i.selected()));
    // The old one: its saved state, automatic; the profile needs both flags.
    let state = items
        .iter()
        .find(|i| i.what.contains("saved state"))
        .unwrap();
    assert!(state.what.contains("notes") && state.selected());
    let prof = items
        .iter()
        .find(|i| i.what.starts_with("macOS guest profile"))
        .unwrap();
    assert_eq!(prof.what, "macOS guest profile notes");
    assert_eq!(prof.tier, Tier::Consent);
    assert_eq!(prof.needs, vec!["--homes", "--include-app-logins"]);
    assert!(prof.warning.as_deref().unwrap().contains("login"));
    // The recent instance: no state, no profile.
    assert_eq!(
        items.iter().filter(|i| i.what.contains("notes 2")).count(),
        1
    );
    // With both flags the profile is selected too.
    let o = GcOptions {
        older_than_days: 90,
        homes: true,
        include_app_logins: true,
        ..Default::default()
    };
    let items = gc_items(&l, &o, NOW, &alive);
    assert!(items.iter().all(|i| i.selected()));
}

#[test]
fn remove_takes_only_listed_parts_of_stopped_instances() {
    let (_t, l) = setup();
    let notes = l.profile("notes", "default");
    // A saved state: gone, and the stamp no longer names it.
    assert!(remove(&l, &notes.state(), &alive).unwrap());
    assert!(!notes.state().exists());
    let s = Stamp::read(&notes.stamp()).unwrap();
    assert_eq!(s.state_id, None);
    // An archive: gone; the clone stays.
    let a = notes.apps().join("Note Pad.app.cpio");
    assert!(remove(&l, &a, &alive).unwrap());
    assert!(!a.exists() && notes.apps().join("Note Pad.app").exists());
    // A suspended instance is refused.
    let chat = l.profile("chat", "default");
    let e = remove(&l, &chat.dir, &alive).unwrap_err();
    assert!(format!("{e}").contains("skipped"));
    assert!(chat.dir.exists());
    // Paths that are no part of a profile.
    for p in [
        l.profiles.join("notes"),
        l.profiles.join("notes/work"),
        notes.data(),
        notes.apps().join("Note Pad.app"),
        l.golden.dir.clone(),
    ] {
        assert!(!remove(&l, &p, &alive).unwrap(), "{}", p.display());
    }
    // A whole stopped profile, through the gc remover of manage.rs.
    crate::macvm::manage::gc_remove_in(&l, &notes.dir).unwrap();
    assert!(!notes.dir.exists());
    assert!(l.profile("notes", "instance-2").dir.exists());
}
