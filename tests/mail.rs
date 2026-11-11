//! The run mailbox end to end, without a sandbox: the host side is the
//! built `paddock mail`, the agent side is a fake agent that uses the same
//! device code that `/dev/paddock/mail/in` and `out` run on the host. A
//! process in a pane of paddock's tmux server stands in for the agent
//! process of a detached run. Temporary folders only.

mod common;

use paddock::devices::{Call, Device};
use paddock::env::HostEnv;
use paddock::mail::{self, device, events, Dir, Message, RunInfo, Starter};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const RUN: &str = "fake-proj-a1b2c3";

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    env: HostEnv,
}

impl World {
    fn new() -> World {
        let tmp = common::tempdir();
        let root = tmp.path().to_path_buf();
        let run = root.join("run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let mut vars = std::collections::BTreeMap::new();
        for (k, d) in [
            ("XDG_CONFIG_HOME", "cfg"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "run"),
        ] {
            vars.insert(k.to_string(), root.join(d).display().to_string());
        }
        vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let env = HostEnv::from_parts(root.join("home"), "t".into(), uid, gid, vars);
        World {
            _tmp: tmp,
            root,
            env,
        }
    }

    fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("HOME", self.root.join("home"))
            .env_remove("PADDOCK_CALLER")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("TMUX")
            .stdin(Stdio::null());
        c
    }

