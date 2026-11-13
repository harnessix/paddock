//! The start of a run, the change list, apply, keep and discard.

use super::*;
use crate::copy::backup;
use crate::copy::tree::Method;
use std::os::unix::fs::PermissionsExt;

#[test]
fn the_start_makes_a_pointer_a_backup_and_a_copy_and_leaves_the_project_alone() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    // The pointer: three fields, no path, no project data.
    let text = get(&w.project, ".paddock/pointer.json").unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["created", "format", "store"]);
    assert_eq!(v["store"], run.store.meta.id.as_str());
    assert!(
        !text.contains("webshop"),
        "the pointer names no path: {text}"
    );
    let in_pointer: Vec<_> = std::fs::read_dir(w.project.join(".paddock"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(in_pointer, ["pointer.json"]);
    // The project is as it was.
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    // The copy has the files, and its `.paddock` is not the pointer.
    let tree = run.tree();
    assert_eq!(get(&tree, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert_eq!(get(&tree, "README.md").unwrap(), "the shop\n");
    assert!(tree.join(".paddock").is_dir());
    assert!(!tree.join(".paddock/pointer.json").exists());
    // The backup of the start holds every file.
    let all = backup::list(&run.store);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].meta.kind, backup::Kind::Start);
    assert_eq!(all[0].count(), 3);
    assert!(all[0].meta.full);
    assert_eq!(get(&all[0].files_dir(), "src/price.ts").unwrap(), "price\n");
    assert_eq!(run.rec.files, 3);
    assert!(run.rec.in_place);
    // Nothing changed yet.
    let st = status(&run);
    assert!(st.tree);
    assert!(st.changes.is_empty() && st.behind.is_empty() && st.applied.is_empty());
}

#[test]
fn the_list_has_new_changed_and_deleted_files_and_the_project_stays() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    put(&tree, "src/cart.ts", "one\nTWO\nthree\nfour\n");
    put(&tree, "src/tax.ts", "tax\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    let st = status(&run);
    assert_eq!(
        rows(&st),
        [
            ("README.md".to_string(), Kind::Deleted, None),
            ("src/cart.ts".to_string(), Kind::Changed, None),
            ("src/tax.ts".to_string(), Kind::New, None),
        ]
    );
    assert_eq!(st.counts_line(), "1 changed, 1 new, 1 deleted");
    assert_eq!(st.ready(), 3);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert!(w.project.join("README.md").exists());
    assert!(!w.project.join("src/tax.ts").exists());
}

#[test]
fn a_touch_or_a_rewrite_with_the_same_bytes_is_no_change() {
    let w = world();
    let run = w.start("claude-webshop-1a2b3c");
    std::thread::sleep(std::time::Duration::from_millis(5));
    put(&run.tree(), "src/cart.ts", "one\ntwo\nthree\n");
    put(&w.project, "src/price.ts", "price\n");
    let st = status(&run);
    assert!(st.changes.is_empty(), "{:?}", rows(&st));
    assert!(st.behind.is_empty());
}

#[test]
fn a_change_of_the_mode_alone_is_a_change_and_an_apply_keeps_the_execute_bit() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    std::fs::set_permissions(
        tree.join("src/cart.ts"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    put(&tree, "bin/run.sh", "#!/bin/sh\n");
    std::fs::set_permissions(
        tree.join("bin/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(
        rows(&status(&run)),
        [
            ("bin/run.sh".to_string(), Kind::New, None),
            ("src/cart.ts".to_string(), Kind::Changed, None),
        ]
    );
    w.apply_all(&mut run);
    for f in ["bin/run.sh", "src/cart.ts"] {
        let m = std::fs::metadata(w.project.join(f))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o777, 0o755, "{f}");
    }
}

#[test]
fn an_apply_writes_the_changes_makes_a_backup_first_and_marks_them_applied() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    put(&tree, "src/cart.ts", "one\nTWO\n");
    put(&tree, "docs/new/guide.md", "guide\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    let done = w.apply_all(&mut run);
    assert_eq!(done.written, ["docs/new/guide.md", "src/cart.ts"]);
    assert_eq!(done.deleted, ["README.md"]);
    assert!(done.conflicts.is_empty());
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\nTWO\n");
    assert_eq!(get(&w.project, "docs/new/guide.md").unwrap(), "guide\n");
    assert!(!w.project.join("README.md").exists());
    // The backup holds what the apply replaced, and knows what was not
    // there.
    let b = backup::find(&run.store, done.backup.as_deref().unwrap()).unwrap();
    assert_eq!(b.meta.kind, backup::Kind::Apply);
    assert!(!b.meta.full);
    assert_eq!(
        get(&b.files_dir(), "src/cart.ts").unwrap(),
        "one\ntwo\nthree\n"
    );
    assert_eq!(get(&b.files_dir(), "README.md").unwrap(), "the shop\n");
    assert_eq!(b.meta.files["docs/new/guide.md"], None);
    assert_eq!(b.meta.run.as_deref(), Some("claude-webshop-1a2b3c"));
    // No open change, three applied paths.
    let st = status(&run);
    assert!(st.changes.is_empty(), "{:?}", rows(&st));
    assert_eq!(
        st.applied,
        ["README.md", "docs/new/guide.md", "src/cart.ts"]
    );
    // A second apply has nothing to do and makes no backup.
    let again = w.apply_all(&mut run);
    assert_eq!(again.count(), 0);
    assert!(again.backup.is_none());
    assert_eq!(backup::list(&run.store).len(), 2);
}

#[test]
fn an_apply_takes_only_the_named_file_or_folder() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    put(&tree, "src/cart.ts", "cart 2\n");
    put(&tree, "src/price.ts", "price 2\n");
    put(&tree, "README.md", "readme 2\n");
    let done = w.apply(&mut run, &["./src/"], OnConflict::Stop);
    assert_eq!(done.written, ["src/cart.ts", "src/price.ts"]);
    assert_eq!(get(&w.project, "README.md").unwrap(), "the shop\n");
    assert_eq!(
        rows(&status(&run)),
        [("README.md".to_string(), Kind::Changed, None)]
    );
    // A path that is no change is an error, not a silent nothing.
    let o = Opts {
        paths: paths(&["src/nothing.ts"]),
        ..Default::default()
    };
    let e = apply::apply(&w.env, &mut run, &o).unwrap_err();
    assert!(
        format!("{e:#}").contains("is no change of this run"),
        "{e:#}"
    );
    // A path that leaves the project is refused.
    let o = Opts {
        paths: paths(&["../other"]),
        ..Default::default()
    };
    assert!(apply::apply(&w.env, &mut run, &o).is_err());
}

#[test]
fn a_kept_file_stays_out_of_an_apply_of_everything() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    put(&tree, "src/cart.ts", "cart 2\n");
    put(&tree, "src/price.ts", "price 2\n");
    assert_eq!(
        apply::keep(&mut run, &paths(&["src/price.ts"]), true).unwrap(),
        ["src/price.ts"]
    );
    let st = status(&run);
    assert!(
        st.changes
            .iter()
            .find(|c| c.path == "src/price.ts")
            .unwrap()
            .kept
    );
    assert_eq!(st.ready(), 1);
    let done = w.apply_all(&mut run);
    assert_eq!(done.written, ["src/cart.ts"]);
    assert_eq!(done.kept, ["src/price.ts"]);
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "price\n");
    // The mark survives a new load of the record.
    let again = Run::load(&run.store, &run.rec.id).unwrap();
    assert!(again.rec.kept.contains("src/price.ts"));
    // Named by its path, a kept file is applied, and the mark goes.
    let done = w.apply(&mut run, &["src/price.ts"], OnConflict::Stop);
    assert_eq!(done.written, ["src/price.ts"]);
    assert!(run.rec.kept.is_empty());
    // Keep, then take the keep away.
    put(&tree, "README.md", "readme 2\n");
    apply::keep(&mut run, &paths(&["README.md"]), true).unwrap();
    assert_eq!(
        apply::keep(&mut run, &paths(&["README.md"]), false).unwrap(),
        ["README.md"]
    );
    assert_eq!(status(&run).ready(), 1);
}

