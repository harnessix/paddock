//! Tests of the sync: a folder that goes, changes or loses its files.

use super::tests::*;
use super::*;
use crate::loft::{Loft, NOTES};
use std::process::Command;

#[test]
fn a_held_lock_times_out_with_an_error() {
    let (t, env) = temp_env();
    let (loft, a, _) = two(t.path(), &env);
    let held = std::fs::File::open(loft.root.join("sync.lock")).unwrap();
    // SAFETY: flock on an open file.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);
    let start = std::time::Instant::now();
    let e = lock_within(&loft, Duration::from_millis(300))
        .err()
        .unwrap();
    assert!(format!("{e:#}").contains("another sync"), "{e:#}");
    assert!(start.elapsed() < Duration::from_secs(5));
    drop(held);
    assert!(lock_within(&loft, Duration::from_millis(300)).is_ok());
    assert!(sync(&loft, &a, "a").is_ok());
}

#[test]
fn a_folder_that_goes_during_a_sync_deletes_no_note() {
    let notes = [
        ("plan.md", "plan"),
        ("tickets/T1.md", "t1"),
        ("from-b.md", "b"),
    ];
    let scenarios = [
        "git clean -fdx",
        "rm -rf .paddock",
        "rm -rf, then an agent writes a note with mkdir -p",
        "every file goes, the folder stays",
    ];
    for at in scenarios {
        let (t, env) = temp_env();
        let (loft, a, b) = two(t.path(), &env);
        write(&a.join(".paddock/plan.md"), "plan");
        write(&a.join(".paddock/tickets/T1.md"), "t1");
        write(&b.join(".paddock/from-b.md"), "b");
        sync(&loft, &a, "a").unwrap();
        sync(&loft, &b, "b").unwrap();
        sync(&loft, &a, "a").unwrap();
        let dot = b.join(NOTES);
        let (bb, d) = (b.clone(), dot.clone());
        let at2 = at.to_string();
        // Between base_of and the scan of a watcher sync.
        BEFORE_SCAN.with(|f| {
            *f.borrow_mut() = Some(Box::new(move || match at2.as_str() {
                "git clean -fdx" => git(&bb, &["clean", "-fdxq"]).clear(),
                "rm -rf .paddock" => std::fs::remove_dir_all(&d).unwrap(),
                "rm -rf, then an agent writes a note with mkdir -p" => {
                    std::fs::remove_dir_all(&d).unwrap();
                    write(&d.join("new.md"), "new");
                }
                _ => {
                    for e in std::fs::read_dir(&d).unwrap().flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            std::fs::remove_dir_all(&p).unwrap();
                        } else {
                            std::fs::remove_file(&p).unwrap();
                        }
                    }
                }
            }))
        });
        let res = sync(&loft, &b, "run-1");
        assert!(BEFORE_SCAN.with(|f| f.borrow().is_none()), "{at}: no pause");
        if let Ok(r) = &res {
            assert!(r.deleted.is_empty(), "{at}: {r:?}");
        }
        assert_all_notes(&loft, &a, &notes, at);
        if at == "rm -rf, then an agent writes a note with mkdir -p" {
            assert_eq!(read(&loft.notes().join("new.md")), "new", "{at}");
        }
        // The folder comes back with every note, and deletes sync again.
        std::fs::create_dir_all(&dot).unwrap();
        let r = sync(&loft, &b, "b").unwrap();
        assert!(r.deleted.is_empty(), "{at}: {r:?}");
        for (k, v) in notes {
            assert_eq!(read(&dot.join(k)), v, "{at}: {k} did not come back");
        }
        std::fs::remove_file(dot.join("plan.md")).unwrap();
        sync_delete(&loft, &b, "b", &["plan.md"], at);
    }
}

#[test]
fn a_restored_backup_of_the_folder_loses_no_note() {
    for how in ["mv", "cp -a"] {
        let (t, env) = temp_env();
        let (loft, a, b) = two(t.path(), &env);
        write(&b.join(".paddock/plan.md"), "plan v1");
        sync(&loft, &b, "b").unwrap();
        sync(&loft, &a, "a").unwrap();
        let dot = b.join(NOTES);
        let backup = t.path().join("backup");
        let o = Command::new("cp")
            .arg("-a")
            .arg(&dot)
            .arg(&backup)
            .status()
            .unwrap();
        assert!(o.success());
        // After the backup: a new note and a change.
        write(&dot.join("new.md"), "new");
        write(&dot.join("plan.md"), "plan v2");
        sync(&loft, &b, "b").unwrap();
        assert_eq!(read(&loft.notes().join("new.md")), "new");
        // The restore.
        std::fs::remove_dir_all(&dot).unwrap();
        if how == "mv" {
            std::fs::rename(&backup, &dot).unwrap();
        } else {
            let o = Command::new("cp")
                .arg("-a")
                .arg(&backup)
                .arg(&dot)
                .status()
                .unwrap();
            assert!(o.success());
        }
        let r = sync(&loft, &b, "b").unwrap();
        assert!(r.deleted.is_empty(), "{how}: {r:?}");
        assert_eq!(read(&loft.notes().join("new.md")), "new", "{how}");
        assert_eq!(read(&loft.notes().join("plan.md")), "plan v2", "{how}");
        assert_eq!(read(&dot.join("new.md")), "new", "{how}");
        assert_eq!(read(&dot.join("plan.md")), "plan v2", "{how}");
        assert_all_notes(&loft, &a, &[("new.md", "new"), ("plan.md", "plan v2")], how);
    }
}

