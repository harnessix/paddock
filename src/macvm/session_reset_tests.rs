//! The tests of the disk pair and of the reset tiers, next to
//! `session_tests.rs`, whose helpers they use.

use super::tests::{full_profile, golden, hooks, now, recorder};
use super::*;

use super::super::backup::Outcome;
use super::super::layout::Layout;
use super::super::text::ResetTier;

/// Write the protective MBR and the GPT signature into a data disk, as
/// the guest's `partitionDisk` leaves them.
fn format_data(p: &Profile) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(p.data())
        .unwrap();
    f.seek(SeekFrom::Start(510)).unwrap();
    f.write_all(&[0x55, 0xAA]).unwrap();
    f.write_all(b"EFI PART").unwrap();
    f.set_len(4096).unwrap();
}

#[test]
fn data_formatted_reads_the_gpt() {
    let t = tempfile::tempdir().unwrap();
    let p = full_profile(t.path());
    std::fs::write(p.data(), b"").unwrap();
    std::fs::File::open(p.data()).unwrap();
    assert!(!data_formatted(&p.data()), "an empty sparse file");
    std::fs::write(p.data(), vec![0u8; 8192]).unwrap();
    assert!(!data_formatted(&p.data()), "zeros");
    format_data(&p);
    assert!(data_formatted(&p.data()));
    assert!(!data_formatted(&p.dir.join("none.img")), "a missing file");
}

#[test]
fn repair_keeps_system_disk() {
    let t = tempfile::tempdir().unwrap();
    let g = golden(t.path());
    let p = full_profile(t.path());
    format_data(&p);
    std::fs::write(p.system(), b"the system disk that formatted the data").unwrap();
    std::fs::write(p.aux(), b"its nvram").unwrap();
    let mut stamp = Stamp::read(&p.stamp()).unwrap();
    stamp.data_prepared = true;
    stamp.write(&p.stamp()).unwrap();
    std::fs::write(&g.disk, b"a newer golden disk").unwrap();
    let mut other = now();
    other.golden = "15.3 (24D1)".into();
    // A crash left a partial stamp: Repair, with the system set kept.
    std::fs::write(partial(&p.stamp()), b"{").unwrap();
    assert_eq!(decide(&p, &other, CLONE_NEEDS), Ok(Start::Repair));
    assert!(removals(Start::Repair).iter().all(|w| !w.system_set()));
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(&p, &g, &other, Start::Repair, &mut h).unwrap();
    let _ = &h;
    drop(rec);
    assert_eq!(b, vec!["pre-repair".to_string()]);
    assert_eq!(
        std::fs::read(p.system()).unwrap(),
        b"the system disk that formatted the data"
    );
    assert_eq!(std::fs::read(p.aux()).unwrap(), b"its nvram");
    assert!(data_formatted(&p.data()));
    assert!(partials(&p).is_empty());
    let stamp = Stamp::read(&p.stamp()).unwrap();
    assert_eq!(
        stamp.golden, "15.2 (24C101)",
        "the golden of the clone stays"
    );
    assert!(stamp.data_prepared);
    // The next start runs on the old system disk and says so once.
    assert_eq!(decide(&p, &other, 0), Ok(Start::Cold));
    let note = golden_note("app", &stamp, &other).unwrap();
    assert!(
        note.contains("15.2 (24C101)") && note.contains("15.3 (24D1)"),
        "{note}"
    );
    assert_eq!(golden_note("app", &stamp, &now()), None);
    // `begin` keeps the golden of the clone too.
    let begun = begin(&p, &other, Start::Cold).unwrap();
    assert_eq!(begun.golden, "15.2 (24C101)");
    assert_eq!(begun.host_build, other.host_build);
}

