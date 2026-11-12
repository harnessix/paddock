//! The walk in real sandboxes: `paddock run dsh --horse` with bwrap and
//! the fake dsh of the walk, which spawns a plain and a hitched fake
//! Claude Code child, then `paddock run claude` with the fake TUI, then
//! `paddock run dsh --horse-id` to resume. Skips without bwrap or FUSE.

use super::*;
use paddock::horse::hitch::EdgeKind;
use paddock::horse::{claude, dna, run};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Install the fake `fake` as the harness `name` of the configuration
/// folder `cfg` of `t`, with `profile` after the name.
fn install(t: &Path, cfg: &str, name: &str, fake: &str, profile: &str) {
    use std::os::unix::fs::PermissionsExt;
    // The install root of a bin executable is its grandparent: keep it out
    // of the test home, so the sandbox can mount it.
    let dir = t.join(format!("harness-{cfg}/bin"));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join(name);
    std::fs::copy(fixture(fake), &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = t.join(cfg).join("paddock/harness.d");
    std::fs::create_dir_all(&d).unwrap();
    let path = serde_json::to_string(exe.to_str().unwrap()).unwrap();
    let text = format!("name = \"{name}\"\n[locate]\ncommands = [{path}]\n{profile}");
    std::fs::write(d.join(format!("{name}.toml")), text).unwrap();
}

/// The test folder: the fake dsh of the walk and the fake claude child in
/// `cfg`, the fake claude TUI in `cfg-tui`. Both share the store.
fn setup() -> common::TestDir {
    let tmp = test_folder();
    let t = tmp.path();
    install(t, "cfg", "dsh", "dsh-e2e", "[launch]\nargs = [\"web\"]\n");
    install(
        t,
        "cfg",
        "claude",
        "claude",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"-p\", \"--dangerously-skip-permissions\"]\nprompt = \"stdin\"\n",
    );
    install(
        t,
        "cfg-tui",
        "claude",
        "claude-tui",
        "[launch]\nargs = []\n",
    );
    let config = "[host]\nspawn = \"allow\"\n[spawn]\nharnesses = [\"claude\"]\nmax_running = 2\ntimeout = 120\n[horse]\nenabled = true\n";
    std::fs::write(t.join("cfg/paddock/config.toml"), config).unwrap();
    std::fs::write(
        t.join("cfg-tui/paddock/config.toml"),
        "[horse]\nenabled = true\n",
    )
    .unwrap();
    let project = t.join("project");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&project)
            .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(project.join("task.md"), "write the note\n").unwrap();
    git(&["add", "task.md"]);
    git(&["commit", "-q", "-m", "one"]);
    tmp
}

