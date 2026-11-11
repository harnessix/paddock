//! The device sandbox with real bubblewrap, through `paddock device check
//! NAME --send REQUEST`, which starts an instance on the host without a
//! run: the view of the device program (`caller`, `ro`, `home`, `connect`,
//! credentials), the ssh agent credential, the lifetime of an instance
//! and the start time. Skipped where the host cannot run bubblewrap. The
//! tests that drive a device from an agent sandbox are in
//! tests/devices_sandbox_runs.rs.

mod common;

use common::device_world::{
    alive, body, can_run, echo_server, python, response, started_ms, view_config, wait_for, World,
};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn the_view_of_a_device_program_through_check() {
    if !can_run() {
        return;
    }
    let (port, _server) = echo_server();
    let w = World::new("");
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        view_config(&w, port),
    )
    .unwrap();
    w.script("up.sh", "tr a-z A-Z");
    std::fs::write(
        w.root.join("dev/upper.py"),
        paddock::devices::program::EXAMPLE,
    )
    .unwrap();
    w.script(
        "caller.sh",
        "cat \"$PADDOCK_CALLER_MOUNT/hello.txt\"\ntouch \"$PADDOCK_CALLER_MOUNT/written\" || echo \"WRITE_FAILED $?\"\necho \"caller=$PADDOCK_CALLER harness=$PADDOCK_CALLER_HARNESS\"",
    );
    std::fs::write(w.root.join("dev/secret-note.txt"), "note-7f3a\n").unwrap();
    w.script(
        "rof.sh",
        "cat \"$NOTE\"\necho \"home=$HOME entries=$(ls -A \"$HOME\" | wc -l)\"\nls \"$HOST_HOME/.ssh\" >/dev/null 2>&1 && echo SSH_VISIBLE || echo SSH_HIDDEN\necho \"term=$TERM device=$PADDOCK_DEVICE\"",
    );
    w.script(
        "net.sh",
        "/usr/bin/python3 -c 'import socket,sys\ns=socket.socket(); s.settimeout(3)\ntry:\n  s.connect((\"127.0.0.1\", int(sys.argv[1]))); s.sendall(b\"ping-9c1e\"); print(s.recv(100).decode())\nexcept Exception as e:\n  print(\"NET_FAILED\", type(e).__name__)' \"$PORT\"",
    );
    w.script(
        "sec.sh",
        "echo \"first=$(cat \"$PADDOCK_SECRET_TOK\")\"\necho \"second=$(cat \"$PADDOCK_SECRET_TOK\")\"\necho \"path=$PADDOCK_SECRET_TOK\"",
    );
    // exec and serve: ABC.
    let out = w.check("up", "abc");
    assert_eq!(body(&out), "ABC", "{out}");
    assert!(out.contains("status exit 0, http 200"), "{out}");
    eprintln!("exec device start: {} ms", started_ms(&out));
    if python().is_some() {
        let out = w.check("ups", "abc");
        assert_eq!(response(&out), "ABC", "{out}");
        eprintln!("serve device start: {} ms", started_ms(&out));
    }
    // caller = ["project:ro"]: reads the project, cannot write it (EROFS).
    let out = w.check("caller", "");
    let b = body(&out);
    assert!(b.contains("hello from the caller"), "{out}");
    assert!(b.contains("WRITE_FAILED"), "{out}");
    assert!(
        response(&out).contains("Read-only file system"),
        "EROFS in the stderr lines of the envelope:\n{out}"
    );
    assert!(b.contains("harness=check"), "{out}");
    assert!(!w.root.join("project/written").exists());
    // ro = [FILE] reads it; the home is empty and hidden; TERM=dumb.
    let out = w.check("rof", "");
    let b = body(&out);
    assert!(b.contains("note-7f3a"), "{out}");
    assert!(b.contains("entries=0"), "an empty home:\n{out}");
    assert!(b.contains("SSH_HIDDEN"), "{out}");
    assert!(b.contains("term=dumb device=rof"), "{out}");
    if python().is_some() {
        // Without connect: no route to the host.
        let out = w.check("net", "");
        assert!(body(&out).contains("NET_FAILED"), "{out}");
        // With connect: 127.0.0.1:2222 inside reaches the server on the
        // host, and the audit has the connection with its bytes.
        let out = w.check("con", "");
        assert!(body(&out).contains("pong:ping-9c1e"), "{out}");
        let a = w.audit();
        let line = a
            .lines()
            .find(|l| l.contains("\"capability\":\"device-connect\""))
            .unwrap_or_else(|| panic!("no connect audit line in:\n{a}"));
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["device"], "con");
        assert_eq!(v["port"], port);
        assert_eq!(v["bytes_out"], 9, "{line}");
        assert_eq!(v["bytes_in"], 14, "{line}");
        assert_eq!(v["lan"], true);
        assert_eq!(v["status"], "ok");
        assert!(!a.contains("ping-9c1e"), "never content:\n{a}");
        let stats = String::from_utf8(
            w.paddock()
                .args(["device", "stats", "con", "--since", "1h"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(
            stats.contains(
                "connect   1 connections (0 failed), 14 bytes to the device, 9 bytes from it"
            ),
            "{stats}"
        );
    }
    // deliver as = "file": the FIFO gives the value at each open.
    let out = w
        .paddock()
        .env("TEST_TOKEN", "tok-secret-1a2b")
        .args(["device", "check", "sec", "--send", ""])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let b = body(&text);
    assert!(
        b.contains("first=tok-secret-1a2b") && b.contains("second=tok-secret-1a2b"),
        "{text}"
    );
    assert!(b.contains("path=/run/paddock/device/secret/tok"), "{text}");
    // Every check stopped its instance: no state file, no folder.
    for d in ["up", "caller", "rof", "sec"] {
        assert!(w.state_files(d).is_empty(), "{d} left a state file");
    }
    assert!(
        !w.root.join("run/paddock/devices").exists()
            || std::fs::read_dir(w.root.join("run/paddock/devices"))
                .unwrap()
                .count()
                == 0
    );
}

#[test]
fn an_ssh_agent_credential_gives_a_filtering_agent_and_no_key_file() {
    if !can_run() {
        return;
    }
    let tools = ["ssh-agent", "ssh-add", "ssh-keygen"];
    if tools.iter().any(|t| {
        Command::new("sh")
            .args(["-c", &format!("command -v {t}")])
            .stdout(Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
    }) {
        eprintln!("skipped: needs ssh-agent, ssh-add and ssh-keygen");
        return;
    }
    let w = World::new("");
    let key = w.root.join("data/id_test");
    assert!(Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "device-test-key",
            "-f"
        ])
        .arg(&key)
        .status()
        .unwrap()
        .success());
    let sock = w.root.join("run/host-agent.sock");
    let mut agent = Command::new("ssh-agent")
        .args(["-D", "-a"])
        .arg(&sock)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the host agent socket", Duration::from_secs(10), || {
        sock.exists()
    });
    assert!(Command::new("ssh-add")
        .arg(&key)
        .env("SSH_AUTH_SOCK", &sock)
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success());
    let fp = String::from_utf8(
        Command::new("ssh-keygen")
            .args(["-l", "-f"])
            .arg(format!("{}.pub", key.display()))
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let fingerprint = fp.split_whitespace().nth(1).unwrap().to_string();
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[credentials.k]\nvia = \"agent\"\nkeys = [\"{key}.pub\"]\n\n[devices.ssh]\nshape = \"exec\"\nenabled = true\ncommand = [\"{dev}/ssh.sh\"]\n[devices.ssh.sandbox]\nro = [\"{dev}\"]\ncredentials = [\"k\"]\nenv = {{ KEYFILE = \"{key}\" }}\n",
            key = key.display()
        ),
    )
    .unwrap();
    w.script(
        "ssh.sh",
        "echo \"sock=$SSH_AUTH_SOCK\"\nssh-add -l\nls \"$KEYFILE\" 2>&1 || echo KEY_FILE_ABSENT\nssh-add -D >/dev/null 2>&1 && echo DELETE_OK || echo DELETE_REFUSED",
    );
    let out = w
        .paddock()
        .env("SSH_AUTH_SOCK", &sock)
        .args(["device", "check", "ssh", "--send", ""])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let _ = agent.kill();
    let _ = agent.wait();
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let b = body(&text);
    assert!(b.contains("sock=/run/paddock/ssh-agent.sock"), "{text}");
    assert!(b.contains(&fingerprint), "the key is listed:\n{text}");
    assert!(
        b.contains("KEY_FILE_ABSENT"),
        "the private key is not in the sandbox:\n{text}"
    );
    assert!(
        b.contains("DELETE_REFUSED"),
        "other agent requests fail:\n{text}"
    );
}

