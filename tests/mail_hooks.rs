//! Mail reaches a Claude Code agent that never reads its mailbox. paddock
//! builds the command of a claude run; a fake Claude Code takes the hooks
//! from its `--settings` and runs them as Claude Code does: each command
//! with `sh -c`, the hook input as JSON on stdin, the answer read from
//! stdout. A Stop answer `{"decision":"block","reason":...}` keeps the turn
//! going with the reason; the next stop then has `stop_hook_active: true`.
//!
//! The broker of the run is in this process, on a socket; the starter is
//! the built `paddock mail`. Temporary folders only, no sandbox, no vendor.

mod common;

use paddock::env::HostEnv;
use paddock::mail::{self, Dir, RunInfo, Starter};
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

struct World {
    _tmp: common::TestDir,
    root: PathBuf,
    env: HostEnv,
    id: String,
    /// The `--settings` of the built claude command.
    settings: Value,
    sock: PathBuf,
    bin: PathBuf,
}

fn world() -> World {
    let tmp = common::tempdir();
    let root = tmp.path().to_path_buf();
    let project = root.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(root.join("home")).unwrap();
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
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let env = HostEnv::from_parts(root.join("home"), "t".into(), uid, gid, vars);
    std::fs::create_dir_all(env.paddock_config()).unwrap();

    let h = paddock::profile::Harness {
        profile: paddock::profile::builtin("claude").unwrap(),
        exe: "/usr/bin/true".into(),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = paddock::build::Options {
        harness: "claude".into(),
        project: Some(project.clone()),
        assume_yes: true,
        ..Default::default()
    };
    let b = paddock::build::build_with(&env, &opts, h).unwrap();
    let cmd = &b.spec.command;
    let at = cmd
        .iter()
        .position(|a| a == "--settings")
        .expect("--settings");
    let settings: Value = serde_json::from_str(&cmd[at + 1]).unwrap();
    let id = b.spec.id.clone();

    // The mailbox as the start of the run makes it.
    mail::create(&env, &id, &Starter::host()).unwrap();
    mail::set_run_info(
        &env,
        &RunInfo {
            id: id.clone(),
            harness: "claude".into(),
            project: project.clone(),
            pid: std::process::id(),
            pid_start: mail::events::proc_start(std::process::id()),
            started: paddock::state::now(),
            detached: true,
            delivery: mail::deliver::of_command(cmd).map(str::to_string),
            horse_session: None,
        },
    )
    .unwrap();

    let ctx = paddock::broker::Context {
        id: id.clone(),
        harness: "claude".into(),
        project,
        repo: None,
        chain: vec![],
        depth: 0,
        role: None,
    };
    let broker = paddock::broker::Broker::new(
        env.clone(),
        ctx,
        paddock::broker::HostPolicy::default(),
        Arc::new(paddock::broker::DenyAll),
    );
    let sock = paddock::sockpath::host_socket(&root.join("run"), uid, "", "host.sock").unwrap();
    paddock::broker::serve(&sock, Arc::new(broker)).unwrap();

    // `paddock-host` as the sandbox has it.
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("paddock-host");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nexec '{}' host \"$@\"\n",
            env!("CARGO_BIN_EXE_paddock")
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    World {
        _tmp: tmp,
        root,
        env,
        id,
        settings,
        sock,
        bin,
    }
}

/// The fake Claude Code.
struct Claude<'a> {
    w: &'a World,
    /// The host socket that its hooks reach.
    sock: PathBuf,
    /// What the agent got from hooks: stop reasons and added context.
    got: Vec<String>,
    /// How many hook commands ran, per event.
    ran: std::collections::BTreeMap<String, usize>,
}

impl<'a> Claude<'a> {
    fn new(w: &'a World) -> Self {
        Claude {
            w,
            sock: w.sock.clone(),
            got: Vec::new(),
            ran: Default::default(),
        }
    }

