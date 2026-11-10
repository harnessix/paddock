//! The walk without a sandbox: the run lifecycle of the library starts
//! each run, the test starts the fake harness as a child process and hands
//! its pipes to the armed custody, as `paddock run` does with the sandbox.
//! The host CLI appends, grafts and reverts, as the user does.

use super::*;
use paddock::build::{build_with, Built, Options};
use paddock::env::HostEnv;
use paddock::horse::hitch::{wire, EdgeKind};
use paddock::horse::run::{by_sandbox, RunHorse};
use paddock::horse::{dna, lifecycle, live, run};
use paddock::profile::{Harness, Profile};
use std::collections::BTreeMap;
use std::os::fd::{FromRawFd as _, OwnedFd};
use std::process::{Child, Stdio};

/// The host environment of the test folder `t`: the same XDG folders as
/// the host CLI.
pub(super) fn host_env(t: &Path) -> HostEnv {
    let mut vars: BTreeMap<String, String> = xdg(t)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.display().to_string()))
        .collect();
    vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    // SAFETY: getuid and getgid cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    HostEnv::from_parts(t.join("home"), "demo".into(), uid, gid, vars)
}

/// A run of `harness` with the fake `fake` in horse mode, as `paddock run`
/// builds it. `id`: the sandbox id.
fn built(env: &HostEnv, t: &Path, harness: &str, fake: &str, id: &str, args: &[&str]) -> Built {
    let profile: Profile = toml::from_str(&format!(
        "name = \"{harness}\"\n[launch]\nargs = [\"web\"]\n"
    ))
    .unwrap();
    let h = Harness {
        profile,
        exe: fixture(fake),
        install_mounts: vec![],
        path_dirs: vec![],
        source: "test".into(),
    };
    let opts = Options {
        harness: harness.into(),
        project: Some(t.join("project")),
        network: Some(paddock::spec::Network::None),
        inherit_env: Some(false),
        assume_yes: true,
        ..Default::default()
    };
    let mut b = build_with(env, &opts, h).unwrap();
    b.horse = true;
    b.spec.id = id.into();
    b.spec.command = std::iter::once(fixture(fake).display().to_string())
        .chain(args.iter().map(|a| a.to_string()))
        .collect();
    b
}

/// A spawned child of the sandbox `parent` whose task comes on stdin.
fn child_of(b: &mut Built, parent: &str, hitched: Option<wire::Hitched>, task: &str) {
    b.stdin = Some(task.as_bytes().to_vec());
    b.spawn = Some(paddock::spawn::ChildInfo {
        spawn_id: format!("spawn-{}", b.spec.id),
        parent: parent.into(),
        chain: vec![parent.into()],
        depth: 1,
        modes: BTreeMap::new(),
        role: Default::default(),
        hitch: hitched,
    });
}

/// Start the command of `b` as the sandbox would, and hand its pipes to
/// the custody that `lifecycle::start` armed.
fn start_armed(t: &Path, b: &Built, env: &[(&str, &str)]) -> Child {
    let hook = live::take_hook().expect("an armed custody");
    // The FUSE device of the sandbox: there is none on the host.
    let _ = live::take_device_setup();
    let home = &b.spec.home_src;
    std::fs::create_dir_all(home).unwrap();
    let err = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(t.join("project/dsh-stderr.txt"))
        .unwrap();
    let cmd = &b.spec.command;
    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("PWD", &b.spec.workdir)
        .envs(env.iter().copied())
        .current_dir(&b.spec.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(err)
        .spawn()
        .unwrap();
    hook(child.stdin.take().unwrap(), child.stdout.take().unwrap());
    child
}

/// The custody of a dsh run reads what the user types from standard
/// input. Give this process a new standard input whose end the test
/// holds: dropping it is the end of the user's input, as Ctrl-D is.
fn user_input() -> OwnedFd {
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills two new descriptors; dup2 replaces fd 0 with the
    // read end. The write end is close-on-exec, so no child holds it.
    unsafe {
        assert_eq!(libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC), 0);
        assert_eq!(libc::dup2(fds[0], 0), 0);
        libc::close(fds[0]);
        OwnedFd::from_raw_fd(fds[1])
    }
}