#[test]
fn a_file_that_both_sides_changed_is_a_conflict_and_is_never_overwritten() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&w.project, "src/cart.ts", "the user\n");
    put(&run.tree(), "src/price.ts", "price 2\n");
    let st = status(&run);
    assert_eq!(
        rows(&st),
        [
            (
                "src/cart.ts".to_string(),
                Kind::Changed,
                Some(Conflict::Both)
            ),
            ("src/price.ts".to_string(), Kind::Changed, None),
        ]
    );
    assert_eq!(st.counts_line(), "2 changed, 0 new, 0 deleted, 1 conflict");
    assert_eq!(st.ready(), 1);
    let done = w.apply_all(&mut run);
    assert_eq!(done.written, ["src/price.ts"]);
    assert_eq!(done.conflicts, ["src/cart.ts"]);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    assert_eq!(get(&run.tree(), "src/cart.ts").unwrap(), "the copy\n");
}

#[test]
fn a_conflict_can_keep_the_original() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&w.project, "src/cart.ts", "the user\n");
    let done = w.apply(&mut run, &["src/cart.ts"], OnConflict::Original);
    assert_eq!(done.kept_original, ["src/cart.ts"]);
    assert!(done.backup.is_none());
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    assert_eq!(get(&run.tree(), "src/cart.ts").unwrap(), "the copy\n");
    assert!(status(&run).changes[0].kept);
}