    /// Run every hook of `event` with `input`; the parsed answers.
    fn hooks(&mut self, event: &str, input: Value) -> Vec<Value> {
        let mut out = Vec::new();
        let groups = self.w.settings["hooks"][event].as_array().cloned();
        for g in groups.unwrap_or_default() {
            for h in g["hooks"].as_array().cloned().unwrap_or_default() {
                let cmd = h["command"].as_str().unwrap().to_string();
                *self.ran.entry(event.into()).or_default() += 1;
                if cmd.starts_with("paddock-host status") {
                    // Not what this test is about.
                    continue;
                }
                let path = format!(
                    "{}:{}",
                    self.w.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                );
                let mut child = Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .env_clear()
                    .env("PATH", path)
                    .env("HOME", self.w.root.join("home"))
                    .env("PADDOCK_HOST_SOCKET", &self.sock)
                    // No gateway device: in a paddock sandbox, /dev/paddock/gw
                    // would take the requests to the real host.
                    .env("PADDOCK_DEVICES", self.w.root.join("no-devices"))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                let mut body = input.clone();
                body["hook_event_name"] = event.into();
                body["session_id"] = "s1".into();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(body.to_string().as_bytes())
                    .unwrap();
                let o = child.wait_with_output().unwrap();
                // Exit 0 always: a failing hook never stops the agent.
                assert_eq!(o.status.code(), Some(0), "{cmd}: {o:?}");
                let text = String::from_utf8_lossy(&o.stdout);
                if !text.trim().is_empty() {
                    out.push(serde_json::from_str(text.trim()).expect("hook JSON"));
                }
            }
        }
        out
    }

    /// One tool call: the PostToolUse hooks.
    fn tool(&mut self) {
        let input = json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});
        for a in self.hooks("PostToolUse", input) {
            if let Some(c) = a["hookSpecificOutput"]["additionalContext"].as_str() {
                self.got.push(c.to_string());
            }
        }
    }

    /// The agent ends its turn. Claude Code runs the Stop hooks; a block
    /// gives the reason to the agent, which works on and ends again. The
    /// number of turns it took.
    fn end_turn(&mut self) -> usize {
        let mut active = false;
        let mut turns = 1;
        loop {
            let answers = self.hooks("Stop", json!({"stop_hook_active": active}));
            let block: Vec<String> = answers
                .iter()
                .filter(|a| a["decision"] == "block")
                .map(|a| a["reason"].as_str().unwrap_or("").to_string())
                .collect();
            if block.is_empty() {
                return turns;
            }
            self.got.extend(block);
            active = true;
            turns += 1;
            assert!(turns < 8, "the stop hook holds the turn open");
        }
    }
}

fn paddock(w: &World, args: &[&str]) -> (i32, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(args)
        .env("XDG_CONFIG_HOME", w.root.join("cfg"))
        .env("XDG_DATA_HOME", w.root.join("data"))
        .env("XDG_STATE_HOME", w.root.join("state"))
        .env("XDG_CACHE_HOME", w.root.join("cache"))
        .env("XDG_RUNTIME_DIR", w.root.join("run"))
        .env("HOME", w.root.join("home"))
        .env_remove("PADDOCK_CALLER")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr),
    )
}

fn delivered_events(w: &World) -> Vec<mail::Message> {
    mail::read(&w.env, &w.id, Dir::Up)
        .into_iter()
        .filter(|m| m.event.as_deref() == Some("delivered"))
        .collect()
}

#[test]
fn an_agent_that_never_reads_mail_gets_it_at_the_end_of_its_turn_once() {
    let w = world();
    let mut claude = Claude::new(&w);
    // A turn without mail ends at once.
    claude.tool();
    assert_eq!(claude.end_turn(), 1);
    assert!(claude.got.is_empty(), "{:?}", claude.got);

    let (code, out) = paddock(&w, &["mail", "send", &w.id, "please also update the docs"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("sent as #") && out.contains("at the end of its turn"),
        "{out}"
    );
    let seq: u64 = out["sent as #".len()..]
        .split(';')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // A tool call does not take a plain message.
    claude.tool();
    assert!(claude.got.is_empty(), "{:?}", claude.got);
    // The stop is blocked once, with the message as the reason; the agent
    // works on and its next stop happens.
    assert_eq!(claude.end_turn(), 2);
    assert_eq!(claude.got.len(), 1, "{:?}", claude.got);
    let reason = &claude.got[0];
    assert!(
        reason.contains(&format!(
            "--- mail #{seq} say from your starter on the host\nplease also update the docs\n"
        )),
        "{reason}"
    );
    assert!(
        reason.contains("paddock-host mail send --reply-to"),
        "{reason}"
    );
    // A second turn end without new mail stops at once: given once.
    assert_eq!(claude.end_turn(), 1);
    assert_eq!(claude.got.len(), 1);

    let ev = delivered_events(&w);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0].seqs, [seq]);
    assert_eq!(ev[0].via.as_deref(), Some("stop"));
    assert!(mail::unread_down(&w.env, &w.id).is_empty());

    // The starter sees how it came.
    let (_, ls) = paddock(&w, &["mail", "ls", &w.id]);
    let line = ls
        .lines()
        .find(|l| l.contains("please also update"))
        .unwrap_or_else(|| panic!("{ls}"));
    assert!(
        line.ends_with("(given to the agent at the end of its turn (stop hook))"),
        "{line}"
    );
    let (_, js) = paddock(&w, &["mail", "ls", &w.id, "--json"]);
    let v: Value = serde_json::from_str(&js).unwrap();
    let m = v
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["seq"] == seq)
        .unwrap();
    assert_eq!(
        (m["state"].as_str(), m["delivered_via"].as_str()),
        (Some("delivered"), Some("stop"))
    );
    let (code, watch) = paddock(&w, &["mail", "watch", &w.id, "--timeout", "1s"]);
    assert_eq!(code, 124, "{watch}");
    assert!(
        watch.contains(&format!(
            "from paddock, event delivered: #{seq} given to the agent at the end of its turn (stop hook)"
        )),
        "{watch}"
    );
}