/// `paddock run ARGS` with bwrap in the test folder, with the
/// configuration folder `cfg`, in a thread. It stops after five minutes.
fn run_in_thread(t: &Path, cfg: &str, args: &[&str]) -> std::thread::JoinHandle<Output> {
    let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
    c.arg("run")
        .args(args)
        .args([
            "--backend",
            "bwrap",
            "--network",
            "none",
            "--env",
            "clean",
            "-y",
        ])
        .arg(t.join("project"))
        .envs(xdg(t))
        .env("XDG_CONFIG_HOME", t.join(cfg))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .current_dir(t.join("project"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    std::thread::spawn(move || {
        let mut child = c.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(300);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        child.wait_with_output().unwrap()
    })
}

/// The Claude Code child sessions in the store: the hitched child by its
/// SpawnedFrom edge, which the parent's paddock writes before the child
/// starts, and the plain child by its session in a child horse. Neither
/// waits for a bound native id.
fn children(store: &Store) -> Vec<String> {
    let mut out = Vec::new();
    for h in store.horses().unwrap_or_default() {
        for e in hitch::edges(store, &h.id).unwrap_or_default() {
            if e.kind == EdgeKind::SpawnedFrom && !out.contains(&e.from) {
                out.push(e.from);
            }
        }
    }
    let parents: Vec<String> = store
        .sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.binding.harness == "dsh")
        .map(|s| s.horse)
        .collect();
    for s in store.sessions().unwrap_or_default() {
        let plain = s.binding.harness == "claude" && !parents.contains(&s.horse);
        if plain && !out.contains(&s.id) {
            out.push(s.id);
        }
    }
    out
}

/// Append "from the test" once to every Claude Code child session: the
/// fake child holds its first turn until it sees that entry, as a host
/// would send it.
fn feed_children(t: PathBuf, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut fed = Vec::<String>::new();
        while !stop.load(Ordering::SeqCst) {
            let found = Store::open_host(&host::host_env(&t))
                .map(|st| children(&st))
                .unwrap_or_default();
            for s in found {
                if !fed.contains(&s) {
                    horse(&t, &["append", &s, "--text", "from the test"]);
                    fed.push(s);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        fed
    })
}

/// The result.json of the spawned child whose id is in `file`.
fn child_result(t: &Path, file: &str) -> serde_json::Value {
    let id = project_file(t, file);
    let p = t
        .join("project/.paddock/inbox")
        .join(format!("spawn-{}", id.trim()))
        .join("result.json");
    std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// The dsh sessions folder of the sandbox home, as the host sees it.
fn dsh_sessions(t: &Path) -> PathBuf {
    fn find(dir: &Path) -> Option<PathBuf> {
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            if !e.file_type().ok()?.is_dir() {
                continue;
            }
            if p.ends_with(".dsh/sessions") {
                return Some(p);
            }
            if let Some(found) = find(&p) {
                return Some(found);
            }
        }
        None
    }
    find(&t.join("data/paddock")).expect("the dsh home of the sandbox")
}

/// End the dsh run of the walk: its pending `shutdown` is answered after
/// the next prompt once the file stop exists.
fn stop_dsh(t: &Path, session: &str, run: std::thread::JoinHandle<Output>) -> Output {
    std::fs::write(t.join("project/stop"), "").unwrap();
    horse(t, &["append", session, "--text", "stop now"]);
    let out = run.join().unwrap();
    assert!(out.status.success(), "{out:?}\n{}", report(t));
    std::fs::remove_file(t.join("project/stop")).unwrap();
    out
}

#[test]
fn the_whole_walk_in_sandboxes() {
    if !common::sandbox_works() {
        eprintln!("skipped: needs bwrap and FUSE");
        return;
    }
    let tmp = setup();
    let t = tmp.path();
    let rep = || report(t);
    let stop = Arc::new(AtomicBool::new(false));
    let feeder = feed_children(t.to_path_buf(), stop.clone());

    // 1. A horse per run, with a DNA, one dsh session and `run.begin`.
    let parent_run = run_in_thread(t, "cfg", &["dsh", "--horse"]);
    wait_until("the dsh session", &rep, || {
        !project_file(t, "sessions.txt").is_empty()
    });
    let store = Store::open_host(&host::host_env(t)).unwrap();
    let parent = project_file(t, "sessions.txt").trim().to_string();
    let ph = store.session(&parent).unwrap().horse;
    let native = store.session(&parent).unwrap().binding.native_id.unwrap();
    wait_until("the DNA entry", &rep, || {
        project_file(t, "trace.txt").contains("DNA of this horse (start)")
    });
    at_safe_point(t, "DNA of this horse (start)");
    let types: Vec<String> = run::events(&store, &ph)
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(types[..2], ["dna.default", "run.begin"], "{}", rep());

    // 2 and 3. The plain child gets a child horse of the run's horse. The
    // hitched child is a session of it, and its result lands once in the
    // parent's session as the newest entry, at a safe point.
    wait_until("both children ended", &rep, || {
        ["plain.txt", "hitched.txt"]
            .iter()
            .all(|f| child_result(t, f)["exit_code"].is_number())
    });
    for f in ["plain.txt", "hitched.txt"] {
        assert_eq!(child_result(t, f)["exit_code"], 0, "{f}\n{}", rep());
    }
    let edges = hitch::edges(&store, &ph).unwrap();
    let hitched: Vec<&String> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::SpawnedFrom && e.to == parent)
        .map(|e| &e.from)
        .collect();
    assert_eq!(hitched.len(), 1, "{}", rep());
    let link = hitched[0].clone();
    let head = format!("hitched {} (claude) finished", &link[..8]);
    wait_until("the hitched result landed", &rep, || {
        delivered(&store, &parent, &head)
            .iter()
            .any(|d| d.landed_seq.is_some())
    });
    let d = delivered(&store, &parent, &head);
    assert_eq!(d.len(), 1, "{}", rep());
    let r = rows(&store, &parent);
    assert_eq!(d[0].landed_seq, Some(r.len() as u64 - 1), "{}", rep());
    assert_eq!(
        r.iter().filter(|r| r.contains(&head)).count(),
        1,
        "{}",
        rep()
    );
    at_safe_point(t, "hitched");
    let plain = store
        .horses()
        .unwrap()
        .into_iter()
        .find(|h| {
            h.id != ph
                && h.sessions
                    .iter()
                    .any(|s| store.session(s).unwrap().binding.harness == "claude")
        })
        .expect("the horse of the plain child");
    let child_dna = dna::get_or_default(&store, &plain.dna);
    assert_eq!(
        child_dna.origin.parent.as_deref(),
        Some(ph.as_str()),
        "{}",
        rep()
    );
    assert!(child_dna.origin.parent_dna.is_some());
    stop.store(true, Ordering::SeqCst);
    assert_eq!(feeder.join().unwrap().len(), 2, "{}", rep());

    // 4. Custody: an entry appended while a turn runs lands after it.
    horse(t, &["append", &parent, "--text", "a long turn"]);
    wait_until("the long turn", &rep, || {
        t.join("project/turn-running").exists()
    });
    horse(t, &["append", &parent, "--text", "custody entry"]);
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        delivered(&store, &parent, "custody entry").is_empty(),
        "{}",
        rep()
    );
    std::fs::write(t.join("project/turn-go"), "").unwrap();
    wait_until("the custody entry landed", &rep, || {
        delivered(&store, &parent, "custody entry")
            .iter()
            .any(|d| d.landed_seq.is_some())
    });
    assert!(
        trace_at(t, "turn-end") < trace_at(t, "custody entry"),
        "{}",
        rep()
    );
    at_safe_point(t, "custody entry");

    // 5. The TUI channel: an interactive Claude Code run in horse mode.
    tui_in_sandbox(t, &store);

    // 6. DNA graft and undo, while the run holds its session.
    graft_and_undo(t, &store, &ph);
    wait_until("the graft and the revert reached dsh", &rep, || {
        let p = project_file(t, "trace.txt");
        p.contains("(graft from") && p.contains("(revert)")
    });
    assert!(
        project_file(t, "prompts.txt").contains("walk-graft"),
        "{}",
        rep()
    );
    at_safe_point(t, "(graft from");
    at_safe_point(t, "(revert)");

    // 7. Stop and resume.
    stop_dsh(t, &parent, parent_run);
    let end = run::events(&store, &ph)
        .unwrap()
        .into_iter()
        .rfind(|e| e["type"] == "run.end" && e["session"] == parent.as_str())
        .expect("the run.end of the parent");
    assert_eq!(end["custody"], "live", "{end}");
    assert!(end.get("error").is_none(), "{end}");
    let cwd = project_file(t, "cwd.txt").trim().to_string();
    write_session_file(&store, &parent, &dsh_sessions(t), &native, &cwd);
    let resumed = run_in_thread(t, "cfg", &["dsh", "--horse-id", &ph]);
    wait_until("the resume", &rep, || {
        project_file(t, "trace.txt").contains(&format!("resume {native}"))
    });
    wait_until("the stop entry landed", &rep, || {
        delivered(&store, &parent, "stop now")
            .iter()
            .any(|d| d.landed_seq.is_some())
    });
    let seq = rows(&store, &parent).len() as u64;
    horse(t, &["append", &parent, "--text", "after the resume"]);
    wait_until("the entry after the resume landed", &rep, || {
        delivered(&store, &parent, "after the resume")
            .iter()
            .any(|d| d.landed_seq == Some(seq))
    });
    stop_dsh(t, &parent, resumed);
    let begins = run::events(&store, &ph)
        .unwrap()
        .into_iter()
        .filter(|e| e["type"] == "run.begin" && e["session"] == parent.as_str())
        .count();
    assert_eq!(begins, 2, "{}", rep());
    assert_eq!(
        store.horse(&ph).unwrap().sessions,
        [parent.clone(), link],
        "{}",
        rep()
    );
}