#[test]
fn an_instance_dies_with_its_paddock_process_and_check_measures_the_start() {
    if !can_run() {
        return;
    }
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.up]\nshape = \"exec\"\nenabled = true\ntimeout = 60\ncommand = [\"{dev}/up.sh\"]\n[devices.up.sandbox]\nro = [\"{dev}\"]\n"
        ),
    )
    .unwrap();
    w.script(
        "up.sh",
        "echo LOGLINE-77 >&2\nread -r line\ncase \"$line\" in sleep) sleep 60 ;; *) echo \"$line\" | tr a-z A-Z ;; esac",
    );
    // Ten starts: the p50 and p95 for the record.
    let mut times = Vec::new();
    for _ in 0..10 {
        times.push(started_ms(&w.check("up", "abc")));
    }
    times.sort_unstable();
    eprintln!(
        "device sandbox start (bwrap, exec, health answered): p50 {} ms, p95 {} ms",
        times[4], times[9]
    );
    // The log of a check stays, with the program's stderr.
    let logs = w.root.join("state/paddock/devices/log/up");
    let entries = std::fs::read_dir(&logs).unwrap().count();
    assert_eq!(entries, 10, "one log per check");
    let out = w.paddock().args(["device", "logs", "up"]).output().unwrap();
    assert!(!out.status.success(), "ten logs: name one");
    assert!(String::from_utf8_lossy(&out.stderr).contains("has 10 logs"));
    let one = std::fs::read_dir(&logs)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let caller = one.file_stem().unwrap().to_string_lossy().into_owned();
    let out = w
        .paddock()
        .args(["device", "logs", "up", "--sandbox", &caller])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.contains("LOGLINE-77"),
        "{text}"
    );
    assert!(text.contains("start shape exec"), "{text}");
    assert!(!text.contains("abc"), "no request in the log:\n{text}");
    // A paddock process killed with SIGKILL takes its instance with it.
    let mut child = w
        .paddock()
        .args(["device", "check", "up", "--send", "sleep"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the state file", Duration::from_secs(20), || {
        !w.state_files("up").is_empty()
    });
    let st = w.state_files("up").remove(0);
    let pid = st["pid"].as_u64().unwrap();
    assert!(alive(pid));
    assert_eq!(st["run"], "sandbox");
    let dir = PathBuf::from(st["dir"].as_str().unwrap());
    assert!(dir.join("dev.sock").exists());
    // SAFETY: SIGKILL to our own child.
    unsafe { libc::kill(child.id() as i32, libc::SIGKILL) };
    let _ = child.wait();
    let gone = wait_for("the instance to die", Duration::from_secs(2), || {
        !alive(pid)
    });
    eprintln!("paddock killed to instance gone: {gone:?}");
    // The state file is stale now; the next start clears it.
    assert_eq!(w.state_files("up").len(), 1);
    let ls =
        String::from_utf8(w.paddock().args(["device", "ls"]).output().unwrap().stdout).unwrap();
    assert!(
        ls.lines()
            .any(|l| l.starts_with("up ") && l.trim_end().ends_with(" -")),
        "a dead instance is not listed:\n{ls}"
    );
    let out = w.check("up", "x");
    // The check cleared the stale file before its start and removed its
    // own at its stop. Under the load of the whole suite, give a slow
    // removal a bounded deadline instead of one look, and name what is
    // left when it does not happen.
    let left = || {
        w.state_files("up")
            .into_iter()
            .map(|s| {
                let pid = s["pid"].as_u64().unwrap_or(0);
                format!(
                    "{} pid {pid} alive {} cmdline {:?}",
                    s["caller"],
                    alive(pid),
                    std::fs::read(format!("/proc/{pid}/cmdline"))
                        .map(|c| String::from_utf8_lossy(&c).replace('\0', " "))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
    };
    let end = Instant::now() + Duration::from_secs(10);
    while !(w.state_files("up").is_empty() && !dir.exists()) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        w.state_files("up").is_empty(),
        "clear_stale did not remove the stale file (the killed instance was pid {pid}): {:?}\n{out}",
        left()
    );
    assert!(!dir.exists(), "and its folder: {}", dir.display());
}

#[test]
fn a_host_home_under_tmp_does_not_hide_the_device_working_directory() {
    if !can_run() {
        return;
    }
    let w = World::new("");
    let host_home = tempfile::tempdir_in("/tmp").unwrap();
    let script = w.script("home-check.sh", "printf '%s\\n' \"$HOME\"; pwd; cat");
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.homecheck]\nshape = \"exec\"\nenabled = true\ncommand = [{}]\n[devices.homecheck.sandbox]\nro = [{}]\n",
            serde_json::to_string(&script).unwrap(),
            serde_json::to_string(&w.root.join("dev").display().to_string()).unwrap(),
        ),
    )
    .unwrap();
    let out = w
        .paddock()
        .env("HOME", host_home.path())
        .args(["device", "check", "homecheck", "--send", "hello"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let result = body(&stdout);
    assert!(
        result.contains(&host_home.path().display().to_string()),
        "{result}"
    );
    assert!(result.contains("hello"), "{result}");
}