#[test]
fn a_conflict_can_take_the_copy_and_the_backup_has_the_users_version() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&w.project, "src/cart.ts", "the user\n");
    let done = w.apply(&mut run, &["src/cart.ts"], OnConflict::Copy);
    assert_eq!(done.written, ["src/cart.ts"]);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    let b = backup::find(&run.store, done.backup.as_deref().unwrap()).unwrap();
    assert_eq!(get(&b.files_dir(), "src/cart.ts").unwrap(), "the user\n");
    assert!(status(&run).changes.is_empty());
}

#[test]
fn a_conflict_can_keep_both_versions() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&w.project, "src/cart.ts", "the user\n");
    let done = w.apply(&mut run, &["src/cart.ts"], OnConflict::Both);
    let (path, aside) = &done.both[0];
    assert_eq!(path, "src/cart.ts");
    assert!(aside.starts_with("src/cart.ts.conflict-1a2b3c-"), "{aside}");
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    assert_eq!(get(&w.project, aside).unwrap(), "the copy\n");
    // The conflict stays in the list, marked as kept.
    let st = status(&run);
    assert_eq!(st.changes.len(), 1);
    assert!(st.changes[0].kept && st.changes[0].conflict.is_some());
}

#[test]
fn a_conflict_can_take_a_clean_merge_and_refuses_one_with_marks() {
    let w = world();
    if w.env.which("git").is_none() {
        return;
    }
    put(&w.project, "notes.txt", "a\nb\nc\nd\ne\nf\ng\n");
    let mut run = w.start("claude-webshop-1a2b3c");
    // Different lines: a clean merge.
    put(&run.tree(), "notes.txt", "A copy\nb\nc\nd\ne\nf\ng\n");
    put(&w.project, "notes.txt", "a\nb\nc\nd\ne\nf\nG user\n");
    // The same line: marks.
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&w.project, "src/cart.ts", "the user\n");
    assert!(apply::merge(&w.env, &run, "notes.txt").unwrap().unwrap().0);
    assert!(
        !apply::merge(&w.env, &run, "src/cart.ts")
            .unwrap()
            .unwrap()
            .0
    );
    let done = w.apply(&mut run, &[], OnConflict::Merge);
    assert_eq!(done.merged, ["notes.txt"]);
    assert_eq!(done.conflicts, ["src/cart.ts"]);
    let merged = "A copy\nb\nc\nd\ne\nf\nG user\n";
    assert_eq!(get(&w.project, "notes.txt").unwrap(), merged);
    // Both sides have the merge now.
    assert_eq!(get(&run.tree(), "notes.txt").unwrap(), merged);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    let st = status(&run);
    assert_eq!(
        rows(&st),
        [(
            "src/cart.ts".to_string(),
            Kind::Changed,
            Some(Conflict::Both)
        )]
    );
    assert_eq!(st.applied, ["notes.txt"]);
}

