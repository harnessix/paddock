#![cfg(target_os = "linux")]
//! The leak check of test-limit.sh. tmux-leaks.sh lists a tmux server with
//! its socket in the folder that it is given, also after the socket file is
//! gone, and the guard of a test folder stops it. test-limit.sh fails a
//! binary that leaves a server in the folder of that binary, and does not
//! count or stop a server of another run.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn script(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".github/scripts")
        .join(name)
}

fn leaks(folder: &Path) -> String {
    let out = Command::new("bash")
        .arg(script("tmux-leaks.sh"))
        .arg(folder)
        .output()
        .expect("bash");
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn tmux(sock: &Path, args: &[&str]) -> bool {
    Command::new("tmux")
        .arg("-S")
        .arg(sock)
        .args(["-f", "/dev/null"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn no_tmux() -> bool {
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipped: no tmux");
        return true;
    }
    false
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run test-limit.sh with the shell script `body` as the test binary.
/// `$1` in `body` is `dir`.
fn test_limit(dir: &Path, body: &str) -> std::process::Child {
    let fake = dir.join("fake-test.sh");
    std::fs::write(&fake, body).unwrap();
    Command::new("bash")
        .arg(script("test-limit.sh"))
        .arg("/bin/sh")
        .arg(&fake)
        .arg(dir)
        .env_remove("GITHUB_ACTIONS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bash")
}

fn alive(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
        !s.rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

#[test]
fn the_check_lists_a_server_in_its_folder_until_the_guard_stops_it() {
    if no_tmux() {
        return;
    }
    let tmp = paddock::view::guard::tempdir();
    let other = paddock::view::guard::tempdir();
    let sock = tmp.path().join("run/paddock/view.sock");
    let outside = other.path().join("run/paddock/view.sock");
    for s in [&sock, &outside] {
        std::fs::create_dir_all(s.parent().unwrap()).unwrap();
        assert!(tmux(s, &["new-session", "-d", "-s", "leak", "sleep 60"]));
    }
    let line = format!(" {}\n", sock.display());
    let list = leaks(tmp.path());
    assert!(list.contains(&line), "{list}");
    // A server outside the folder is not in the list.
    assert!(!list.contains(&outside.display().to_string()), "{list}");
    assert_eq!(list.lines().count(), 1, "{list}");
    // The socket file leaves its path, the server stays: the check still
    // finds it. The guard finds the moved socket by its name.
    let moved = tmp.path().join("moved/view.sock");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&sock, &moved).unwrap();
    assert!(leaks(tmp.path()).contains(&line), "{}", leaks(tmp.path()));
    std::fs::rename(&moved, &sock).unwrap();
    let folder = tmp.path().to_path_buf();
    drop(tmp);
    // kill-server returns before the server process is gone.
    wait_for("the guard stops the server", || {
        !leaks(&folder).contains(&line)
    });
    assert!(tmux(&outside, &["has-session", "-t", "leak"]));
}

#[test]
fn test_limit_fails_a_binary_that_leaves_a_server_in_its_folder_and_stops_it() {
    if no_tmux() {
        return;
    }
    let dir = paddock::view::guard::tempdir();
    // Like a test: a folder of tempfile in TMPDIR, a server in it, and no
    // guard. The binary itself passes.
    let child = test_limit(
        dir.path(),
        "set -e\n\
         d=$(mktemp -d \"$TMPDIR/.tmpXXXXXX\")\n\
         printf '%s' \"$d/view.sock\" > \"$1/sock\"\n\
         tmux -S \"$d/view.sock\" -f /dev/null new-session -d -s leak 'sleep 60'\n\
         echo 'test result: ok'\n",
    );
    let out: Output = child.wait_with_output().unwrap();
    let sock = std::fs::read_to_string(dir.path().join("sock")).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The pid of the leaked server, from "SOCK (pid PID);".
    let pid = stderr
        .split(&format!("{sock} (pid "))
        .nth(1)
        .and_then(|r| r.split(')').next())
        .map(str::to_string);
    if let Some(pid) = &pid {
        let gone = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            !alive(pid)
        });
        if !gone {
            let _ = Command::new("kill").arg(pid).status();
        }
        assert!(gone, "test-limit did not stop the leaked server {pid}");
    }
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr.contains("test-limit: sh left tmux servers running with a socket in a test folder"),
        "{stderr}"
    );
    assert!(pid.is_some(), "no socket and pid in: {stderr}");
    // The folder of the binary is gone after the run.
    assert!(!Path::new(&sock).exists());
}

#[test]
fn test_limit_does_not_count_or_stop_a_server_of_another_run() {
    if no_tmux() {
        return;
    }
    let dir = paddock::view::guard::tempdir();
    let started = dir.path().join("started");
    let go = dir.path().join("go");
    // The binary passes and starts no server. While it runs, another test
    // run starts a server in a test folder of its own.
    let child = test_limit(
        dir.path(),
        ": > \"$1/started\"\n\
         i=0\n\
         while [ ! -e \"$1/go\" ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done\n\
         echo 'test result: ok'\n",
    );
    wait_for("the binary starts", || started.exists());
    let other = paddock::view::guard::tempdir();
    let sock = other.path().join("run/paddock/view.sock");
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    assert!(tmux(
        &sock,
        &["new-session", "-d", "-s", "other", "sleep 60"]
    ));
    std::fs::write(&go, "").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(
        tmux(&sock, &["has-session", "-t", "other"]),
        "test-limit stopped the server of another run"
    );
}
