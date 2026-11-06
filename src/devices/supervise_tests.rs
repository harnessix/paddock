//! The supervisor in a thread of the test process, with scripts in a
//! temporary folder and no isolation, through the `Program` client.

use super::*;
use crate::devices::config::{DeviceDef, Shape};
use crate::devices::program::{Collect, Failure, Outcome, Program, EXAMPLE};
use crate::devices::{Call, DevError, DevicesCfg};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

pub(crate) fn script(dir: &Path, name: &str, body: &str) -> String {
    let f = dir.join(name);
    std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    f.display().to_string()
}

pub(crate) fn python(dir: &Path, name: &str, body: &str) -> Option<Vec<String>> {
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("skipped: no /usr/bin/python3");
        return None;
    }
    let f = dir.join(name);
    std::fs::write(&f, body).unwrap();
    Some(vec![
        "/usr/bin/python3".into(),
        "-I".into(),
        f.display().to_string(),
    ])
}

pub(crate) fn def(shape: &str, extra: &str) -> DeviceDef {
    DevicesCfg::parse(&format!(
        "[devices.tool]\nshape = \"{shape}\"\nenabled = true\ncommand = [\"x\"]\n{extra}"
    ))
    .unwrap()
    .user
    .remove(0)
}

/// Start the supervisor on `dir/dev.sock` with a log in `dir/log`.
pub(crate) fn start(dir: &Path, mut opts: Options) -> Running {
    opts.log = Some(dir.join("log"));
    let t = Instant::now();
    let r = Supervisor::start(opts).unwrap().spawn();
    let p = Program::connect(r.socket()).unwrap();
    if let Err(e) = p.health(Duration::from_secs(10)) {
        eprintln!("health: {e}");
        eprintln!(
            "log:\n{}",
            std::fs::read_to_string(dir.join("log")).unwrap_or_default()
        );
    }
    eprintln!("supervisor start to health: {:?}", t.elapsed());
    r
}

pub(crate) fn opts(dir: &Path, shape: Shape, command: &[String]) -> Options {
    Options::new(&dir.join("dev.sock"), shape, command.to_vec())
}

fn call(p: &Program, d: &DeviceDef, req: &[u8]) -> Result<(Outcome, Vec<u8>), Failure> {
    let call = Call::for_test("box1", Duration::from_secs(60));
    let mut sink = Collect::default();
    p.call(&call, d, req, &mut sink).map(|o| (o, sink.0))
}

fn program(r: &Running) -> Program {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = crate::devices::user::tests::ctx(dir.path(), crate::broker::Answer::Session);
    std::mem::forget(dir);
    Program::connect(r.socket())
        .unwrap()
        .with_caller(crate::devices::program::Caller::new("tool", &ctx))
}

fn body_text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn group_alive(pid: i32) -> bool {
    // SAFETY: a null signal to a process group.
    unsafe { libc::kill(-pid, 0) == 0 }
}