    /// `paddock ARGS`: exit code, stdout, stderr.
    fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> (i32, String, String) {
        let mut c = self.paddock();
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn().unwrap();
        if let Some(b) = stdin {
            child.stdin.take().unwrap().write_all(b).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// A run with a mailbox, as `paddock run` makes it. `pid` is its
    /// paddock process.
    fn mailbox(&self, pid: u32, detached: bool) {
        mail::create(&self.env, RUN, &Starter::host()).unwrap();
        mail::set_run_info(
            &self.env,
            &RunInfo {
                id: RUN.into(),
                harness: "fake".into(),
                project: self.root.join("project"),
                pid,
                pid_start: events::proc_start(pid),
                started: paddock::state::now(),
                detached,
                delivery: None,
                horse_session: None,
            },
        )
        .unwrap();
        mail::append(
            &self.env,
            RUN,
            Dir::Up,
            Message::event("started", "the sandbox started"),
        )
        .unwrap();
    }

    /// `paddock mail watch ARGS` in the background: its lines arrive on
    /// the channel as they are printed.
    fn watch(&self, args: &[&str]) -> (std::process::Child, mpsc::Receiver<String>) {
        let mut child = self
            .paddock()
            .args(["mail", "watch"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        (child, rx)
    }
}

/// The fake agent: the device code of `/dev/paddock/mail`, with the
/// sandbox id that the mount would give.
struct Agent {
    input: device::In,
    output: device::Out,
}

impl Agent {
    fn new(w: &World) -> Agent {
        let d = device::devices(&w.env, RUN);
        let names: Vec<&str> = d.iter().map(|d| d.name()).collect();
        assert_eq!(names, [device::IN, device::OUT]);
        Agent {
            input: device::In::new(&w.env),
            output: device::Out::new(&w.env),
        }
    }

    fn read(&self, timeout: u64) -> Result<Vec<Message>, i32> {
        let call = Call::for_test(RUN, Duration::from_secs(timeout + 5));
        let req = format!("{{\"timeout\":{timeout}}}");
        self.input
            .call(&call, req.as_bytes())
            .map(|b| {
                String::from_utf8(b)
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect()
            })
            .map_err(|e| e.errno())
    }

    fn send(&self, body: &str) -> Result<u64, i32> {
        let call = Call::for_test(RUN, Duration::from_secs(10));
        self.output
            .call(&call, body.as_bytes())
            .map(|b| {
                serde_json::from_slice::<serde_json::Value>(&b).unwrap()["seq"]
                    .as_u64()
                    .unwrap()
            })
            .map_err(|e| e.errno())
    }
}

/// Lines from a watch until `pred` matches one, or the time passes.
fn lines_until(
    rx: &mpsc::Receiver<String>,
    within: Duration,
    pred: impl Fn(&str) -> bool,
) -> Vec<String> {
    let end = Instant::now() + within;
    let mut got = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(l) => {
                let done = pred(&l);
                got.push(l);
                if done {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    got
}

#[test]
fn send_and_watch_round_trip_in_order_once_across_restarts() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    let agent = Agent::new(&w);
    // Host to agent: three messages, in order, each read once.
    for t in ["first", "second", "third"] {
        let (code, out, err) = w.run(&["mail", "send", RUN, t], None);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.starts_with("sent as #"), "{out}");
    }
    let (code, _, err) = w.run(
        &["mail", "send", "a1b2", "--ask", "-"],
        Some(b"which branch?\n"),
    );
    assert_eq!(
        code, 0,
        "the short id works, the text comes from stdin: {err}"
    );
    let got = agent.read(2).unwrap();
    let texts: Vec<&str> = got.iter().map(|m| m.text.trim()).collect();
    assert_eq!(texts, ["first", "second", "third", "which branch?"]);
    assert!(got.windows(2).all(|p| p[0].seq < p[1].seq));
    assert_eq!(got[3].kind, mail::Kind::Ask);
    // The agent restarts: a new reader of the device gets nothing old.
    let agent = Agent::new(&w);
    assert_eq!(agent.read(1), Err(libc::ETIME), "read once: still waiting");

    // Agent to host: plain text and JSON, with a reply.
    agent.send("hello from the agent\n").unwrap();
    let ask = got[3].seq;
    agent
        .send(&format!(
            "{{\"kind\":\"say\",\"text\":\"main\",\"reply_to\":{ask}}}"
        ))
        .unwrap();
    agent
        .send("{\"kind\":\"status\",\"text\":\"working\"}")
        .unwrap();
    let (mut watch, rx) = w.watch(&[RUN, "--timeout", "3s"]);
    let lines = lines_until(&rx, Duration::from_secs(10), |_| false);
    assert_eq!(
        watch.wait().unwrap().code(),
        Some(124),
        "the timeout ends the watch"
    );
    assert_eq!(
        lines.len(),
        5,
        "started, the read, and three messages: {lines:#?}"
    );
    assert!(
        lines[0].contains("from paddock, event started:"),
        "{}",
        lines[0]
    );
    // The starter learns that the agent read its four messages.
    assert!(
        lines[1].contains("from paddock, event delivered: #2, #3, #4, #5 read by the agent"),
        "{}",
        lines[1]
    );
    let tag = format!("from sandbox {RUN}, agent text");
    assert!(
        lines[2].contains(&format!("{tag}, say: hello from the agent")),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].contains(&format!("say (reply to #{ask}): main")),
        "{}",
        lines[3]
    );
    assert!(lines[4].contains("status: working"), "{}", lines[4]);

    // A new watch (paddock restarted) goes on where the last one stopped.
    let (code, out, _) = w.run(&["mail", "watch", RUN, "--timeout", "1s"], None);
    assert_eq!((code, out.as_str()), (124, ""), "nothing twice");
    agent.send("one more").unwrap();
    let (code, out, _) = w.run(&["mail", "watch", RUN, "--timeout", "1s"], None);
    assert_eq!(code, 124);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("say: one more"), "{out}");
    // Another reader has its own place; --all replays.
    let (_, out, _) = w.run(
        &[
            "mail",
            "watch",
            RUN,
            "--reader",
            "script",
            "--timeout",
            "1s",
        ],
        None,
    );
    assert_eq!(out.lines().count(), 6, "{out}");
    let (_, out, _) = w.run(
        &["mail", "watch", RUN, "--all", "--json", "--timeout", "1s"],
        None,
    );
    let v: serde_json::Value = serde_json::from_str(out.lines().nth(2).unwrap()).unwrap();
    assert_eq!(
        (v["run"].as_str(), v["from"].as_str()),
        (Some(RUN), Some("agent"))
    );

    // The end ends the watch, with exit 0.
    let (mut watch, rx) = w.watch(&[RUN]);
    std::thread::sleep(Duration::from_millis(500));
    events::ended(&w.env, RUN, Some(0)).unwrap();
    let lines = lines_until(&rx, Duration::from_secs(5), |l| l.contains("event ended"));
    assert_eq!(watch.wait().unwrap().code(), Some(0));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("event ended: the sandbox ended with exit code 0"),
        "{}",
        lines[0]
    );
    // Nobody reads the box of an ended run.
    let (code, _, err) = w.run(&["mail", "send", RUN, "late"], None);
    assert_ne!(code, 0);
    assert!(err.contains("the run ended"), "{err}");

    let (_, out, _) = w.run(&["mail", "ls"], None);
    assert!(out.contains("ended (0)") && out.contains(RUN), "{out}");
    let (_, out, _) = w.run(&["mail", "show", RUN, &ask.to_string()], None);
    assert!(
        out.contains("kind: ask") && out.contains("which branch?"),
        "{out}"
    );
}

#[test]
fn killing_the_agent_process_behind_a_live_pane_is_one_gone_line() {
    let w = World::new();
    let Some(tmux) = paddock::view::Tmux::find(&w.env) else {
        eprintln!("skipped: no tmux");
        return;
    };
    tmux.new_session(RUN, &["sleep".into(), "300".into()])
        .unwrap();
    let pid_out = Command::new(&tmux.exe)
        .arg("-S")
        .arg(&tmux.sock)
        .args(["list-panes", "-t", &format!("={RUN}"), "-F", "#{pane_pid}"])
        .output()
        .unwrap();
    let pid: u32 = String::from_utf8_lossy(&pid_out.stdout)
        .trim()
        .parse()
        .unwrap();
    w.mailbox(pid, true);
    let (mut watch, rx) = w.watch(&[RUN]);
    let first = lines_until(&rx, Duration::from_secs(5), |_| true);
    assert!(first[0].contains("event started"), "{first:?}");
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let killed = Instant::now();
    let lines = lines_until(&rx, Duration::from_secs(10), |l| l.contains("event gone"));
    let took = killed.elapsed();
    assert_eq!(
        watch.wait().unwrap().code(),
        Some(0),
        "the end ends the watch"
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(took < Duration::from_secs(5), "took {took:?}");
    assert!(
        lines[0].contains("the agent process is gone"),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("do not type into it"), "{}", lines[0]);
    assert!(tmux.has(RUN), "the pane stays");
    // One line only: later reads find the end and add nothing.
    assert!(events::check_gone(&w.env, RUN).is_none());
    let (code, out, _) = w.run(&["mail", "watch", RUN, "--all"], None);
    assert_eq!(code, 0);
    assert_eq!(out.matches("event gone").count(), 1, "{out}");
    let _ = Command::new(&tmux.exe)
        .arg("-S")
        .arg(&tmux.sock)
        .arg("kill-server")
        .status();
}

#[test]
fn an_approval_card_is_a_card_waiting_line() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    let p = paddock::broker::Pending {
        id: "r-1-abcd".into(),
        sandbox: RUN.into(),
        harness: "fake".into(),
        project: w.root.join("project"),
        capability: "exec".into(),
        summary: "run `make deploy` on the host".into(),
        detail: "make deploy".into(),
        reason: "the tests pass".into(),
        created: paddock::state::now(),
        expires: 0,
        session_allowed: true,
        started_by: vec![],
        choices: vec![],
        connect: None,
    };
    paddock::broker::write_pending(&w.env, &p).unwrap();
    let (code, out, _) = w.run(&["mail", "watch", RUN, "--timeout", "1s"], None);
    assert_eq!(code, 124);
    let card = out
        .lines()
        .find(|l| l.contains("event card"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        card.contains("an approval card waits for the user: exec: run `make deploy` on the host (reason: the tests pass)"),
        "{card}"
    );
}

#[test]
fn writes_to_another_box_and_long_messages_are_refused() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    let agent = Agent::new(&w);
    // The sandbox id comes from the mount: naming another run is refused.
    let e = agent.send("{\"text\":\"hi\",\"to\":\"other-proj-ffffff\"}");
    assert_eq!(e, Err(libc::EACCES));
    let e = agent.send("{\"text\":\"hi\",\"kind\":\"event\"}");
    assert_eq!(
        e,
        Err(libc::EINVAL),
        "an agent cannot write paddock's events"
    );
    // A run without a mailbox has no devices, and the host finds no box.
    assert!(device::devices(&w.env, "other-proj-ffffff").is_empty());
    let (code, _, err) = w.run(&["mail", "send", "other-proj-ffffff", "hi"], None);
    assert_ne!(code, 0);
    assert!(err.contains("no mailbox for other-proj-ffffff"), "{err}");

    // 65 KiB: refused on both sides, with the loft route.
    let big = "x".repeat(65 * 1024);
    let (code, _, err) = w.run(&["mail", "send", RUN, "-"], Some(big.as_bytes()));
    assert_ne!(code, 0);
    assert!(err.contains("more than the limit of 65536 bytes"), "{err}");
    assert!(
        err.contains("loft") && err.contains("--ref .paddock/"),
        "{err}"
    );
    assert_eq!(agent.send(&big), Err(libc::EMSGSIZE));
    let (code, _, err) = w.run(&["host", "mail", "send", "-"], Some(big.as_bytes()));
    assert_eq!(code, 10, "{err}");
    assert!(
        err.contains("loft") && err.contains("--ref .paddock/"),
        "{err}"
    );
    assert!(mail::read(&w.env, RUN, Dir::Down).is_empty());
    assert_eq!(
        mail::read(&w.env, RUN, Dir::Up).len(),
        1,
        "only the start event"
    );
}

#[test]
fn watch_delivers_each_event_without_waiting_for_another() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    let (mut child, rx) = w.watch(&[RUN, "--timeout", "10s"]);
    assert!(rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .contains("started"));
    let long = format!("done: {} tail", "x".repeat(3000));
    mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, &long),
    )
    .unwrap();
    let line = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("status must arrive on its own");
    assert!(line.ends_with(&long));
    events::ended(&w.env, RUN, Some(0)).unwrap();
    assert!(rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .contains("event ended"));
    assert!(child.wait().unwrap().success());
}