/// An interactive Claude Code run of the fake TUI in horse mode, with the
/// store of the walk: with a channel path its entry lands between turns,
/// without one it falls back with one line and the entry stays queued.
fn tui_in_sandbox(t: &Path, store: &Store) {
    let channel = claude::channel_pin(claude::VERSION).is_some();
    let had: Vec<String> = store
        .sessions()
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    let tui = run_in_thread(t, "cfg-tui", &["claude"]);
    let session = || {
        store
            .sessions()
            .ok()?
            .into_iter()
            .find(|s| s.binding.harness == "claude" && !had.contains(&s.id))
    };
    let rep = || report(t);
    wait_until("the TUI session", &rep, || session().is_some());
    let sid = session().unwrap().id;
    horse(t, &["append", &sid, "--text", "from the tui walk"]);
    if channel {
        wait_until("the TUI entry landed", &rep, || {
            delivered(store, &sid, "from the tui walk")
                .iter()
                .any(|d| d.landed_seq.is_some())
        });
    }
    std::fs::write(t.join("project/done"), "").unwrap();
    let out = tui.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let end = run::events(store, &store.session(&sid).unwrap().horse)
        .unwrap()
        .into_iter()
        .find(|e| e["type"] == "run.end")
        .expect("the run.end of the TUI");
    if channel {
        assert_eq!(end["custody"], "channel", "{end}");
        assert!(store.inbox(&sid).unwrap().is_empty());
    } else {
        assert_eq!(end["custody"], "final-only", "{end}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("horse: no safe-point delivery for claude"),
            "{stderr}"
        );
        let inbox = store.inbox(&sid).unwrap();
        assert!(
            inbox.iter().any(|e| e.text == "from the tui walk"),
            "{inbox:?}"
        );
    }
}

