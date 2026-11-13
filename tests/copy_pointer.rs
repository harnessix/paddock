//! `paddock pointer`, and what `paddock explain` and the help say about a
//! run on a copy, with the built paddock and a temp home.

mod common;

use common::copy_world::{put, text, world};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

#[test]
fn the_pointer_commands() {
    let w = world();
    // No pointer yet: nothing is made by a look.
    let o = w.run(&["pointer", "status"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stdout).contains("▲ no pointer"),
        "{}",
        text(&o.stdout)
    );
    assert!(!w.project.join(".paddock").exists());
    assert!(text(&w.run(&["backups"]).stdout).contains("No backups yet."));
    let o = w.run(&["pointer", "create"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v = w.json(&["pointer", "status", "--json"]);
    assert_eq!(v["api_version"], "paddock.pointer/v1");
    assert_eq!(v["project"], w.project.display().to_string());
    assert_eq!(
        (v["state"].as_str(), v["ok"].as_bool()),
        (Some("linked"), Some(true))
    );
    assert_eq!(v["text"], "The pointer and the store agree.");
    assert!(v["action"].is_null());
    assert_eq!(v["pointer"]["format"], 1);
    let id = v["pointer"]["store"].as_str().unwrap();
    assert_eq!(v["store"]["id"], id);
    assert_eq!(v["store"]["original"], w.project.display().to_string());
    assert!(v["store"]["dir"]
        .as_str()
        .unwrap()
        .ends_with(&format!("data/paddock/copies/{id}")));
    assert_eq!(
        (
            v["store"]["runs"].clone(),
            v["store"]["copies"].clone(),
            v["store"]["backups"].clone()
        ),
        (0.into(), 0.into(), 0.into())
    );
    assert!(v["store"]["size"]["bytes"].is_u64() && v["store"]["size"]["kind"].is_string());
    assert!(["encrypted", "not_encrypted", "unknown"]
        .contains(&v["encryption"]["state"].as_str().unwrap()));
    let words = text(&w.run(&["pointer", "status"]).stdout);
    assert!(words.contains("● linked"), "{words}");
    assert!(
        words.contains("A copy of this folder carries only the pointer, not the files."),
        "{words}"
    );
    assert!(words.contains("Disk       "), "{words}");
    // A second create and a new store for a linked folder are refused.
    assert!(!w.run(&["pointer", "create"]).status.success());
    let o = w.run(&["pointer", "new", "--yes"]);
    assert!(
        text(&o.stderr).contains("has its store already"),
        "{}",
        text(&o.stderr)
    );
    // A copy of the folder: its own store on request.
    let dup = w.root.join("webshop-copy");
    std::fs::create_dir_all(dup.join(".paddock")).unwrap();
    std::fs::copy(
        w.project.join(".paddock/pointer.json"),
        dup.join(".paddock/pointer.json"),
    )
    .unwrap();
    let st = |dir: &Path| {
        let o = w
            .cmd()
            .current_dir(dir)
            .args(["pointer", "status", "--json"])
            .output()
            .unwrap();
        serde_json::from_slice::<Value>(&o.stdout).unwrap()
    };
    let v = st(&dup);
    assert_eq!(
        (v["state"].as_str(), v["ok"].as_bool()),
        (Some("copy"), Some(false))
    );
    assert_eq!(v["action"], "paddock pointer new");
    let o = w
        .cmd()
        .current_dir(&dup)
        .args(["pointer", "new"])
        .output()
        .unwrap();
    assert!(!o.status.success(), "it asks first");
    let o = w
        .cmd()
        .current_dir(&dup)
        .args(["pointer", "new", "--yes"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v2 = st(&dup);
    assert_eq!(v2["state"], "linked");
    assert_ne!(v2["pointer"]["store"], id);
    assert_eq!(st(&w.project)["pointer"]["store"], id);
    // A folder that came back: the link, on request.
    let back = w.root.join("webshop-back");
    std::fs::rename(&dup, &back).unwrap();
    assert_eq!(st(&back)["state"], "moved");
    let restored = w.root.join("webshop-restored");
    std::fs::create_dir_all(restored.join(".paddock")).unwrap();
    std::fs::copy(
        back.join(".paddock/pointer.json"),
        restored.join(".paddock/pointer.json"),
    )
    .unwrap();
    std::fs::remove_dir_all(&back).unwrap();
    assert_eq!(st(&restored)["state"], "restored");
    assert_eq!(st(&restored)["action"], "paddock pointer link");
    let o = w
        .cmd()
        .current_dir(&restored)
        .args(["pointer", "link", "--yes"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(st(&restored)["state"], "linked");
}

#[test]
fn explain_says_whether_a_run_uses_a_copy_and_makes_nothing() {
    let w = world();
    let explain = |args: &[&str], dir: &Path| {
        let o = w
            .cmd()
            .args(["explain", "fake"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", text(&o.stderr));
        text(&o.stdout)
    };
    // A folder without git: a copy by default.
    let out = explain(&[], &w.project);
    assert!(
        out.contains("copy      a copy, because of the folder has no git history."),
        "{out}"
    );
    assert!(
        out.contains("`--direct` works in the folder itself"),
        "{out}"
    );
    assert!(
        !w.project.join(".paddock/pointer.json").exists(),
        "an explain makes no pointer"
    );
    assert!(
        !w.root.join("data/paddock/copies").exists(),
        "an explain makes no store"
    );
    let out = explain(&["--direct"], &w.project);
    assert!(
        out.contains("copy      no: the agent works in the folder itself."),
        "{out}"
    );
    // A git project: the folder itself, until the user asks.
    let repo = w.root.join("repo");
    put(&repo, "a.txt", "a\n");
    let init = Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .env("HOME", w.root.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status();
    if !init.is_ok_and(|s| s.success()) {
        eprintln!("skipped the git part: git is not here");
        return;
    }
    let out = explain(&[], &repo);
    assert!(out.contains("copy      no:"), "{out}");
    let out = explain(&["--copy"], &repo);
    assert!(
        out.contains("copy      a copy, because of --copy."),
        "{out}"
    );
    let out = explain(&["--launcher"], &repo);
    assert!(
        out.contains("copy      a copy, because of a launcher run."),
        "{out}"
    );
    // The JSON.
    let o = w
        .cmd()
        .args(["explain", "fake", "--json"])
        .arg(&w.project)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["copy"]["use"], true);
    assert_eq!(v["copy"]["why"], "the folder has no git history");
    assert!(v["copy"]["text"]
        .as_str()
        .unwrap()
        .contains("The store is in"));
    let o = w
        .cmd()
        .args(["explain", "fake", "--json", "--direct"])
        .arg(&w.project)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v["copy"],
        serde_json::json!({"use": false, "why": null, "text": null})
    );
    // The setting, and the flags that do not go together.
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        "[copy]\ndefault = \"never\"\n",
    )
    .unwrap();
    assert!(explain(&[], &w.project).contains("copy      no:"));
    assert!(explain(&["--copy"], &w.project).contains("copy      a copy"));
    let o = w.run(&["explain", "fake", "--copy", "--direct"]);
    assert_eq!(o.status.code(), Some(2));
    let o = w.run(&["explain", "fake", "--copy", "--worktree"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn the_help_names_the_commands_and_the_conflict_choices() {
    let w = world();
    let help = text(&w.run(&["apply", "--help"]).stdout);
    for s in [
        "paddock changes 1a2b3c",
        "paddock undo",
        "--on-conflict",
        "paddock makes a backup",
    ] {
        assert!(help.contains(s), "missing {s:?} in:\n{help}");
    }
    let top = text(&w.run(&["--help"]).stdout);
    for c in [
        "changes", "apply", "keep", "backups", "restore", "pointer", "discard", "undo",
    ] {
        assert!(
            top.lines().any(|l| l.trim_start().starts_with(c)),
            "{c} in:\n{top}"
        );
    }
    let run = text(&w.run(&["run", "--help"]).stdout);
    assert!(run.contains("--copy") && run.contains("--direct"), "{run}");
    assert!(
        !run.contains("--launcher"),
        "the launcher flag is not for users"
    );
}
