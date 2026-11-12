//! Tests of the sync: deletes, the loft trash and restores.

use super::folder_tests::*;
use super::tests::*;
use super::*;
use crate::loft::{Loft, NOTES};

/// The paths in the trash of `loft` with their content, the newest
/// version of each path.
fn in_trash(loft: &Loft) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for e in crate::loft::trash::list(loft) {
        let p = crate::loft::trash::root(loft).join(&e.at).join(&e.path);
        out.entry(e.path).or_insert_with(|| read(&p));
    }
    out
}

#[test]
fn a_slow_rm_during_syncs_leaves_every_note_in_the_trash_and_restore_brings_it_back() {
    // `rm -rf .paddock` on a slow file system: 20 notes, two at a time,
    // with a sync more than DELETE_WAIT after each step. No step is most
    // of the folder, so each delete reaches the loft after its wait.
    let (t, env) = temp_env();
    let (loft, a, b) = two(t.path(), &env);
    let notes: Vec<(String, String)> = (0..20)
        .map(|i| (format!("d/n{i:02}.md"), format!("note {i}")))
        .collect();
    for (k, v) in &notes {
        write(&b.join(NOTES).join(k), v);
    }
    sync(&loft, &b, "b").unwrap();
    sync(&loft, &a, "a").unwrap();
    let dot = b.join(NOTES);
    let mut trashed = 0;
    for step in 0..10 {
        rm(&dot, &notes, step * 2..step * 2 + 2);
        later();
        let r = sync(&loft, &b, "run-1").unwrap();
        trashed += r.trashed.len();
        sync(&loft, &a, "a").unwrap();
    }
    assert!(
        trashed >= 10,
        "the deletes did not reach the loft: {trashed}"
    );
    // The rm ends with the marker and the folder.
    std::fs::remove_dir_all(&dot).unwrap();
    // No note is lost: each one is in the loft or in its trash, with its
    // content.
    let bin = in_trash(&loft);
    for (k, v) in &notes {
        let at = loft.notes().join(k);
        match bin.get(k) {
            Some(t) => {
                assert_eq!(t, v, "{k} in the trash");
                assert!(!at.exists(), "{k} in the loft and in the trash");
            }
            None => assert_eq!(read(&at), *v, "{k} is lost"),
        }
    }
    assert_eq!(bin.len(), trashed);
    let list = crate::loft::trash::render(&env, &loft);
    assert!(
        list.contains("d/n00.md") && list.contains("30 days"),
        "{list}"
    );
    // Each restore brings a note back to the loft and to the work tree a.
    for k in bin.keys() {
        let lines = crate::loft::trash::restore(&loft, Some(&a), k, "host").unwrap();
        assert!(
            lines.iter().any(|l| l == &format!("restored: {k}")),
            "{k}: {lines:?}"
        );
    }
    assert!(crate::loft::trash::list(&loft).is_empty());
    assert!(crate::loft::trash::restore(&loft, Some(&a), "d/n00.md", "host").is_err());
    for (k, v) in &notes {
        assert_eq!(read(&loft.notes().join(k)), *v, "{k} in the loft");
        assert_eq!(read(&a.join(NOTES).join(k)), *v, "{k} in a");
    }
    // The folder of b comes back: it gets every note.
    std::fs::create_dir_all(&dot).unwrap();
    sync(&loft, &b, "b").unwrap();
    for (k, v) in &notes {
        assert_eq!(read(&dot.join(k)), *v, "{k} in b");
    }
}