fn wait_for(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + within;
    while !f() {
        assert!(
            Instant::now() < end,
            "{what} did not happen within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn exec_the_request_is_stdin_and_never_an_argument() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "echo \"argc=$# args=$*\"\ncat\necho oops >&2",
    );
    let r = start(
        dir.path(),
        opts(dir.path(), Shape::Exec, &[s, "-c".into(), ".".into()]),
    );
    let p = program(&r);
    let d = def("exec", "max_body = \"1K\"\n");
    let t = Instant::now();
    let (o, body) = call(&p, &d, b"--evil; rm -rf /\n$(id)").unwrap();
    eprintln!("one exec round trip: {:?}", t.elapsed());
    assert_eq!((o.http, o.status.as_str()), (200, "exit 0"));
    let out = body_text(&body);
    let (head, rest) = out.split_once("\n\n").unwrap();
    assert_eq!(head, "status: exit 0\nstderr: oops");
    let mut lines = rest.lines();
    assert_eq!(lines.next(), Some("argc=2 args=-c ."));
    assert_eq!(lines.next(), Some("--evil; rm -rf /"));
    assert_eq!(lines.next(), Some("$(id)"));
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(log.contains("] oops"), "{log}");
    assert!(log.contains("caller box1 POST /v1/call"), "{log}");
    assert!(!log.contains("evil"), "{log}");
}

#[test]
fn exec_exit_codes_map_to_statuses() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "code=$(cat)\nfor i in 1 2 3 4 5 6 7; do echo \"line $i\" >&2; done\nexit $code",
    );
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &[s]));
    let p = program(&r);
    let d = def("exec", "");
    let (o, body) = call(&p, &d, b"0").unwrap();
    assert_eq!(o.status, "exit 0");
    assert!(body_text(&body).starts_with("status: exit 0\nstderr: line 1\n"));
    let fail = |code: &[u8]| call(&p, &d, code).unwrap_err();
    assert_eq!(fail(b"65").error, DevError::Invalid);
    assert_eq!(fail(b"75").error, DevError::Waiting);
    let f = fail(b"77");
    assert_eq!((f.error, f.http), (DevError::Denied, Some(403)));
    let (o, body) = call(&p, &d, b"3").unwrap();
    assert_eq!((o.http, o.status.as_str()), (200, "exit 3"));
    let out = body_text(&body);
    assert!(out.starts_with("status: exit 3\nstderr: line 1\n"), "{out}");
    assert!(
        out.contains("stderr: line 5\nstderr: (2 more lines)\n\n"),
        "{out}"
    );
    assert!(!out.contains("line 6"), "{out}");
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(log.contains("] line 7"), "{log}");
    assert!(log.contains("status 403"), "{log}");
}

#[test]
fn exec_a_program_that_never_reads_stdin_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "echo done; exit 0");
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &[s]));
    let p = program(&r);
    let d = def("exec", "max_request = \"2M\"\n");
    let big = vec![b'x'; 1 << 20];
    let t = Instant::now();
    let (o, body) = call(&p, &d, &big).unwrap();
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(o.status, "exit 0");
    assert!(body_text(&body).ends_with("\n\ndone\n"));
}

#[test]
fn exec_the_timeout_kills_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let s = script(
        dir.path(),
        "tool",
        &format!(
            "echo $$ > {}\necho started\nsleep 30\necho never",
            pidfile.display()
        ),
    );
    let mut o = opts(dir.path(), Shape::Exec, &[s]);
    o.timeout = 1;
    let r = start(dir.path(), o);
    let p = program(&r);
    let d = def("exec", "timeout = 1\n");
    let t = Instant::now();
    let (o, body) = call(&p, &d, b"").unwrap();
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(o.status, "timeout");
    let out = body_text(&body);
    assert!(
        out.starts_with("status: timeout\nmessage: the command did not finish in 1 seconds"),
        "{out}"
    );
    assert!(
        out.ends_with("\n\nstarted\n") && !out.contains("never"),
        "{out}"
    );
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_for(
        "the process group to end",
        Duration::from_millis(500),
        || !group_alive(pid),
    );
}

#[test]
fn exec_too_much_output_is_too_large() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "head -c 1025 /dev/zero");
    let mut o = opts(dir.path(), Shape::Exec, std::slice::from_ref(&s));
    o.max_response = 1024;
    let r = start(dir.path(), o);
    let p = program(&r);
    let d = def("exec", "max_response = \"1K\"\n");
    let f = call(&p, &d, b"").unwrap_err();
    assert_eq!(
        (f.error, f.status.as_str()),
        (DevError::TooLarge, "too large")
    );
    let s2 = script(dir.path(), "tool", "head -c 1024 /dev/zero");
    assert_eq!(s, s2);
    assert_eq!(
        call(&p, &d, b"").unwrap().0.bytes,
        1024 + "status: exit 0\n\n".len()
    );
}

