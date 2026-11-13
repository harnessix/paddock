//! Restore of a backup, and the undo of an apply or a restore.

use super::*;
use crate::copy::backup::{self, Kind as BackupKind};
use crate::copy::restore;

fn undo(w: &World, run: &Run, force: bool) -> Option<restore::Outcome> {
    let b = restore::last_undoable(&run.store)?;
    let plan = restore::undo_plan(&b, &w.project, force).unwrap();
    Some(restore::undo(&run.store, b, &w.project, &plan).unwrap())
}

#[test]
fn an_undo_takes_the_last_apply_back_and_the_changes_are_open_again() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    put(&tree, "src/cart.ts", "the copy\n");
    put(&tree, "docs/new.md", "new\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    let applied = w.apply_all(&mut run);
    let out = undo(&w, &run, false).unwrap();
    assert_eq!(out.of, applied.backup.unwrap());
    assert_eq!(out.restored, ["README.md", "src/cart.ts"]);
    assert_eq!(out.removed, ["docs/new.md"]);
    assert!(out.skipped.is_empty());
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert_eq!(get(&w.project, "README.md").unwrap(), "the shop\n");
    assert!(
        !w.project.join("docs").exists(),
        "the new folder goes with its file"
    );
    // The copy still has the work, and the list shows it again.
    let run = Run::load(&run.store, &run.rec.id).unwrap();
    let st = status(&run);
    assert_eq!(st.changes.len(), 3, "{:?}", rows(&st));
    assert!(st.changes.iter().all(|c| c.conflict.is_none()));
    assert!(st.applied.is_empty());
    // Nothing more to take back: the undo itself is no action to undo.
    assert!(restore::last_undoable(&run.store).is_none());
    let kinds: Vec<(BackupKind, bool)> = backup::list(&run.store)
        .iter()
        .map(|b| (b.meta.kind, b.meta.by_undo))
        .collect();
    assert!(kinds.contains(&(BackupKind::Restore, true)));
}

#[test]
fn an_undo_leaves_a_file_that_the_user_changed_after_the_apply() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "src/price.ts", "price 2\n");
    w.apply_all(&mut run);
    put(&w.project, "src/cart.ts", "the user, after the apply\n");
    let out = undo(&w, &run, false).unwrap();
    assert_eq!(out.restored, ["src/price.ts"]);
    assert_eq!(out.skipped, ["src/cart.ts"]);
    assert_eq!(
        get(&w.project, "src/cart.ts").unwrap(),
        "the user, after the apply\n"
    );
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "price\n");
}

#[test]
fn a_forced_undo_restores_that_file_too_and_keeps_the_users_version_in_a_backup() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    w.apply_all(&mut run);
    put(&w.project, "src/cart.ts", "the user, after the apply\n");
    let out = undo(&w, &run, true).unwrap();
    assert_eq!(out.restored, ["src/cart.ts"]);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    let safety = backup::find(&run.store, out.backup.as_deref().unwrap()).unwrap();
    assert_eq!(
        get(&safety.files_dir(), "src/cart.ts").unwrap(),
        "the user, after the apply\n"
    );
}

#[test]
fn the_undo_of_a_second_apply_goes_back_to_the_first_one() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "version 2\n");
    w.apply_all(&mut run);
    put(&run.tree(), "src/cart.ts", "version 3, longer\n");
    w.apply_all(&mut run);
    assert_eq!(
        get(&w.project, "src/cart.ts").unwrap(),
        "version 3, longer\n"
    );
    undo(&w, &run, false).unwrap();
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "version 2\n");
    // Version 3 is an open change of the copy, not a conflict.
    let run = Run::load(&run.store, &run.rec.id).unwrap();
    assert_eq!(
        rows(&status(&run)),
        [("src/cart.ts".to_string(), Kind::Changed, None)]
    );
    // The next undo takes the first apply back.
    undo(&w, &run, false).unwrap();
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert!(undo(&w, &run, false).is_none());
}

