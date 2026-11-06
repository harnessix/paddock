use super::*;
use std::os::unix::fs::MetadataExt;

const GIB: u64 = 1 << 30;

fn profile(t: &Path) -> Profile {
    let p = Profile::new(t.join("profiles").join("app").join("default"));
    std::fs::create_dir_all(&p.dir).unwrap();
    std::fs::write(p.data(), b"data bytes").unwrap();
    std::fs::write(p.system(), b"system bytes").unwrap();
    std::fs::write(p.aux(), b"aux bytes").unwrap();
    std::fs::write(p.machine_id(), b"mid").unwrap();
    let now = super::super::session::Now {
        golden: "15.2 (24C101)".into(),
        host_build: "24C101".into(),
        vz: "vz".into(),
        protocol: super::super::proto::VERSION,
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: super::super::session::Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    };
    let mut stamp = now.stamp("abcd-3".into());
    stamp.state_id = Some("abcd-3".into());
    stamp.state_len = Some(5);
    stamp.data_prepared = true;
    stamp.write(&p.stamp()).unwrap();
    std::fs::write(p.state(), b"state").unwrap();
    p
}

fn copy(src: &Path, dst: &Path) -> io::Result<()> {
    super::super::session::copy_clone(src, dst)
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().mode() & 0o7777
}

#[test]
fn lands_0600_and_retention() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let day = 24 * 3600;
    let t0 = 1_700_000_000;
    // Five automatic backups, one a day, and one manual in between.
    let mut made = Vec::new();
    for i in 0..5u64 {
        match backup(&p, "pre-repair", false, t0 + i * day, 10 * GIB, &copy).unwrap() {
            Outcome::Made(d) => made.push(d),
            other => panic!("{other:?}"),
        }
    }
    let manual = match backup(&p, "manual", true, t0 + 2 * day + 1, 10 * GIB, &copy).unwrap() {
        Outcome::Made(d) => d,
        other => panic!("{other:?}"),
    };
    assert!(made[0].starts_with(p.backups()));
    assert_eq!(mode(&made[0]), 0o700);
    for f in FILES {
        assert_eq!(mode(&made[0].join(f)), 0o600, "{f}");
    }
    assert_eq!(
        std::fs::read(made[0].join("data.img")).unwrap(),
        b"data bytes"
    );
    assert_eq!(
        std::fs::read(made[0].join("system.img")).unwrap(),
        b"system bytes"
    );
    assert!(made[0].join("stamp.json").exists());
    assert!(super::super::layout::partials_in(&p.backups()).is_empty());
    let all = list(&p);
    assert_eq!(all.len(), 6);
    assert!(all.iter().all(|b| !b.data_only));
    assert_eq!(newest(&p).unwrap().dir, made[4]);
    // Retention at day 4: the newest 3 automatic stay, the manual stays.
    let gone = retain(&p, t0 + 4 * day).unwrap();
    assert_eq!(gone, vec![made[0].clone(), made[1].clone()]);
    assert!(manual.exists());
    // Retention 31 days later: every automatic goes, the manual stays.
    let gone = retain(&p, t0 + 4 * day + 31 * day).unwrap();
    assert_eq!(gone.len(), 3);
    let left = list(&p);
    assert_eq!(left.len(), 1);
    assert!(left[0].meta.manual);
    assert_eq!(left[0].when(), "2023-11-16 22:13");
    // A data-only file of before is listed as such and never restored.
    std::fs::write(p.backups().join("old.img"), b"x").unwrap();
    let old = list(&p).into_iter().find(|b| b.data_only).unwrap();
    assert!(restore(&p, &old, t0, &copy)
        .unwrap_err()
        .to_string()
        .contains("data disk alone"));
    assert!(newest(&p).unwrap().meta.manual);
    // `find` by name, by a part of it, or by path.
    assert_eq!(find(&p, &left[0].name).unwrap().dir, left[0].dir);
    assert_eq!(find(&p, "manual-").unwrap().dir, left[0].dir);
    assert_eq!(
        find(&p, &left[0].dir.display().to_string()).unwrap().dir,
        left[0].dir
    );
    assert!(find(&p, "nothing").is_none());
}

#[test]
fn skipped_without_space() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let needed = used_bytes(&p.data());
    assert!(needed > 0);
    assert_eq!(
        backup(&p, "pre-reset", false, 1, needed - 1, &copy).unwrap(),
        Outcome::Skipped {
            needed,
            free: needed - 1
        }
    );
    assert!(list(&p).is_empty());
    let line = text::backup_skipped(needed, needed - 1);
    assert!(line.contains("no backup"), "{line}");
    // No data disk: nothing to back up. No system disk: not a pair.
    std::fs::remove_file(p.system()).unwrap();
    assert!(backup(&p, "x", false, 1, GIB, &copy)
        .unwrap_err()
        .to_string()
        .contains("no system.img"));
    std::fs::remove_file(p.data()).unwrap();
    assert_eq!(
        backup(&p, "x", false, 1, GIB, &copy).unwrap(),
        Outcome::Nothing
    );
}

#[test]
fn restore_clears_state_id() {
    let t = tempfile::tempdir().unwrap();
    let p = profile(t.path());
    let made = match backup(&p, "manual", true, 100, 10 * GIB, &copy).unwrap() {
        Outcome::Made(d) => d,
        other => panic!("{other:?}"),
    };
    std::fs::write(p.data(), b"newer data").unwrap();
    std::fs::write(p.system(), b"newer system").unwrap();
    let b = find(&p, "manual").unwrap();
    let aside = restore(&p, &b, 200, &copy).unwrap();
    assert_eq!(std::fs::read(p.data()).unwrap(), b"data bytes");
    assert_eq!(std::fs::read(p.system()).unwrap(), b"system bytes");
    assert_eq!(std::fs::read(p.machine_id()).unwrap(), b"mid");
    assert!(aside
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("pre-restore-"));
    assert_eq!(
        std::fs::read(aside.join("data.img")).unwrap(),
        b"newer data"
    );
    assert_eq!(
        std::fs::read(aside.join("system.img")).unwrap(),
        b"newer system"
    );
    assert!(aside.join("stamp.json").exists());
    assert_eq!(mode(&p.data()), 0o600);
    let stamp = Stamp::read(&p.stamp()).unwrap();
    assert_eq!(stamp.state_id, None);
    assert_eq!(stamp.data_id, "abcd-3");
    assert!(stamp.data_prepared);
    assert!(
        p.state().exists(),
        "the state file stays until the next start discards it"
    );
    let now = super::super::session::Now {
        golden: "15.2 (24C101)".into(),
        host_build: "24C101".into(),
        vz: "vz".into(),
        protocol: super::super::proto::VERSION,
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: super::super::session::Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    };
    use super::super::session::{decide, Reason, Start};
    assert_eq!(decide(&p, &now, 0), Ok(Start::Stale(Reason::StateDamaged)));
    assert!(super::super::layout::partials_in(&p.dir).is_empty());
    assert!(made.join(META).exists());
    // Both the backup and the pre-restore copy are restorable backups.
    let names: Vec<bool> = list(&p).iter().map(|b| b.data_only).collect();
    assert_eq!(names, vec![false, false]);
}