/// A headless Claude Code child of the parent run: start, hold, end.
/// Returns its run and its output.
fn run_child(t: &Path, env: &HostEnv, mut b: Built) -> (RunHorse, String) {
    let l = lifecycle::start(env, &mut b).unwrap().unwrap();
    let store = Store::open_host(env).unwrap();
    let rh = by_sandbox::find(&store, &b.spec.id).unwrap().unwrap();
    // The fake holds its first turn until this file exists.
    std::fs::create_dir_all(&b.spec.home_src).unwrap();
    std::fs::write(b.spec.home_src.join("go"), "").unwrap();
    let mut child = start_armed(t, &b, &[]);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "the fake claude failed: {status}\n{}",
        report(t)
    );
    let out = lifecycle::end(Some(l), 0, None).expect("the custody output");
    (rh, out)
}

/// The variables of a child run in its own process.
const CHILD_RUN: &str = "PADDOCK_E2E_CHILD_RUN";

/// Each run holds its custody in its own paddock process, as a spawned
/// child does: run the child `id` of `walk-parent` in a new process of
/// this test binary. `hitched`: its place in the parent's horse.
fn child_process(
    t: &Path,
    id: &str,
    hitched: Option<&wire::Hitched>,
    task: &str,
) -> (RunHorse, String) {
    let job = serde_json::json!({"folder": t, "id": id, "hitched": hitched, "task": task});
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "host::a_child_run_in_its_own_process",
            "--nocapture",
        ])
        .env(CHILD_RUN, job.to_string())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the child run {id}: {}{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        report(t)
    );
    let done: serde_json::Value =
        serde_json::from_slice(&std::fs::read(t.join(format!("{id}.json"))).unwrap()).unwrap();
    let rh = RunHorse {
        horse: done["horse"].as_str().unwrap().into(),
        session: done["session"].as_str().unwrap().into(),
        harness: done["harness"].as_str().unwrap().into(),
    };
    (rh, done["output"].as_str().unwrap().into())
}

/// The child half of [`child_process`]. Without its variable it has
/// nothing to do and passes.
#[test]
fn a_child_run_in_its_own_process() {
    let Ok(job) = std::env::var(CHILD_RUN) else {
        return;
    };
    let job: serde_json::Value = serde_json::from_str(&job).unwrap();
    let t = PathBuf::from(job["folder"].as_str().unwrap());
    let id = job["id"].as_str().unwrap();
    let hitched: Option<wire::Hitched> = serde_json::from_value(job["hitched"].clone()).unwrap();
    let env = host_env(&t);
    let args = ["-p", "--dangerously-skip-permissions"];
    let mut b = built(&env, &t, "claude", "claude", id, &args);
    child_of(
        &mut b,
        "walk-parent",
        hitched,
        job["task"].as_str().unwrap(),
    );
    let (rh, out) = run_child(&t, &env, b);
    let done = serde_json::json!({
        "horse": rh.horse, "session": rh.session, "harness": rh.harness, "output": out
    });
    std::fs::write(t.join(format!("{id}.json")), done.to_string()).unwrap();
}

/// The last run event of `session` of `horse`.
fn last_event(store: &Store, horse: &str, session: &str) -> serde_json::Value {
    run::events(store, horse)
        .unwrap()
        .into_iter()
        .rfind(|e| e["session"] == session)
        .unwrap()
}

