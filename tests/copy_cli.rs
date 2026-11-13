//! The commands of a run on a copy (`paddock changes`, `apply`, `keep`,
//! `discard`, `undo`, `backups`, `restore`) and their JSON, with the built
//! paddock and a temp home. The copy is made through the library, as the
//! start of a run makes it, so no sandbox is needed here.
//! `tests/copy_run.rs` has the runs in a sandbox.

mod common;

use common::copy_world::{get, put, strs, text, world, ID};
use serde_json::Value;

#[test]
fn the_changes_json_has_every_field_that_the_screens_read() {
    let w = world();
    let run = w.start(ID);
    let tree = run.tree();
    put(&tree, "src/cart.ts", "one\nTWO\nthree\nfour\n");
    put(&tree, "src/tax.ts", "tax\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    put(&tree, "src/price.ts", "the copy\n");
    put(&w.project, "src/price.ts", "the user\n");
    put(&w.project, "mine.txt", "of the user\n");
    put(&tree, "logo.png", "PNG\0binary");
    let v = w.json(&["changes", "1a2b3c", "--json"]);
    assert_eq!(v["api_version"], "paddock.changes/v1");
    // The run.
    let r = &v["run"];
    assert_eq!(r["id"], ID);
    assert_eq!(r["short"], "1a2b3c");
    assert_eq!(r["harness"], "fake");
    assert_eq!(r["project"], w.project.display().to_string());
    assert_eq!(r["copy"], tree.display().to_string());
    assert_eq!(r["state"], "ended");
    assert_eq!(r["in_place"], true);
    assert_eq!(r["git"], false);
    assert_eq!(r["files"], 3);
    assert!(r["started"].is_u64() && r["ended"].is_null());
    assert!(["reflink", "clone", "copy"].contains(&r["method"].as_str().unwrap()));
    assert_eq!(r["shared"], r["method"] != "copy");
    // The counts.
    assert_eq!(
        v["counts"],
        serde_json::json!({
            "new": 2, "changed": 2, "deleted": 1, "conflicts": 1,
            "kept": 0, "ready": 4, "applied": 0, "behind": 1,
        })
    );
    assert_eq!(v["token"].as_str().unwrap().len(), 16);
    assert_eq!(
        v["behind"],
        serde_json::json!([{"path": "mine.txt", "kind": "new"}])
    );
    assert_eq!(v["applied"], serde_json::json!([]));
    assert_eq!(v["auto_apply"], false);
    // The pointer state travels with the list.
    assert_eq!(v["pointer"]["state"], "linked");
    assert_eq!(v["pointer"]["ok"], true);
    // One change of each kind.
    let by_path = |p: &str| {
        v["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == p)
            .unwrap_or_else(|| panic!("no change {p}"))
    };
    let c = by_path("src/cart.ts");
    assert_eq!(c["kind"], "changed");
    assert_eq!(c["state"], "open");
    assert_eq!(c["conflict"], Value::Null);
    assert_eq!(
        (c["text"].clone(), c["added"].clone(), c["removed"].clone()),
        (true.into(), 2.into(), 1.into())
    );
    assert_eq!(c["copy"]["size"], 19);
    assert_eq!(c["original"]["size"], 14);
    assert!(c["copy"]["mtime"].is_u64() && c["original"]["mtime"].is_u64());
    assert_eq!(c["copy"]["mode"], "0644");
    assert_eq!(c["kept"], false);
    assert!(c["writing"].is_boolean());
    let c = by_path("src/tax.ts");
    assert_eq!(
        (c["kind"].as_str(), c["original"].is_null()),
        (Some("new"), true)
    );
    assert_eq!(c["added"], 1);
    let c = by_path("README.md");
    assert_eq!(
        (c["kind"].as_str(), c["copy"].is_null()),
        (Some("deleted"), true)
    );
    assert_eq!(c["removed"], 1);
    let c = by_path("src/price.ts");
    assert_eq!(c["state"], "conflict");
    assert_eq!(c["conflict"], "both");
    assert_eq!(c["conflict_text"], "both sides changed it");
    assert!(c["merge"].is_null() || c["merge"] == "conflicts", "{c}");
    let c = by_path("logo.png");
    assert_eq!(c["text"], false);
    assert!(c["added"].is_null());
    assert_eq!(v["pointer"]["text"], "The pointer and the store agree.");
    assert_eq!(v["pointer"]["store"], run.store.meta.id.as_str());
    assert!(v["pointer"]["action"].is_null());
    // The brief form reads no file: the same rows, without the details.
    let b = w.json(&["changes", "1a2b3c", "--json", "--brief"]);
    assert_eq!(b["counts"], v["counts"]);
    assert_eq!(b["token"], v["token"]);
    let c = b["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "src/cart.ts")
        .unwrap();
    assert_eq!(c["kind"], "changed");
    assert_eq!(c["copy"]["size"], 19);
    for k in ["text", "added", "removed", "merge"] {
        assert!(c[k].is_null(), "{k} is null in the brief form");
    }
    // The list of the runs.
    let all = w.json(&["changes", "--json"]);
    assert_eq!(all["api_version"], "paddock.copies/v1");
    assert_eq!(all["runs"][0]["id"], ID);
    assert_eq!(all["runs"][0]["counts"]["conflicts"], 1);
}

#[test]
fn the_list_in_words_and_the_diff() {
    let w = world();
    // No run yet.
    let o = w.run(&["changes"]);
    assert!(
        text(&o.stdout).contains("No run works on a copy"),
        "{}",
        text(&o.stdout)
    );
    let run = w.start(ID);
    let o = w.run(&["changes", "1a2b3c"]);
    assert!(
        text(&o.stdout)
            .contains("No changes yet. The agent works in its copy. Your folder stays as it is."),
        "{}",
        text(&o.stdout)
    );
    let tree = run.tree();
    put(&tree, "src/cart.ts", "one\nTWO\nthree\n");
    put(&tree, "src/tax.ts", "tax\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    put(&tree, "src/price.ts", "the copy\n");
    put(&w.project, "src/price.ts", "the user\n");
    let o = w.run(&["changes", ID]);
    let out = text(&o.stdout);
    assert!(o.status.success(), "{out}");
    // A mark and a word for each state: never a color alone.
    for line in [
        "copy: 2 changed, 1 new, 1 deleted, 1 conflict",
        "  - deleted   README.md",
        "  ~ changed   src/cart.ts",
        "  ! conflict  src/price.ts  both sides changed it",
        "  + new       src/tax.ts",
        "`paddock apply 1a2b3c` writes 3 change(s) to your folder, after a backup.",
        "A conflict is never overwritten.",
    ] {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
    assert!(!out.contains('\x1b'), "no color codes without a terminal");
    let o = w.run(&["changes", "1a2b3c", "--diff", "src/cart.ts"]);
    let want = "\
--- your folder/src/cart.ts
+++ the copy/src/cart.ts
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
    assert_eq!(text(&o.stdout), want);
    let all = text(&w.run(&["changes", "1a2b3c", "--diff"]).stdout);
    assert!(
        all.contains("+++ the copy/src/tax.ts") && all.contains("-the shop"),
        "{all}"
    );
    // An unknown run, and an unknown path.
    let o = w.run(&["changes", "ffffff"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("no run ffffff with a copy"),
        "{}",
        text(&o.stderr)
    );
    let o = w.run(&["changes", "1a2b3c", "--diff", "nothing.txt"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("is no change of this run"));
}

#[test]
fn apply_then_undo_through_the_commands() {
    let w = world();
    let run = w.start(ID);
    let tree = run.tree();
    put(&tree, "src/cart.ts", "the copy\n");
    put(&tree, "docs/new.md", "new\n");
    std::fs::remove_file(tree.join("README.md")).unwrap();
    // Without a terminal and without --yes, nothing is written.
    let o = w.run(&["apply", "1a2b3c"]);
    assert!(!o.status.success());
    let out = text(&o.stdout);
    assert!(out.contains("1 file(s) leave your folder."), "{out}");
    assert!(out.contains("paddock makes a backup first."), "{out}");
    assert!(
        text(&o.stderr).contains("no terminal to ask"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    // With the token of the list.
    let token = w.json(&["changes", "1a2b3c", "--json"])["token"]
        .as_str()
        .unwrap()
        .to_string();
    let o = w.run(&["apply", "1a2b3c", "--yes", "--token", "0000000000000000"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("not the ones that you saw"));
    let v = w.json(&["apply", "1a2b3c", "--yes", "--json", "--token", &token]);
    assert_eq!(v["api_version"], "paddock.apply/v1");
    assert_eq!(v["run"], ID);
    assert_eq!(strs(&v["written"]), ["docs/new.md", "src/cart.ts"]);
    assert_eq!(strs(&v["deleted"]), ["README.md"]);
    for k in [
        "merged",
        "both",
        "kept_original",
        "conflicts",
        "kept",
        "unsafe_links",
    ] {
        assert_eq!(v[k], serde_json::json!([]), "{k}");
    }
    let backup = v["backup"].as_str().unwrap().to_string();
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    assert!(!w.project.join("README.md").exists());
    let list = text(&w.run(&["changes", "1a2b3c"]).stdout);
    assert!(list.contains("  ✓ applied   src/cart.ts"), "{list}");
    assert!(
        list.contains("All changes are in your folder. `paddock undo` takes the last apply back."),
        "{list}"
    );
    // The backups: the start and the apply.
    let b = w.json(&["backups", "--json"]);
    assert_eq!(b["api_version"], "paddock.backups/v1");
    assert_eq!(b["project"], w.project.display().to_string());
    let rows = b["backups"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], backup.as_str());
    assert_eq!(
        (rows[0]["kind"].as_str(), rows[0]["words"].as_str()),
        (Some("apply"), Some("before apply"))
    );
    assert_eq!(rows[0]["files"], 2);
    assert_eq!(rows[0]["paths"], 3);
    assert_eq!(rows[0]["undoable"], true);
    assert_eq!(rows[0]["run"], ID);
    assert_eq!(rows[0]["harness"], "fake");
    assert_eq!(
        (rows[1]["kind"].as_str(), rows[1]["words"].as_str()),
        (Some("start"), Some("start of run"))
    );
    assert_eq!(rows[1]["files"], 3);
    assert_eq!(rows[1]["full"], true);
    assert_eq!(rows[1]["undoable"], false);
    for r in rows {
        for k in [
            "short", "created", "bytes", "shared", "method", "undone", "by_undo",
        ] {
            assert!(r.get(k).is_some(), "{k} in {r}");
        }
    }
    assert_eq!(b["snapshots"], serde_json::json!([]));
    let words = text(&w.run(&["backups"]).stdout);
    assert!(
        words.contains("before apply") && words.contains("start of run"),
        "{words}"
    );
    // The undo.
    let o = w.run(&["undo"]);
    assert!(!o.status.success(), "an undo asks first");
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    let u = w.json(&["undo", "--yes", "--json"]);
    assert_eq!(u["api_version"], "paddock.undo/v1");
    assert_eq!(u["of"], backup.as_str());
    assert_eq!(strs(&u["restored"]), ["README.md", "src/cart.ts"]);
    assert_eq!(strs(&u["removed"]), ["docs/new.md"]);
    assert_eq!(u["skipped"], serde_json::json!([]));
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    assert_eq!(get(&w.project, "README.md").unwrap(), "the shop\n");
    assert!(!w.project.join("docs").exists());
    // The changes are open again, and the apply shows as undone.
    assert_eq!(
        w.json(&["changes", "1a2b3c", "--json"])["counts"]["ready"],
        3
    );
    let b = w.json(&["backups", "--json"]);
    let applied = b["backups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == backup.as_str())
        .unwrap();
    assert!(applied["undone"].is_u64());
    assert_eq!(applied["undoable"], false);
    // Nothing more to take back: the old undo answers, and it has no snapshot.
    let o = w.run(&["undo", "--yes"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("no snapshot for this project"),
        "{}",
        text(&o.stderr)
    );
}

#[test]
fn a_conflict_stops_an_apply_until_the_user_chooses() {
    let w = world();
    let run = w.start(ID);
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&w.project, "src/cart.ts", "the user\n");
    put(&run.tree(), "src/price.ts", "price 2\n");
    let o = w.run(&["apply", "1a2b3c", "--yes"]);
    assert_eq!(
        o.status.code(),
        Some(1),
        "an open conflict is not a success"
    );
    let out = text(&o.stdout);
    assert!(out.contains("Applied 1 file(s)"), "{out}");
    assert!(
        out.contains("Not written: 1 conflict(s): src/cart.ts"),
        "{out}"
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    assert_eq!(get(&w.project, "src/price.ts").unwrap(), "price 2\n");
    let o = w.run(&["apply", "1a2b3c", "--yes", "--on-conflict", "nonsense"]);
    assert_eq!(o.status.code(), Some(2));
    let v = w.json(&[
        "apply",
        "1a2b3c",
        "src/cart.ts",
        "--yes",
        "--json",
        "--on-conflict",
        "both",
    ]);
    let aside = v["both"][0][1].as_str().unwrap();
    assert!(aside.starts_with("src/cart.ts.conflict-1a2b3c-"), "{aside}");
    assert_eq!(get(&w.project, aside).unwrap(), "the copy\n");
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the user\n");
    let v = w.json(&[
        "apply",
        "1a2b3c",
        "src/cart.ts",
        "--yes",
        "--json",
        "--on-conflict",
        "copy",
    ]);
    assert_eq!(strs(&v["written"]), ["src/cart.ts"]);
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
}

#[test]
fn keep_and_discard_through_the_commands() {
    let w = world();
    let run = w.start(ID);
    let tree = run.tree();
    put(&tree, "src/cart.ts", "the copy\n");
    put(&tree, "src/price.ts", "price 2\n");
    put(&tree, "notes.txt", "notes\n");
    let v = w.json(&["keep", "1a2b3c", "notes.txt", "--json"]);
    assert_eq!(v["api_version"], "paddock.keep/v1");
    assert_eq!(
        (v["kept"].clone(), strs(&v["paths"])),
        (true.into(), vec!["notes.txt"])
    );
    let list = text(&w.run(&["changes", "1a2b3c"]).stdout);
    assert!(list.contains("  = kept      notes.txt"), "{list}");
    let v = w.json(&["apply", "1a2b3c", "--yes", "--json"]);
    assert_eq!(strs(&v["kept"]), ["notes.txt"]);
    assert!(!w.project.join("notes.txt").exists());
    let v = w.json(&["keep", "1a2b3c", "notes.txt", "--off", "--json"]);
    assert_eq!(v["kept"], false);
    // A discard of one path asks first, then brings the copy's file back.
    put(&tree, "src/cart.ts", "the copy, again and longer\n");
    let o = w.run(&["discard", "1a2b3c", "src/cart.ts"]);
    assert!(!o.status.success());
    assert_eq!(
        get(&tree, "src/cart.ts").unwrap(),
        "the copy, again and longer\n"
    );
    let v = w.json(&["discard", "1a2b3c", "src/cart.ts", "--yes", "--json"]);
    assert_eq!(v["api_version"], "paddock.discard/v1");
    assert_eq!(v["copy_removed"], false);
    assert_eq!(strs(&v["paths"]), ["src/cart.ts"]);
    assert_eq!(
        get(&tree, "src/cart.ts").unwrap(),
        "the copy\n",
        "back to the applied content"
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    // A discard of the whole copy.
    let v = w.json(&["discard", "1a2b3c", "--yes", "--json"]);
    assert_eq!(v["copy_removed"], true);
    assert!(!tree.exists());
    let gone = text(&w.run(&["changes", ID]).stdout);
    assert!(
        gone.contains("The copy is gone. The backups are still here"),
        "{gone}"
    );
    assert_eq!(
        w.json(&["changes", ID, "--json"])["run"]["state"],
        "removed"
    );
    assert_eq!(
        w.json(&["backups", "--json"])["backups"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // A run without a copy keeps the older meaning of the command.
    let o = w.run(&["discard", "nothing-here-999999"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("name the files to delete, or --all"),
        "{}",
        text(&o.stderr)
    );
}

#[test]
fn a_restore_through_the_commands() {
    let w = world();
    let run = w.start(ID);
    put(&run.tree(), "src/cart.ts", "the copy\n");
    put(&run.tree(), "docs/new.md", "new\n");
    w.json(&["apply", "1a2b3c", "--yes", "--json"]);
    let start = run.rec.base.clone();
    let short = paddock::ids::short(&start).to_string();
    let o = w.run(&["restore", &short]);
    assert!(!o.status.success(), "a restore asks first");
    let out = text(&o.stdout);
    assert!(out.contains("1 come back: src/cart.ts"), "{out}");
    assert!(
        out.contains("1 are newer than the backup, and stay (--exact removes them): docs/new.md"),
        "{out}"
    );
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "the copy\n");
    let v = w.json(&["restore", &short, "--yes", "--json"]);
    assert_eq!(v["api_version"], "paddock.restore/v1");
    assert_eq!(v["of"], start.as_str());
    assert_eq!(strs(&v["restored"]), ["src/cart.ts"]);
    assert_eq!(strs(&v["newer"]), ["docs/new.md"]);
    assert!(v["backup"].is_string());
    assert_eq!(get(&w.project, "src/cart.ts").unwrap(), "one\ntwo\nthree\n");
    let v = w.json(&["restore", &start, "--exact", "--yes", "--json"]);
    assert_eq!(strs(&v["removed"]), ["docs/new.md"]);
    // From another folder, with --project.
    let o = w
        .cmd()
        .current_dir(&w.root)
        .args(["restore", &short, "--yes", "--project"])
        .arg(&w.project)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert!(
        text(&o.stdout).contains("The folder is the same as the backup"),
        "{}",
        text(&o.stdout)
    );
    let o = w.run(&["restore", "ffffff", "--yes"]);
    assert!(
        text(&o.stderr).contains("no backup ffffff"),
        "{}",
        text(&o.stderr)
    );
    // The kinds in the list: a restore is a backup too.
    let kinds: Vec<String> = w.json(&["backups", "--json"])["backups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["restore", "restore", "apply", "start"]);
}
