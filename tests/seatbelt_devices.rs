//! The device sandbox on macOS, end to end: `paddock device check NAME
//! --send REQUEST` starts an instance under `sandbox-exec` on the host
//! without a run, and a Seatbelt run drives one with `paddock-host
//! device`. The view of the device program (`caller`, `ro`, `home`,
//! `connect`, credentials), the ssh agent credential, the lifetime of an
//! instance, `paddock ps`, `paddock device ls|logs|stop|restart|stats`,
//! and the ignored `mem` of a table. Each test uses a private scratch root
//! and a separate fixture home. No real harness, no outside host. The
//! Linux twins are tests/devices_sandbox.rs and
//! tests/devices_sandbox_runs.rs.
#![cfg(target_os = "macos")]

mod common;

use common::device_world::{
    alive, body, echo_server, python, response, started_ms, view_config, wait_for, World,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The fake harness of the run tests: a request, then a wait for a flag
/// file, then a request again. No `/dev/paddock` on macOS: the catalog
/// comes from `paddock-host device list`.
const AGENT: &str = r#"
paddock-host device list > flags/catalog1 2>/dev/null || echo NOLIST
echo "FIRST $(printf abc | paddock-host device up - | tail -n 1) EXIT $?"
while [ ! -e flags/go ]; do sleep 0.1; done
echo "SECOND $(printf def | paddock-host device up - | tail -n 1) EXIT $?"
while [ ! -e flags/go2 ]; do sleep 0.1; done
echo "THIRD $(printf ghi | paddock-host device up - | tail -n 1) EXIT $?"
"#;

fn say(o: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The one state file of `device`: it says `seatbelt`, no scope, a socket
/// in the short socket folder.
fn seatbelt_state(w: &World, device: &str) -> serde_json::Value {
    let st = w.state_files(device).remove(0);
    assert_eq!(st["backend"], "seatbelt", "{st}");
    assert_eq!(st["run"], "sandbox", "{st}");
    assert!(st["scope"].is_null(), "{st}");
    let sock = st["socket"].as_str().unwrap();
    assert!(
        paddock::sockpath::fits(Path::new(sock), paddock::sockpath::MACOS_MAX),
        "{sock}"
    );
    use std::os::unix::fs::PermissionsExt;
    let folder = Path::new(sock).parent().unwrap();
    assert_eq!(
        std::fs::metadata(folder).unwrap().permissions().mode() & 0o077,
        0
    );
    assert!(sock.ends_with("/dev.sock"), "{sock}");
    assert!(Path::new(sock).exists(), "{sock}");
    st
}

#[test]
fn the_view_of_a_device_program_under_seatbelt() {
    let (port, _server) = echo_server();
    let w = World::new("");
    let shared_tmp = tempfile::Builder::new()
        .prefix("pd-")
        .tempdir_in("/tmp")
        .unwrap();
    let escape = shared_tmp.path().join("escape");
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
        "cat \"$PADDOCK_CALLER_MOUNT/hello.txt\"\ntouch \"$PADDOCK_CALLER_MOUNT/written\" || echo \"WRITE_FAILED $?\"\necho \"caller=$PADDOCK_CALLER harness=$PADDOCK_CALLER_HARNESS project=$PADDOCK_CALLER_PROJECT mount=$PADDOCK_CALLER_MOUNT\"",
    );
    std::fs::write(w.root.join("dev/secret-note.txt"), "note-7f3a\n").unwrap();
    w.script(
        "rof.sh",
        &format!("cat \"$NOTE\"\necho \"home=$HOME entries=$(ls -A \"$HOME\" | wc -l | tr -d ' ')\"\nls \"$HOST_HOME/.ssh\" >/dev/null 2>&1 && echo SSH_VISIBLE || echo SSH_HIDDEN\ncat \"$HOST_HOME/.bash_history\" >/dev/null 2>&1 && echo HISTORY_VISIBLE || echo HISTORY_HIDDEN\necho \"term=$TERM device=$PADDOCK_DEVICE tmp=$TMPDIR\"\ntouch '{}' 2>/dev/null && echo SHARED_TMP_WRITABLE || echo SHARED_TMP_DENIED", escape.display()),
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
    eprintln!("exec device start (seatbelt): {} ms", started_ms(&out));
    if python().is_some() {
        let out = w.check("ups", "abc");
        assert_eq!(response(&out), "ABC", "{out}");
        eprintln!("serve device start (seatbelt): {} ms", started_ms(&out));
    }
    // caller = ["project:ro"]: the project at its host path, read-only.
    // Seatbelt refuses the write with EPERM.
    let out = w.check("caller", "");
    let b = body(&out);
    assert!(b.contains("hello from the caller"), "{out}");
    assert!(b.contains("WRITE_FAILED"), "{out}");
    assert!(
        response(&out).contains("Operation not permitted"),
        "EPERM in the stderr lines of the envelope:\n{out}"
    );
    assert!(b.contains("harness=check"), "{out}");
    let project = std::fs::canonicalize(w.root.join("project")).unwrap();
    assert!(
        b.contains(&format!("mount={}", project.display())),
        "the mount is the host path of the project:\n{out}"
    );
    assert!(!w.root.join("project/written").exists());
    // ro = [FILE] reads it; the home is empty and the fixture home hidden;
    // TERM=dumb; TMPDIR is private; the shared /tmp is denied.
    let out = w.check("rof", "");
    let b = body(&out);
    assert!(b.contains("note-7f3a"), "{out}");
    assert!(b.contains("entries=0"), "an empty home:\n{out}");
    assert!(b.contains("SSH_HIDDEN"), "{out}");
    assert!(b.contains("HISTORY_HIDDEN"), "{out}");
    assert!(b.contains("term=dumb device=rof tmp="), "{out}");
    assert!(b.contains("SHARED_TMP_DENIED"), "{out}");
    assert!(!escape.exists());
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
        assert_eq!(v["status"], "ok");
        assert!(!a.contains("ping-9c1e"), "never content:\n{a}");
    }
    // deliver as = "file": the FIFO gives the value at each open, at a
    // host path of the instance folder.
    let out = w
        .paddock()
        .env("TEST_TOKEN", "tok-secret-1a2b")
        .args(["device", "check", "sec", "--send", ""])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", say(&out));
    let b = body(&text);
    assert!(b.contains("first=tok-secret-1a2b"), "{text}");
    assert!(b.contains("second=tok-secret-1a2b"), "{text}");
    assert!(b.contains("/secret/tok"), "{text}");
    assert!(
        !b.contains("/run/paddock/device/"),
        "a host path, not the Linux one:\n{text}"
    );
    // Nothing of the instances stays: no state file, no socket folder.
    for d in ["up", "caller", "rof", "sec"] {
        assert!(w.state_files(d).is_empty(), "{d}");
    }
    // The empty `devices` folder of the run may stay, as on Linux
    // (tests/devices_sandbox.rs): no instance folder is left in it.
    let devices = w.root.join("run/paddock/devices");
    let left: Vec<String> = std::fs::read_dir(&devices)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(left.is_empty(), "left in {}: {left:?}", devices.display());
}

#[test]
fn an_ssh_agent_credential_gives_a_filtering_agent_and_no_key_file() {
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
    // The host agent's socket belongs to this test's short runtime folder.
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
        "echo \"sock=$SSH_AUTH_SOCK\"\nssh-add -l\ncat \"$KEYFILE\" >/dev/null 2>&1 && echo KEY_FILE_READABLE || echo KEY_FILE_DENIED\nssh-add -D >/dev/null 2>&1 && echo DELETE_OK || echo DELETE_REFUSED",
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
    let _ = std::fs::remove_file(&sock);
    assert!(out.status.success(), "{}", say(&out));
    let b = body(&text);
    let socket = b
        .lines()
        .find_map(|line| line.strip_prefix("sock="))
        .unwrap();
    let run = w.root.join("run/paddock");
    let uid = unsafe { libc::getuid() };
    let fallback = paddock::sockpath::short_dir(
        &PathBuf::from(format!("/tmp/paddock-{uid}")),
        &run,
        "",
        paddock::sockpath::MACOS_MAX,
    );
    let parent = Path::new(socket).parent().and_then(Path::parent);
    assert!(
        parent == Some(run.as_path()) || parent == Some(fallback.as_path()),
        "{text}"
    );
    assert!(b.contains("-host/ssh-agent.sock"), "{text}");
    assert!(b.contains(&fingerprint), "the key is listed:\n{text}");
    assert!(
        b.contains("KEY_FILE_DENIED"),
        "the private key is not readable in the sandbox:\n{text}"
    );
    assert!(
        b.contains("DELETE_REFUSED"),
        "other agent requests fail:\n{text}"
    );
}

#[test]
fn an_instance_dies_with_its_paddock_process_and_logs_show_the_program() {
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.up]\nshape = \"exec\"\nenabled = true\ntimeout = 60\ncommand = [\"{dev}/up.sh\"]\n[devices.up.sandbox]\nro = [\"{dev}\"]\nmem = \"1G\"\ncpus = 1\n"
        ),
    )
    .unwrap();
    w.script(
        "up.sh",
        "echo LOGLINE-77 >&2\nread -r line\ncase \"$line\" in sleep) sleep 60 ;; *) echo \"$line\" | tr a-z A-Z ;; esac",
    );
    // Ten starts: the p50 and p95 for the record. mem and cpus are
    // ignored, the instance starts.
    let mut times = Vec::new();
    for _ in 0..10 {
        let out = w.check("up", "abc");
        // up.sh ends with `echo`: the body ends with its newline.
        assert_eq!(body(&out), "ABC\n", "{out}");
        times.push(started_ms(&out));
    }
    times.sort_unstable();
    eprintln!(
        "device sandbox start (seatbelt, exec, health answered): p50 {} ms, p95 {} ms",
        times[4], times[9]
    );
    // The log of a check stays, with the program's stderr.
    let logs = w.root.join("state/paddock/devices/log/up");
    assert_eq!(
        std::fs::read_dir(&logs).unwrap().count(),
        10,
        "one log per check"
    );
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
    // doctor names the ignored limits.
    let out = w.paddock().arg("doctor").output().unwrap();
    assert!(
        say(&out).contains("[devices.up.sandbox] mem and cpus are ignored: memory and cpu limits of devices need systemd. Not on macOS"),
        "{}",
        say(&out)
    );
    // A paddock process killed with SIGKILL takes its instance with it:
    // the parent pid check of the supervisor, within 2 s.
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
    let st = seatbelt_state(&w, "up");
    let pid = st["pid"].as_u64().unwrap();
    assert!(alive(pid));
    let sock = PathBuf::from(st["socket"].as_str().unwrap());
    // SAFETY: SIGKILL to our own child.
    unsafe { libc::kill(child.id() as i32, libc::SIGKILL) };
    let _ = child.wait();
    let gone = wait_for("the instance to die", Duration::from_secs(2), || {
        !alive(pid)
    });
    eprintln!("paddock killed to instance gone: {gone:?}");
    // The state file is stale now; the next start clears it and the
    // socket folders of the dead instance.
    assert_eq!(w.state_files("up").len(), 1);
    let out = w.check("up", "xyz");
    assert_eq!(body(&out), "XYZ\n", "{out}");
    let left = w.state_files("up");
    assert!(left.is_empty(), "old pid {pid}: {left:?}");
    assert!(!sock.exists(), "{}", sock.display());
}