#[test]
fn a_restore_over_a_newer_file_makes_a_conflict_copy() {
    let (t, env) = temp_env();
    let (loft, a, b) = two(t.path(), &env);
    write(&b.join(".paddock/plan.md"), "old plan");
    write(&b.join(".paddock/notes/x.md"), "x");
    sync(&loft, &b, "b").unwrap();
    sync(&loft, &a, "a").unwrap();
    std::fs::remove_file(b.join(".paddock/plan.md")).unwrap();
    std::fs::remove_file(b.join(".paddock/notes/x.md")).unwrap();
    sync_delete(&loft, &b, "b", &["notes/x.md", "plan.md"], "two deletes");
    sync(&loft, &a, "a").unwrap();
    assert!(!a.join(".paddock/plan.md").exists());
    // A new plan.md, written in a after the delete.
    write(&a.join(".paddock/plan.md"), "new plan");
    // The restore syncs a first, so the new plan is in the loft, and the
    // old one becomes the conflict copy.
    let lines = crate::loft::trash::restore(&loft, Some(&a), ".paddock/plan.md", "host").unwrap();
    let copy = lines
        .iter()
        .find_map(|l| {
            l.strip_prefix("restored: plan.md as ")
                .and_then(|r| r.split(',').next())
        })
        .unwrap_or_else(|| panic!("no conflict copy: {lines:?}"))
        .to_string();
    assert!(copy.starts_with("plan.md.conflict-host-"), "{copy}");
    assert_eq!(read(&loft.notes().join("plan.md")), "new plan");
    assert_eq!(read(&loft.notes().join(&copy)), "old plan");
    assert_eq!(read(&a.join(NOTES).join(&copy)), "old plan");
    assert_eq!(read(&a.join(".paddock/plan.md")), "new plan");
    // A folder name restores what is below it.
    let lines = crate::loft::trash::restore(&loft, Some(&a), "notes/", "host").unwrap();
    assert!(
        lines.contains(&"restored: notes/x.md".to_string()),
        "{lines:?}"
    );
    sync(&loft, &b, "b").unwrap();
    assert_eq!(read(&b.join(".paddock/notes/x.md")), "x");
    assert_eq!(read(&b.join(NOTES).join(&copy)), "old plan");
    assert!(crate::loft::trash::list(&loft).is_empty());
    assert!(crate::loft::trash::restore(&loft, None, "../x", "host").is_err());
}

#[test]
fn gc_removes_only_trash_folders_older_than_the_limit() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    // One real delete now, and three older folders.
    std::fs::remove_file(b.join(".paddock/n0.md")).unwrap();
    sync_delete(&loft, &b, "b", &["n0.md"], "one delete");
    let now = crate::state::now() + LATER.with(|l| l.get()) / 1000;
    let day = 24 * 3600;
    let root = crate::loft::trash::root(&loft);
    for age in [40, 20, 1] {
        let at = crate::loft::trash::stamp_for_tests(now - age * day);
        write(&root.join(at).join("old.md"), &format!("{age} days"));
    }
    let gone = |env: &HostEnv| -> BTreeSet<String> {
        crate::loft::trash::gc_items(env, now)
            .into_iter()
            .map(|i| match i.action {
                crate::storage::Action::LoftTrash { dir } => read(&dir.join("old.md")),
                a => panic!("{a:?}"),
            })
            .collect()
    };
    assert_eq!(gone(&env), ["40 days".to_string()].into());
    write_cfg(&env, "[loft]\nmode = \"sync\"\ntrash_days = 10\n");
    assert_eq!(
        gone(&env),
        ["40 days".to_string(), "20 days".to_string()].into()
    );
    for i in crate::loft::trash::gc_items(&env, now) {
        let crate::storage::Action::LoftTrash { dir } = &i.action else {
            unreachable!()
        };
        crate::loft::trash::remove(&env, dir).unwrap();
        assert!(!dir.exists());
    }
    let left: BTreeSet<String> = in_trash(&loft).into_values().collect();
    assert_eq!(left, ["1 days".to_string(), "note 0".to_string()].into());
    // Only a folder of a loft trash.
    assert!(crate::loft::trash::remove(&env, &loft.notes()).is_err());
    assert!(crate::loft::trash::remove(&env, &t.path().join("x/trash/20261112T000000Z")).is_err());
}