#[test]
fn exec_a_cancelled_call_kills_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("pid");
    let s = script(
        dir.path(),
        "tool",
        &format!("echo $$ > {}\nsleep 30", pidfile.display()),
    );
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &[s]));
    // The broker drops the connection: a raw client that hangs up.
    {
        let mut c = UnixStream::connect(r.socket()).unwrap();
        c.write_all(b"POST /v1/call HTTP/1.1\r\nHost: paddock\r\nContent-Length: 2\r\n\r\nhi")
            .unwrap();
        wait_for("the child to start", Duration::from_secs(5), || {
            pidfile.exists()
        });
    }
    let t = Instant::now();
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_for(
        "the child to die after the hang-up",
        Duration::from_millis(300),
        || !group_alive(pid),
    );
    eprintln!("cancel to kill: {:?}", t.elapsed());
    // The client path: a Call whose cancel flag flips.
    std::fs::remove_file(&pidfile).unwrap();
    let p = program(&r);
    let d = def("exec", "");
    let cancel = Arc::new(AtomicBool::new(false));
    let call = Call::with_cancel("box1", Duration::from_secs(60), cancel.clone());
    let c2 = cancel.clone();
    let pf = pidfile.clone();
    std::thread::spawn(move || {
        wait_for("the child to start", Duration::from_secs(5), || pf.exists());
        c2.store(true, Ordering::SeqCst);
    });
    let mut sink = Collect::default();
    let f = p.call(&call, &d, b"", &mut sink).unwrap_err();
    assert_eq!(
        (f.error, f.status.as_str()),
        (DevError::Cancelled, "cancelled")
    );
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_for(
        "the child to die after the cancel",
        Duration::from_millis(400),
        || !group_alive(pid),
    );
}

#[test]
fn exec_the_working_folder_is_empty_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let s = script(
        dir.path(),
        "tool",
        &format!(
            "pwd > {o}\nls -A | wc -l >> {o}\ntouch leftover",
            o = out.display()
        ),
    );
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &[s]));
    let p = program(&r);
    call(&p, &def("exec", ""), b"").unwrap();
    let text = std::fs::read_to_string(&out).unwrap();
    let mut lines = text.lines();
    let work = PathBuf::from(lines.next().unwrap());
    assert!(
        work.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("req-"),
        "{work:?}"
    );
    assert_eq!(lines.next().unwrap().trim(), "0", "empty at the start");
    assert!(!work.exists(), "{work:?} is gone after the request");
    let parent = work.parent().unwrap();
    assert!(
        parent
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("paddock-device-"),
        "one private parent per supervisor: {parent:?}"
    );
    assert!(parent.is_dir());
    drop(r);
    assert!(!parent.exists(), "the parent goes with the supervisor");
}

#[test]
fn exec_sets_the_caller_variables_and_nothing_of_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "env | sort");
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &[s]));
    let p = program(&r);
    let (_, body) = call(&p, &def("exec", "timeout = 7\n"), b"SECRET=1").unwrap();
    let out = body_text(&body);
    for want in [
        "PADDOCK_CALLER=box1",
        "PADDOCK_CALLER_HARNESS=fake",
        "PADDOCK_DEVICE=tool",
        "PADDOCK_DEVICE_PROTOCOL=1",
        "PADDOCK_REQUEST=1",
        "PADDOCK_DEADLINE=7",
        "PADDOCK_MAX_RESPONSE=1048576",
        "TERM=dumb",
    ] {
        assert!(out.lines().any(|l| l == want), "{want} missing in\n{out}");
    }
    assert!(out.contains("\nPADDOCK_CALLER_PROJECT=/"), "{out}");
    assert!(out.contains("\nPATH="), "{out}");
    assert!(!out.contains("SECRET"), "{out}");
    assert!(!out.contains("Paddock-"), "{out}");
    assert!(
        !out.contains("PADDOCK_CALLER_MOUNT"),
        "no mount without a profile\n{out}"
    );
}

