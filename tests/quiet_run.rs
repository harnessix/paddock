//! `paddock -q run` (QUIET1): the notes of the start do not print, a fatal
//! error does, and `paddock ps --json` still has the notes as `run_notes`.
//! Without `-q` the notes print as before. A fake harness in a real
//! sandbox (bwrap), a temp home only. Skipped where bwrap cannot start.

mod common;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn cmd(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("cfg"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("PADDOCK_NO_PROMOTE", "1")
        .env("PADDOCK_NO_DEDUPE", "1")
        .env_remove("PADDOCK_LOG")
        .env_remove("TMUX")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(Stdio::null());
    for v in paddock::proxy::PROXY_VARS {
        c.env_remove(v);
    }
    c
}

struct World {
    _tmp: common::TestDir,
    root: PathBuf,
    project: PathBuf,
}

/// A temp host with the fake harness `fake`, which runs `script`.
fn world(script: &str) -> Option<World> {
    if cfg!(target_os = "macos") || !common::bwrap_works() {
        eprintln!("skipped: bwrap cannot start here");
        return None;
    }
    let tmp = common::tempdir();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    for d in ["cfg/paddock/harness.d", "home", "p", "run"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::set_permissions(
        root.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/config.toml"),
        "[devices]\nmount = false\n[kv]\nenabled = false\n",
    )
    .unwrap();
    std::fs::write(
        root.join("cfg/paddock/harness.d/fake.toml"),
        format!(
            "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n",
            serde_json::to_string(script).unwrap()
        ),
    )
    .unwrap();
    let project = root.join("p");
    std::fs::write(project.join("a.txt"), "a\n").unwrap();
    Some(World {
        _tmp: tmp,
        root,
        project,
    })
}

impl World {
    fn run(&self, quiet: &[&str]) -> Command {
        let mut c = cmd(&self.root);
        c.args(quiet)
            .args([
                "run",
                "fake",
                "--backend",
                "bwrap",
                "--network",
                "none",
                "-y",
            ])
            .arg(&self.project)
            .current_dir(&self.project);
        c
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The lines of paddock on stderr that are not errors.
fn notes(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.starts_with("paddock: ") && !l.starts_with("paddock: error: "))
        .collect()
}

#[test]
fn without_q_the_start_notes_print_as_before() {
    let Some(w) = world("true") else { return };
    let o = w.run(&[]).output().unwrap();
    let e = err(&o);
    assert!(o.status.success(), "{e}");
    assert!(
        e.lines()
            .any(|l| l.starts_with("paddock: fake in ") && l.contains("(backend bwrap")),
        "{e}"
    );
}

#[test]
fn q_hides_the_notes_and_the_record_keeps_them() {
    let Some(w) = world("sleep 4") else { return };
    let child = w
        .run(&["-q"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The run record has the notes that -q hid.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = None;
    while Instant::now() < deadline && seen.is_none() {
        let o = cmd(&w.root).args(["ps", "--json"]).output().unwrap();
        let rows: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
        // `paddock.ps/v1`: an object, the runs in `sandboxes`.
        seen = rows["sandboxes"]
            .as_array()
            .and_then(|a| a.iter().find_map(|r| r.get("run_notes").cloned()));
        std::thread::sleep(Duration::from_millis(200));
    }
    let o = child.wait_with_output().unwrap();
    let e = err(&o);
    assert!(o.status.success(), "{e}");
    assert!(notes(&e).is_empty(), "-q printed notes: {e}");
    let seen = seen.unwrap_or_else(|| panic!("no run_notes in paddock ps --json; stderr: {e}"));
    let list = seen.as_array().unwrap();
    assert!(
        list.iter().any(|n| n["level"] == "info"
            && n["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("fake in ") && t.contains("(backend bwrap"))),
        "{seen}"
    );
    assert!(
        list.iter()
            .all(|n| n["level"].is_string() && n["text"].is_string()),
        "{seen}"
    );
}

#[test]
fn q_and_qq_keep_a_fatal_error() {
    // No sandbox starts: the harness does not exist.
    let tmp = common::tempdir();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::create_dir_all(root.join("p")).unwrap();
    for q in ["-q", "-qq"] {
        let o = cmd(&root)
            .args([q, "run", "no-such-harness", "-y"])
            .arg(root.join("p"))
            .current_dir(root.join("p"))
            .output()
            .unwrap();
        let e = err(&o);
        assert!(!o.status.success(), "{q}: {e}");
        assert!(e.lines().any(|l| l.starts_with("paddock: ")), "{q}: {e}");
    }
}
