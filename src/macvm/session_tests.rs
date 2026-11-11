use super::*;
use std::os::unix::fs::MetadataExt;

pub(crate) fn now() -> Now {
    Now {
        golden: "15.2 (24C101)".into(),
        host_build: "24C101".into(),
        vz: "vz-1".into(),
        protocol: Version { major: 1, minor: 3 },
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    }
}

pub(crate) fn golden(t: &Path) -> Golden {
    let g = Golden::new(t.join("golden"));
    std::fs::create_dir_all(&g.dir).unwrap();
    std::fs::write(&g.disk, b"golden disk").unwrap();
    std::fs::write(&g.aux, b"golden aux").unwrap();
    std::fs::write(&g.hardware_model, b"hw").unwrap();
    g
}

pub(crate) fn mid() -> Result<Vec<u8>> {
    Ok(b"machine-id-bytes".to_vec())
}

/// The tests leave a new data disk blank, as a failed host format does.
pub(super) fn no_format(_: &Path) -> Result<()> {
    anyhow::bail!("the tests leave the data disk blank")
}

pub(super) fn hooks<'a>(backup: &'a mut dyn FnMut(&Profile, &str) -> Result<()>) -> Hooks<'a> {
    Hooks {
        clone: &copy_clone,
        machine_id: &mid,
        backup,
        format_data: &no_format,
    }
}

/// A backup closure that records its reasons.
pub(super) fn recorder(into: &mut Vec<String>) -> impl FnMut(&Profile, &str) -> Result<()> + '_ {
    move |_: &Profile, reason: &str| {
        into.push(reason.to_string());
        Ok(())
    }
}

/// A complete profile at rest, with a saved state that matches.
pub(super) fn full_profile(t: &Path) -> Profile {
    let g = golden(t);
    let p = Profile::new(t.join("profiles").join("app").join("home"));
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(&p, &g, &now(), Start::Fresh, &mut h).unwrap();
    let mut stamp = begin(&p, &now(), Start::Fresh).unwrap();
    std::fs::write(p.state(), b"saved state bytes").unwrap();
    finish(&p, &mut stamp, true, Some(17), 1_700_000_000).unwrap();
    p
}

pub(super) fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().mode() & 0o7777
}

