//! A terminal that closes with paddock in it (CORE5): `paddock run` runs on
//! a real pty, as in a tmux pane, and the test closes the pty. paddock must
//! stop the sandbox and run its end steps. The last end step writes the
//! `end` file of the run's mailbox, so that file proves that they ran.
//!
//! Two cases: paddock is the session leader of the pty (the pane runs it
//! directly), and a shell is the session leader with paddock in its
//! foreground group (the pane runs a shell). The failure message has the
//! exit status of paddock: a signal there names what killed it.
//!
//! The pty comes from tests/common/pty.rs: no child keeps a copy of the
//! master, so the close of the test hangs the pty up, as a real terminal
//! does.
//!
//! bwrap on Linux (skipped where it cannot start), Seatbelt on macOS.

mod common;

use common::device_world::{finish, wait_for, World};
use common::pty::spawn_on_pty;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "macos")]
const BACKEND: &str = "seatbelt";
#[cfg(not(target_os = "macos"))]
const BACKEND: &str = "bwrap";

fn world() -> Option<World> {
    if cfg!(not(target_os = "macos")) && !common::device_world::can_run() {
        return None;
    }
    let w = World::new("[devices]\nmount = false\n[kv]\nenabled = false\n");
    std::fs::create_dir_all(w.root.join("project/flags")).unwrap();
    w.harness("tty", ": > flags/started\nawait flags/never\n");
    Some(w)
}

/// The `end` files of the mailboxes: one per run whose end steps ran.
fn ended(w: &World) -> Vec<PathBuf> {
    std::fs::read_dir(w.root.join("state/paddock/mail"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path().join("end"))
                .filter(|p| p.exists())
                .collect()
        })
        .unwrap_or_default()
}

/// Start `cmd` as the session leader of a new pty, wait for the harness,
/// close the pty, and check that the end steps ran.
fn close_the_terminal(w: &World, cmd: Command, what: &str) {
    let (child, term) = spawn_on_pty(cmd);
    wait_for("the harness start", Duration::from_secs(60), || {
        w.root.join("project/flags/started").exists()
    });
    assert!(ended(w).is_empty(), "{what}: the run has not ended yet");
    let seen = term.close();
    let out = finish(child, Duration::from_secs(60));
    // With a shell between, paddock can outlive it for a moment.
    let t = std::time::Instant::now();
    while ended(w).is_empty() && t.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !ended(w).is_empty(),
        "{what}: the end steps did not run. Session status: {:?}\n--- the terminal:\n{}",
        out.status,
        String::from_utf8_lossy(&seen)
    );
}

fn run_args(w: &World) -> Vec<String> {
    let mut a: Vec<String> = [
        "run",
        "tty",
        "--backend",
        BACKEND,
        "--network",
        "none",
        "--env",
        "clean",
        "-y",
        "--direct",
    ]
    .map(String::from)
    .to_vec();
    a.push(w.root.join("project").display().to_string());
    a
}

#[test]
fn a_closed_terminal_runs_the_end_steps_of_its_session_leader() {
    let Some(w) = world() else { return };
    let mut cmd = w.paddock();
    cmd.args(run_args(&w));
    close_the_terminal(&w, cmd, "paddock as the session leader");
}

#[test]
fn a_closed_terminal_runs_the_end_steps_under_a_shell() {
    let Some(w) = world() else { return };
    // `; :` keeps the shell: it does not exec paddock, so the shell is the
    // session leader and paddock is in its foreground group.
    let mut cmd = w.paddock();
    let program = cmd.get_program().to_owned();
    let mut sh = Command::new("/bin/sh");
    for (k, v) in cmd.get_envs() {
        match v {
            Some(v) => sh.env(k, v),
            None => sh.env_remove(k),
        };
    }
    if let Some(d) = cmd.get_current_dir() {
        sh.current_dir(d);
    }
    sh.arg("-c")
        .arg("\"$0\" \"$@\"; :")
        .arg(&program)
        .args(run_args(&w));
    cmd = sh;
    close_the_terminal(&w, cmd, "paddock under a shell");
}

/// The terminal of the tests itself, without paddock and without a
/// sandbox: its close hangs up the session leader, as a real terminal
/// does. With `openpty(3)`, the child kept a copy of the master and lived.
#[test]
fn the_test_terminal_hangs_up_its_session_leader() {
    let mut sh = Command::new("/bin/sh");
    sh.args(["-c", "sleep 120"]);
    let (child, term) = spawn_on_pty(sh);
    std::thread::sleep(Duration::from_millis(300));
    term.close();
    let out = finish(child, Duration::from_secs(20));
    use std::os::unix::process::ExitStatusExt as _;
    assert_eq!(out.status.signal(), Some(libc::SIGHUP), "{:?}", out.status);
}