#[test]
fn the_conflict_kinds_with_a_delete_and_with_two_new_files() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    // The copy deleted it, the user changed it.
    std::fs::remove_file(tree.join("README.md")).unwrap();
    put(&w.project, "README.md", "the user\n");
    // The user deleted it, the copy changed it.
    std::fs::remove_file(w.project.join("src/price.ts")).unwrap();
    put(&tree, "src/price.ts", "the copy\n");
    // Both made a file.
    put(&tree, "new.txt", "the copy\n");
    put(&w.project, "new.txt", "the user\n");
    // Both made the same file: nothing to do.
    put(&tree, "same.txt", "equal\n");
    put(&w.project, "same.txt", "equal\n");
    // Both deleted it: nothing to do.
    std::fs::remove_file(tree.join("src/cart.ts")).unwrap();
    std::fs::remove_file(w.project.join("src/cart.ts")).unwrap();
    assert_eq!(
        rows(&status(&run)),
        [
            (
                "README.md".to_string(),
                Kind::Deleted,
                Some(Conflict::CopyDeleted)
            ),
            (
                "new.txt".to_string(),
                Kind::Changed,
                Some(Conflict::BothNew)
            ),
            (
                "src/price.ts".to_string(),
                Kind::New,
                Some(Conflict::OriginalDeleted)
            ),
        ]
    );
    // Nothing is written without a decision.
    let done = w.apply_all(&mut run);
    assert_eq!(done.count(), 0);
    assert_eq!(done.conflicts.len(), 3);
    assert_eq!(get(&w.project, "README.md").unwrap(), "the user\n");
    assert!(!w.project.join("src/price.ts").exists());
    // "Keep both": a deleted side has no second version, so the file that
    // exists stays or comes back.
    let done = w.apply(&mut run, &[], OnConflict::Both);
    assert_eq!(done.kept_original, ["README.md"]);
    assert_eq!(done.written, ["src/price.ts"]);
    assert_eq!(done.both.len(), 1);
    assert_eq!(get(&w.project, "README.md").unwrap(), "the user\n");
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "the copy\n");
    assert_eq!(get(&w.project, "new.txt").unwrap(), "the user\n");
}

#[test]
fn a_file_that_only_the_user_changed_is_behind_and_an_apply_leaves_it() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&w.project, "src/cart.ts", "the user\n");
    put(&w.project, "user-new.txt", "mine\n");
    std::fs::remove_file(w.project.join("README.md")).unwrap();
    put(&run.tree(), "src/price.ts", "price 2\n");
    let st = status(&run);
    assert_eq!(
        rows(&st),
        [("src/price.ts".to_string(), Kind::Changed, None)]
    );
    let behind: Vec<(&str, Kind)> = st
        .behind
        .iter()
        .map(|b| (b.path.as_str(), b.kind))
        .collect();
    assert_eq!(
        behind,
        [
            ("README.md", Kind::Deleted),
            ("src/cart.ts", Kind::Changed),
            ("user-new.txt", Kind::New),
        ]
    );
    w.apply_all(&mut run);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    assert_eq!(get(&w.project, "user-new.txt").unwrap(), "mine\n");
    assert!(!w.project.join("README.md").exists());
}

