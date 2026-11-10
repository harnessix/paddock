use super::*;
use crate::storage::settings::GIB;
use crate::storage::tests::setup;

fn vol(free: u64, total: u64) -> Volume {
    Volume {
        path: "/data".into(),
        holds: vec!["worktrees".into()],
        free,
        total,
        dev: 1,
    }
}

fn config(env: &HostEnv, text: &str) {
    let dir = env.paddock_config();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

#[test]
fn the_floors_give_a_level_per_volume() {
    let s = Settings::default();
    // The disk of the report: 906 GiB with 9.3 GiB free.
    assert_eq!(vol(9 * GIB, 906 * GIB).level(&s), Level::Full);
    assert_eq!(vol(15 * GIB, 906 * GIB).level(&s), Level::Low);
    assert_eq!(vol(49 * GIB, 906 * GIB).level(&s), Level::Low);
    assert_eq!(vol(50 * GIB, 906 * GIB).level(&s), Level::Ok);
    // A small disk: 5% and 10% count, not the GiB numbers.
    assert_eq!(vol(4 * GIB, 100 * GIB).level(&s), Level::Full);
    assert_eq!(vol(6 * GIB, 100 * GIB).level(&s), Level::Low);
    assert_eq!(vol(11 * GIB, 100 * GIB).level(&s), Level::Ok);
    // Floors at 0 are off.
    let off = Settings {
        hard: Floor { gib: 0, percent: 5 },
        soft: Floor {
            gib: 50,
            percent: 0,
        },
        ..Settings::default()
    };
    assert_eq!(vol(0, 906 * GIB).level(&off), Level::Ok);
    assert_eq!(
        worst(&[vol(60 * GIB, 906 * GIB), vol(1, 906 * GIB)], &s),
        Level::Full
    );
    assert_eq!(worst(&[], &s), Level::Ok);
}

#[test]
fn the_volumes_are_one_entry_per_file_system() {
    let (t, env) = setup();
    let p = t.path().join("project");
    std::fs::create_dir_all(&p).unwrap();
    // The paddock folders do not exist yet: their nearest parent counts.
    let v = volumes(&folders(&env, Some(&p)));
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(
        v[0].holds,
        vec!["worktrees", "sandbox homes", "build cache", "the project"]
    );
    assert!(v[0].total > 0 && v[0].free <= v[0].total);
}

#[test]
fn a_notification_goes_out_once_per_hour_and_key() {
    let (_t, env) = setup();
    let now = 1_800_000_000;
    assert!(notify_due(&env, "low", now, NOTIFY_EVERY));
    assert!(!notify_due(&env, "low", now + 60, NOTIFY_EVERY));
    assert!(notify_due(&env, "full", now + 60, NOTIFY_EVERY));
    assert!(!notify_due(
        &env,
        "low",
        now + NOTIFY_EVERY - 1,
        NOTIFY_EVERY
    ));
    assert!(notify_due(&env, "low", now + NOTIFY_EVERY, NOTIFY_EVERY));
    // A clock that went back does not block it for long.
    assert!(notify_due(
        &env,
        "full",
        now - 10 * NOTIFY_EVERY,
        NOTIFY_EVERY
    ));
    // A broken record file is replaced.
    std::fs::write(notify_file(&env), "nonsense").unwrap();
    assert!(notify_due(&env, "low", now, NOTIFY_EVERY));
}

#[test]
fn below_the_hard_floor_the_run_is_refused_after_the_cleanup() {
    let (t, env) = setup();
    // A floor as large as the disk: never enough space.
    config(
        &env,
        "[storage]\nhard_floor_gib = 1048576\nhard_floor_percent = 100\n",
    );
    let e = before_run(&env, Some(t.path()), true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not enough free space"), "{e}");
    assert!(e.contains("paddock gc --worktrees --yes"), "{e}");
    assert!(e.contains("hard_floor_gib"), "{e}");
    assert!(e.contains(" free of "), "{e}");
    // The foreground cleanup ran first, though the background one was not due.
    let st = crate::storage::auto::load(&env).expect("the cleanup ran");
    assert!(st.ended.is_some());
}

#[test]
fn below_the_soft_floor_the_run_starts_with_one_notification_per_hour() {
    let (t, env) = setup();
    config(
        &env,
        "[storage]\nhard_floor_gib = 0\nsoft_floor_gib = 1048576\nsoft_floor_percent = 100\n",
    );
    before_run(&env, Some(t.path()), true).unwrap();
    let seen = std::fs::read_to_string(notify_file(&env)).unwrap();
    assert!(seen.contains("\"low\""), "{seen}");
    // The second start in the hour sends none.
    assert!(!notify_due(&env, "low", state::now(), NOTIFY_EVERY));
    before_run(&env, Some(t.path()), true).unwrap();
    // Above both floors: nothing to say.
    config(&env, "[storage]\nhard_floor_gib = 0\nsoft_floor_gib = 0\n");
    before_run(&env, Some(t.path()), true).unwrap();
}

#[test]
fn a_bad_setting_does_not_stop_a_run() {
    let (t, env) = setup();
    config(&env, "[storage]\nhard_floor_percent = \"lots\"\n");
    let (s, w) = Settings::load(&env);
    assert_eq!(s, Settings::default());
    assert_eq!(w.len(), 1);
    // The default floors on the test volume decide, not the bad value.
    let level = worst(&volumes(&folders(&env, Some(t.path()))), &s);
    assert_eq!(
        before_run(&env, Some(t.path()), true).is_ok(),
        level != Level::Full
    );
}

#[test]
fn the_watch_notifies_at_the_hard_floor_and_stops_when_dropped() {
    let (t, env) = setup();
    config(
        &env,
        "[storage]\nhard_floor_gib = 1048576\nhard_floor_percent = 100\n",
    );
    assert!(check_running(&env, t.path()));
    std::fs::remove_file(notify_file(&env)).unwrap();
    let w = Watch::start_every(&env, t.path(), Duration::from_millis(20));
    let start = Instant::now();
    while !notify_file(&env).exists() {
        assert!(start.elapsed() < Duration::from_secs(5), "no check ran");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(w);
    // After the drop, the thread ends: no new record.
    std::thread::sleep(Duration::from_millis(50));
    std::fs::remove_file(notify_file(&env)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(!notify_file(&env).exists());
    // Above the floor: no notification.
    config(&env, "[storage]\nhard_floor_gib = 0\n");
    assert!(!check_running(&env, t.path()));
}

#[test]
fn the_doctor_shows_the_volumes_the_floors_and_the_last_cleanup() {
    let (_t, env) = setup();
    let l = doctor_lines(&env).join("\n");
    assert!(l.contains("Disk space:"), "{l}");
    assert!(l.contains("runs stop below"), "{l}");
    assert!(l.contains("has not run yet"), "{l}");
    std::fs::create_dir_all(env.paddock_data().join("homes")).unwrap();
    crate::storage::auto::pass(&env, &Settings::default(), true, None);
    let l = doctor_lines(&env).join("\n");
    assert!(l.contains("last automatic cleanup"), "{l}");
    assert!(l.contains("sizes"), "{l}");
    config(&env, "[storage]\nauto_gc = false\nsoft_floor_gib = -3\n");
    let l = doctor_lines(&env).join("\n");
    assert!(l.contains("cleanup is off"), "{l}");
    assert!(l.contains("soft_floor_gib must be"), "{l}");
}

#[test]
fn doctor_marks_nearly_full_filesystems_as_warning() {
    let (_t, env) = setup();
    config(
        &env,
        "[storage]\nhard_floor_gib = 0\nsoft_floor_gib = 1048576\nsoft_floor_percent = 100\n",
    );
    let lines = doctor_lines(&env);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("WARNING") && line.contains(" free of ")),
        "{lines:?}"
    );
}