#[test]
fn exec_stream_gives_raw_stdout_and_fails_after_bytes_on_a_bad_exit() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "code=$(cat)\n[ \"$code\" = 65 ] && exit 65\nprintf abc\nexit $code",
    );
    let mut o = opts(dir.path(), Shape::Exec, &[s]);
    o.stream = true;
    let r = start(dir.path(), o);
    let p = program(&r);
    let d = def("exec", "stream = true\n");
    let (o, body) = call(&p, &d, b"0").unwrap();
    assert_eq!(body, b"abc");
    assert_eq!(o.status, "200");
    let call_ = Call::for_test("box1", Duration::from_secs(60));
    let mut sink = Collect::default();
    let f = p.call(&call_, &d, b"3", &mut sink).unwrap_err();
    assert_eq!(sink.0, b"abc", "the bytes came first");
    assert_eq!((f.error, f.status.as_str()), (DevError::Failed, "exit 3"));
    assert_eq!(call(&p, &d, b"65").unwrap_err().error, DevError::Invalid);
}

#[test]
fn exec_the_example_script_upper_cases() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "upper.py", EXAMPLE) else {
        return;
    };
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &cmd));
    let p = program(&r);
    let (o, body) = call(&p, &def("exec", ""), b"abc").unwrap();
    assert_eq!(o.status, "exit 0");
    assert_eq!(body, b"status: exit 0\n\nABC");
}

#[test]
fn serve_the_example_script_answers_and_health_is_waited_for() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "upper.py", EXAMPLE) else {
        return;
    };
    // The program takes a second to bind: the health wait covers it.
    let slow = script(
        dir.path(),
        "slow",
        &format!("sleep 1\nexec {}", cmd.join(" ")),
    );
    let t = Instant::now();
    let r = start(dir.path(), opts(dir.path(), Shape::Serve, &[slow]));
    let p = program(&r);
    p.health(Duration::from_secs(10)).unwrap();
    assert!(t.elapsed() >= Duration::from_secs(1), "{:?}", t.elapsed());
    let d = def("serve", "");
    let t = Instant::now();
    let (o, body) = call(&p, &d, b"abc").unwrap();
    eprintln!("one serve round trip: {:?}", t.elapsed());
    assert_eq!((o.http, o.status.as_str(), o.bytes), (200, "200", 3));
    assert_eq!(body, b"ABC");
    assert!(p.catalog().is_none(), "the example has no catalog");
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(log.contains("program ready"), "{log}");
}

/// A serve program for the tests: the body picks the answer.
pub(crate) const TEST_SERVER: &str = r#"
import os, sys, socketserver, json
from http.server import BaseHTTPRequestHandler
sock = os.environ["PADDOCK_DEVICE_SOCKET"]
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/v1/catalog":
            body = json.dumps({"request": "a word", "response": "the word, changed", "examples": ["wait", "cost"], "cost": 0.001}).encode()
            self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body); return
        self.send_response(200)
        if os.environ.get("TEST_MIN"): self.send_header("Paddock-Protocol-Min", os.environ["TEST_MIN"])
        self.send_header("Content-Length", "0"); self.end_headers()
    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(n)
        sys.stderr.write("got %d bytes from %s\n" % (n, self.headers.get("Paddock-Caller")))
        if body == b"die":
            os._exit(1)
        if body == b"wait":
            self.send_response(202); self.send_header("Content-Length", "0"); self.end_headers(); return
        if body == b"cost":
            self.send_response(200); self.send_header("Paddock-Cost", "0.002"); self.send_header("Content-Length", "2"); self.end_headers(); self.wfile.write(b"ok"); return
        if body == b"chunked":
            self.send_response(200); self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
            for piece in [b"one,", b"two,", b"three"]:
                self.wfile.write(b"%x\r\n%s\r\n" % (len(piece), piece))
            self.wfile.write(b"0\r\n\r\n"); return
        if body == b"big":
            out = b"x" * 1025
            self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out); return
        if body == b"slow":
            import time; time.sleep(3)
        out = body.upper()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