#[test]
fn wait_handles_multiple_runs_status_end_and_timeout() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    let other = "fake-proj-d4e5f6";
    mail::create(&w.env, other, &Starter::host()).unwrap();
    let long = format!("done: {} final", "x".repeat(3000));
    mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, &long),
    )
    .unwrap();
    let (code, out, _) = w.run(&["wait", "--any", "--status", "a1b2", other], None);
    assert_eq!(code, 0);
    assert!(out.contains(&long), "{out}");
    assert!(!out.contains(other));
    let (code, _, _) = w.run(
        &["wait", "--all", "--status", RUN, other, "--timeout", "1s"],
        None,
    );
    assert_eq!(code, 124);
    mail::append(
        &w.env,
        other,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, "blocked: needs input"),
    )
    .unwrap();
    let (code, out, _) = w.run(&["wait", "--status", RUN, other], None);
    assert_eq!(code, 1);
    assert!(out.contains("blocked: needs input"));
    let (code, _, _) = w.run(&["wait", RUN, "--timeout", "1s"], None);
    assert_eq!(code, 124, "status alone does not end a normal wait");
    events::ended(&w.env, RUN, Some(0)).unwrap();
    let (code, out, _) = w.run(&["wait", RUN], None);
    assert_eq!(code, 0);
    assert!(out.contains(&long));
    let (code, _, _) = w.run(&["wait", "missing"], None);
    assert_eq!(code, 2);
}

