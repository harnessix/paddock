//! `paddock run --detach` end to end: the run starts in a session of
//! paddock's tmux server and the command returns at once; the fake harness
//! gets the `[detach]` args, a readable prompt file and a terminal;
//! `paddock-host done` ends the run and `paddock wait` reports it; a client
//! with write access keeps the run open. A fake harness, temporary folders
//! and a tmux server on a socket in the temporary runtime folder, killed at
//! the end. No terminal window opens.

mod common;

use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Child;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The fake harness: `$0` is the prompt file (`[detach] args`). Its first
/// line is the task; the mailbox reminder follows. It records what it got
/// in the project, then acts on the task.
const SCRIPT: &str = r#"f="$0"
if test -t 0 && test -t 1; then tty=yes; else tty=no; fi
{ echo "tty $tty"; test -r "$f" && echo "readable"; echo "task $(head -n1 "$f")"; grep -c "paddock-host mail wait" "$f"; } > .detach-result
case "$(head -n1 "$f")" in
  ok) paddock-host done --status ok "all good" ;;
  failed) paddock-host done --status failed "it broke" ;;
  late) while [ ! -e .go ]; do sleep 0.2; done; paddock-host done "late but fine" ;;
  idle) echo idle-line; exec sleep 180 ;;
  again) echo first-line; while [ ! -e .more ]; do sleep 0.2; done; echo second-line; exec sleep 180 ;;
  screen)
    printf "  Hooks need review\n  1 hook is new or changed.\n  Hooks can run outside the sandbox after you trust them.\n› 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)\n  enter confirm · esc skip\n"
    stty -icanon -echo min 1
    c=$(dd bs=1 count=1 2>/dev/null)
    printf '\033[2J\033[Hgot %s\n' "$c"
    echo "key $c" > .screen-answer
    paddock-host done "continued after $c" ;;
esac
exec sleep 120
"#;

struct World {
    _tmp: common::TestDir,
    root: PathBuf,
}

impl World {
    fn new(detach: bool) -> Self {
        Self::with(detach, "")
    }

    /// `extra` goes at the end of the fake profile.
    fn with(detach: bool, extra: &str) -> Self {
        let tmp = common::tempdir();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let cfg = root.join("cfg/paddock/harness.d");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(root.join("project")).unwrap();
        let run = root.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let table = if detach {
            "[detach]\nargs = [\"{prompt_file}\"]\n"
        } else {
            ""
        };
        std::fs::write(
            cfg.join("fake.toml"),
            format!(
                "name = \"fake\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n{table}{extra}",
                serde_json::to_string(SCRIPT).unwrap()
            ),
        )
        .unwrap();
        World { _tmp: tmp, root }
    }

    fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("TMUX")
            .current_dir(self.root.join("project"))
            .stdin(Stdio::null());
        c
    }

    /// `paddock run fake --detach --prompt TASK`: its id.
    fn detach(&self, task: &str) -> String {
        let start = Instant::now();
        let out = self
            .paddock()
            .args([
                "run",
                "fake",
                "--backend",
                "bwrap",
                "--network",
                "none",
                "--env",
                "clean",
                "-y",
                // The script writes its result in the project folder.
                "--direct",
                "--detach",
                "--prompt",
                task,
            ])
            .arg(self.root.join("project"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "it did not return at once"
        );
        let text = String::from_utf8(out.stdout).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        let id = lines[0].to_string();
        assert_eq!(
            lines[1],
            format!("paddock view {} --write", paddock::ids::short(&id))
        );
        assert_eq!(
            lines[2],
            format!("mail: paddock mail watch {}", paddock::ids::short(&id))
        );
        assert!(self.has_session(&id), "no tmux session {id}");
        id
    }

    fn wait(&self, id: &str, timeout: &str) -> (Option<i32>, String) {
        let out = self
            .paddock()
            .args(["wait", id, "--timeout", timeout])
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    /// The shared view socket. macOS keeps it in `/tmp/paddock-<uid>`,
    /// named by a hash of the run directory as paddock sees it.
    fn sock(&self) -> PathBuf {
        let uid = unsafe { libc::getuid() };
        let mut runs = vec![self.root.join("run/paddock")];
        if let Ok(real) = self.root.canonicalize() {
            runs.push(real.join("run/paddock"));
        }
        let socks: Vec<PathBuf> = runs
            .iter()
            .map(|r| paddock::sockpath::socket_dir(r, uid, "").join("view.sock"))
            .collect();
        socks
            .iter()
            .find(|s| s.exists())
            .unwrap_or(&socks[0])
            .clone()
    }

    fn tmux(&self) -> Command {
        let mut c = Command::new("tmux");
        c.arg("-S").arg(self.sock()).env_remove("TMUX");
        c
    }

    fn has_session(&self, id: &str) -> bool {
        self.tmux()
            .args(["has-session", "-t", &format!("={id}")])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// A client with write access, in control mode: it needs no terminal.
    #[cfg(target_os = "linux")]
    fn attach(&self, id: &str) -> Child {
        self.tmux()
            .args(["-C", "attach", "-t", id])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    fn result(&self) -> String {
        let f = self.root.join("project/.detach-result");
        let start = Instant::now();
        loop {
            if let Ok(t) = std::fs::read_to_string(&f) {
                if t.contains("task") {
                    return t;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "the harness never ran"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `[detach]` and `[view]` in the user configuration, not the harness profile.
    #[cfg(target_os = "linux")]
    fn user_config(&self, text: &str) {
        let dir = self.root.join("cfg/paddock");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn sandbox(&self, id: &str) -> serde_json::Value {
        let out = self.paddock().args(["ps", "--json"]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        v["sandboxes"]
            .as_array()
            .and_then(|a| a.iter().find(|s| s["id"] == id).cloned())
            .unwrap_or(serde_json::Value::Null)
    }

    /// Poll until `ps --json` reports `idle_since` for this run.
    #[cfg(target_os = "linux")]
    fn wait_idle(&self, id: &str) -> u64 {
        let start = Instant::now();
        loop {
            let sb = self.sandbox(id);
            if let Some(t) = sb.get("idle_since").and_then(|v| v.as_u64()) {
                assert!(t > 0, "{sb}");
                return t;
            }
            assert!(
                start.elapsed() < Duration::from_secs(40),
                "the run never became idle: {sb}"
            );
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    #[cfg(target_os = "linux")]
    fn messages(&self) -> String {
        let out = self.tmux().args(["show-messages"]).output().unwrap();
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

fn has_tmux() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_profile_without_detach_refuses() {
    let w = World::new(false);
    let out = w
        .paddock()
        .args(["run", "fake", "--detach", "--prompt", "ok"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has no [detach] table"), "{err}");
    assert!(!w.sock().exists(), "a refused run started tmux");
}

#[test]
fn detach_needs_a_prompt() {
    let w = World::new(true);
    let out = w
        .paddock()
        .args(["run", "fake", "--detach"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--prompt-file FILE or --prompt TEXT"), "{err}");
}

#[test]
fn a_run_that_fails_to_launch_leaves_no_board_record() {
    let w = World::new(true);
    // A tmux that fails every command: the pane never starts.
    let bin = w.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("tmux"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(
        bin.join("tmux"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let ticket = w.root.join("project/.paddock/tickets/KV2-worker.md");
    std::fs::create_dir_all(ticket.parent().unwrap()).unwrap();
    std::fs::write(&ticket, "ok\n").unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let out = w
        .paddock()
        .env("PATH", path)
        .args(["run", "fake", "-y", "--detach", "--prompt-file"])
        .arg(&ticket)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let board = w.root.join("state/paddock/board");
    let left: Vec<_> = std::fs::read_dir(&board)
        .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "records left: {left:?}");
}

#[test]
fn detach_starts_a_session_on_the_private_socket_and_returns() {
    if !has_tmux() {
        eprintln!("skipped: needs tmux");
        return;
    }
    let w = World::new(true);
    // Without bwrap the run in the pane fails, but the session is there,
    // and the pane stays after its end.
    let id = w.detach("hang");
    let out = w.paddock().args(["ps", "--json"]).output().unwrap();
    assert!(out.status.success());
    if !common::bwrap_works() {
        // The run in the pane ends; wait sees that and has no `done`.
        let (code, out) = w.wait(&id, "60s");
        assert_eq!(code, Some(1), "{out}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn done_ends_the_run_and_wait_reports_it() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    let id = w.detach("ok");
    let (code, out) = w.wait(&id, "60s");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(out.trim(), format!("{id}: ok: all good"));
    let r = w.result();
    assert!(r.contains("tty yes"), "{r}");
    assert!(r.contains("readable"), "{r}");
    assert!(
        r.contains("task ok\n1\n"),
        "the prompt ends with the mailbox reminder: {r}"
    );
    // The mailbox saw the run from its start to its end.
    let out = w
        .paddock()
        .args(["mail", "watch", &id, "--timeout", "20s"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(text.contains("event started:"), "{text}");
    assert!(
        text.contains("event ended: the sandbox ended with exit code"),
        "{text}"
    );

    let w = World::new(true);
    let id = w.detach("failed");
    let (code, out) = w.wait(&id, "60s");
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(out.trim(), format!("{id}: failed: it broke"));
}

#[test]
#[cfg(target_os = "linux")]
fn ps_marks_the_run_and_wait_times_out() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    let id = w.detach("hang");
    w.result();
    let out = w.paddock().args(["ps", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let sb = v["sandboxes"]
        .as_array()
        .and_then(|a| a.iter().find(|s| s["id"] == id.as_str()).cloned())
        .expect("the sandbox is in ps");
    assert_eq!(sb["detached"], true, "{sb}");
    assert_eq!(sb["view_tmux"], true, "{sb}");
    let start = Instant::now();
    let (code, _) = w.wait(&id, "2s");
    assert_eq!(code, Some(124));
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(w.paddock().args(["stop", &id]).status().unwrap().success());
    let (code, _) = w.wait(&id, "60s");
    assert_eq!(code, Some(1));
}

#[test]
#[cfg(target_os = "linux")]
fn wait_and_stop_take_the_short_id() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    let id = w.detach("hang");
    // The sandbox shows in ps once it runs.
    w.result();
    let short = paddock::ids::short(&id);
    assert_eq!(w.sandbox(&id)["short"], short);
    let (code, _) = w.wait(&short[..4], "2s");
    assert_eq!(code, Some(124));
    assert!(w
        .paddock()
        .args(["stop", short])
        .status()
        .unwrap()
        .success());
    // The ended run still resolves.
    let (code, _) = w.wait(short, "60s");
    assert_eq!(code, Some(1));
}

/// Without bwrap and tmux: a fake running sandbox (a `sleep` with a run
/// record) and a fake ended run, named by short ids.
#[test]
fn stop_and_wait_take_a_short_id_of_records() {
    let w = World::new(true);
    let mut sleep = Command::new("sleep").arg("60").spawn().unwrap();
    let id = "fake-project-9c8b7a";
    let dir = w.root.join("run/paddock").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let info = serde_json::json!({
        "id": id, "harness": "fake", "project": w.root.join("project"),
        "backend": "bwrap", "network": "none", "gui": "none",
        "pid": sleep.id(), "started": 1,
    });
    std::fs::write(dir.join("info.json"), info.to_string()).unwrap();
    let out = w.paddock().args(["stop", "9c8b"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains(id), "{out:?}");
    assert!(
        !sleep.wait().unwrap().success(),
        "the sandbox got no SIGTERM"
    );

    let ended = "fake-project-5e4d3c";
    let records = w.root.join("state/paddock/detach");
    std::fs::create_dir_all(&records).unwrap();
    let rec = serde_json::json!({
        "id": ended, "harness": "fake", "project": w.root.join("project"),
        "started": 1, "ended": 2,
        "done": {"status": "ok", "text": "all good", "at": 2},
    });
    std::fs::write(records.join(format!("{ended}.json")), rec.to_string()).unwrap();
    let (code, out) = w.wait("5e4d3c", "5s");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(out.trim(), format!("{ended}: ok: all good"));
}

#[test]
fn wait_reports_a_dead_detached_process_without_a_mailbox() {
    let w = World::new(true);
    let id = "fake-project-9a8b7c";
    let records = w.root.join("state/paddock/detach");
    std::fs::create_dir_all(&records).unwrap();
    let rec = serde_json::json!({
        "id": id, "harness": "fake", "project": w.root.join("project"),
        "started": 1, "pid": 2147483647,
    });
    std::fs::write(records.join(format!("{id}.json")), rec.to_string()).unwrap();
    let (code, out) = w.wait(id, "5s");
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("ended without a final status"), "{out}");
}

#[test]
fn wait_reports_a_stopped_run_as_failed() {
    let w = World::new(true);
    let id = "fake-project-9a8b7d";
    let records = w.root.join("state/paddock/detach");
    std::fs::create_dir_all(&records).unwrap();
    let rec = serde_json::json!({
        "id": id, "harness": "fake", "project": w.root.join("project"),
        "started": 1, "ended": 2, "stopped": false,
    });
    std::fs::write(records.join(format!("{id}.json")), rec.to_string()).unwrap();
    std::fs::write(records.join(format!("{id}.stop")), "stopped\n").unwrap();
    let (code, out) = w.wait(id, "5s");
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("stopped before a final result"), "{out}");
}

#[test]
fn wait_uses_the_stopped_record_after_its_marker_is_removed() {
    let w = World::new(true);
    let id = "fake-project-9a8b7e";
    let records = w.root.join("state/paddock/detach");
    std::fs::create_dir_all(&records).unwrap();
    let rec = serde_json::json!({
        "id": id, "harness": "fake", "project": w.root.join("project"),
        "started": 1, "ended": 2, "stopped": true,
    });
    std::fs::write(records.join(format!("{id}.json")), rec.to_string()).unwrap();
    let (code, out) = w.wait(id, "5s");
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(out.trim(), format!("{id}: stopped before a final result"));
}

#[test]
fn wait_keeps_detached_final_results_after_a_stop() {
    let w = World::new(true);
    let id = "fake-project-9a8b7f";
    let records = w.root.join("state/paddock/detach");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(records.join(format!("{id}.stop")), "stopped\n").unwrap();
    for (status, expected) in [("ok", 0), ("failed", 1), ("idle", 1)] {
        for stopped in [false, true] {
            let rec = serde_json::json!({
                "id": id, "harness": "fake", "project": w.root.join("project"),
                "started": 1, "ended": 2, "stopped": stopped,
                "done": {"status": status, "text": "full result", "at": 2},
            });
            std::fs::write(records.join(format!("{id}.json")), rec.to_string()).unwrap();
            let (code, out) = w.wait(id, "5s");
            assert_eq!(code, Some(expected), "{out}");
            assert_eq!(out.trim(), format!("{id}: {status}: full result"));
        }
    }
}

#[test]
fn wait_does_not_finish_a_live_run_just_because_stop_was_requested() {
    let w = World::new(true);
    let id = "fake-project-9a8b80";
    let records = w.root.join("state/paddock/detach");
    std::fs::create_dir_all(&records).unwrap();
    let rec = serde_json::json!({
        "id": id, "harness": "fake", "project": w.root.join("project"),
        "started": 1, "pid": std::process::id(), "stopped": true,
    });
    std::fs::write(records.join(format!("{id}.json")), rec.to_string()).unwrap();
    std::fs::write(records.join(format!("{id}.stop")), "stopped\n").unwrap();
    let (code, out) = w.wait(id, "1s");
    assert_eq!(code, Some(124), "{out}");
}

#[test]
fn a_short_id_that_fits_no_sandbox_fails() {
    let w = World::new(true);
    for (cmd, input, says) in [
        ("wait", "1a2", "too short"),
        ("stop", "ffff", "no sandbox with id ffff"),
    ] {
        let out = w.paddock().args([cmd, input]).output().unwrap();
        assert!(!out.status.success(), "{out:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(says), "{err}");
    }
}

#[test]
#[cfg(target_os = "linux")]
fn a_write_client_keeps_the_run_open_after_done() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    let id = w.detach("late");
    w.result();
    let mut client = w.attach(&id);
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(w.root.join("project/.go"), "").unwrap();
    // done arrives, but the user is attached with write access.
    let (code, _) = w.wait(&id, "5s");
    assert_eq!(code, Some(124), "done ended a run with a write client");
    let _ = client.kill();
    let _ = client.wait();
    let (code, out) = w.wait(&id, "60s");
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(out.trim(), format!("{id}: ok: late but fine"));
}

#[test]
#[cfg(target_os = "linux")]
fn a_quiet_pane_becomes_idle_and_then_ends() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    w.user_config("[detach]\nidle_after = \"2s\"\nidle_timeout = \"8s\"\n");
    let id = w.detach("idle");
    w.result();
    assert!(w.wait_idle(&id) > 0);
    let (code, out) = w.wait(&id, "40s");
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.trim().starts_with(&format!(
            "{id}: idle: ended after 10 seconds without output ⏎ last screen: ⏎ "
        )),
        "{out}"
    );
    assert!(out.contains("idle-line"), "{out}");
}

#[test]
#[cfg(target_os = "linux")]
fn output_before_the_idle_timeout_clears_the_idle_state() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    w.user_config("[detach]\nidle_after = \"8s\"\nidle_timeout = \"40s\"\n");
    let id = w.detach("again");
    w.result();
    let _ = w.wait_idle(&id);
    std::fs::write(w.root.join("project/.more"), "").unwrap();
    let start = Instant::now();
    loop {
        let sb = w.sandbox(&id);
        if sb.get("idle_since").is_none() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "output did not clear idle_since: {sb}"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    let (code, out) = w.wait(&id, "3s");
    assert_eq!(code, Some(124), "{out}");
    assert!(w.has_session(&id));
}

#[test]
#[cfg(target_os = "linux")]
fn a_write_client_keeps_an_idle_run_and_hears_about_it() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    w.user_config("[detach]\nidle_after = \"2s\"\nidle_timeout = \"4s\"\n");
    let id = w.detach("idle");
    let mut client = w.attach(&id);
    w.result();
    let start = Instant::now();
    let mut told = false;
    while start.elapsed() < Duration::from_secs(45) {
        let (code, out) = w.wait(&id, "1s");
        assert_eq!(
            code,
            Some(124),
            "idle timeout ended a run with a write client: {out}"
        );
        if w.messages().contains("this run is idle") {
            told = true;
            break;
        }
    }
    let note = w.messages();
    assert!(told, "the write client was not told: {note}");
    assert!(w.has_session(&id));
    let _ = client.kill();
    let _ = client.wait();
    let (code, out) = w.wait(&id, "30s");
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.trim()
            .starts_with(&format!("{id}: idle: ended after 6 seconds without output")),
        "{out}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn idle_timeout_zero_never_ends_the_run() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::new(true);
    w.user_config("[detach]\nidle_after = \"2s\"\nidle_timeout = \"0\"\n");
    let id = w.detach("idle");
    w.result();
    let _ = w.wait_idle(&id);
    std::thread::sleep(Duration::from_secs(8));
    let sb = w.sandbox(&id);
    assert!(
        sb.get("idle_since").and_then(|v| v.as_u64()).is_some(),
        "{sb}"
    );
    let (code, out) = w.wait(&id, "2s");
    assert_eq!(code, Some(124), "{out}");
    assert!(w.has_session(&id));
}

/// The Codex hooks review screen and its safe answer, as in codex.toml.
#[cfg(target_os = "linux")]
const SCREEN_RULE: &str = r#"
[[screens.rule]]
id = "codex-hooks-review"
name = "Codex hooks review"
send = ["3"]
answer = "3. Continue without trusting (hooks won't run)"
note = "continued without trusting {n} hook"
rows = [
  "Hooks need review",
  "{n} hook is new or changed.",
  "Hooks can run outside the sandbox after you trust them.",
  "› 1. Review hooks",
  "2. Trust all and continue",
  "3. Continue without trusting (hooks won't run)",
  "enter confirm · esc skip",
]
"#;

#[test]
#[cfg(target_os = "linux")]
fn a_detached_run_answers_a_known_start_screen_and_records_it() {
    if !common::bwrap_works() || !has_tmux() {
        eprintln!("skipped: needs bwrap with a user namespace and tmux");
        return;
    }
    let w = World::with(true, SCREEN_RULE);
    let id = w.detach("screen");
    let out = w
        .paddock()
        .args(["wait", &id, "--timeout", "60s"])
        .output()
        .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(out.status.code(), Some(0), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), format!("{id}: ok: continued after 3"));
    assert!(
        stderr.contains("paddock: answered Codex hooks review: continued without trusting 1 hook"),
        "{stderr}"
    );
    let key = std::fs::read_to_string(w.root.join("project/.screen-answer")).unwrap();
    assert_eq!(key.trim(), "key 3");
    let out = w.paddock().args(["ps", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ended = v["ended"]
        .as_array()
        .and_then(|a| a.iter().find(|e| e["id"] == id.as_str()).cloned())
        .expect("the run is in the ended list");
    assert_eq!(
        ended["notes"][0],
        "answered Codex hooks review: continued without trusting 1 hook"
    );
}