#[test]
fn a_moved_folder_is_not_a_mass_delete() {
    let (t, env) = temp_env();
    let (loft, a, b, notes) = ten(t.path(), &env);
    // `mv` of 6 of the 10 notes into a folder.
    let dot = b.join(NOTES);
    for (k, v) in &notes[..6] {
        std::fs::remove_file(dot.join(k)).unwrap();
        write(&dot.join("old").join(k), v);
    }
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.pushed.len(), 6, "{r:?}");
    assert_eq!(r.waiting.len(), 6, "{r:?}");
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.trashed.len(), 6, "{r:?}");
    sync(&loft, &b, "b").unwrap();
    sync(&loft, &a, "a").unwrap();
    // Each note once, at its new name, in b, in a and in the loft.
    for (k, v) in &notes[..6] {
        for d in [&dot, &a.join(NOTES), &loft.notes()] {
            assert!(!d.join(k).exists(), "{k} came back in {}", d.display());
            assert_eq!(read(&d.join("old").join(k)), *v);
        }
    }
}

#[test]
fn a_jump_of_the_wall_clock_does_not_end_the_wait() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    std::fs::remove_file(b.join(".paddock/n0.md")).unwrap();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.waiting, ["n0.md"]);
    JUMP.with(|j| j.set(3600 * 1000));
    let r = sync(&loft, &b, "b").unwrap();
    JUMP.with(|j| j.set(0));
    assert_eq!(r.waiting, ["n0.md"], "{r:?}");
    assert!(loft.notes().join("n0.md").is_file());
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.trashed, ["n0.md"], "{r:?}");
}

#[test]
fn a_sync_that_finds_a_delete_writes_nothing_new_into_the_folder() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    let dot = b.join(NOTES);
    write(&loft.notes().join("from-loft.md"), "pull me");
    write(&loft.notes().join("n5.md"), "note 5, v2");
    std::fs::remove_file(dot.join("n0.md")).unwrap();
    let mark = read(&dot.join(MARK));
    for step in ["first scan", "second scan"] {
        let r = sync(&loft, &b, "b").unwrap();
        assert!(r.pulled.is_empty(), "{step}: {r:?}");
        assert_eq!(r.deferred, ["from-loft.md", "n5.md"], "{step}");
        assert!(!dot.join("from-loft.md").exists(), "{step}");
        assert_eq!(read(&dot.join("n5.md")), "note 5", "{step}");
        assert_eq!(read(&dot.join(MARK)), mark, "{step}: a new marker");
        later();
    }
    // The delete is done: the next sync pulls, and writes a new marker.
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.pulled, ["from-loft.md", "n5.md"], "{r:?}");
    assert_eq!(read(&dot.join("n5.md")), "note 5, v2");
    assert_ne!(read(&dot.join(MARK)), mark);
}

#[test]
fn the_watcher_stop_finishes_a_delete_that_waits() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    std::fs::remove_file(b.join(".paddock/n0.md")).unwrap();
    let w = Watch::start(&env, &b, "s-1").unwrap();
    let lines = w.stop();
    assert!(!loft.notes().join("n0.md").exists(), "{lines:?}");
    assert_eq!(
        in_trash(&loft).get("n0.md").map(String::as_str),
        Some("note 0")
    );
    assert!(!lines.iter().any(|l| l.starts_with("waits")), "{lines:?}");
}

#[test]
fn a_restore_reaches_a_work_tree_that_deleted_the_note_too() {
    let (t, env) = temp_env();
    let (loft, a, b, _) = ten(t.path(), &env);
    // Both work trees delete n0; b syncs, a does not yet.
    std::fs::remove_file(b.join(".paddock/n0.md")).unwrap();
    std::fs::remove_file(a.join(".paddock/n0.md")).unwrap();
    sync_delete(&loft, &b, "b", &["n0.md"], "b deletes");
    crate::loft::trash::restore(&loft, Some(&b), "n0.md", "host").unwrap();
    assert_eq!(read(&b.join(".paddock/n0.md")), "note 0");
    // The base of a still has n0: without it, the delete of a would move
    // the restored note to the trash again.
    later();
    for _ in 0..3 {
        sync(&loft, &a, "a").unwrap();
        later();
    }
    assert_eq!(read(&loft.notes().join("n0.md")), "note 0");
    assert_eq!(read(&a.join(".paddock/n0.md")), "note 0");
    assert!(crate::loft::trash::list(&loft).is_empty());
}