#[test]
fn decide_table() {
    let t = tempfile::tempdir().unwrap();
    let n = now();
    let free = CLONE_NEEDS;
    // No folder: fresh.
    let p = Profile::new(t.path().join("none"));
    assert_eq!(decide(&p, &n, free), Ok(Start::Fresh));
    // An empty folder: fresh.
    std::fs::create_dir_all(&p.dir).unwrap();
    assert_eq!(decide(&p, &n, free), Ok(Start::Fresh));
    // A complete profile with a matching state: warm.
    let p = full_profile(t.path());
    assert_eq!(decide(&p, &n, free), Ok(Start::Warm));
    // No state file: cold.
    std::fs::remove_file(p.state()).unwrap();
    assert_eq!(decide(&p, &n, free), Ok(Start::Cold));
    assert_eq!(
        line(Start::Cold, Stamp::read(&p.stamp()).ok().as_ref(), &n),
        "cold boot"
    );
    // The state file is shorter than the stamp says: damaged.
    std::fs::write(p.state(), b"short").unwrap();
    assert_eq!(decide(&p, &n, free), Ok(Start::Stale(Reason::StateDamaged)));
    std::fs::write(p.state(), b"saved state bytes").unwrap();
    // Every reason, each from the full profile.
    let mut other = n.clone();
    other.golden = "15.3 (24D1)".into();
    assert_eq!(
        decide(&p, &other, free),
        Ok(Start::Stale(Reason::GoldenChanged))
    );
    let mut other = n.clone();
    other.host_build = "24D1".into();
    assert_eq!(
        decide(&p, &other, free),
        Ok(Start::Stale(Reason::HostChanged))
    );
    let mut other = n.clone();
    other.vz = "vz-2".into();
    assert_eq!(
        decide(&p, &other, free),
        Ok(Start::Stale(Reason::HostChanged))
    );
    let mut other = n.clone();
    other.protocol = Version { major: 2, minor: 0 };
    assert_eq!(
        decide(&p, &other, free),
        Ok(Start::Stale(Reason::ProtocolChanged))
    );
    // A minor change talks.
    let mut other = n.clone();
    other.protocol = Version { major: 1, minor: 9 };
    assert_eq!(decide(&p, &other, free), Ok(Start::Warm));
    let mut other = n.clone();
    other.display.scale = 1;
    assert_eq!(
        decide(&p, &other, free),
        Ok(Start::Stale(Reason::DisplayChanged))
    );
    // A stamp of an unknown schema.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(p.stamp()).unwrap()).unwrap();
    v["schema"] = serde_json::json!(99);
    std::fs::write(p.stamp(), v.to_string()).unwrap();
    assert_eq!(decide(&p, &n, free), Ok(Start::Stale(Reason::Unreadable)));
    // A session that ended without a save: the state id does not match.
    let p = full_profile(t.path().join("b").as_path());
    let stamp = begin(&p, &n, Start::Warm).unwrap();
    assert!(!stamp.clean);
    assert_eq!(decide(&p, &n, free), Ok(Start::Stale(Reason::StateDamaged)));
    remove(&p, What::State).unwrap();
    assert_eq!(decide(&p, &n, free), Ok(Start::Cold));
    assert_eq!(
        line(Start::Cold, Stamp::read(&p.stamp()).ok().as_ref(), &n),
        "the last session ended without a save, so this start is a cold boot. Your data is on the data disk"
    );
    // Every reason has a line of its own.
    let old = Stamp::read(&p.stamp()).unwrap();
    let lines: std::collections::BTreeSet<String> = [
        Reason::GoldenChanged,
        Reason::HostChanged,
        Reason::ProtocolChanged,
        Reason::DisplayChanged,
        Reason::StateDamaged,
        Reason::Unreadable,
    ]
    .into_iter()
    .map(|r| line(Start::Stale(r), Some(&old), &n))
    .collect();
    assert_eq!(lines.len(), 6);
    assert!(lines.iter().all(|l| l.contains("cold boot")), "{lines:?}");
}

#[test]
fn decide_repair_on_partial_or_missing_stamp() {
    let t = tempfile::tempdir().unwrap();
    let n = now();
    let p = full_profile(t.path());
    // A partial file at the top level.
    std::fs::write(partial(&p.system()), b"half").unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Repair));
    std::fs::remove_file(partial(&p.system())).unwrap();
    // A partial folder one level down.
    std::fs::create_dir(partial(&p.apps().join("X.app"))).unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Repair));
    std::fs::remove_dir(partial(&p.apps().join("X.app"))).unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Warm));
    // A missing stamp with data.img present.
    std::fs::remove_file(p.stamp()).unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Repair));
    // A damaged stamp.
    std::fs::write(p.stamp(), b"{not json").unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Repair));
    std::fs::write(p.stamp(), b"{\"schema\": 1}").unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Repair));
    // A missing system part with a good stamp.
    let p = full_profile(t.path().join("b").as_path());
    std::fs::remove_file(p.system()).unwrap();
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Repair));
}

#[test]
fn decide_refuses_without_space() {
    let t = tempfile::tempdir().unwrap();
    let n = now();
    let p = Profile::new(t.path().join("none"));
    let e = decide(&p, &n, 1_288_490_188).unwrap_err();
    assert_eq!(
        e.to_string(),
        "not enough space for a cold start of the profile (needs 8 GiB, 1.2 GiB free). Free space on the disk, for example with paddock gc, then try again"
    );
    assert_eq!(decide(&p, &n, CLONE_NEEDS), Ok(Start::Fresh));
    // A warm or cold start needs no space.
    let p = full_profile(t.path());
    assert_eq!(decide(&p, &n, 0), Ok(Start::Warm));
    let mut other = n.clone();
    other.host_build = "x".into();
    assert_eq!(decide(&p, &other, 0), Ok(Start::Stale(Reason::HostChanged)));
    // A new golden clones again, so it needs the space.
    let mut other = n.clone();
    other.golden = "x".into();
    assert!(decide(&p, &other, 0).is_err());
    assert_eq!(
        save_refusal(5 << 30, 1_288_490_188),
        "not enough space for the saved state (needs 5 GiB, 1.2 GiB free): the next start is a cold boot. Free space, for example with paddock gc"
    );
}