#[test]
fn wait_reports_a_stopped_mailbox_without_a_run_record() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    paddock::detach::mark_stopped(&w.env, RUN).unwrap();
    mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, "working"),
    )
    .unwrap();
    events::ended(&w.env, RUN, Some(0)).unwrap();
    let (code, out, err) = w.run(&["wait", RUN], None);
    assert_eq!(code, 1, "{out}\n{err}");
    assert_eq!(out.trim(), format!("{RUN}: stopped before a final result"));
}

#[test]
fn wait_keeps_mailbox_status_after_a_stop() {
    for (status, recorded, expected) in [
        ("done: full result", paddock::detach::Status::Failed, 0),
        ("blocked: needs input", paddock::detach::Status::Ok, 1),
    ] {
        let w = World::new();
        w.mailbox(std::process::id(), false);
        let record = paddock::detach::Record {
            id: RUN.into(),
            pid: Some(std::process::id()),
            done: Some(paddock::detach::Done {
                status: recorded,
                text: "older detached result".into(),
                at: 1,
            }),
            ..Default::default()
        };
        record.save(&w.env).unwrap();
        paddock::detach::mark_stopped(&w.env, RUN).unwrap();
        mail::append(
            &w.env,
            RUN,
            Dir::Up,
            Message::new("agent", mail::Kind::Status, status),
        )
        .unwrap();

        let (code, out, err) = w.run(&["wait", "--status", RUN], None);
        assert_eq!(code, expected, "{out}\n{err}");
        assert_eq!(out.trim(), format!("{RUN}: {status}"));
        let (code, _, _) = w.run(&["wait", RUN, "--timeout", "1s"], None);
        assert_eq!(code, 124, "normal wait still waits for the run to end");
        events::ended(&w.env, RUN, Some(1)).unwrap();
        let (code, out, err) = w.run(&["wait", RUN], None);
        assert_eq!(code, expected, "{out}\n{err}");
        assert_eq!(out.trim(), format!("{RUN}: {status}"));
    }
}