#[test]
fn locked_data_refuses() {
    let t = tempfile::tempdir().unwrap();
    let p = full_profile(t.path());
    format_data(&p);
    std::fs::remove_file(p.system()).unwrap();
    let want = Refusal::DataLocked {
        profile: p.dir.clone(),
        backup: None,
    };
    assert_eq!(decide(&p, &now(), CLONE_NEEDS), Err(want.clone()));
    let line = want.to_string();
    assert!(line.contains("paddock app reset app --data"), "{line}");
    assert!(line.contains("Nothing was changed"), "{line}");
    let d = Display {
        w: 2560,
        h: 1600,
        scale: 2,
    };
    assert_eq!(
        super::super::start::predict(&p, "15.2 (24C101)", d),
        Err(want)
    );
    // Even with a partial next to it: the refusal comes first.
    std::fs::write(partial(&p.system()), b"half").unwrap();
    assert!(matches!(
        decide(&p, &now(), CLONE_NEEDS),
        Err(Refusal::DataLocked { .. })
    ));
    // A backup of the pair is named when there is one.
    let b = p.backups().join("manual-2026-11-05T10-00-00Z");
    std::fs::create_dir_all(&b).unwrap();
    for f in super::super::backup::FILES {
        std::fs::write(b.join(f), b"x").unwrap();
    }
    match decide(&p, &now(), CLONE_NEEDS) {
        Err(Refusal::DataLocked { backup, .. }) => {
            assert_eq!(backup, Some(b.clone()));
        }
        other => panic!("{other:?}"),
    }
    // A blank data disk without its system disk is a plain rebuild.
    std::fs::remove_file(partial(&p.system())).unwrap();
    std::fs::write(p.data(), vec![0u8; 4096]).unwrap();
    assert_eq!(decide(&p, &now(), CLONE_NEEDS), Ok(Start::Repair));
    assert!(!data_formatted(&p.dir.join("missing")));
}

#[test]
fn system_set_bound_to_formatted_data() {
    for start in [
        Start::Repair,
        Start::Stale(Reason::HostChanged),
        Start::Stale(Reason::ProtocolChanged),
        Start::Stale(Reason::DisplayChanged),
        Start::Stale(Reason::StateDamaged),
        Start::Stale(Reason::Unreadable),
        Start::Warm,
        Start::Cold,
        Start::Fresh,
    ] {
        let r = removals(start);
        assert!(
            r.iter().all(|w| !w.system_set() && *w != What::Data),
            "{start:?}: {r:?}"
        );
    }
    // The one start that clones over an existing profile is decided only
    // for a blank data disk.
    let t = tempfile::tempdir().unwrap();
    let p = full_profile(t.path());
    let mut other = now();
    other.golden = "15.3 (24D1)".into();
    std::fs::remove_file(p.state()).unwrap();
    assert_eq!(
        decide(&p, &other, CLONE_NEEDS),
        Ok(Start::Stale(Reason::GoldenChanged))
    );
    format_data(&p);
    assert_eq!(decide(&p, &other, 0), Ok(Start::Cold));
}

// ---- Reset tiers --------------------------------------------------------