#[test]
fn after_an_apply_an_edit_of_one_side_is_no_conflict() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "version 2\n");
    w.apply_all(&mut run);
    // The user goes on in the original: the copy is behind, no conflict.
    put(&w.project, "src/cart.ts", "version 2, then the user\n");
    let st = status(&run);
    assert!(st.changes.is_empty(), "{:?}", rows(&st));
    assert_eq!(st.behind.len(), 1);
    // The agent goes on in the copy too: now both changed.
    put(&run.tree(), "src/cart.ts", "version 3 of the agent\n");
    assert_eq!(
        rows(&status(&run)),
        [(
            "src/cart.ts".to_string(),
            Kind::Changed,
            Some(Conflict::Both)
        )]
    );
    // Only the agent goes on: a plain change again.
    put(&run.tree(), "src/price.ts", "price 2\n");
    w.apply(&mut run, &["src/price.ts"], OnConflict::Stop);
    put(&run.tree(), "src/price.ts", "price 3, a longer one\n");
    let st = status(&run);
    assert!(st
        .changes
        .iter()
        .any(|c| c.path == "src/price.ts" && c.conflict.is_none()));
    w.apply(&mut run, &["src/price.ts"], OnConflict::Stop);
    assert_eq!(
        get(&w.project, "src/price.ts").unwrap(),
        "price 3, a longer one\n"
    );
}

#[test]
fn an_apply_with_the_token_of_an_old_list_stops() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    let seen = status(&run).token;
    // The user changes the file after the list.
    put(&w.project, "src/cart.ts", "the user, later\n");
    let o = Opts {
        token: Some(seen.clone()),
        on_conflict: OnConflict::Copy,
        ..Default::default()
    };
    let e = apply::apply(&w.env, &mut run, &o).unwrap_err();
    assert!(
        format!("{e:#}").contains("not the ones that you saw"),
        "{e:#}"
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user, later\n");
    assert_eq!(
        backup::list(&run.store).len(),
        1,
        "no backup for a refused apply"
    );
    // With the token of the list of now, it works.
    let o = Opts {
        token: Some(status(&run).token),
        on_conflict: OnConflict::Copy,
        ..Default::default()
    };
    assert_ne!(o.token.as_ref(), Some(&seen));
    apply::apply(&w.env, &mut run, &o).unwrap();
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
}

#[test]
fn a_discard_brings_the_copy_back_to_its_base_and_never_touches_the_project() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    put(&tree, "src/cart.ts", "the copy\n");
    put(&tree, "src/deep/new.ts", "new\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    put(&tree, "src/price.ts", "price 2\n");
    let done = apply::discard(&mut run, &paths(&["src/cart.ts", "src/deep", "README.md"])).unwrap();
    assert_eq!(done, ["README.md", "src/cart.ts", "src/deep/new.ts"]);
    assert_eq!(get(&tree, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert_eq!(get(&tree, "README.md").unwrap(), "the shop\n");
    assert!(!tree.join("src/deep").exists(), "the empty folder goes too");
    assert!(tree.join("src").is_dir());
    assert_eq!(
        rows(&status(&run)),
        [("src/price.ts".to_string(), Kind::Changed, None)]
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    // After an apply, the base of a discard is the applied content.
    w.apply_all(&mut run);
    put(&tree, "src/price.ts", "price 3, longer\n");
    apply::discard(&mut run, &paths(&["src/price.ts"])).unwrap();
    assert_eq!(get(&tree, "src/price.ts").unwrap(), "price 2\n");
    let st = status(&run);
    assert!(st.changes.is_empty(), "{:?}", rows(&st));
}

#[test]
fn a_discard_of_the_whole_copy_removes_it_and_keeps_the_backups() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "src/cart.ts", "the copy\n");
    apply::discard_all(&w.env, &mut run).unwrap();
    assert!(!run.tree().exists());
    assert!(!status(&run).tree);
    assert_eq!(backup::list(&run.store).len(), 1);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    // An action on a copy that is gone says so.
    let e = apply::apply(&w.env, &mut run, &Opts::default()).unwrap_err();
    assert!(format!("{e:#}").contains("is gone"), "{e:#}");
    let again = Run::load(&run.store, &run.rec.id).unwrap();
    assert!(again.rec.removed.is_some());
}

#[test]
fn build_output_and_git_stay_out_of_the_list_and_git_comes_along() {
    let w = world();
    put(&w.project, ".git/HEAD", "ref: refs/heads/main\n");
    put(&w.project, ".git/objects/ab/cdef", "object\n");
    put(&w.project, "node_modules/left-pad/index.js", "pad\n");
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    assert!(run.rec.git);
    // `.git` is in the copy, so git works there. It is not in the backup.
    assert_eq!(get(&tree, ".git/HEAD").unwrap(), "ref: refs/heads/main\n");
    assert_eq!(get(&tree, ".git/objects/ab/cdef").unwrap(), "object\n");
    assert!(run.rec.carried.contains(&".git".to_string()));
    let base = run.base().unwrap();
    assert!(base.meta.files.keys().all(|p| !p.starts_with(".git")));
    assert!(base
        .meta
        .files
        .keys()
        .all(|p| !p.starts_with("node_modules")));
    // Build output comes along only when it costs no bytes.
    assert_eq!(
        tree.join("node_modules/left-pad/index.js").exists(),
        run.rec.method != Method::Copy
    );
    // What the agent does there is no change, and an apply never writes it.
    put(&tree, ".git/HEAD", "ref: refs/heads/agent\n");
    put(&tree, ".git/hooks/pre-commit", "#!/bin/sh\necho owned\n");
    put(&tree, "target/debug/app", "binary\n");
    put(&tree, "node_modules/evil/index.js", "x\n");
    put(&tree, "src/cart.ts", "the copy\n");
    assert_eq!(
        rows(&status(&run)),
        [("src/cart.ts".to_string(), Kind::Changed, None)]
    );
    w.apply_all(&mut run);
    assert_eq!(
        get(&w.project, ".git/HEAD").unwrap(),
        "ref: refs/heads/main\n"
    );
    assert!(!w.project.join(".git/hooks/pre-commit").exists());
    assert!(!w.project.join("target").exists());
    assert!(!w.project.join("node_modules/evil").exists());
}

#[test]
fn the_notes_folder_of_the_copy_is_never_a_change() {
    let w = world();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), ".paddock/plan.md", "a note\n");
    assert!(status(&run).changes.is_empty());
    w.apply_all(&mut run);
    assert!(!w.project.join(".paddock/plan.md").exists());
}