class S(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
S(sock, H).serve_forever()
"#;

#[test]
fn serve_statuses_cost_and_chunked_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "server.py", TEST_SERVER) else {
        return;
    };
    let mut o = opts(dir.path(), Shape::Serve, &cmd);
    o.max_response = 1024;
    let r = start(dir.path(), o);
    let p = program(&r);
    let d = def("serve", "max_response = \"1K\"\n");
    assert_eq!(call(&p, &d, b"wait").unwrap_err().error, DevError::Waiting);
    let (o, body) = call(&p, &d, b"cost").unwrap();
    assert_eq!((o.cost, body.as_slice()), (Some(2000), &b"ok"[..]));
    let (o, body) = call(&p, &d, b"chunked").unwrap();
    assert_eq!(body, b"one,two,three");
    assert_eq!(o.bytes, 13);
    assert_eq!(call(&p, &d, b"big").unwrap_err().error, DevError::TooLarge);
    let c = p.catalog().unwrap();
    assert_eq!(c["request"], "a word");
    assert_eq!(c["examples"][1], "cost");
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(log.contains("[serve] got 4 bytes from box1"), "{log}");
}

#[test]
fn serve_a_child_that_exits_mid_request_gives_503_and_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "server.py", TEST_SERVER) else {
        return;
    };
    let r = start(dir.path(), opts(dir.path(), Shape::Serve, &cmd));
    let p = program(&r);
    let d = def("serve", "");
    let t = Instant::now();
    let f = call(&p, &d, b"die").unwrap_err();
    assert_eq!((f.error, f.status.as_str()), (DevError::Failed, "503"));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    // The next request waits for the restart (1 s backoff) and works.
    let (o, body) = call(&p, &d, b"again").unwrap();
    assert_eq!((o.http, body.as_slice()), (200, &b"AGAIN"[..]));
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(
        log.contains("program exited") && log.contains("program restarting"),
        "{log}"
    );
}

#[test]
fn serve_three_crashes_within_a_minute_make_it_failed() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "server.py", TEST_SERVER) else {
        return;
    };
    let r = start(dir.path(), opts(dir.path(), Shape::Serve, &cmd));
    let p = program(&r);
    let d = def("serve", "");
    for _ in 0..3 {
        assert_eq!(call(&p, &d, b"die").unwrap_err().status, "503");
        // Wait for the restart or the final state.
        let _ = p.health(Duration::from_secs(10));
    }
    let f = call(&p, &d, b"hello").unwrap_err();
    assert_eq!(
        (f.error, f.status.as_str(), f.http),
        (DevError::Failed, "failed", Some(503))
    );
    let e = p.health(Duration::from_secs(1)).unwrap_err();
    assert!(e.contains("(failed)"), "{e}");
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(log.contains("exited 3 times within a minute"), "{log}");
}

#[test]
fn serve_protocol_min_is_refused_with_the_exact_message() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "server.py", TEST_SERVER) else {
        return;
    };
    let wrap = script(
        dir.path(),
        "min2",
        &format!("TEST_MIN=2 exec {}", cmd.join(" ")),
    );
    let r = start(dir.path(), opts(dir.path(), Shape::Serve, &[wrap]));
    let p = program(&r);
    assert_eq!(
        p.health(Duration::from_secs(10)).unwrap_err(),
        "the device program needs protocol 2; this paddock speaks 1"
    );
}