/// A real trainer uses its own fake harness and the target's scoped peek
/// grant, while the worker and its project remain separate.
#[test]
fn trainer_reads_through_peek_and_cannot_write_the_target() {
    if !common::sandbox_works() {
        eprintln!("skipped: trainer needs bwrap and FUSE");
        return;
    }
    let tmp = setup();
    let t = tmp.path();
    let fake_dsh = t.join("harness-cfg/bin/dsh");
    let script = std::fs::read_to_string(&fake_dsh).unwrap();
    let script = script.replace(
        "        paddock-host spawn claude --prompt-file task.md --reason plain > plain.txt\n        paddock-host spawn claude --hitch --prompt-file task.md --reason hitched > hitched.txt",
        "        : # this worker does not spawn children",
    );
    std::fs::write(&fake_dsh, script).unwrap();
    install(
        t,
        "cfg",
        "claude",
        "claude-trainer",
        "[launch]\nargs = []\n[oneshot]\nargs = [\"-p\", \"--dangerously-skip-permissions\"]\nprompt = \"stdin\"\n",
    );
    std::fs::write(t.join("project/canary"), "original").unwrap();
    let env = host::host_env(t);
    let parent_run = run_in_thread(t, "cfg", &["dsh", "--horse"]);
    let rep = || report(t);
    wait_until("the fake worker's horse session", &rep, || {
        !project_file(t, "sessions.txt").is_empty()
    });
    let session = project_file(t, "sessions.txt").trim().to_owned();
    wait_until("the target's native message", &rep, || {
        Store::open_host(&env)
            .ok()
            .and_then(|store| store.session(&session).ok())
            .is_some_and(|s| s.head.is_some())
    });
    let run = paddock::mail::list(&env)
        .into_iter()
        .find(|id| paddock::mail::run_info(&env, id).is_some_and(|info| info.harness == "dsh"))
        .expect("worker mailbox");
    let mut command = Command::new(env!("CARGO_BIN_EXE_paddock"));
    command
        .args(["train", &run, "--harness", "claude"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .envs(xdg(t))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let messages = paddock::mail::read(&env, &run, paddock::mail::Dir::Down);
    let logs = std::fs::read_dir(env.paddock_run().join(&run).join("spawn"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("log")).ok())
        .collect::<Vec<_>>()
        .join("\n");
    let canary = project_file(t, "canary");
    let _ = stop_dsh(t, &session, parent_run);
    assert!(
        output.status.success(),
        "trainer failed: {}\ntrainer log: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        logs,
        rep()
    );
    assert_eq!(canary, "original");
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.from == "trainer" && m.interrupt)
            .count(),
        1
    );
}