#[test]
fn a_symlink_in_the_copy_is_never_followed_and_one_that_leaves_is_not_applied() {
    let w = world();
    let secret = w.t.path().join("secret");
    put(&secret, "key.txt", "the key\n");
    let mut run = w.start("claude-webshop-1a2b3c");
    let tree = run.tree();
    // The agent swaps a folder for a link to a folder outside.
    std::fs::remove_dir_all(tree.join("src")).unwrap();
    std::os::unix::fs::symlink(&secret, tree.join("src")).unwrap();
    // A link inside the project, and one that climbs out.
    std::os::unix::fs::symlink("README.md", tree.join("readme-link")).unwrap();
    std::os::unix::fs::symlink("../../secret/key.txt", tree.join("out-link")).unwrap();
    let st = status(&run);
    let got: Vec<&str> = st.changes.iter().map(|c| c.path.as_str()).collect();
    // The link is an entry of its own. Nothing below it is read.
    assert_eq!(
        got,
        [
            "out-link",
            "readme-link",
            "src",
            "src/cart.ts",
            "src/price.ts"
        ]
    );
    assert!(st.changes.iter().all(|c| !c.path.contains("key.txt")));
    let done = w.apply(&mut run, &["out-link", "readme-link"], OnConflict::Stop);
    assert_eq!(done.unsafe_links, ["out-link"]);
    assert_eq!(done.written, ["readme-link"]);
    assert_eq!(
        std::fs::read_link(w.project.join("readme-link")).unwrap(),
        Path::new("README.md")
    );
    assert!(std::fs::symlink_metadata(w.project.join("out-link")).is_err());
    // The rest: the files below `src` leave, and the link is not written.
    let done = w.apply_all(&mut run);
    assert_eq!(done.unsafe_links, ["out-link", "src"]);
    assert_eq!(done.deleted, ["src/cart.ts", "src/price.ts"]);
    assert!(std::fs::symlink_metadata(w.project.join("src")).is_err());
    assert_eq!(get(&secret, "key.txt").unwrap(), "the key\n");
}

