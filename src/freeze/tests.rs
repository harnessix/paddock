use super::*;

#[test]
fn only_paddock_scopes_are_frozen() {
    assert!(ours("paddock-sb-claude-x_1.scope"));
    assert!(ours("paddock-spawn-abc.scope"));
    assert!(!ours("app-com.anthropic.Claude-2408241.scope"));
    assert!(!ours("paddock-sb-x.service"));
    assert!(!ours("paddock-sb/../x.scope"));
    assert_eq!(
        scope_name("claude-repo-8a48be"),
        "paddock-sb-claude-repo-8a48be.scope"
    );
    assert_eq!(scope_name("a b/c"), "paddock-sb-a_b_c.scope");
}

#[test]
fn suspend_config_parses_both_lease_rules() {
    assert_eq!(SuspendCfg::parse("").unwrap().leases, Leases::Keep);
    assert_eq!(
        SuspendCfg::parse("[suspend]\n").unwrap().leases,
        Leases::Keep
    );
    assert_eq!(
        SuspendCfg::parse("[suspend]\nleases = \"keep\"\n")
            .unwrap()
            .leases,
        Leases::Keep
    );
    assert_eq!(
        SuspendCfg::parse("[suspend]\nleases = \"release\"\n")
            .unwrap()
            .leases,
        Leases::Release
    );
    assert!(SuspendCfg::parse("[suspend]\nleases = \"drop\"\n").is_err());
    assert!(SuspendCfg::parse("[suspend]\nother = 1\n").is_err());
}

use std::cell::RefCell;
use std::collections::BTreeMap;

/// A fake process table: the children of each pid.
struct Table(RefCell<BTreeMap<u32, Vec<u32>>>);

impl Table {
    fn new(edges: &[(u32, u32)]) -> Self {
        let t = Table(RefCell::new(BTreeMap::new()));
        for &(p, c) in edges {
            t.fork(p, c);
        }
        t
    }
    fn fork(&self, parent: u32, child: u32) {
        self.0.borrow_mut().entry(parent).or_default().push(child);
    }
    fn children(&self, pid: u32) -> Vec<u32> {
        self.0.borrow().get(&pid).cloned().unwrap_or_default()
    }
}

#[test]
fn the_tree_walk_signals_every_descendant_once() {
    let t = Table::new(&[(10, 11), (10, 12), (11, 13), (13, 14)]);
    let order = RefCell::new(Vec::new());
    let mut pauses = 0;
    let (sent, settled) = signal_tree(
        10,
        &mut |p| t.children(p),
        &mut |p| order.borrow_mut().push(p),
        &mut || pauses += 1,
    );
    assert!(settled);
    assert_eq!(sent, BTreeSet::from([10, 11, 12, 13, 14]));
    // Each one once, and a parent before its children.
    let order = order.into_inner();
    assert_eq!(order.len(), 5);
    let at = |p| order.iter().position(|&x| x == p).unwrap();
    assert!(at(10) < at(11) && at(11) < at(13) && at(13) < at(14));
    assert!(at(10) < at(12));
    // One more pass found nothing new.
    assert_eq!(pauses, 1);
}

#[test]
fn a_fork_during_the_freeze_is_caught_by_the_next_pass() {
    let t = Table::new(&[(10, 11)]);
    let mut pass = 0;
    let (sent, settled) = signal_tree(10, &mut |p| t.children(p), &mut |_| {}, &mut || {
        // 11 forked 20 before its stop landed, and 20 forked 21.
        pass += 1;
        if pass == 1 {
            t.fork(11, 20);
        } else if pass == 2 {
            t.fork(20, 21);
        }
    });
    assert!(settled);
    assert_eq!(sent, BTreeSet::from([10, 11, 20, 21]));
    assert_eq!(pass, 3);
}

#[test]
fn a_tree_that_keeps_growing_does_not_settle() {
    let t = Table::new(&[]);
    let mut next = 100;
    let (sent, settled) = signal_tree(1, &mut |p| t.children(p), &mut |_| {}, &mut || {
        next += 1;
        t.fork(1, next);
    });
    assert!(!settled);
    assert_eq!(sent.len(), MAX_PASSES);
}