#[test]
fn a_seatbelt_run_drives_the_instance_and_the_host_sees_it() {
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    let flags = w.root.join("project/flags");
    std::fs::create_dir_all(&flags).unwrap();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.up]\nshape = \"exec\"\nenabled = true\nmode = \"allow\"\ngrant = [\"all\"]\ncommand = [\"{dev}/up.sh\"]\n[devices.up.sandbox]\nro = [\"{dev}\"]\n\n[devices.ws]\nshape = \"exec\"\nenabled = true\nmode = \"allow\"\ngrant = [\"all\"]\nstart = \"with-sandbox\"\ncommand = [\"{dev}/up.sh\"]\n[devices.ws.sandbox]\nro = [\"{dev}\"]\n"
        ),
    )
    .unwrap();
    w.script("up.sh", "tr a-z A-Z");
    w.harness("agent", AGENT);
    let child = w.run_on("agent", "seatbelt");
    // first-request: `up` has no state file before the first request;
    // with-sandbox: `ws` has one from the start.
    wait_for("the ws instance", Duration::from_secs(60), || {
        !w.state_files("ws").is_empty()
    });
    wait_for("the first request", Duration::from_secs(60), || {
        flags.join("catalog1").exists() && !w.state_files("up").is_empty()
    });
    let catalog: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(flags.join("catalog1")).unwrap()).unwrap();
    let up = catalog["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "up")
        .unwrap();
    assert_eq!(up["run"], "sandbox", "{up}");
    assert_eq!(up["platform"], "macos", "{up}");
    let st = seatbelt_state(&w, "up");
    seatbelt_state(&w, "ws");
    let id = st["caller"].as_str().unwrap().to_string();
    let pid = st["pid"].as_u64().unwrap();
    // paddock ps: the device rows.
    let ps = w.ps();
    let row = ps["sandboxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap();
    let devs = row["devices"].as_array().unwrap();
    assert!(
        devs.iter().any(|d| d["device"] == "up" && d["pid"] == pid),
        "{row}"
    );
    assert!(devs.iter().any(|d| d["device"] == "ws"), "{row}");
    let text = String::from_utf8(w.paddock().arg("ps").output().unwrap().stdout).unwrap();
    assert!(
        text.contains(&format!("device up (sandbox, running 0m, pid {pid})")),
        "{text}"
    );
    let ls =
        String::from_utf8(w.paddock().args(["device", "ls"]).output().unwrap().stdout).unwrap();
    assert!(ls.contains(paddock::ids::short(&id)), "{ls}");
    // paddock device stop NAME --sandbox ID: the instance ends, the next
    // request starts a new one.
    let out = w
        .paddock()
        .args(["device", "stop", "up", "--sandbox", &id])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", say(&out));
    wait_for("the instance to stop", Duration::from_secs(5), || {
        !alive(pid)
    });
    assert!(w.state_files("up").is_empty());
    std::fs::write(flags.join("go"), "").unwrap();
    wait_for("the second request", Duration::from_secs(60), || {
        !w.state_files("up").is_empty()
    });
    let pid2 = w.state_files("up")[0]["pid"].as_u64().unwrap();
    assert_ne!(pid, pid2);
    // restart: a new pid, the device answers after it.
    let out = w
        .paddock()
        .args(["device", "restart", "up", "--sandbox", &id])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success()
            && text.contains("restarted the device up")
            && text.contains("started in"),
        "{}",
        say(&out)
    );
    let pid3 = w.state_files("up")[0]["pid"].as_u64().unwrap();
    assert_ne!(pid2, pid3);
    // The log of the instance is readable while it runs.
    let out = w
        .paddock()
        .args(["device", "logs", "up", "--sandbox", &id])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("start shape exec"));
    std::fs::write(flags.join("go2"), "").unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{}", say(&out));
    assert!(!text.contains("NOLIST"), "{text}");
    assert!(text.contains("FIRST ABC EXIT 0"), "{text}");
    assert!(text.contains("SECOND DEF EXIT 0"), "{text}");
    assert!(text.contains("THIRD GHI EXIT 0"), "{text}");
    // The instances end with the run.
    wait_for("the instances to end", Duration::from_secs(2), || {
        !alive(pid3) && w.state_files("up").is_empty() && w.state_files("ws").is_empty()
    });
    assert!(!w.root.join("run/paddock/devices/up").exists());
    // stats: three requests of the run.
    let stats = String::from_utf8(
        w.paddock()
            .args(["device", "stats", "up", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&stats).unwrap();
    assert_eq!(v["requests"], 3, "{stats}");
    assert_eq!(v["errors"], 0, "{stats}");
}