/// Ten notes in b, written in order, then synced to the loft and to a.
pub(super) fn ten(t: &Path, env: &HostEnv) -> (Loft, PathBuf, PathBuf, Vec<(String, String)>) {
    let (loft, a, b) = two(t, env);
    let notes: Vec<(String, String)> = (0..10)
        .map(|i| (format!("n{i}.md"), format!("note {i}")))
        .collect();
    for (k, v) in &notes {
        write(&b.join(NOTES).join(k), v);
    }
    sync(&loft, &b, "b").unwrap();
    sync(&loft, &a, "a").unwrap();
    (loft, a, b, notes)
}

pub(super) fn rm(dot: &Path, notes: &[(String, String)], range: std::ops::Range<usize>) {
    for (k, _) in &notes[range] {
        std::fs::remove_file(dot.join(k)).unwrap();
    }
}

pub(super) fn as_refs(notes: &[(String, String)]) -> Vec<(&str, &str)> {
    notes
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

#[test]
fn a_folder_that_loses_its_files_in_steps_during_syncs_deletes_no_note() {
    for at in ["the rm goes on, then ends", "the rm reaches the marker"] {
        let (t, env) = temp_env();
        let (loft, a, b, notes) = ten(t.path(), &env);
        let dot = b.join(NOTES);
        // An rm in creation order, the marker last: during the scan of
        // the first sync, the first three notes go.
        let done = std::rc::Rc::new(std::cell::Cell::new(false));
        let (d, n, flag) = (dot.clone(), notes.clone(), done.clone());
        IN_SCAN.with(|f| {
            *f.borrow_mut() = Some(Box::new(move |dir: &Path| {
                if !flag.get() && dir == d.as_path() {
                    flag.set(true);
                    rm(&d, &n, 0..3);
                }
            }))
        });
        let r = sync(&loft, &b, "run-1").unwrap();
        IN_SCAN.with(|f| *f.borrow_mut() = None);
        assert!(done.get(), "{at}: the hook did not run");
        assert!(r.deleted.is_empty(), "{at}: {r:?}");
        assert_eq!(r.waiting, ["n0.md", "n1.md", "n2.md"], "{at}");
        assert_all_notes(&loft, &a, &as_refs(&notes), at);
        if at == "the rm goes on, then ends" {
            // The second sync, later: seven of ten are gone, with the
            // marker still there. No delete, and the notes come back.
            rm(&dot, &notes, 3..7);
            later();
            let r = sync(&loft, &b, "run-1").unwrap();
            assert!(r.deleted.is_empty(), "{at}: {r:?}");
            assert!(r.waiting.is_empty(), "{at}: {r:?}");
            assert!(
                r.skipped.iter().any(|s| s.contains("7 of the 10 files")),
                "{at}: {r:?}"
            );
            assert_all_notes(&loft, &a, &as_refs(&notes), at);
            // The rm ends; later the folder comes back.
            std::fs::remove_dir_all(&dot).unwrap();
            later();
            assert!(sync(&loft, &b, "run-1").is_err(), "{at}");
            std::fs::create_dir_all(&dot).unwrap();
        } else {
            // The rm goes on and removes the marker: the folder is not the
            // one of the base any more, so the pending deletes go.
            rm(&dot, &notes, 3..5);
            std::fs::remove_file(dot.join(MARK)).unwrap();
            later();
            let r = sync(&loft, &b, "run-1").unwrap();
            assert!(r.deleted.is_empty(), "{at}: {r:?}");
            assert!(
                r.skipped.iter().any(|s| s
                    .contains("n0.md, n1.md, n2.md deleted in the work tree")
                    && s.contains("do not reach the loft")),
                "{at}: {r:?}"
            );
        }
        later();
        let r = sync(&loft, &b, "b").unwrap();
        assert!(r.deleted.is_empty(), "{at}: {r:?}");
        for (k, v) in &notes {
            assert_eq!(read(&dot.join(k)), *v, "{at}: {k} did not come back");
        }
        assert_all_notes(&loft, &a, &as_refs(&notes), at);
    }
}

#[test]
fn a_delete_waits_for_a_second_scan_of_the_same_folder() {
    let (t, env) = temp_env();
    let (loft, a, b, notes) = ten(t.path(), &env);
    let dot = b.join(NOTES);
    // Too early: the second sync is less than DELETE_WAIT later.
    std::fs::remove_file(dot.join("n0.md")).unwrap();
    for _ in 0..2 {
        let r = sync(&loft, &b, "b").unwrap();
        assert!(r.deleted.is_empty(), "{r:?}");
        assert_eq!(r.waiting, ["n0.md"]);
        assert!(loft.notes().join("n0.md").is_file());
    }
    let st = pending(&loft, &b).unwrap();
    assert_eq!(
        st,
        ["n0.md (deleted in the work tree, waits for a second sync)"]
    );
    // The file comes back before the second scan: no delete, later either.
    write(&dot.join("n0.md"), "note 0");
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert!(r.deleted.is_empty() && r.waiting.is_empty(), "{r:?}");
    std::fs::remove_file(dot.join("n0.md")).unwrap();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.waiting, ["n0.md"], "a new delete waits again: {r:?}");
    assert!(r.deleted.is_empty(), "{r:?}");
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.deleted, ["n0.md"]);
    // Two more of the nine is not most of the folder: they go, later.
    rm(&dot, &notes, 1..3);
    sync_delete(&loft, &b, "b", &["n1.md", "n2.md"], "two more");
    sync(&loft, &a, "a").unwrap();
    for k in ["n0.md", "n1.md", "n2.md"] {
        assert!(!a.join(NOTES).join(k).exists(), "{k}");
        assert!(!loft.notes().join(k).exists(), "{k}");
    }
}