#[test]
fn prepare_leaves_no_partial() {
    let t = tempfile::tempdir().unwrap();
    let g = golden(t.path());
    let p = Profile::new(t.path().join("profiles").join("app").join("home"));
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(&p, &g, &now(), Start::Fresh, &mut h).unwrap();
    assert_eq!(mode(&p.dir), 0o700);
    assert_eq!(mode(&p.data()), 0o600);
    assert_eq!(mode(&p.system()), 0o600);
    assert_eq!(std::fs::metadata(p.data()).unwrap().len(), DATA_DISK_BYTES);
    assert_eq!(std::fs::read(p.system()).unwrap(), b"golden disk");
    assert_eq!(std::fs::read(p.aux()).unwrap(), b"golden aux");
    assert_eq!(std::fs::read(p.machine_id()).unwrap(), b"machine-id-bytes");
    for d in p.folders() {
        assert!(d.is_dir(), "{}", d.display());
        assert_eq!(mode(&d), 0o700);
    }
    assert!(partials(&p).is_empty());
    let stamp = Stamp::read(&p.stamp()).unwrap();
    assert!(stamp.clean);
    assert!(stamp.data_id.ends_with("-0"));
    // The stamp keeps unknown fields.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(p.stamp()).unwrap()).unwrap();
    v["later"] = serde_json::json!({"x": 1});
    std::fs::write(p.stamp(), v.to_string()).unwrap();
    let s = Stamp::read(&p.stamp()).unwrap();
    assert_eq!(s.extra["later"]["x"], 1);
    s.write(&p.stamp()).unwrap();
    assert!(std::fs::read_to_string(p.stamp())
        .unwrap()
        .contains("later"));
    // A second prepare of Warm or Cold changes nothing.
    std::fs::write(p.data(), b"my data").unwrap();
    let before: Vec<_> = std::fs::read_dir(&p.dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name(), e.metadata().unwrap().len()))
        .collect();
    prepare(&p, &g, &now(), Start::Warm, &mut h).unwrap();
    prepare(&p, &g, &now(), Start::Cold, &mut h).unwrap();
    let after: Vec<_> = std::fs::read_dir(&p.dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name(), e.metadata().unwrap().len()))
        .collect();
    assert_eq!(before, after);
    assert_eq!(std::fs::read(p.data()).unwrap(), b"my data");
    let _ = &h;
    drop(rec);
    assert!(b.is_empty(), "a fresh start makes no backup");
}