#[test]
fn the_whole_walk_on_the_host() {
    let tmp = test_folder();
    let t = tmp.path();
    let env = host_env(t);
    let store = Store::open_host(&env).unwrap();
    let rep = || report(t);

    // 1. A horse per run: a dsh run in horse mode gets a new horse with a
    // DNA, one session and `run.begin`. Its first prompt is the DNA entry.
    let mut pb = built(&env, t, "dsh", "dsh-e2e", "walk-parent", &["web"]);
    let parent_run = lifecycle::start(&env, &mut pb).unwrap().unwrap();
    let parent = by_sandbox::find(&store, "walk-parent").unwrap().unwrap();
    let native = store
        .session(&parent.session)
        .unwrap()
        .binding
        .native_id
        .expect("a bound dsh session");
    let typing = user_input();
    let mut dsh = start_armed(t, &pb, &[("FAKE_DSH_NATIVE", &native)]);
    wait_until("the DNA entry", &rep, || {
        project_file(t, "trace.txt").contains("prompt <paddock-append")
    });
    assert_eq!(store.horses().unwrap().len(), 1, "{}", rep());
    assert_eq!(
        store.horse(&parent.horse).unwrap().sessions,
        std::slice::from_ref(&parent.session)
    );
    let types: Vec<String> = run::events(&store, &parent.horse)
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(types, ["dna.default", "run.begin"], "{}", rep());
    at_safe_point(t, "prompt <paddock-append");
    assert!(
        project_file(t, "prompts.txt").contains("role=\\\"system\\\""),
        "{}",
        rep()
    );

    // 2. DNA inherit: a plain child gets a child horse whose parent DNA is
    // the DNA of the run, with its traits.
    let file = t.join("dna.toml");
    std::fs::write(&file, "[traits]\nstyle = \"walk\"\n").unwrap();
    assert!(horse(t, &["dna", "set", &parent.horse, file.to_str().unwrap()]).contains("is now"));
    let parent_dna = store.horse(&parent.horse).unwrap().dna;
    let (plain, out) = child_process(t, "walk-plain", None, "the plain task");
    assert!(out.contains("claude: turn"), "{out}");
    assert_ne!(plain.horse, parent.horse);
    let child_dna = dna::get_or_default(&store, &store.horse(&plain.horse).unwrap().dna);
    assert_eq!(
        child_dna.origin.parent.as_deref(),
        Some(parent.horse.as_str())
    );
    assert_eq!(child_dna.origin.parent_dna.as_ref(), Some(&parent_dna));
    assert!(horse(t, &["dna", "show", &plain.horse]).contains("walk"));
    assert_eq!(
        last_event(&store, &plain.horse, &plain.session)["type"],
        "run.end"
    );

    // 3. Hitch: the child is a session of the parent's horse. Its result
    // reaches the parent's inbox once and lands as the newest entry, at a
    // safe point.
    let link = wire::begin(&store, &parent, "claude").unwrap();
    let (hitched, out) = child_process(t, "walk-hitched", Some(&link), "the hitched task");
    assert_eq!(hitched.horse, parent.horse);
    assert_eq!(hitched.session, link.session);
    // What the spawner does at the end of a hitched child.
    let text = format!("exited: exit 0\n{}", out.trim_end());
    assert!(wire::finish_once(&store, &link, &text).unwrap().is_some());
    assert!(wire::finish_once(&store, &link, &text).unwrap().is_none());
    let head = format!("hitched {} (claude) finished", &link.session[..8]);
    wait_until("the hitched result landed", &rep, || {
        delivered(&store, &parent.session, &head)
            .iter()
            .any(|d| d.landed_seq.is_some())
    });
    let d = delivered(&store, &parent.session, &head);
    assert_eq!(d.len(), 1, "{}", rep());
    let r = rows(&store, &parent.session);
    assert_eq!(d[0].landed_seq, Some(r.len() as u64 - 1), "{}", rep());
    assert!(r.last().unwrap().contains(&head), "{}", rep());
    assert_eq!(r.iter().filter(|r| r.contains(&head)).count(), 1);
    assert!(
        store.inbox(&parent.session).unwrap().is_empty(),
        "{}",
        rep()
    );
    at_safe_point(t, "hitched");
    let edges = hitch::edges(&store, &parent.horse).unwrap();
    for kind in [EdgeKind::SpawnedFrom, EdgeKind::Append] {
        let n = edges
            .iter()
            .filter(|e| e.kind == kind && e.from == link.session && e.to == parent.session)
            .count();
        assert_eq!(n, 1, "{kind:?}\n{}", rep());
    }
    assert_eq!(
        hitch::visible_sessions(&store, &parent.horse, &link.session),
        std::slice::from_ref(&link.session)
    );

    // 4. Custody: an entry that the host appends while a turn runs lands
    // only after that turn.
    horse(t, &["append", &parent.session, "--text", "a long turn"]);
    wait_until("the long turn", &rep, || {
        t.join("project/turn-running").exists()
    });
    horse(t, &["append", &parent.session, "--text", "custody entry"]);
    // Some passes of the custody go by while the turn runs.
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        delivered(&store, &parent.session, "custody entry").is_empty(),
        "{}",
        rep()
    );
    std::fs::write(t.join("project/turn-go"), "").unwrap();
    wait_until("the custody entry landed", &rep, || {
        delivered(&store, &parent.session, "custody entry")
            .iter()
            .any(|d| d.landed_seq.is_some())
    });
    assert!(
        trace_at(t, "turn-end") < trace_at(t, "custody entry"),
        "{}",
        rep()
    );
    at_safe_point(t, "custody entry");

    // 5. The TUI channel: an interactive Claude Code run gets an entry
    // between its turns.
    tui::walk(t, &store);

    // 6. DNA graft and undo, while the run holds its session.
    graft_and_undo(t, &store, &parent.horse);
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

    // 7. Stop: the end of the user's input ends the run with `run.end`.
    drop(typing);
    let status = dsh.wait().unwrap();
    assert!(status.success(), "the fake dsh failed: {status}\n{}", rep());
    assert!(trace(t).contains(&"shutdown".to_string()), "{}", rep());
    let _ = lifecycle::end(Some(parent_run), 0, None);
    let end = last_event(&store, &parent.horse, &parent.session);
    assert_eq!(end["type"], "run.end", "{end}");
    assert_eq!(end["custody"], "live", "{end}");
    assert_eq!(end["ok"], true, "{end}");
    assert!(end.get("error").is_none(), "{end}");

    // Resume: `--horse-id` continues the same horse and its newest session.
    let sessions_root = pb.spec.home_src.join(".dsh/sessions");
    let cwd = pb.spec.workdir.display().to_string();
    write_session_file(&store, &parent.session, &sessions_root, &native, &cwd);
    let seq = rows(&store, &parent.session).len().to_string();
    let mut rb = built(&env, t, "dsh", "dsh-e2e", "walk-resume", &["web"]);
    rb.spec.intent.horse_id = Some(parent.horse.clone());
    let resumed_run = lifecycle::start(&env, &mut rb).unwrap().unwrap();
    assert_eq!(
        by_sandbox::find(&store, "walk-resume").unwrap().unwrap(),
        parent
    );
    let typing = user_input();
    let mut dsh = start_armed(
        t,
        &rb,
        &[("FAKE_DSH_NATIVE", &native), ("FAKE_DSH_SEQ", &seq)],
    );
    wait_until("the resume", &rep, || {
        project_file(t, "trace.txt").contains(&format!("resume {native}"))
    });
    horse(
        t,
        &["append", &parent.session, "--text", "after the resume"],
    );
    wait_until("the entry after the resume landed", &rep, || {
        delivered(&store, &parent.session, "after the resume")
            .iter()
            .any(|d| d.landed_seq == Some(seq.parse().unwrap()))
    });
    drop(typing);
    assert!(dsh.wait().unwrap().success(), "{}", rep());
    let _ = lifecycle::end(Some(resumed_run), 0, None);
    let end = last_event(&store, &parent.horse, &parent.session);
    assert_eq!(end["type"], "run.end", "{end}");
    assert_eq!(end["ok"], true, "{end}");
    assert!(end.get("error").is_none(), "{end}");
    let begins = run::events(&store, &parent.horse)
        .unwrap()
        .into_iter()
        .filter(|e| e["type"] == "run.begin" && e["session"] == parent.session.as_str())
        .count();
    assert_eq!(begins, 2, "{}", rep());
    assert_eq!(
        store.horse(&parent.horse).unwrap().sessions,
        [parent.session.clone(), link.session.clone()]
    );
}
