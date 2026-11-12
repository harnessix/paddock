//! `paddock device-serve` as the binary: the start, one request, the log
//! and the death with its parent. The in-process tests of the supervisor
//! are in src/devices/supervise_tests.rs.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let f = dir.join(name);
    std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    f.display().to_string()
}

fn alive(pid: i32) -> bool {
    // SAFETY: a null signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_until(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < within,
            "{what} did not happen within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// One raw request over the socket: the response text.
fn post(sock: &Path, body: &[u8]) -> String {
    let mut c = UnixStream::connect(sock).unwrap();
    c.write_all(
        format!(
            "POST /v1/call HTTP/1.1\r\nHost: paddock\r\nPaddock-Caller: box1\r\nPaddock-Deadline: 30\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )
    .unwrap();
    c.write_all(body).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut out = String::new();
    let _ = c.read_to_string(&mut out);
    out
}

fn health(sock: &Path) -> bool {
    let Ok(mut c) = UnixStream::connect(sock) else {
        return false;
    };
    let _ = c.write_all(b"GET /v1/health HTTP/1.1\r\nHost: paddock\r\n\r\n");
    c.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut out = String::new();
    let _ = c.read_to_string(&mut out);
    out.starts_with("HTTP/1.1 200")
}

#[test]
fn the_binary_serves_a_request_and_dies_with_its_parent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let sock = dir.path().join("dev.sock");
    let log = dir.path().join("log");
    let pidfile = dir.path().join("child-pid");
    let tool = script(
        dir.path(),
        "tool",
        &format!(
            "echo $$ > {}\nread -r line\ncase \"$line\" in sleep) sleep 30 ;; *) echo \"got $line\" ;; esac",
            pidfile.display()
        ),
    );
    let supervisor_pid = dir.path().join("supervisor-pid");
    // A parent that can be killed: the supervisor is its child.
    let mut parent = Command::new("sh")
        .arg("-c")
        .arg("\"$0\" device-serve --socket \"$1\" --shape exec --timeout 5 --log \"$2\" -- \"$3\" & echo $! > \"$4\"; wait")
        .arg(env!("CARGO_BIN_EXE_paddock"))
        .arg(&sock)
        .arg(&log)
        .arg(&tool)
        .arg(&supervisor_pid)
        .env("HOME", dir.path().join("home"))
        .env("XDG_CONFIG_HOME", dir.path().join("cfg"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("XDG_RUNTIME_DIR", dir.path().join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    wait_until("the health answer", Duration::from_secs(10), || {
        health(&sock)
    });
    let out = post(&sock, b"hello MARKER-9c1e\n");
    assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
    assert!(
        out.ends_with("\r\n\r\nstatus: exit 0\n\ngot hello MARKER-9c1e\n"),
        "{out}"
    );
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("caller box1 POST /v1/call"), "{text}");
    assert!(!text.contains("MARKER"), "no body in the log\n{text}");
    // A request that runs long, then the parent dies.
    std::fs::remove_file(&pidfile).unwrap();
    let s2 = sock.clone();
    let long = std::thread::spawn(move || post(&s2, b"sleep\n"));
    wait_until("the child to start", Duration::from_secs(5), || {
        pidfile.exists()
    });
    let child: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let sup: i32 = std::fs::read_to_string(&supervisor_pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(child) && alive(sup));
    // SAFETY: kill our own parent shell.
    unsafe { libc::kill(parent.id() as i32, libc::SIGKILL) };
    let _ = parent.wait();
    wait_until(
        "the supervisor and its child to end",
        Duration::from_secs(2),
        || !alive(sup) && !alive(child),
    );
    let out = long.join().unwrap();
    assert!(
        !out.contains("got"),
        "the long request never finished: {out:?}"
    );
    assert!(!sock.exists() || UnixStream::connect(&sock).is_err());
}

#[test]
fn the_binary_stops_on_sigterm_and_removes_its_socket() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let sock = dir.path().join("dev.sock");
    let tool = script(dir.path(), "tool", "cat");
    let mut sup = Command::new(env!("CARGO_BIN_EXE_paddock"))
        .args(["device-serve", "--socket"])
        .arg(&sock)
        .args(["--shape", "exec", "--"])
        .arg(&tool)
        .env("HOME", dir.path().join("home"))
        .env("XDG_CONFIG_HOME", dir.path().join("cfg"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_CACHE_HOME", dir.path().join("cache"))
        .env("XDG_RUNTIME_DIR", dir.path().join("run"))
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until("the health answer", Duration::from_secs(10), || {
        health(&sock)
    });
    assert!(post(&sock, b"abc").ends_with("status: exit 0\n\nabc"));
    // SAFETY: SIGTERM to our child.
    unsafe { libc::kill(sup.id() as i32, libc::SIGTERM) };
    let t = Instant::now();
    let status = sup.wait().unwrap();
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert!(status.success(), "{status}");
    assert!(!sock.exists(), "the socket is removed at the stop");
    let mut err = String::new();
    sup.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(
        err.contains("req ") && err.contains(" stop"),
        "the log went to stderr:\n{err}"
    );
}
