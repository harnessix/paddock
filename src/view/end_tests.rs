//! Tests of the end of a session and of its server.
use super::super::guard::TmuxGuard;
use super::super::tests::{env_in, wait_for};
use super::super::Tmux;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// A tmux client that stays connected to the server of a test until it
/// drops. While it is connected, the server does not exit on its own after
/// the last session (`exit-empty` waits for the clients), as when the
/// client of another paddock is connected. Its job also keeps a server
/// that was told to exit, so the drop ends the job and waits for its end:
/// the job removes its done file. The guard of the server must come
/// before the holder, so it drops after it.
struct Holder {
    child: std::process::Child,
    done: PathBuf,
}

impl Holder {
    fn new(tmux: &Tmux, dir: &Path) -> Holder {
        let done = dir.join("holder-done");
        let job = format!(
            "i=0; while [ ! -e '{d}' ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done; \
             rm -f '{d}'",
            d = done.display()
        );
        let child = tmux.cmd().args(["run-shell", &job]).spawn().unwrap();
        wait_for("the holding client", || {
            tmux.cmd()
                .args(["list-clients"])
                .stdout(Stdio::piped())
                .output()
                .is_ok_and(|o| o.status.success())
        });
        Holder { child, done }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.done, "");
        let start = Instant::now();
        while self.done.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.wait();
    }
}

#[test]
fn kill_of_the_last_session_ends_the_server() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path(), &[]);
    let Some(tmux) = Tmux::find(&env) else {
        eprintln!("skipped: tmux is not installed");
        return;
    };
    let _server = TmuxGuard::of(&tmux);
    let sleep = ["sleep", "60"].map(std::ffi::OsString::from);
    tmux.new_session("sbend1", &sleep).unwrap();
    tmux.new_session("sbend2", &sleep).unwrap();
    let holder = Holder::new(&tmux, t.path());
    let out = tmux
        .cmd()
        .args(["display-message", "-p", "#{pid}"])
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    let pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(!pid.is_empty());
    assert!(tmux.kill("sbend1"));
    // One session is left: the server stays.
    assert_eq!(tmux.sessions(), vec!["sbend2".to_string()]);
    assert!(tmux.kill("sbend2"));
    // No session is left: the server ends, also with a client connected.
    wait_for("the server ends", || !tmux.run(&["list-sessions"]));
    assert!(!tmux.kill("sbend2"));
    // The job of the holder keeps the process until it ends.
    drop(holder);
    wait_for("the server process ends", || {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
}

#[test]
fn kill_does_not_end_the_session_of_a_run_that_starts_meanwhile() {
    let t = tempfile::tempdir().unwrap();
    let env = env_in(t.path(), &[]);
    let Some(real) = Tmux::find(&env) else {
        eprintln!("skipped: tmux is not installed");
        return;
    };
    let _server = TmuxGuard::of(&real);
    // A tmux on PATH that pauses before the call that can stop the server,
    // until the test lets it go. In that pause, a new run starts its
    // session.
    let bin = t.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let paused = t.path().join("paused");
    let go = t.path().join("go");
    let wrapper = bin.join("tmux");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ncase \" $* \" in *' kill-server '*)\n  : > '{p}'\n  i=0\n  \
             while [ ! -e '{g}' ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done ;;\nesac\n\
             exec '{r}' \"$@\"\n",
            p = paused.display(),
            g = go.display(),
            r = real.exe.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &wrapper,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let slow = Tmux::find(&env_in(t.path(), &[("PATH", &path)])).unwrap();
    assert_eq!(slow.exe, wrapper);
    assert_eq!(slow.sock, real.sock);

    let sleep = ["sleep", "60"].map(std::ffi::OsString::from);
    real.new_session("sbold1", &sleep).unwrap();
    // The server must not exit on its own between two tmux calls of kill.
    let holder = Holder::new(&real, t.path());
    let ender = std::thread::spawn(move || slow.kill("sbold1"));
    wait_for("the pause before kill-server", || paused.exists());
    real.new_session("sbnew1", &sleep).unwrap();
    std::fs::write(&go, "").unwrap();
    assert!(ender.join().unwrap());
    assert!(!real.has("sbold1"));
    assert!(real.has("sbnew1"), "kill ended the session of the new run");
    drop(holder);
}