#[test]
fn a_symlink_in_the_project_does_not_lead_an_apply_out() {
    let w = world();
    let outside = w.t.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let mut run = w.start("claude-webshop-1a2b3c");
    put(&run.tree(), "docs/a.md", "a\n");
    // After the list: `docs` of the project becomes a link to a folder
    // outside it.
    std::os::unix::fs::symlink(&outside, w.project.join("docs")).unwrap();
    let r = apply::apply(&w.env, &mut run, &Opts::default());
    assert!(r.is_err(), "{r:?}");
    assert!(!outside.join("a.md").exists());
}

#[test]
fn a_plain_copy_above_the_limit_is_refused_and_leaves_nothing_behind() {
    let w = world();
    crate::copy::tree::NO_SHARE.with(|c| c.set(true));
    let cfg = Config {
        max: 10,
        ..Config::default()
    };
    let r = start::create(
        &w.env,
        &cfg,
        &w.project,
        ("claude-webshop-1a2b3c", "claude"),
        true,
        &mut |_, _| false,
    );
    let e = format!("{:#}", r.err().unwrap());
    assert!(e.contains("[copy] max") && e.contains("--direct"), "{e}");
    let store = crate::copy::store::Store::all(&cfg.roots(&w.env));
    assert_eq!(store.len(), 1);
    assert!(backup::list(&store[0]).is_empty());
    assert!(crate::copy::run::of_store(&store[0]).is_empty());
    // Under the limit, the plain copy works, with its bytes counted.
    let run = w.start("claude-webshop-1a2b3c");
    crate::copy::tree::NO_SHARE.with(|c| c.set(false));
    assert_eq!(run.rec.method, Method::Copy);
    assert_eq!(run.rec.bytes, 9 + 14 + 6);
    assert_eq!(
        get(&run.tree(), "src/cart.ts").unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[test]
fn a_plain_copy_takes_git_along_and_leaves_build_output_out() {
    let w = world();
    put(&w.project, ".git/HEAD", "ref: refs/heads/main\n");
    put(&w.project, "node_modules/left-pad/index.js", "pad\n");
    crate::copy::tree::NO_SHARE.with(|c| c.set(true));
    let run = w.start("claude-webshop-1a2b3c");
    crate::copy::tree::NO_SHARE.with(|c| c.set(false));
    assert_eq!(
        get(&run.tree(), ".git/HEAD").unwrap(),
        "ref: refs/heads/main\n"
    );
    assert!(!run.tree().join("node_modules").exists());
    assert_eq!(run.rec.carried, [".git"]);
}

#[test]
fn two_runs_of_one_project_share_the_store_and_each_has_its_own_base() {
    let w = world();
    let mut first = w.start("claude-webshop-1a2b3c");
    put(&first.tree(), "src/cart.ts", "first\n");
    w.apply_all(&mut first);
    let second = w.start("codex-webshop-4d5e6f");
    assert_eq!(first.store.meta.id, second.store.meta.id);
    assert_ne!(first.rec.base, second.rec.base);
    // The second run starts from the project of now.
    assert_eq!(get(&second.tree(), "src/cart.ts").unwrap(), "first\n");
    assert!(status(&second).changes.is_empty());
    let found = crate::copy::run::find(&w.env, &w.cfg, "4d5e6f").unwrap();
    assert_eq!(found.rec.id, "codex-webshop-4d5e6f");
    assert!(crate::copy::run::find(&w.env, &w.cfg, "ffffff").is_err());
    assert_eq!(crate::copy::run::all(&w.env, &w.cfg).len(), 2);
}

#[test]
fn old_backups_go_but_never_the_base_of_a_copy_that_is_there() {
    let w = world();
    let first = w.start("claude-webshop-000001");
    for n in 2..=(backup::KEEP_START + 3) {
        let mut r = w.start(&format!("claude-webshop-{n:06}"));
        apply::discard_all(&w.env, &mut r).unwrap();
    }
    let all = backup::list(&first.store);
    assert!(all.len() <= backup::KEEP_START + 1, "{}", all.len());
    assert!(all.iter().any(|b| b.meta.id == first.rec.base));
    assert!(status(&first).tree);
}