/// Copy each file of `from` over the same path in `to`: a restore in
/// place (rsync into the folder), so `to` keeps its inode.
fn copy_over(from: &Path, to: &Path) {
    for (k, v) in tree(from) {
        write(&to.join(k), &v);
    }
}

#[test]
fn a_backup_restored_in_place_after_a_sync_with_a_delete_loses_no_note() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    let dot = b.join(NOTES);
    let backup = t.path().join("backup");
    copy_over(&dot, &backup);
    let mark = read(&dot.join(MARK));
    // After the backup: a change and a delete in one sync. The change
    // goes to the loft, the delete waits, and the marker stays.
    write(&dot.join("n1.md"), "note 1, v2");
    std::fs::remove_file(dot.join("n0.md")).unwrap();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.pushed, ["n1.md"], "{r:?}");
    assert_eq!(r.waiting, ["n0.md"], "{r:?}");
    assert_eq!(read(&dot.join(MARK)), mark);
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.trashed, ["n0.md"], "{r:?}");
    // The first sync without a delete writes a new marker.
    sync(&loft, &b, "b").unwrap();
    assert_ne!(
        read(&dot.join(MARK)),
        mark,
        "the base of v2 has the old marker"
    );
    // The restore in place: the same folder, the old marker.
    copy_over(&backup, &dot);
    assert_eq!(read(&dot.join("n1.md")), "note 1");
    sync(&loft, &b, "b").unwrap();
    let all: Vec<String> = tree(&loft.notes()).into_values().collect();
    for v in ["note 1", "note 1, v2"] {
        assert!(all.iter().any(|x| x == v), "{v} is lost: {all:?}");
    }
    assert_eq!(read(&loft.notes().join("n1.md")), "note 1, v2");
}

#[test]
fn a_restore_keeps_the_base_of_a_work_tree_with_a_newer_edit() {
    let (t, env) = temp_env();
    let (loft, a, b, _) = ten(t.path(), &env);
    // a edits n1 and does not sync; b deletes it, then it is restored.
    write(&a.join(".paddock/n1.md"), "note 1, v2");
    std::fs::remove_file(b.join(".paddock/n1.md")).unwrap();
    sync_delete(&loft, &b, "b", &["n1.md"], "b deletes");
    crate::loft::trash::restore(&loft, Some(&b), "n1.md", "host").unwrap();
    assert_eq!(read(&loft.notes().join("n1.md")), "note 1");
    // The edit of a is newer than the restored version: it takes the name.
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.pushed, ["n1.md"], "{r:?}");
    assert!(r.conflicts.is_empty(), "{r:?}");
    assert_eq!(read(&loft.notes().join("n1.md")), "note 1, v2");
    assert_eq!(read(&a.join(".paddock/n1.md")), "note 1, v2");
    assert!(
        !tree(&loft.notes()).keys().any(|k| k.contains(".conflict-")),
        "{:?}",
        tree(&loft.notes())
    );
}

#[test]
fn a_trash_move_that_fails_does_not_block_the_pulls() {
    let (t, env) = temp_env();
    let (loft, _a, b, _) = ten(t.path(), &env);
    let dot = b.join(NOTES);
    // A file where the trash folder must go: each move to it fails.
    std::fs::write(crate::loft::trash::root(&loft), "not a folder").unwrap();
    write(&loft.notes().join("from-loft.md"), "pull me");
    std::fs::remove_file(dot.join("n0.md")).unwrap();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.waiting, ["n0.md"], "{r:?}");
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert!(r.trashed.is_empty(), "{r:?}");
    assert!(r.skipped.iter().any(|s| s.starts_with("n0.md: ")), "{r:?}");
    // The note stays in the loft, and the next sync pulls again.
    assert_eq!(read(&loft.notes().join("n0.md")), "note 0");
    later();
    let r = sync(&loft, &b, "b").unwrap();
    assert!(r.deferred.is_empty(), "{r:?}");
    assert_eq!(read(&dot.join("from-loft.md")), "pull me");
    // The delete is not undone: the next sync tries the move again.
    assert!(!dot.join("n0.md").exists(), "the delete came back: {r:?}");
    assert!(r.skipped.iter().any(|s| s.starts_with("n0.md: ")), "{r:?}");
    assert!(crate::loft::sync::pending(&loft, &b)
        .unwrap()
        .iter()
        .any(|l| l.starts_with("n0.md (") && l.contains("trash failed")));
    // When the trash works again, the delete reaches it at once.
    std::fs::remove_file(crate::loft::trash::root(&loft)).unwrap();
    let r = sync(&loft, &b, "b").unwrap();
    assert_eq!(r.trashed, ["n0.md"], "{r:?}");
    assert!(!loft.notes().join("n0.md").exists());
    assert!(!dot.join("n0.md").exists());
}