#[test]
fn prepare_interrupted_then_repair_keeps_data() {
    let t = tempfile::tempdir().unwrap();
    let g = golden(t.path());
    let p = full_profile(t.path());
    std::fs::write(p.data(), b"precious").unwrap();
    std::fs::write(p.downloads().join("file"), b"dl").unwrap();
    let old_stamp = Stamp::read(&p.stamp()).unwrap();
    // A blank data disk and a golden change, interrupted after the clone:
    // the clone closure writes the partial file and then panics, as a
    // crash would.
    let mut other = now();
    other.golden = "15.3 (24D1)".into();
    let clone = |src: &Path, dst: &Path| -> io::Result<()> {
        copy_clone(src, dst)?;
        panic!("crash after the clone");
    };
    let mut backups = Vec::new();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut h = Hooks {
            clone: &clone,
            machine_id: &mid,
            backup: &mut |_: &Profile, reason: &str| {
                backups.push(reason.to_string());
                Ok(())
            },
            format_data: &no_format,
        };
        prepare(&p, &g, &other, Start::Stale(Reason::GoldenChanged), &mut h)
    }));
    assert!(r.is_err());
    assert_eq!(
        backups,
        Vec::<String>::new(),
        "a blank disk: nothing to back up"
    );
    assert!(partial(&p.system()).exists());
    assert!(!p.stamp().exists(), "the stamp goes before the clone");
    assert_eq!(decide(&p, &other, CLONE_NEEDS), Ok(Start::Repair));
    // The repair.
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(&p, &g, &other, Start::Repair, &mut h).unwrap();
    let _ = &h;
    drop(rec);
    assert_eq!(
        b,
        Vec::<String>::new(),
        "no pair to back up: system.img was gone"
    );
    assert!(partials(&p).is_empty());
    assert_eq!(std::fs::read(p.data()).unwrap(), b"precious");
    assert_eq!(std::fs::read(p.downloads().join("file")).unwrap(), b"dl");
    assert_eq!(std::fs::read(p.system()).unwrap(), b"golden disk");
    assert!(!p.state().exists());
    let stamp = Stamp::read(&p.stamp()).unwrap();
    assert_eq!(stamp.golden, "15.3 (24D1)");
    assert_eq!(
        line(Start::Repair, None, &other),
        "the profile was incomplete, paddock rebuilt its system part and kept your data. The start continues with a cold boot"
    );
    assert_eq!(decide(&p, &other, 0), Ok(Start::Cold));
    // The repaired profile made a new data id: the old stamp was gone.
    assert_ne!(stamp.data_id, old_stamp.data_id);
}

#[test]
fn only_data_and_all_remove_data_img() {
    let t = tempfile::tempdir().unwrap();
    for what in What::ALL {
        let p = full_profile(t.path().join(format!("{what:?}")).as_path());
        std::fs::write(partial(&p.aux()), b"x").unwrap();
        std::fs::write(p.apps().join("A.app"), b"x").unwrap();
        let names: Vec<String> = targets(&p, what)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        let touches_data = matches!(what, What::Data | What::All);
        assert_eq!(
            names.iter().any(|n| n == "data.img" || n == "home"),
            touches_data,
            "{what:?}: {names:?}"
        );
        remove(&p, what).unwrap();
        assert_eq!(p.data().exists(), !touches_data, "{what:?}");
        match what {
            What::Partials => assert!(!partial(&p.aux()).exists() && p.aux().exists()),
            What::Apps => assert!(!p.apps().exists() && p.downloads().exists()),
            What::All => assert!(!p.dir.exists()),
            _ => {}
        }
    }
}

#[test]
fn stale_golden_keeps_data_and_app_copy() {
    let t = tempfile::tempdir().unwrap();
    let g = golden(t.path());
    let p = full_profile(t.path());
    std::fs::write(p.data(), b"precious").unwrap();
    std::fs::write(p.apps().join("Sample Notes.app"), b"bundle").unwrap();
    std::fs::write(p.machine_id(), b"keep the id").unwrap();
    std::fs::write(&g.disk, b"new golden disk").unwrap();
    let mut other = now();
    other.golden = "15.3 (24D1)".into();
    assert_eq!(
        decide(&p, &other, CLONE_NEEDS),
        Ok(Start::Stale(Reason::GoldenChanged))
    );
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(&p, &g, &other, Start::Stale(Reason::GoldenChanged), &mut h).unwrap();
    assert_eq!(std::fs::read(p.system()).unwrap(), b"new golden disk");
    assert_eq!(std::fs::read(p.data()).unwrap(), b"precious");
    assert_eq!(
        std::fs::read(p.apps().join("Sample Notes.app")).unwrap(),
        b"bundle"
    );
    assert_eq!(std::fs::read(p.machine_id()).unwrap(), b"keep the id");
    assert!(!p.state().exists());
    assert!(partials(&p).is_empty());
    assert_eq!(decide(&p, &other, 0), Ok(Start::Cold));
    // The other stale reasons discard only the state.
    let p2 = full_profile(t.path().join("c").as_path());
    let old = Stamp::read(&p2.stamp()).unwrap();
    let mut host = now();
    host.host_build = "25A1".into();
    assert_eq!(migrate(&old, &host, false), vec![Migration::State]);
    assert_eq!(
        migrate(&old, &other, false),
        vec![Migration::System, Migration::Aux, Migration::State]
    );
    assert_eq!(
        migrate(&old, &other, true),
        vec![],
        "a formatted disk keeps its system disk"
    );
    assert_eq!(migrate(&old, &now(), false), vec![]);
    assert!(p2.state().exists());
    prepare(&p2, &g, &host, Start::Stale(Reason::HostChanged), &mut h).unwrap();
    assert!(!p2.state().exists());
    assert_eq!(std::fs::read(p2.system()).unwrap(), b"golden disk");
    assert!(p2.stamp().exists());
    let _ = &h;
    drop(rec);
    assert_eq!(b, Vec::<String>::new(), "a blank data disk needs no backup");
}