#[test]
fn a_restore_of_the_start_brings_changed_and_deleted_files_back() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    std::fs::remove_file(run.tree().join("README.md")).unwrap();
    put(&run.tree(), "docs/new.md", "new\n");
    w.apply_all(&mut run);
    put(&w.project, "mine.txt", "of the user\n");
    let start = run.base().unwrap();
    // Without `exact`, newer files stay.
    let plan = restore::plan(&start, &w.project, &[], false).unwrap();
    assert_eq!(plan.restore, ["README.md", "src/cart.ts"]);
    assert!(plan.remove.is_empty());
    assert_eq!(plan.newer, ["docs/new.md", "mine.txt"]);
    let out = restore::restore(&run.store, &start, &w.project, &plan, false).unwrap();
    assert_eq!(out.restored, ["README.md", "src/cart.ts"]);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert_eq!(get(&w.project, "README.md").unwrap(), "the shop\n");
    assert_eq!(get(&w.project, "mine.txt").unwrap(), "of the user\n");
    // The pointer folder is never part of a restore.
    assert!(w.project.join(".paddock/pointer.json").is_file());
    // A restore has an undo: the applied state comes back.
    let b = restore::last_undoable(&run.store).unwrap();
    assert_eq!(b.meta.kind, BackupKind::Restore);
    let p = restore::undo_plan(&b, &w.project, false).unwrap();
    restore::undo(&run.store, b, &w.project, &p).unwrap();
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    assert!(!w.project.join("README.md").exists());
}

#[test]
fn an_exact_restore_also_removes_newer_files_and_one_path_restores_alone() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "src/price.ts", "price 2\n");
    put(&run.tree(), "docs/new.md", "new\n");
    w.apply_all(&mut run);
    let start = run.base().unwrap();
    // One file.
    let plan = restore::plan(&start, &w.project, &paths(&["src/price.ts"]), false).unwrap();
    assert_eq!(plan.restore, ["src/price.ts"]);
    restore::restore(&run.store, &start, &w.project, &plan, false).unwrap();
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "price\n");
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    // A path that the backup does not have.
    let e = restore::plan(&start, &w.project, &paths(&["nothing/here"]), false).unwrap_err();
    assert!(format!("{e:#}").contains("has no file at"), "{e:#}");
    // The whole backup, exactly.
    let plan = restore::plan(&start, &w.project, &[], true).unwrap();
    assert_eq!(plan.restore, ["src/cart.ts"]);
    assert_eq!(plan.remove, ["docs/new.md"]);
    let out = restore::restore(&run.store, &start, &w.project, &plan, false).unwrap();
    assert_eq!(out.removed, ["docs/new.md"]);
    assert!(!w.project.join("docs").exists());
    let safety = backup::find(&run.store, out.backup.as_deref().unwrap()).unwrap();
    assert_eq!(get(&safety.files_dir(), "docs/new.md").unwrap(), "new\n");
    // Nothing differs now: the plan is empty and a restore makes no backup.
    let plan = restore::plan(&start, &w.project, &[], true).unwrap();
    assert!(plan.is_empty());
    let n = backup::list(&run.store).len();
    restore::restore(&run.store, &start, &w.project, &plan, false).unwrap();
    assert_eq!(backup::list(&run.store).len(), n);
}

#[test]
fn a_restore_of_a_backup_from_before_an_apply_sets_only_its_paths() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "docs/new.md", "new\n");
    let applied = w.apply_all(&mut run);
    put(&w.project, "src/price.ts", "the user\n");
    let b = backup::find(&run.store, applied.backup.as_deref().unwrap()).unwrap();
    let plan = restore::plan(&b, &w.project, &[], false).unwrap();
    assert_eq!(plan.restore, ["src/cart.ts"]);
    assert_eq!(plan.remove, ["docs/new.md"]);
    assert!(plan.newer.is_empty());
    restore::restore(&run.store, &b, &w.project, &plan, false).unwrap();
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "the user\n");
}

#[test]
fn a_backup_is_found_by_the_end_of_its_id() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    let id = run.rec.base.clone();
    let short = crate::ids::short(&id).to_string();
    assert_eq!(backup::find(&run.store, &id).unwrap().meta.id, id);
    assert_eq!(backup::find(&run.store, &short).unwrap().meta.id, id);
    let e = backup::find(&run.store, "ffff").unwrap_err();
    assert!(format!("{e:#}").contains("no backup"), "{e:#}");
}