#[test]
fn a_loop_or_a_listed_zero_in_the_table_ends() {
    // A reused pid can make the table point back up the tree.
    let t = Table::new(&[(1, 2), (2, 3), (3, 1), (3, 0)]);
    let (sent, settled) = signal_tree(1, &mut |p| t.children(p), &mut |_| {}, &mut || {});
    assert!(settled);
    assert_eq!(sent, BTreeSet::from([1, 2, 3]));
}

fn running(pid: u32, host_pid: u32) -> Running {
    serde_json::from_value(serde_json::json!({
        "id": "sbtest", "harness": "fake", "project": "/tmp", "backend": "seatbelt",
        "network": "none", "gui": "off", "pid": pid, "started": 0, "host_pid": host_pid,
    }))
    .unwrap()
}

#[test]
fn an_app_under_seatbelt_is_not_frozen() {
    let mut r = running(std::process::id(), std::process::id());
    r.gui = "app".into();
    let e = signal_run(&r, true).unwrap_err();
    assert!(e.to_string().contains("app mode"), "{e}");
}

#[test]
fn a_pid_that_is_not_the_child_of_its_paddock_is_not_signaled() {
    // pid 1 is never a child of this test.
    let e = signal_run(&running(1, std::process::id()), true).unwrap_err();
    assert!(e.to_string().contains("no longer the sandbox"), "{e}");
    let e = signal_run(&running(std::process::id(), 1), true).unwrap_err();
    assert!(e.to_string().contains("no longer the sandbox"), "{e}");
}

/// The state letter of a Linux process: T is stopped.
#[cfg(not(target_os = "linux"))]
fn state(pid: u32) -> Option<char> {
    // macOS has no /proc: ps prints the state letter first (T: stopped).
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().chars().next()
}

#[cfg(target_os = "linux")]
fn state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .next()?
        .chars()
        .next()
}

