//! `paddock tui` starts the terminal interface. Without one, the paddock of
//! these tests, a development build, says how to build it: it never
//! downloads, and it never runs npx or node. Fake programs on a temporary
//! PATH. No Node.js needed, and no network. The download of a release
//! build has its tests in src/update/tui_tests.rs, with a local release
//! server.

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
    // The tests may run in a paddock sandbox. These starts are on a host.
    c.env_remove("PADDOCK").env_remove("PADDOCK_TUI");
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

/// The one line of a development build without the interface.
fn assert_says_how_to_build(out: &std::process::Output) {
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(
        err.starts_with("The paddock TUI is not installed."),
        "{err}"
    );
    assert!(err.contains("development build"), "{err}");
    let dir = Path::new(env!("CARGO_BIN_EXE_paddock")).parent().unwrap();
    let dir = dir.canonicalize().unwrap();
    assert!(
        err.trim_end()
            .ends_with(&format!("with: tui/scripts/install.sh {}", dir.display())),
        "{err}"
    );
    assert!(
        !err.contains("npm install") && !err.contains("npx"),
        "{err}"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn without_the_tui_a_development_build_says_how_to_build_it() {
    let tmp = common::tempdir();
    let out = paddock_tui(tmp.path(), &[]);
    assert_says_how_to_build(&out);
}

/// The interface is one program with its own runtime. paddock does not ask
/// node for its version, old or new.
#[test]
fn an_old_node_is_never_asked() {
    let tmp = common::tempdir();
    let rec = tmp.path().join("ran.txt");
    script(
        tmp.path(),
        "node",
        &format!("echo node >> '{}'\necho v18.19.0", rec.display()),
    );
    script(
        tmp.path(),
        "npx",
        &format!("echo npx >> '{}'\nexit 3", rec.display()),
    );
    let out = paddock_tui(tmp.path(), &[]);
    assert_says_how_to_build(&out);
    assert!(!rec.exists(), "{:?}", std::fs::read_to_string(&rec));
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
    // A planted program in the project is not on the absolute PATH.
    script(&project, "paddock-tui", "exit 9");
    let rec = tmp.path().join("args.txt");
    script(&bin, "node", "echo v22.11.0");
    script(
        &bin,
        "npx",
        &format!("printf '%s\\n' \"$@\" > '{}'", rec.display()),
    );
    let out = paddock(tmp.path())
        .args(["tui", "--readonly"])
        .env(
            "PATH",
            std::env::join_paths([bin.as_path(), Path::new(".")]).unwrap(),
        )
        .current_dir(&project)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        !rec.exists(),
        "npx ran: {:?}",
        std::fs::read_to_string(&rec)
    );
    assert_says_how_to_build(&out);
}

/// `PADDOCK_TUI` names the program of a developer. It comes before every
/// other place.
#[test]
fn paddock_tui_variable_names_the_program_to_start() {
    let tmp = common::tempdir();
    let dev = tmp.path().join("dev");
    std::fs::create_dir_all(&dev).unwrap();
    script(&dev, "cli.js", "echo dev \"$@\"\nexit 5");
    script(tmp.path(), "paddock-tui", "exit 7");
    let out = paddock(tmp.path())
        .args(["tui", "--plain"])
        .env("PATH", tmp.path())
        .env("PADDOCK_TUI", dev.join("cli.js"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "dev --plain\n");

    // A wrong value stops the start. paddock does not fall back to another
    // program in silence.
    let out = paddock(tmp.path())
        .arg("tui")
        .env("PATH", tmp.path())
        .env("PADDOCK_TUI", "dev/cli.js")
        .current_dir(tmp.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("PADDOCK_TUI=dev/cli.js is not the absolute path of a program"),
        "{err}"
    );
}

/// The copy that paddock installs into its data folder, when it cannot
/// write next to its binary, starts too.
#[test]
fn the_copy_in_the_data_folder_starts() {
    let tmp = common::tempdir();
    let own = tmp
        .path()
        .join("folders/data/paddock/tui")
        .join(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(&own).unwrap();
    script(&own, "paddock-tui", "echo own \"$@\"\nexit 6");
    let out = paddock_tui(tmp.path(), &["--readonly"]);
    assert_eq!(out.status.code(), Some(6), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "own --readonly\n");
}

/// In a sandbox, paddock installs nothing. It names the command for the
/// user on the host.
#[test]
fn in_a_sandbox_tui_asks_for_the_host() {
    let tmp = common::tempdir();
    let out = paddock(tmp.path())
        .arg("tui")
        .env("PATH", tmp.path())
        .env("PADDOCK", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(
        err.contains("sandbox (PADDOCK=1)") && err.trim_end().ends_with("on the host: paddock tui"),
        "{err}"
    );
}
