//! `scope = "shared"`: one instance per user, leases, the idle exit, the
//! table hash and the start paths.
//!
//! The instance is a real `paddock deviced` process on the socket
//! `$XDG_RUNTIME_DIR/paddock/devices/NAME.sock`, and each broker is a real
//! `paddock device check NAME --hold N` process, which takes one lease for
//! as long as it runs, as a run does. So a broker that `kill -9` removes
//! drops its lease the way a killed paddock process does.
//!
//! The device sandbox inside the instance runs with the thread launcher
//! (`PADDOCK_TEST_DEVICE_LAUNCHER=thread`), so it needs no bubblewrap. The steps that need bubblewrap, FUSE or a systemd
//! user manager say so and skip; packaging/macos/checks/devices-shared.sh
//! runs them on a real host.

mod common;

use common::shared_world::{config, until, world, Broker, World};
use std::path::Path;
use std::time::{Duration, Instant};

/// Two runs with the same shared device: one instance, one pid, both get
/// answers, two users. The first ends, one user is left. The second ends,
/// and the instance exits after `idle`.
///
/// The instance is started as a unit starts it: `paddock deviced` with no
/// owner. An instance that a run owns ends with that run, which would race
/// with the idle exit when the owner is the last user.
#[test]
fn two_runs_share_one_instance_and_it_exits_when_both_ended() {
    let w = world();
    let mut instance = Broker(Some(
        w.paddock()
            .args(["deviced", "up"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let pid = until(10, || w.pid(), |p| *p > 0);
    assert!(pid > 0, "no state file:\n{}", w.log());
    assert_eq!(pid, instance.pid());
    assert_eq!(w.state().unwrap()["owner"], serde_json::Value::Null);
    let mut first = w.broker("alpha", 20);
    assert!(
        until(10, || socket_answers(&w.socket()), |a| *a),
        "{}",
        w.log()
    );
    assert_eq!(until(5, || w.users(), |u| *u == 1), 1, "{}", w.log());
    let mut second = w.broker("beta", 4);
    assert_eq!(until(10, || w.users(), |u| *u == 2), 2, "{}", w.log());
    // One instance, not two: the pid did not change and there is one
    // state file.
    assert_eq!(w.pid(), pid, "the second run started another instance");
    let dir = w.root.join("state/paddock/devices/instances/up");
    let files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["shared.json"], "{files:?}");
    assert_eq!(w.state().unwrap()["run"], "shared");
    // Both got an answer, each with its own caller.
    let out2 = second.wait();
    assert!(out2.contains("BETA"), "{out2}");
    assert_eq!(
        until(10, || w.users(), |u| *u == 1),
        1,
        "the lease of the run that ended is still counted:\n{}",
        w.log()
    );
    let out1 = first.wait();
    assert!(out1.contains("ALPHA"), "{out1}");
    // The callers differ: the program sees PADDOCK_CALLER per request.
    let caller = |t: &str| {
        t.lines()
            .find_map(|l| l.split("caller=").nth(1))
            .map(|c| c.split(' ').next().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    assert_ne!(caller(&out1), "", "{out1}");
    assert_ne!(caller(&out1), caller(&out2), "{out1}\n{out2}");
    // No user left: the instance exits after idle (2 s), not before.
    let left = Instant::now();
    assert!(
        until(15, || w.state().is_none(), |gone| *gone),
        "the instance is still there:\n{}",
        w.log()
    );
    assert!(
        left.elapsed() >= Duration::from_millis(1500),
        "the instance exited {:?} after its last user, before idle:\n{}",
        left.elapsed(),
        w.log()
    );
    assert!(
        w.log().contains("idle for 2 s with no user: exit"),
        "{}",
        w.log()
    );
    let out = instance.wait();
    assert!(!out.contains("paddock:"), "{out}");
}

/// A broker that `kill -9` removes drops its lease by the close of the
/// socket, within two seconds.
#[test]
fn a_broker_killed_with_nine_drops_its_lease() {
    let w = world();
    let mut keeper = w.broker("keep", 30);
    assert_eq!(until(10, || w.users(), |u| *u == 1), 1, "{}", w.log());
    let mut doomed = w.broker("doomed", 30);
    assert_eq!(until(10, || w.users(), |u| *u == 2), 2, "{}", w.log());
    let gone = doomed.pid();
    assert!(gone > 0);
    let t = Instant::now();
    doomed.kill9();
    assert_eq!(
        until(5, || w.users(), |u| *u == 1),
        1,
        "the lease of a killed broker is still counted:\n{}",
        w.log()
    );
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "the lease took {:?} to drop",
        t.elapsed()
    );
    // The instance kept running for the other user.
    assert!(w.pid() > 0, "{}", w.log());
    keeper.wait();
}

/// A table change gives `reconfigured` on the next request, and `paddock
/// device restart` clears it.
#[test]
fn a_table_change_is_refused_until_a_restart() {
    let w = world();
    let mut keeper = w.broker("first", 30);
    let pid = until(10, || w.pid(), |p| *p > 0);
    assert!(pid > 0, "{}", w.log());
    // The user edits the table. The instance serves the table it read.
    let s = w.root.join("dev/up.sh").display().to_string();
    std::fs::write(w.config(), config(&s, "")).unwrap();
    std::fs::write(
        w.config(),
        std::fs::read_to_string(w.config())
            .unwrap()
            .replace("timeout = 10", "timeout = 11"),
    )
    .unwrap();
    let (ok, text) = w.run(&["device", "check", "up", "--send", "late"]);
    assert!(
        !ok,
        "a broker with another view of the table was served:\n{text}"
    );
    assert!(
        text.contains("reconfigured") || text.contains("was reconfigured"),
        "{text}"
    );
    assert!(text.contains("paddock device restart up"), "{text}");
    // The instance is still there for the run that started it.
    assert_eq!(w.pid(), pid, "{}", w.log());
    // restart ends the old instance. On this host no user manager owns the
    // lifetime, so the next request of a run starts the new one, and the
    // command says so instead of starting one that would end with it.
    let (ok, text) = w.run(&["device", "restart", "up"]);
    assert!(ok, "{text}");
    assert!(text.contains("stopped the shared instance of up"), "{text}");
    assert!(text.contains("paddock device install up"), "{text}");
    assert!(
        w.state().is_none(),
        "the restart kept the old instance:\n{}",
        w.log()
    );
    // The next request is served, by an instance that read the new table.
    let (ok, text) = w.run(&["device", "check", "up", "--send", "now"]);
    assert!(ok, "{text}");
    assert!(text.contains("NOW"), "{text}");
    assert_ne!(w.pid(), pid, "{}", w.log());
    // The run that was refused is served again: a new instance is another
    // instance, so the device forgets the refusal.
    let (ok, text) = w.run(&["device", "check", "up", "--send", "after"]);
    assert!(ok, "{text}");
    assert!(text.contains("AFTER"), "{text}");
    keeper.wait();
}

/// Without a user manager the first broker owns the instance, the notice
/// names that sandbox, the instance ends with that broker, and the next
/// request of another broker starts a new one.
#[test]
fn without_a_user_manager_the_first_broker_owns_the_instance() {
    let w = world();
    let mut owner = w.broker("mine", 3);
    let pid = until(10, || w.pid(), |p| *p > 0);
    assert!(pid > 0, "{}", w.log());
    let st = w.state().unwrap();
    let owner_id = st["owner"].as_str().unwrap_or("").to_string();
    assert!(!owner_id.is_empty(), "no owner in the state file: {st}");
    assert_eq!(st["unit"], serde_json::Value::Null);
    // The notice names the device, the sandbox and the fix.
    let log = w.log();
    assert!(log.contains("has no user manager here"), "{log}");
    assert!(log.contains("paddock device install up"), "{log}");
    assert!(
        log.contains(&owner_id),
        "the notice does not name the owner:\n{log}"
    );
    // `paddock ps` names the owner too.
    let (ok, text) = w.run(&["ps"]);
    assert!(ok, "{text}");
    assert!(text.contains("shared device up"), "{text}");
    assert!(text.contains("owned by"), "{text}");
    // A second broker uses the same instance.
    let mut second = w.broker("second", 12);
    assert_eq!(until(10, || w.users(), |u| *u == 2), 2, "{}", w.log());
    assert_eq!(w.pid(), pid);
    // The owner ends: the instance ends with it.
    let out = owner.wait();
    assert!(out.contains("MINE"), "{out}");
    assert!(
        until(10, || w.state().is_none(), |gone| *gone),
        "the instance outlived the run that owns it:\n{}",
        w.log()
    );
    // The next request of the second broker starts a new instance.
    let (ok, text) = w.run(&["device", "check", "up", "--send", "again"]);
    assert!(ok, "{text}");
    assert!(text.contains("AGAIN"), "{text}");
    let newpid = until(10, || w.pid(), |p| *p > 0);
    assert_ne!(newpid, pid, "{}", w.log());
    second.wait();
}

/// Ten brokers that start at once get one instance: the start lock lets
/// one of them in, and the rest find a socket that answers.
#[test]
fn ten_brokers_that_start_at_once_get_one_instance() {
    let w = world();
    let mut brokers: Vec<Broker> = (0..10).map(|i| w.broker(&format!("n{i}"), 6)).collect();
    assert_eq!(
        until(20, || w.users(), |u| *u == 10),
        10,
        "not every broker took a lease:\n{}",
        w.log()
    );
    let pid = w.pid();
    assert!(pid > 0);
    // One state file while all ten hold a lease, so one instance for all.
    let dir = w.root.join("state/paddock/devices/instances/up");
    let files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["shared.json"], "{files:?}");
    let mut answered = 0;
    for b in brokers.iter_mut() {
        let out = b.wait();
        if out.contains("shared instance ready") {
            answered += 1;
        }
        assert!(
            out.contains(&format!("pid {pid}")),
            "a broker reached another instance:\n{out}"
        );
    }
    assert_eq!(answered, 10);
}

/// `paddock device stop` ends the instance now, and `paddock device ls`
/// and `show` report the shared lifetime.
#[test]
fn stop_ends_the_instance_and_the_commands_report_it() {
    let w = world();
    let mut keeper = w.broker("hold", 30);
    assert_eq!(until(10, || w.users(), |u| *u == 1), 1, "{}", w.log());
    let (ok, text) = w.run(&["device", "ls"]);
    assert!(ok, "{text}");
    assert!(text.contains("shared, 1 user"), "{text}");
    let (ok, text) = w.run(&["device", "show", "up"]);
    assert!(ok, "{text}");
    assert!(text.contains("idle      2 s"), "{text}");
    assert!(text.contains("devices/up.sock"), "{text}");
    assert!(
        text.contains("the first request starts the shared instance"),
        "{text}"
    );
    // --sandbox is refused: one instance serves every sandbox.
    let (ok, text) = w.run(&["device", "stop", "up", "--sandbox", "nope"]);
    assert!(!ok, "{text}");
    assert!(text.contains("one instance serves every sandbox"), "{text}");
    let (ok, text) = w.run(&["device", "stop", "up"]);
    assert!(ok, "{text}");
    assert!(text.contains("stopped the shared instance of up"), "{text}");
    assert!(w.state().is_none(), "{}", w.log());
    assert!(!socket_answers(&w.socket()), "the socket still answers");
    let (ok, text) = w.run(&["device", "ls"]);
    assert!(ok, "{text}");
    assert!(text.contains("shared, stopped"), "{text}");
    let (ok, text) = w.run(&["device", "stop", "up"]);
    assert!(ok, "{text}");
    assert!(text.contains("no running shared instance"), "{text}");
    keeper.wait();
}

fn socket_answers(p: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(p).is_ok()
}

/// `paddock device install` is for shared devices only, and `uninstall`
/// without an install says so.
#[test]
fn install_is_refused_for_a_run_scope_device() {
    let w = World::new("");
    let s = w.script("up.sh", "cat");
    std::fs::write(
        w.config(),
        format!("[devices.up]\nshape = \"exec\"\nenabled = true\ncommand = [\"{s}\"]\n"),
    )
    .unwrap();
    let (ok, text) = w.run(&["device", "install", "up"]);
    assert!(!ok, "{text}");
    assert!(text.contains("Only a shared device"), "{text}");
    std::fs::write(w.config(), config(&s, "")).unwrap();
    let (ok, text) = w.run(&["device", "uninstall", "up"]);
    assert!(ok, "{text}");
    assert!(text.contains("nothing to remove"), "{text}");
}

/// An agent sandbox drives the shared instance through `/dev/paddock/up`:
/// two runs, one instance, the user count of two, both runs end, and the
/// instance exits after `idle`. The scenario is in
/// tests/common/scenarios.rs, and tests/devices_stress.rs runs it twice
/// at once.
///
/// Needs bubblewrap and FUSE; a host without them skips it.
/// packaging/macos/checks/devices-shared.sh runs it on a real host.
#[test]
fn an_agent_sandbox_uses_the_shared_instance() {
    common::scenarios::agent_uses_shared_instance();
}