#[test]
fn signal_run_stops_and_continues_a_real_process_tree() {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args(["-c", "sleep 60 & (sleep 60 & wait) & wait"])
        .stdin(std::process::Stdio::null());
    // SAFETY: own_group only makes async-signal-safe calls.
    unsafe { cmd.pre_exec(|| own_group()) };
    let mut child = cmd.spawn().unwrap();
    let pid = child.id();
    let tree = || {
        signal_tree(
            pid,
            &mut crate::platform::children_of,
            &mut |_| {},
            &mut || {},
        )
        .0
    };
    let start = std::time::Instant::now();
    while tree().len() < 4 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let pids = tree();
    assert_eq!(
        pids.len(),
        4,
        "sh, sleep, the subshell and its sleep: {pids:?}"
    );
    // SAFETY: getpgid only reads.
    assert_eq!(unsafe { libc::getpgid(pid as i32) }, pid as i32);
    let r = running(pid, std::process::id());
    signal_run(&r, true).unwrap();
    let wait_for = |want: bool| {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if pids.iter().all(|&p| (state(p) == Some('T')) == want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    };
    assert!(wait_for(true), "not all stopped: {pids:?}");
    signal_run(&r, false).unwrap();
    assert!(wait_for(false), "not all running again: {pids:?}");
    // SAFETY: the test's own process group.
    unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
    let _ = child.wait();
}

#[test]
fn this_test_runs_outside_a_paddock_scope() {
    // The terminal's or the test runner's scope is never paddock's.
    assert_eq!(own_scope(), None);
}

/// The shim side of the control socket: it records each line.
#[derive(Default)]
struct Shim {
    lines: std::sync::Mutex<Vec<String>>,
}

impl crate::vm::bridge::Guest for Shim {
    fn connect(&self, port: u32) -> Result<std::os::fd::OwnedFd> {
        bail!("no port {port}")
    }
    fn stop(&self) {
        self.lines.lock().unwrap().push("stop".into());
    }
    fn pause(&self) -> Result<()> {
        self.lines.lock().unwrap().push("pause".into());
        Ok(())
    }
    fn resume(&self) -> Result<()> {
        self.lines.lock().unwrap().push("resume".into());
        Ok(())
    }
}

#[test]
fn macvm_lines_of_each_op() {
    use macvm::{lines, Op};
    assert_eq!(lines(false, Op::Freeze), (vec!["pause"], true));
    assert_eq!(lines(true, Op::Freeze), (vec![], true));
    assert_eq!(lines(true, Op::Thaw), (vec!["resume"], false));
    assert_eq!(lines(false, Op::Thaw), (vec![], false));
    assert_eq!(lines(true, Op::Quit), (vec!["resume", "stop"], false));
    assert_eq!(lines(false, Op::Quit), (vec!["stop"], false));
}

#[test]
fn macvm_pause_resume_and_quit_while_frozen() {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(
        t.path().join("home"),
        "u".into(),
        501,
        20,
        [
            ("XDG_RUNTIME_DIR", "run"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CONFIG_HOME", "config"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), t.path().join(v).display().to_string()))
        .collect(),
    );
    // The shim of the session: this process, with its control socket.
    let sock = t.path().join("control.sock");
    let shim = std::sync::Arc::new(Shim::default());
    let listener = crate::vm::bridge::bind_control(&sock).unwrap();
    crate::vm::bridge::serve_control(listener, shim.clone());
    let layout = crate::macvm::layout::Layout::new(&env);
    std::fs::create_dir_all(&layout.sessions).unwrap();
    let app = "chatgpt-desktop";
    let s = crate::macvm::host::Session {
        pid: std::process::id(),
        app: app.into(),
        home: crate::macvm::host::DEFAULT_HOME.into(),
        profile: t.path().join("profile"),
        control: sock.clone(),
        started: crate::state::now(),
        ..Default::default()
    };
    std::fs::write(
        layout.record(app, crate::macvm::host::DEFAULT_HOME),
        serde_json::to_vec(&s).unwrap(),
    )
    .unwrap();
    crate::macvm::host_running::register(&env, &s, &[], "ask");
    let id = crate::macvm::host_running::running_id(app);
    let seen = || shim.lines.lock().unwrap().clone();

    // The freeze sends pause, and the state is kept.
    assert_eq!(
        freeze(&env, &id, false, "wait for CI").unwrap(),
        vec![id.clone()]
    );
    assert_eq!(seen(), vec!["pause"]);
    assert!(frozen(&env, &id).is_some());
    // A second freeze is a no-op.
    assert!(freeze(&env, &id, false, "again").unwrap().is_empty());
    assert_eq!(seen(), vec!["pause"]);
    // The thaw sends resume.
    assert_eq!(thaw(&env, &id, false).unwrap(), vec![id.clone()]);
    assert_eq!(seen(), vec!["pause", "resume"]);
    assert!(frozen(&env, &id).is_none());
    // A thaw of a running session sends nothing.
    assert!(thaw(&env, &id, false).unwrap().is_empty());
    // A quit while frozen: resume, then the quit. The shim here never
    // ends, so the stop gives up at once with its timeout line.
    freeze(&env, &id, false, "lunch").unwrap();
    let r = crate::macvm::host::stop(&env, app, 1, Duration::ZERO);
    assert!(r.is_err(), "the fake shim does not end");
    assert_eq!(seen(), vec!["pause", "resume", "pause", "resume", "stop"]);
    assert!(frozen(&env, &id).is_none());
    // Both go to the audit log, as every freeze.
    let audit = std::fs::read_to_string(crate::broker::audit_file(&env)).unwrap_or_default();
    assert!(
        audit.contains("\"frozen\"") && audit.contains("\"thawed\""),
        "{audit}"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires a working systemd user manager on the host"]
fn host_scope_sets_memory_high() {
    use std::process::{Command, Stdio};
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let env = HostEnv::detect().unwrap();
    let child = ChildGuard(
        Command::new("sh")
            .args(["-c", "kill -STOP $$; read -r x"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let id = format!("memory-high-test-{}", child.0.id());
    let unit = scope_name(&id);
    wait_stopped(child.0.id()).unwrap();
    assert!(move_to_scope(&env, child.0.id(), &unit, &id, Some(512 << 20)).unwrap());
    resume(child.0.id());
    let text = std::fs::read_to_string(format!("/proc/{}/cgroup", child.0.id())).unwrap();
    let path = text.lines().find_map(|l| l.strip_prefix("0::/")).unwrap();
    let high = std::fs::read_to_string(Path::new("/sys/fs/cgroup").join(path).join("memory.high"))
        .unwrap();
    assert_eq!(high.trim(), (512u64 << 20).to_string());
}
