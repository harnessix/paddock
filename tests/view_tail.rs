//! `paddock view ID --tail`: the end of a run's output for the terminal UI,
//! from the view log in a temporary folder. No harness, no tmux.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};

fn paddock(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env_remove("TMUX")
        .current_dir(root)
        .stdin(Stdio::null());
    c
}

fn world() -> tempfile::TempDir {
    let tmp = common::tempdir();
    let root = tmp.path();
    for d in ["home", "cfg/paddock", "run/paddock/view"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    tmp
}

#[test]
fn view_tail_prints_the_last_lines_without_escapes() {
    let tmp = world();
    let root = tmp.path();
    let id = "fake-t-1a2b3c";
    let mut log = String::new();
    for i in 1..=50 {
        log.push_str(&format!("line {i}\n"));
    }
    log.push_str("\x1b[32mgreen\x1b[0m \x1b]0;evil title\x07end\n\n");
    std::fs::write(root.join(format!("run/paddock/view/{id}.log")), log).unwrap();

    let out = paddock(root)
        .args(["view", id, "--tail", "3", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["api_version"], "paddock.view-tail/v1");
    assert_eq!(v["id"], id);
    assert_eq!(v["source"], "log");
    assert_eq!(
        v["lines"],
        serde_json::json!(["line 49", "line 50", "green end"])
    );

    let out = paddock(root).args(["view", id, "--tail"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 40, "{text}");
    assert!(text.ends_with("green end\n"), "{text}");
    assert!(!text.contains('\x1b'), "{text:?}");
}

#[test]
fn view_tail_of_a_run_without_output_is_empty() {
    let tmp = world();
    let out = paddock(tmp.path())
        .args(["view", "fake-t-000000", "--tail", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["source"].is_null(), "{v}");
    assert_eq!(v["lines"], serde_json::json!([]));
}

#[test]
fn view_tail_and_write_do_not_go_together() {
    let tmp = world();
    let out = paddock(tmp.path())
        .args(["view", "fake-t-000000", "--tail", "--write"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

/// The sources hold about 120 lines, so `--tail` promises no more.
#[test]
fn view_tail_gives_at_most_120_lines() {
    let tmp = world();
    let root = tmp.path();
    let id = "fake-t-4d5e6f";
    let log: String = (1..=300).map(|i| format!("l{i}\n")).collect();
    std::fs::write(root.join(format!("run/paddock/view/{id}.log")), log).unwrap();
    let out = paddock(root)
        .args(["view", id, "--tail", "500", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 120, "{v}");
    assert_eq!(lines[119], "l300");
    assert_eq!(lines[0], "l181");
}