#[test]
fn a_blocked_stop_and_a_broken_host_let_the_stop_happen() {
    let w = world();
    let (code, out) = paddock(&w, &["mail", "send", &w.id, "late news"]);
    assert_eq!(code, 0, "{out}");
    let mut claude = Claude::new(&w);
    // Claude Code's loop guard: this stop follows a blocked one.
    let answers = claude.hooks("Stop", json!({"stop_hook_active": true}));
    assert!(answers.is_empty(), "{answers:?}");
    // The host does not answer.
    claude.sock = w.root.join("run/nobody.sock");
    assert_eq!(claude.end_turn(), 1);
    claude.tool();
    assert!(claude.got.is_empty());
    // The message waits, and nothing claims that it came.
    assert_eq!(mail::unread_down(&w.env, &w.id).len(), 1);
    assert!(delivered_events(&w).is_empty());
    let (_, ls) = paddock(&w, &["mail", "ls", &w.id]);
    assert!(
        ls.lines()
            .any(|l| l.contains("late news") && l.ends_with("(unread)")),
        "{ls}"
    );
    // A run without a mailbox: the stop happens too.
    claude.sock = w.sock.clone();
    std::fs::remove_file(mail::dir(&w.env, &w.id).unwrap().join("starter.json")).unwrap();
    assert_eq!(claude.end_turn(), 1);
}

#[test]
fn an_interrupt_comes_after_the_next_tool_call_and_a_read_counts_as_delivery() {
    let w = world();
    let (_, out) = paddock(&w, &["mail", "send", &w.id, "first, plain"]);
    assert!(out.contains("at the end of its turn"), "{out}");
    let (_, out) = paddock(
        &w,
        &["mail", "send", "--interrupt", &w.id, "stop the deploy"],
    );
    assert!(out.contains("after its next tool call"), "{out}");
    let mut claude = Claude::new(&w);
    claude.tool();
    assert_eq!(claude.got.len(), 1, "{:?}", claude.got);
    let ctx = &claude.got[0];
    assert!(ctx.contains("an urgent message came"), "{ctx}");
    assert!(
        ctx.contains("first, plain") && ctx.contains("(interrupt)"),
        "{ctx}"
    );
    // Nothing is left for the stop.
    assert_eq!(claude.end_turn(), 1);
    assert_eq!(claude.got.len(), 1);

    // The agent reads a message itself: that is a delivery too, and the
    // hooks never give it again.
    paddock(&w, &["mail", "send", &w.id, "read me yourself"]);
    let read = Command::new(w.bin.join("paddock-host"))
        .args(["mail", "wait", "--timeout", "5"])
        .env("PADDOCK_HOST_SOCKET", &w.sock)
        .env("PADDOCK_DEVICES", w.root.join("no-devices"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&read.stdout).contains("read me yourself"),
        "{read:?}"
    );
    assert_eq!(claude.end_turn(), 1);
    let vias: Vec<_> = delivered_events(&w)
        .iter()
        .map(|e| (e.seqs.clone(), e.via.clone().unwrap()))
        .collect();
    assert_eq!(
        vias,
        [
            (vec![1, 2], "tool".to_string()),
            (vec![4], "read".to_string())
        ],
        "{vias:?}"
    );
    let (_, ls) = paddock(&w, &["mail", "ls", &w.id]);
    for (text, how) in [
        ("first, plain", "after a tool call (interrupt)"),
        ("read me yourself", "(read by the agent)"),
    ] {
        assert!(
            ls.lines().any(|l| l.contains(text) && l.contains(how)),
            "{text}: {ls}"
        );
    }
}