#[test]
fn wait_uses_the_exit_code_when_there_is_no_final_status_or_stop() {
    for (exit, expected, text) in [
        (Some(0), 0, "exit code 0"),
        (Some(3), 1, "exit code 3"),
        (None, 2, "no exit code"),
    ] {
        let w = World::new();
        w.mailbox(std::process::id(), false);
        events::ended(&w.env, RUN, exit).unwrap();
        let (code, out, err) = w.run(&["wait", RUN], None);
        assert_eq!(code, expected, "{out}\n{err}");
        assert!(out.contains(text), "{out}");
    }
}

#[test]
fn run_end_hook_receives_id_and_full_status_once() {
    let w = World::new();
    w.mailbox(std::process::id(), false);
    let config = w.root.join("cfg/paddock");
    std::fs::create_dir_all(&config).unwrap();
    let output = w.root.join("hook-output");
    let script = w.root.join("hook.sh");
    std::fs::write(
        &script,
        format!(
            "printf '%s\\n%s\\n' \"$PADDOCK_RUN_ID\" \"$PADDOCK_RUN_STATUS\" >> '{}'\n",
            output.display()
        ),
    )
    .unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!("[hooks]\nrun_end = ['sh', '{}']\n", script.display()),
    )
    .unwrap();
    let status = format!("done: {}", "x".repeat(3000));
    mail::append(
        &w.env,
        RUN,
        Dir::Up,
        Message::new("agent", mail::Kind::Status, &status),
    )
    .unwrap();
    events::ended(&w.env, RUN, Some(0)).unwrap();
    assert!(events::ended(&w.env, RUN, Some(0)).is_none());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !output.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = std::fs::read_to_string(output).unwrap();
    assert_eq!(text, format!("{RUN}\n{status}\n"));
}