/// A layout with one prepared profile `app/default`, its pair formatted.
fn reset_fixture(t: &Path) -> (Layout, Profile) {
    let l = Layout::at(t.join("macvm"));
    let g = golden(t);
    let p = l.profile("app", "default");
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(&p, &g, &now(), Start::Fresh, &mut h).unwrap();
    let mut stamp = begin(&p, &now(), Start::Fresh).unwrap();
    stamp.app = Some(AppStamp {
        version: "1".into(),
        build: "2".into(),
        copied: None,
    });
    std::fs::write(p.state(), b"saved state bytes").unwrap();
    finish(&p, &mut stamp, true, Some(17), 1_700_000_000).unwrap();
    format_data(&p);
    std::fs::write(p.apps().join("A.app"), b"x").unwrap();
    std::fs::write(p.drop_dir().join("d"), b"x").unwrap();
    std::fs::write(p.downloads().join("dl"), b"x").unwrap();
    std::fs::create_dir_all(p.backups().join("manual-1")).unwrap();
    (l, p)
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn no_backup(_: &Profile) -> Result<Outcome> {
    Ok(Outcome::Nothing)
}

#[test]
fn reset_tiers_match_table() {
    let t = tempfile::tempdir().unwrap();
    // The plain tier: the system set, the data disk and the stamp stay.
    let (l, p) = reset_fixture(t.path());
    let r = reset(
        &l,
        "app",
        "default",
        ResetTier::System,
        None,
        1,
        &mut no_backup,
    )
    .unwrap();
    assert_eq!(r.aside, None);
    assert_eq!(
        names_in(&p.dir),
        [
            "aux.img",
            "backups",
            "data.img",
            "downloads",
            "machine-id",
            "profile.lock",
            "stamp.json",
            "system.img"
        ]
    );
    let removed: Vec<String> = r
        .removed
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(removed, ["state.vzs", "apps", "drop", "logs"]);
    let stamp = Stamp::read(&p.stamp()).unwrap();
    assert_eq!(stamp.app, None, "the app is copied again at the next start");
    assert_eq!(stamp.state_id, None);
    assert!(stamp.clean);
    assert!(data_formatted(&p.data()));
    assert_eq!(decide(&p, &now(), 0), Ok(Start::Cold));
    assert_eq!(
        reset_removals(ResetTier::System),
        [What::State, What::Apps, What::Drop, What::Logs]
    );
    assert!(reset_removals(ResetTier::System)
        .iter()
        .all(|w| !w.system_set()));

    // --data: the whole pair goes, after the backup; the rest stays.
    let (l, p) = reset_fixture(t.path().join("data").as_path());
    let rules = t.path().join("data").join("rules.toml");
    std::fs::write(&rules, b"[rules]").unwrap();
    let mut calls = Vec::new();
    let r = reset(
        &l,
        "app",
        "default",
        ResetTier::Data,
        Some(&rules),
        1,
        &mut |p: &Profile| {
            calls.push(p.dir.clone());
            Ok(Outcome::Made(p.backups().join("pre-reset-1")))
        },
    )
    .unwrap();
    assert_eq!(calls, vec![p.dir.clone()], "the backup comes first");
    assert_eq!(r.aside, Some(p.backups().join("pre-reset-1")));
    assert_eq!(names_in(&p.dir), ["backups", "downloads", "profile.lock"]);
    assert!(rules.exists(), "the rules file stays");
    assert_eq!(
        decide(&p, &now(), CLONE_NEEDS),
        Ok(Start::Fresh),
        "a new pair from the golden"
    );
    assert!(reset_removals(ResetTier::Data).contains(&What::Data));
    assert!(reset_removals(ResetTier::Data).contains(&What::MachineId));
    // A skipped backup stops the reset before anything goes.
    let (l2, p2) = reset_fixture(t.path().join("skip").as_path());
    let e = reset(
        &l2,
        "app",
        "default",
        ResetTier::Data,
        None,
        1,
        &mut |_: &Profile| {
            Ok(Outcome::Skipped {
                needed: 5 << 30,
                free: 1 << 30,
            })
        },
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("5 GiB") && e.contains("1 GiB"), "{e}");
    assert!(p2.data().exists() && p2.system().exists());

    // --all: the folder moves to the trash with the rules file.
    let (l, p) = reset_fixture(t.path().join("all").as_path());
    let rules = t.path().join("all").join("rules.toml");
    std::fs::write(&rules, b"[rules]").unwrap();
    let r = reset(
        &l,
        "app",
        "default",
        ResetTier::All,
        Some(&rules),
        1_700_000_000,
        &mut no_backup,
    )
    .unwrap();
    assert!(!p.dir.exists());
    assert!(
        !p.dir.parent().unwrap().exists(),
        "the empty app folder goes too"
    );
    let trash = r.aside.unwrap();
    assert_eq!(trash, l.trash.join("app-default-2023-11-14T22-13-20Z"));
    assert!(trash.join("data.img").exists() && trash.join("system.img").exists());
    assert_eq!(
        std::fs::read(trash.join("firewall.toml")).unwrap(),
        b"[rules]"
    );
    assert!(!rules.exists());
    assert_eq!(reset_removals(ResetTier::All), [What::All]);
}

#[test]
fn reset_keeps_data_by_default() {
    let t = tempfile::tempdir().unwrap();
    let (l, p) = reset_fixture(t.path());
    let before = std::fs::read(p.data()).unwrap();
    let system = std::fs::read(p.system()).unwrap();
    reset(
        &l,
        "app",
        "default",
        ResetTier::System,
        None,
        1,
        &mut no_backup,
    )
    .unwrap();
    assert_eq!(std::fs::read(p.data()).unwrap(), before);
    assert_eq!(std::fs::read(p.system()).unwrap(), system);
    assert!(p.downloads().join("dl").exists());
    assert!(p.backups().join("manual-1").exists());
    // No profile: one line, nothing made.
    let e = reset(
        &l,
        "other",
        "default",
        ResetTier::System,
        None,
        1,
        &mut no_backup,
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("other has no profile"), "{e}");
    assert!(!l.profile("other", "default").dir.exists());
}

#[test]
fn reset_refused_while_locked() {
    let t = tempfile::tempdir().unwrap();
    let (l, p) = reset_fixture(t.path());
    let held = super::super::lock::ProfileLock::take_as(&p, 812).unwrap();
    for tier in [ResetTier::System, ResetTier::Data, ResetTier::All] {
        let e = reset(&l, "app", "default", tier, None, 1, &mut no_backup)
            .unwrap_err()
            .to_string();
        assert!(e.contains("812"), "{tier:?}: {e}");
    }
    assert!(p.data().exists() && p.system().exists() && p.state().exists());
    drop(held);
    assert!(reset(
        &l,
        "app",
        "default",
        ResetTier::System,
        None,
        1,
        &mut no_backup
    )
    .is_ok());
}