#[test]
fn a_folder_without_its_marker_during_a_sync_gets_no_write() {
    let (t, env) = temp_env();
    let (loft, a, b, notes) = ten(t.path(), &env);
    let dot = b.join(NOTES);
    // A note that only the loft has, and one that only b has.
    write(&a.join(".paddock/from-a.md"), "a");
    sync(&loft, &a, "a").unwrap();
    write(&dot.join("late.md"), "late");
    // An rm that removed the marker and some notes, but is not done.
    let d = dot.clone();
    let n = notes.clone();
    BEFORE_SCAN.with(|f| {
        *f.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(d.join(MARK)).unwrap();
            rm(&d, &n, 0..4);
        }))
    });
    let list = || {
        let mut v: Vec<String> = std::fs::read_dir(&dot)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    // What the folder has after the rm step of the hook.
    let mut want = list();
    want.retain(|k| k != MARK && !notes[..4].iter().any(|(n, _)| n == k));
    let r = sync(&loft, &b, "run-1").unwrap();
    assert!(BEFORE_SCAN.with(|f| f.borrow().is_none()), "no pause");
    assert!(r.deleted.is_empty() && r.pulled.is_empty(), "{r:?}");
    assert!(
        r.skipped
            .iter()
            .any(|s| s.contains("changed during the sync")),
        "{r:?}"
    );
    // Nothing new in the folder: no pull, no marker. So the rm can end.
    assert!(want.contains(&"late.md".to_string()) && !want.contains(&"from-a.md".to_string()));
    assert_eq!(list(), want);
    assert!(!dot.join(MARK).exists());
    // What only b had still reaches the loft.
    assert_eq!(read(&loft.notes().join("late.md")), "late");
    assert_all_notes(&loft, &a, &as_refs(&notes), "after the sync");
    // The rm ends with no error.
    std::fs::remove_dir_all(&dot).unwrap();
}

#[test]
fn a_new_device_number_of_the_same_folder_keeps_the_base() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    // After a reboot, btrfs or FUSE can give the folder another st_dev.
    let f = base_file(&loft, &b);
    let mut base: serde_json::Value = serde_json::from_str(&read(&f)).unwrap();
    let dev = base["folder"][0].as_u64().unwrap();
    base["folder"][0] = (dev + 1).into();
    write(&f, &serde_json::to_string(&base).unwrap());
    // A change in the loft is a plain pull with the old base, not a
    // conflict of a fresh start.
    write(&loft.notes().join("n0.md"), "note 0, v2");
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.pulled, ["n0.md"], "{r:?}");
    assert!(r.conflicts.is_empty(), "{r:?}");
    assert_eq!(read(&b.join(".paddock/n0.md")), "note 0, v2");
    // A delete still waits and then syncs.
    std::fs::remove_file(b.join(".paddock/n1.md")).unwrap();
    sync_delete(&loft, &b, "b", &["n1.md"], "new st_dev");
    // The base now has the real device again.
    let base: serde_json::Value = serde_json::from_str(&read(&f)).unwrap();
    assert_eq!(base["folder"][0].as_u64(), Some(dev));
}