#[test]
fn a_restore_as_a_conflict_copy_keeps_the_base_of_the_original_path() {
    let (t, env) = temp_env();
    let (loft, a, b, _) = ten(t.path(), &env);
    // b deletes n1, a gets the delete, then a writes a new n1.
    std::fs::remove_file(b.join(".paddock/n1.md")).unwrap();
    sync_delete(&loft, &b, "b", &["n1.md"], "b deletes");
    sync(&loft, &a, "a").unwrap();
    assert!(!a.join(".paddock/n1.md").exists());
    write(&a.join(".paddock/n1.md"), "new note 1");
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.pushed, ["n1.md"], "{r:?}");
    // a edits it again and does not sync before the restore.
    write(&a.join(".paddock/n1.md"), "new note 1, v2");
    let lines = crate::loft::trash::restore(&loft, None, "n1.md", "host").unwrap();
    assert!(lines.iter().any(|l| l.contains(" as ")), "{lines:?}");
    let copies: Vec<String> = tree(&loft.notes())
        .into_iter()
        .filter(|(k, _)| k.contains(".conflict-"))
        .map(|(k, v)| {
            assert_eq!(v, "note 1");
            k
        })
        .collect();
    assert_eq!(copies.len(), 1, "{:?}", tree(&loft.notes()));
    // The base of a still has n1: the edit goes to the loft at the name.
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.pushed, ["n1.md"], "{r:?}");
    assert!(r.conflicts.is_empty(), "{r:?}");
    assert_eq!(read(&loft.notes().join("n1.md")), "new note 1, v2");
    assert_eq!(read(&a.join(".paddock").join(&copies[0])), "note 1");
}

#[test]
fn a_restore_with_the_same_content_is_not_trashed_again_by_a_work_tree_that_deleted_it() {
    let (t, env) = temp_env();
    let (loft, a, b, _) = ten(t.path(), &env);
    // a deletes n1, and its delete waits. b deletes n1 too, and it goes
    // to the trash.
    std::fs::remove_file(a.join(".paddock/n1.md")).unwrap();
    let r = sync(&loft, &a, "a").unwrap();
    assert_eq!(r.waiting, ["n1.md"], "{r:?}");
    std::fs::remove_file(b.join(".paddock/n1.md")).unwrap();
    sync_delete(&loft, &b, "b", &["n1.md"], "b deletes");
    // The loft gets the same content at the name again, then the restore.
    write(&loft.notes().join("n1.md"), "note 1");
    let lines = crate::loft::trash::restore(&loft, None, "n1.md", "host").unwrap();
    assert!(
        lines.iter().any(|l| l.contains("the same content already")),
        "{lines:?}"
    );
    // The wait of the delete of a is over, but the restore dropped it.
    let r = sync(&loft, &a, "a").unwrap();
    assert!(r.trashed.is_empty(), "{r:?}");
    assert_eq!(r.pulled, ["n1.md"], "{r:?}");
    assert_eq!(read(&loft.notes().join("n1.md")), "note 1");
    assert_eq!(read(&a.join(".paddock/n1.md")), "note 1");
    assert!(
        !in_trash(&loft).keys().any(|k| k.ends_with("n1.md")),
        "{:?}",
        in_trash(&loft)
    );
}