#[test]
fn begin_and_finish_move_the_counter() {
    let t = tempfile::tempdir().unwrap();
    let p = full_profile(t.path());
    let s0 = Stamp::read(&p.stamp()).unwrap();
    assert_eq!(s0.state_id.as_deref(), Some(s0.data_id.as_str()));
    let mut s1 = begin(&p, &now(), Start::Warm).unwrap();
    assert_eq!(s1.data_id, s0.next_data_id());
    assert!(!s1.clean);
    // The warm start keeps the state id of the file it restores.
    assert_eq!(s1.state_id, s0.state_id);
    finish(&p, &mut s1, true, None, 1_700_000_000).unwrap();
    let s2 = Stamp::read(&p.stamp()).unwrap();
    assert!(s2.clean);
    assert_eq!(s2.state_id, None);
    assert_eq!(s2.saved.as_deref(), Some("2023-11-14T22:13:20Z"));
    assert_eq!(mode(&p.stamp()), 0o600);
}

#[test]
fn display_and_sizes_parse() {
    let d: Display = "2560x1600@2".parse().unwrap();
    assert_eq!(
        d,
        Display {
            w: 2560,
            h: 1600,
            scale: 2
        }
    );
    assert_eq!(d.to_string(), "2560x1600@2");
    for bad in ["2560x1600", "x@2", "10x10@1", "2560x1600@5", "ax1@1"] {
        assert!(bad.parse::<Display>().is_err(), "{bad}");
    }
    assert_eq!(parse_size("2G"), Some(2 << 30));
    assert_eq!(parse_size("500M"), Some(500 << 20));
    assert_eq!(parse_size("1.5 GiB"), Some(3 << 29));
    assert_eq!(parse_size("1024"), Some(1024));
    assert_eq!(parse_size("x"), None);
    assert_eq!(size_text(8 << 30), "8 GiB");
    assert_eq!(size_text(1_288_490_188), "1.2 GiB");
    assert_eq!(size_text(512 << 20), "0.5 GiB");
    assert_eq!(safe_name("15.3 (24D1)"), "15.3_24D1");
    assert_eq!(
        commit(Path::new("/x/y")).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(free_bytes(Path::new("/definitely/missing/path")) > 0 || cfg!(not(unix)));
}

/// The migration table: each pair of versions, and what goes.
#[test]
fn migrate_table() {
    use Migration::*;
    let n = now();
    let same = n.stamp("abcd-0".into());
    let golden_changed = Now {
        golden: "15.3 (24D1)".into(),
        ..n.clone()
    };
    let paddock_minor = Now {
        paddock: "0.9.1".into(),
        agent: "0.9.1".into(),
        protocol: Version { major: 1, minor: 4 },
        ..n.clone()
    };
    let paddock_major = Now {
        paddock: "1.0.0".into(),
        agent: "1.0.0".into(),
        protocol: Version { major: 2, minor: 0 },
        ..n.clone()
    };
    let host_build = Now {
        host_build: "24D1".into(),
        ..n.clone()
    };
    let framework = Now {
        vz: "vz-2".into(),
        ..n.clone()
    };
    let display = Now {
        display: Display {
            w: 3456,
            h: 2234,
            scale: 2,
        },
        ..n.clone()
    };
    let table: &[(&str, &Now, Vec<Migration>, Option<Reason>)] = &[
        ("same golden and paddock", &n, vec![], None),
        (
            "new golden",
            &golden_changed,
            vec![System, Aux, State],
            Some(Reason::GoldenChanged),
        ),
        ("new paddock, same major", &paddock_minor, vec![], None),
        (
            "new major",
            &paddock_major,
            vec![State],
            Some(Reason::ProtocolChanged),
        ),
        (
            "new host build",
            &host_build,
            vec![State],
            Some(Reason::HostChanged),
        ),
        (
            "new framework",
            &framework,
            vec![State],
            Some(Reason::HostChanged),
        ),
        (
            "new display",
            &display,
            vec![State],
            Some(Reason::DisplayChanged),
        ),
    ];
    for (name, host, want, reason) in table {
        assert_eq!(&migrate(&same, host, false), want, "{name}");
        assert_eq!(stale_reason(&same, host, false), *reason, "{name}");
        // With a formatted data disk the golden is no reason: the
        // profile keeps its system disk.
        let bound_reason = reason.filter(|r| *r != Reason::GoldenChanged);
        assert_eq!(
            stale_reason(&same, host, true),
            bound_reason,
            "{name} (bound)"
        );
        assert!(
            !migrate(&same, host, true)
                .iter()
                .any(|m| matches!(m, System | Aux | Backup)),
            "{name} (bound)"
        );
        // Every step that removes something is in the removals of the
        // start that `decide` would give, so `prepare` carries it out.
        if let Some(r) = reason {
            let removals = removals(Start::Stale(*r));
            for step in want.iter().filter_map(|m| m.removal()) {
                assert!(removals.contains(&step), "{name}: {step:?}");
            }
            assert!(!removals.contains(&What::Data), "{name}");
        }
    }
    // A new golden wins over every other difference: one backup, one clone.
    let everything = Now {
        golden: "15.3 (24D1)".into(),
        ..paddock_major.clone()
    };
    assert_eq!(migrate(&same, &everything, false), vec![System, Aux, State]);
    assert_eq!(migrate(&same, &everything, true), vec![State]);
    // An unknown schema: the state goes, data.img stays.
    let mut foreign = same.clone();
    foreign.schema = SCHEMA + 1;
    assert_eq!(migrate(&foreign, &n, false), vec![State]);
    assert_eq!(stale_reason(&foreign, &n, false), Some(Reason::Unreadable));
    assert!(!removals(Start::Stale(Reason::Unreadable)).contains(&What::Data));
    let t = tempfile::tempdir().unwrap();
    let p = full_profile(t.path());
    std::fs::write(p.data(), b"precious").unwrap();
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(p.stamp()).unwrap()).unwrap();
    raw["schema"] = serde_json::json!(SCHEMA + 1);
    std::fs::write(p.stamp(), serde_json::to_vec(&raw).unwrap()).unwrap();
    assert_eq!(
        decide(&p, &n, CLONE_NEEDS),
        Ok(Start::Stale(Reason::Unreadable))
    );
    let mut b = Vec::new();
    let mut rec = recorder(&mut b);
    let mut h = hooks(&mut rec);
    prepare(
        &p,
        &golden(t.path()),
        &n,
        Start::Stale(Reason::Unreadable),
        &mut h,
    )
    .unwrap();
    assert!(!p.state().exists());
    assert_eq!(std::fs::read(p.data()).unwrap(), b"precious");
    drop(rec);
    assert!(b.is_empty(), "no backup for a stale state alone");
    for m in [Backup, System, Aux, State] {
        assert!(!m.word().is_empty());
    }
}