#[test]
fn concurrency_beyond_the_queue_is_busy() {
    // Many open connections: macOS starts with 256 files (nofile.rs).
    crate::platform::nofile::raise();
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "sleep 3");
    let mut o = opts(dir.path(), Shape::Exec, &[s]);
    o.concurrency = 2;
    let r = start(dir.path(), o);
    let mut conns = Vec::new();
    for _ in 0..(2 + http::QUEUE + 20) {
        let mut c = UnixStream::connect(r.socket()).unwrap();
        c.write_all(b"POST /v1/call HTTP/1.1\r\nHost: paddock\r\nPaddock-Deadline: 30\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
        c.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        conns.push(c);
    }
    std::thread::sleep(Duration::from_millis(300));
    // The client maps it.
    let p = program(&r);
    let f = call(&p, &def("exec", ""), b"").unwrap_err();
    assert_eq!((f.error, f.status.as_str()), (DevError::Busy, "busy"));
    let mut busy = 0;
    for c in &mut conns {
        let mut buf = [0u8; 64];
        // macOS fails setsockopt with EINVAL once the supervisor closed a
        // busy connection; the read then returns at once, so the 500 ms
        // timeout of the connect loop is enough.
        let _ = c.set_read_timeout(Some(Duration::from_millis(100)));
        if let Ok(n) = c.read(&mut buf) {
            if buf[..n].starts_with(b"HTTP/1.1 429") {
                busy += 1;
            }
        }
    }
    assert_eq!(busy, 20, "2 run, {} wait, the rest are busy", http::QUEUE);
    drop(conns);
}

#[test]
fn relay_round_trips_eight_mib() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("0.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        for c in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut c = c;
                let mut c2 = c.try_clone().unwrap();
                let _ = std::io::copy(&mut c, &mut c2);
            });
        }
    });
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let s = script(dir.path(), "tool", "cat");
    let mut o = opts(dir.path(), Shape::Exec, &[s]);
    o.connect = vec![(port, sock)];
    let r = start(dir.path(), o);
    let mut tcp = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let data: Vec<u8> = (0..(8u32 << 20)).map(|i| (i % 251) as u8).collect();
    let d2 = data.clone();
    let mut w = tcp.try_clone().unwrap();
    let writer = std::thread::spawn(move || {
        w.write_all(&d2).unwrap();
        w.shutdown(std::net::Shutdown::Write).unwrap();
    });
    let t = Instant::now();
    let mut back = Vec::new();
    tcp.read_to_end(&mut back).unwrap();
    writer.join().unwrap();
    eprintln!("relay 8 MiB round trip: {:?}", t.elapsed());
    assert_eq!(back.len(), data.len());
    assert!(back == data);
    drop(r);
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(
        log.contains(&format!(
            "connect {port}: closed, 8388608 bytes up, 8388608 bytes down"
        )),
        "{log}"
    );
}

#[test]
fn the_log_rotates_once_at_its_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    let log = Log::open_with_cap(Some(&path), 300).unwrap();
    for i in 0..40 {
        log.line(&format!("line {i} with some text to fill the file"));
    }
    let old = dir.path().join("log.1");
    assert!(old.exists(), "rotated once");
    let a = std::fs::metadata(&path).unwrap().len();
    let b = std::fs::metadata(&old).unwrap().len();
    assert!(a <= 300 && b <= 300, "{a} {b}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("line 39"), "{text}");
    assert!(
        !text.contains("line 0 "),
        "the first lines are in .1 or gone"
    );
    assert_eq!(LOG_CAP, 10 << 20);
}

#[test]
fn health_and_catalog_of_an_exec_program() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "cat");
    let r = start(dir.path(), opts(dir.path(), Shape::Exec, &[s]));
    let p = program(&r);
    p.health(Duration::from_secs(2)).unwrap();
    assert!(p.catalog().is_none(), "404 for exec");
    let mut c = UnixStream::connect(r.socket()).unwrap();
    c.write_all(b"GET /nothing HTTP/1.1\r\nHost: paddock\r\n\r\n")
        .unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 404"), "{out}");
    let mut c = UnixStream::connect(r.socket()).unwrap();
    c.write_all(b"POST /v1/call HTTP/1.1\r\nHost: paddock\r\nContent-Length: 99999999\r\n\r\n")
        .unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");
    // The client maps 413 before the body went out.
    let d = def("exec", "max_request = \"2M\"\n");
    let f = call(&p, &d, &vec![b'x'; (1 << 20) + 1]).unwrap_err();
    assert_eq!(f.error, DevError::RequestTooLarge);
}
