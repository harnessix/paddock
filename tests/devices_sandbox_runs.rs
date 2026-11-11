//! The device sandbox driven from an agent sandbox, with real bubblewrap
//! and FUSE: the instance of a run, `paddock ps`, `paddock device
//! ls|logs|stop|restart|stats` (tests/common/scenarios.rs), two runs with
//! their own instances, a killed run, and `paddock freeze` of the device
//! scope (a systemd user instance). Skipped where the host cannot run
//! bubblewrap or has no FUSE. The view of a device program is in
//! tests/devices_sandbox.rs.

mod common;

use common::device_world::{alive, can_run, fuse_works, wait_for, World};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// An agent sandbox drives the instance of its run and the host sees it.
/// The scenario is in tests/common/scenarios.rs, and
/// tests/devices_stress.rs runs it twice at once.
#[test]
fn an_agent_sandbox_drives_the_instance_and_the_host_sees_it() {
    common::scenarios::agent_drives_instance();
}

#[test]
fn two_runs_get_their_own_instances_and_a_killed_run_leaves_none() {
    if !can_run() {
        return;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return;
    }
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.ws]\nshape = \"exec\"\nenabled = true\nmode = \"allow\"\ngrant = [\"all\"]\nstart = \"with-sandbox\"\ncommand = [\"{dev}/up.sh\"]\n[devices.ws.sandbox]\nro = [\"{dev}\"]\n"
        ),
    )
    .unwrap();
    w.script("up.sh", "tr a-z A-Z");
    w.harness("waiter", "if [ ! -e /dev/paddock/ws ]; then echo NODEV; exit 0; fi\nwhile [ ! -e flags/go ]; do sleep 0.1; done");
    std::fs::create_dir_all(w.root.join("project/flags")).unwrap();
    let mut a = w.run("waiter");
    let b = w.run("waiter");
    wait_for("two instances", Duration::from_secs(90), || {
        w.state_files("ws").len() == 2
    });
    let files = w.state_files("ws");
    let sockets: Vec<String> = files
        .iter()
        .map(|f| f["socket"].as_str().unwrap().to_string())
        .collect();
    assert_ne!(sockets[0], sockets[1]);
    assert!(sockets.iter().all(|s| Path::new(s).exists()));
    let pids: Vec<u64> = files.iter().map(|f| f["pid"].as_u64().unwrap()).collect();
    // kill -9 of one paddock process: its instance is gone within 2 s.
    // SAFETY: SIGKILL to our own child.
    unsafe { libc::kill(a.id() as i32, libc::SIGKILL) };
    let _ = a.wait();
    let both_gone = || pids.iter().filter(|p| !alive(**p)).count() == 1;
    let took = wait_for("one instance to die", Duration::from_secs(2), both_gone);
    eprintln!("kill -9 of paddock to instance gone: {took:?}");
    std::fs::write(w.root.join("project/flags/go"), "").unwrap();
    let out = b.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_for("the other instance to end", Duration::from_secs(2), || {
        pids.iter().all(|p| !alive(*p))
    });
    // The stale state file of the killed run goes at the next start.
    w.paddock()
        .args(["device", "check", "ws"])
        .output()
        .unwrap();
    assert!(w.state_files("ws").is_empty());
}

#[test]
fn freeze_reaches_the_device_scope() {
    if !can_run() {
        return;
    }
    if !fuse_works() || !common::systemd_user() {
        eprintln!("skipped: needs FUSE, a systemd user instance and the session bus");
        return;
    }
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.ws]\nshape = \"exec\"\nenabled = true\nmode = \"allow\"\ngrant = [\"all\"]\nstart = \"with-sandbox\"\ncommand = [\"{dev}/up.sh\"]\n[devices.ws.sandbox]\nro = [\"{dev}\"]\n"
        ),
    )
    .unwrap();
    w.script("up.sh", "tr a-z A-Z");
    w.harness("waiter", "if [ ! -e /dev/paddock/ws ]; then echo NODEV; exit 0; fi\nwhile [ ! -e flags/go ]; do sleep 0.1; done");
    std::fs::create_dir_all(w.root.join("project/flags")).unwrap();
    let mut child = w
        .paddock()
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(),
        )
        .args([
            "run",
            "waiter",
            "--backend",
            "bwrap",
            "--network",
            "none",
            "--env",
            "clean",
            "-y",
        ])
        .arg(w.root.join("project"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for("the instance", Duration::from_secs(90), || {
        !w.state_files("ws").is_empty()
    });
    let st = w.state_files("ws").remove(0);
    let id = st["caller"].as_str().unwrap().to_string();
    // A `with-sandbox` instance starts before the run registers itself:
    // `paddock freeze` finds the run once `paddock ps` lists it.
    wait_for("the run in paddock ps", Duration::from_secs(60), || {
        w.ps()["sandboxes"]
            .as_array()
            .is_some_and(|s| s.iter().any(|r| r["id"] == id))
    });
    let scope = st["scope"].as_str().unwrap_or("").to_string();
    assert!(
        scope.starts_with("paddock-dev-") && scope.ends_with(".scope"),
        "{st}"
    );
    let pid = st["pid"].as_u64().unwrap();
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
    let path = cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .unwrap()
        .trim()
        .to_string();
    assert!(path.ends_with(&scope), "{path} vs {scope}");
    let freeze_file = PathBuf::from(format!("/sys/fs/cgroup{path}/cgroup.freeze"));
    let frozen = || std::fs::read_to_string(&freeze_file).unwrap().trim() == "1";
    assert!(!frozen());
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    let out = w
        .paddock()
        .env("DBUS_SESSION_BUS_ADDRESS", &bus)
        .args(["freeze", &id])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_for("the device scope to freeze", Duration::from_secs(5), frozen);
    let out = w
        .paddock()
        .env("DBUS_SESSION_BUS_ADDRESS", &bus)
        .args(["thaw", &id])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_for("the device scope to thaw", Duration::from_secs(5), || {
        !frozen()
    });
    std::fs::write(w.root.join("project/flags/go"), "").unwrap();
    let _ = child.wait();
}
