//! `paddock tui` starts the terminal interface, or says what to install.
//! Fake programs on a temporary PATH. No Node.js needed.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

fn script(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// paddock with HOME and the XDG folders inside `root`, never the user's.
fn paddock(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    for (key, dir) in [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let d = root.join("folders").join(dir);
        std::fs::create_dir_all(&d).unwrap();
        c.env(key, d);
    }
    c
}

fn paddock_tui(path: &Path, args: &[&str]) -> std::process::Output {
    paddock(path)
        .arg("tui")
        .args(args)
        .env("PATH", path)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn without_node_tui_says_what_to_install() {
    let tmp = common::tempdir();
    let out = paddock_tui(tmp.path(), &[]);
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("needs Node.js 22 or newer"), "{err}");
    assert!(
        err.contains("paddock-tui binary of a paddock release next to paddock"),
        "{err}"
    );
    assert!(err.contains("cd tui && npm ci && npm run build"), "{err}");
    assert!(!err.contains("npm install"), "{err}");
    assert!(out.stdout.is_empty());
}

#[test]
fn an_old_node_is_named_in_the_message() {
    let tmp = common::tempdir();
    script(tmp.path(), "node", "echo v18.19.0");
    script(tmp.path(), "npx", "exit 3");
    let out = paddock_tui(tmp.path(), &[]);
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("version 18"), "{err}");
}

#[test]
fn tui_runs_paddock_tui_with_its_arguments_and_this_paddock() {
    let tmp = common::tempdir();
    let rec = tmp.path().join("args.txt");
    script(
        tmp.path(),
        "paddock-tui",
        &format!(
            "printf '%s\\n' \"$PADDOCK_BIN\" \"$@\" > '{}'\nexit 7",
            rec.display()
        ),
    );
    let out = paddock_tui(tmp.path(), &["--plain", "--help"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    let text = std::fs::read_to_string(&rec).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[1..], ["--plain", "--help"], "{text}");
    assert!(lines[0].ends_with("/paddock"), "{text}");
}

/// npx would run code from npm, and npm exec prefers a planted
/// `node_modules/@harnessix/paddock-tui` and reads `.npmrc` of the current
/// folder, which the agent can write. paddock never runs it.
#[test]
fn npx_is_never_run_even_with_a_planted_package_in_the_project() {
    let tmp = common::tempdir();
    let bin = tmp.path().join("bin");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(project.join("node_modules/@harnessix/paddock-tui")).unwrap();
    std::fs::write(project.join(".npmrc"), "registry=http://127.0.0.1:9/\n").unwrap();
    let rec = tmp.path().join("args.txt");
    script(&bin, "node", "echo v22.11.0");
    script(
        &bin,
        "npx",
        &format!("printf '%s\\n' \"$@\" > '{}'", rec.display()),
    );
    let out = paddock(tmp.path())
        .args(["tui", "--readonly"])
        .env("PATH", &bin)
        .current_dir(&project)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    assert!(
        !rec.exists(),
        "npx ran: {:?}",
        std::fs::read_to_string(&rec)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("does not download it with npx"), "{err}");
    assert!(
        err.contains("paddock-tui binary of a paddock release next to paddock"),
        "{err}"
    );
    assert!(err.contains("cd tui && npm ci && npm run build"), "{err}");
    assert!(!err.contains("npm install"), "{err}");
}
