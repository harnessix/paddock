//! `OutOfProcess` with a test `InstanceSource` that starts the supervisor
//! in a thread of this process, in a temporary folder, with no isolation.

use super::*;
use crate::broker::Answer;
use crate::devices::config::Shape;
use crate::devices::program::EXAMPLE;
use crate::devices::supervise::tests::{def, opts, python, script, TEST_SERVER};
use crate::devices::supervise::{Options, Running, Supervisor};
use crate::devices::user::tests::ctx;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

struct TestSource {
    opts: Options,
    log: PathBuf,
    running: Mutex<Option<Running>>,
}

impl TestSource {
    fn new(dir: &std::path::Path, mut opts: Options) -> Arc<Self> {
        let log = dir.join("log");
        opts.log = Some(log.clone());
        Arc::new(Self {
            opts,
            log,
            running: Mutex::new(None),
        })
    }
}

impl InstanceSource for TestSource {
    fn socket(&self, _ctx: &DeviceCtx) -> Result<PathBuf, String> {
        let mut g = self.running.lock().unwrap();
        if g.is_none() {
            let r = Supervisor::start(self.opts.clone())
                .map_err(|e| format!("{e:#}"))?
                .spawn();
            *g = Some(r);
        }
        Ok(g.as_ref().unwrap().socket().to_path_buf())
    }
    fn instance(&self) -> String {
        "test".into()
    }
    fn log_path(&self) -> Option<PathBuf> {
        Some(self.log.clone())
    }
}

fn device(
    dir: &std::path::Path,
    d: DeviceDef,
    o: Options,
    answer: Answer,
) -> (OutOfProcess, DeviceCtx) {
    let (ctx, _) = ctx(dir, answer);
    let src = TestSource::new(dir, o);
    (OutOfProcess::new(d, ctx.clone(), src, GrantHit::NoKey), ctx)
}

fn call() -> Call {
    Call::for_test("box1", Duration::from_secs(60))
}

fn audit(ctx: &DeviceCtx) -> String {
    std::fs::read_to_string(crate::broker::audit_file(&ctx.env)).unwrap_or_default()
}

#[test]
fn the_request_is_stdin_and_never_an_argument() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "echo \"argc=$# args=$*\"\ncat\necho oops >&2",
    );
    let o = opts(dir.path(), Shape::Exec, &[s, "-c".into(), ".".into()]);
    let (d, ctx) = device(
        dir.path(),
        def("exec", "max_body = \"1K\"\n"),
        o,
        Answer::Session,
    );
    let out = d.call(&call(), b"--evil; rm -rf /\n$(id)").unwrap();
    let out = String::from_utf8(out).unwrap();
    let (head, body) = out.split_once("\n\n").unwrap();
    assert_eq!(head, "status: exit 0\nstderr: oops");
    assert_eq!(body.lines().next(), Some("argc=2 args=-c ."));
    assert_eq!(body.lines().nth(1), Some("--evil; rm -rf /"));
    let a = audit(&ctx);
    for want in [
        "\"run\":\"sandbox\"",
        "\"shape\":\"exec\"",
        "\"instance\":\"test\"",
        "\"status\":\"exit 0\"",
        "\"http\":200",
        "bytes in,",
        "bytes out",
        "\"grant\":\"all (no grant key)\"",
    ] {
        assert!(a.contains(want), "{want} missing in {a}");
    }
    assert!(!a.contains("evil"), "{a}");
    let details = d.details().unwrap();
    assert_eq!(details["instance"], "running");
    assert_eq!(details["shape"], "exec");
    // The program runs on this host: the catalog says which system.
    assert_eq!(details["platform"], std::env::consts::OS);
}

#[test]
fn audit_and_log_never_hold_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "cat; echo MARKER-err-7f3a >&2");
    let o = opts(dir.path(), Shape::Exec, &[s]);
    let (d, ctx) = device(dir.path(), def("exec", ""), o, Answer::Session);
    let out = d.call(&call(), b"MARKER-req-7f3a").unwrap();
    assert!(String::from_utf8_lossy(&out).contains("MARKER-req-7f3a"));
    let a = audit(&ctx);
    assert!(!a.contains("MARKER-req"), "{a}");
    let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
    assert!(!log.contains("MARKER-req"), "{log}");
    assert!(
        log.contains("MARKER-err"),
        "stderr is the device log\n{log}"
    );
}

#[test]
fn a_declined_device_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mark = dir.path().join("ran");
    let s = script(dir.path(), "tool", &format!("touch {}", mark.display()));
    let o = opts(dir.path(), Shape::Exec, &[s]);
    let (d, ctx) = device(dir.path(), def("exec", ""), o, Answer::Deny);
    assert_eq!(d.call(&call(), b"x").unwrap_err(), DevError::Denied);
    assert!(!mark.exists());
    assert!(audit(&ctx).contains("\"status\":\"denied\""));
    assert_eq!(d.details().unwrap()["instance"], "stopped", "never started");
}

#[test]
fn errors_of_the_program_reach_the_reader_and_the_audit() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(dir.path(), "tool", "code=$(cat); exit $code");
    let o = opts(dir.path(), Shape::Exec, &[s]);
    let (d, ctx) = device(dir.path(), def("exec", ""), o, Answer::Session);
    assert_eq!(d.call(&call(), b"65").unwrap_err(), DevError::Invalid);
    assert_eq!(d.call(&call(), b"75").unwrap_err(), DevError::Waiting);
    assert_eq!(d.call(&call(), b"77").unwrap_err(), DevError::Denied);
    let a = audit(&ctx);
    for want in [
        "\"status\":\"rejected\"",
        "\"status\":\"waiting\"",
        "\"status\":\"declined\"",
        "\"http\":403",
    ] {
        assert!(a.contains(want), "{want} missing in {a}");
    }
}

#[test]
fn paddock_cost_reaches_the_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "server.py", TEST_SERVER) else {
        return;
    };
    let o = opts(dir.path(), Shape::Serve, &cmd);
    let d = def("serve", "cost = 0.001\nbudget = 1.0\n");
    let (dev, ctx) = device(dir.path(), d, o, Answer::Session);
    assert_eq!(dev.call(&call(), b"cost").unwrap(), b"ok");
    // The declared cost is 0.001; the program said 0.002: the larger.
    let l = crate::devices::ledger::Ledger::open(&crate::devices::ledger::path(&ctx.env)).unwrap();
    assert_eq!(l.spent("box1", "tool").unwrap(), 2000);
    assert_eq!(dev.call(&call(), b"plain").unwrap(), b"PLAIN");
    assert_eq!(
        l.spent("box1", "tool").unwrap(),
        3000,
        "the declared cost without a header"
    );
    // A request the program never got costs nothing.
    assert_eq!(dev.call(&call(), b"wait").unwrap_err(), DevError::Waiting);
    assert_eq!(l.spent("box1", "tool").unwrap(), 3000);
    let details = dev.details().unwrap();
    assert_eq!(
        details["examples"][0], "wait",
        "the catalog of the program is merged"
    );
    assert_eq!(dev.info().request, "a word");
    assert_eq!(details["cost"], "0.001", "the table wins for cost");
}

#[test]
fn concurrency_two_runs_at_most_two_at_once_and_none_fails() {
    let dir = tempfile::tempdir().unwrap();
    let marks = dir.path().join("marks");
    std::fs::create_dir(&marks).unwrap();
    let s = script(
        dir.path(),
        "tool",
        &format!(
            "touch {m}/$$\nsleep 0.2\nrm {m}/$$\necho done",
            m = marks.display()
        ),
    );
    let mut o = opts(dir.path(), Shape::Exec, &[s]);
    o.concurrency = 2;
    let (d, ctx) = device(
        dir.path(),
        def("exec", "concurrency = 2\n"),
        o,
        Answer::Session,
    );
    let d = Arc::new(d);
    // Warm up: the first request starts the instance and shows the card.
    d.call(&call(), b"").unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (s2, m2) = (stop.clone(), marks.clone());
    let counter = std::thread::spawn(move || {
        let mut max = 0;
        while !s2.load(Ordering::SeqCst) {
            let n = std::fs::read_dir(&m2).map(|r| r.count()).unwrap_or(0);
            max = max.max(n);
            std::thread::sleep(Duration::from_millis(2));
        }
        max
    });
    let t = Instant::now();
    let handles: Vec<_> = (0..32)
        .map(|_| {
            let d = d.clone();
            std::thread::spawn(move || d.call(&call(), b""))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let took = t.elapsed();
    stop.store(true, Ordering::SeqCst);
    let max = counter.join().unwrap();
    eprintln!("32 requests, concurrency 2: {took:?}, at most {max} at once");
    assert!(results.iter().all(|r| r.is_ok()), "{results:?}");
    assert!(max <= 2, "{max} ran at once");
    assert!(
        took < Duration::from_millis(32 * 200 / 2 + 1000),
        "{took:?}"
    );
    let a = audit(&ctx);
    assert_eq!(a.matches("\"status\":\"exit 0\"").count(), 33);
}

#[test]
fn three_crashes_mark_the_instance_failed_in_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "server.py", TEST_SERVER) else {
        return;
    };
    let o = opts(dir.path(), Shape::Serve, &cmd);
    let (d, ctx) = device(dir.path(), def("serve", ""), o, Answer::Session);
    for _ in 0..3 {
        assert_eq!(d.call(&call(), b"die").unwrap_err(), DevError::Failed);
    }
    // The health check before the next request sees `failed`, or the
    // request does.
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        assert_eq!(d.call(&call(), b"hello").unwrap_err(), DevError::Failed);
        if d.details().unwrap()["instance"] == "failed" {
            break;
        }
        assert!(Instant::now() < end, "never failed");
        std::thread::sleep(Duration::from_millis(200));
    }
    let details = d.details().unwrap();
    let fix = details["how_to_fix"].as_str().unwrap();
    assert!(
        fix.contains("three times") && fix.contains("Its log is"),
        "{fix}"
    );
    assert_eq!(d.call(&call(), b"hello").unwrap_err(), DevError::Failed);
    let a = audit(&ctx);
    assert!(a.contains("\"status\":\"503\""), "{a}");
    assert!(a.contains("\"status\":\"failed\""), "{a}");
}

#[test]
fn the_example_script_runs_under_both_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let Some(cmd) = python(dir.path(), "upper.py", EXAMPLE) else {
        return;
    };
    let (d, _) = device(
        dir.path(),
        def("exec", ""),
        opts(dir.path(), Shape::Exec, &cmd),
        Answer::Session,
    );
    assert_eq!(d.call(&call(), b"abc").unwrap(), b"status: exit 0\n\nABC");
    drop(d);
    let dir2 = tempfile::tempdir().unwrap();
    let (d, _) = device(
        dir2.path(),
        def("serve", ""),
        opts(dir.path(), Shape::Serve, &cmd),
        Answer::Session,
    );
    assert_eq!(d.call(&call(), b"abc").unwrap(), b"ABC");
    let details = d.details().unwrap();
    assert_eq!(details["shape"], "serve");
    assert_eq!(details["instance"], "running");
}

/// Records each piece with the time it came.
struct Stamps(Vec<(Instant, Vec<u8>)>);

impl crate::devices::Sink for Stamps {
    fn push(&mut self, bytes: &[u8]) {
        self.0.push((Instant::now(), bytes.to_vec()));
    }
}

#[test]
fn a_stream_device_delivers_each_piece_as_the_program_writes_it() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "printf a\nsleep 0.2\nprintf b\n[ \"$(cat)\" = fail ] && exit 3\nexit 0",
    );
    let mut o = opts(dir.path(), Shape::Exec, &[s]);
    o.stream = true;
    let (d, ctx) = device(
        dir.path(),
        def("exec", "stream = true\n"),
        o,
        Answer::Session,
    );
    assert!(d.info().stream);
    let mut sink = Stamps(Vec::new());
    d.call_stream(&call(), b"ok", &mut sink).unwrap();
    let pieces: Vec<&[u8]> = sink.0.iter().map(|(_, p)| p.as_slice()).collect();
    assert_eq!(pieces, [&b"a"[..], &b"b"[..]], "raw output, piece by piece");
    let gap = sink.0[1].0.duration_since(sink.0[0].0);
    assert!(gap >= Duration::from_millis(150), "{gap:?}");
    // A non-zero exit after the first piece: the pieces, then the error.
    let mut sink = Stamps(Vec::new());
    assert_eq!(
        d.call_stream(&call(), b"fail", &mut sink).unwrap_err(),
        DevError::Failed
    );
    let pieces: Vec<&[u8]> = sink.0.iter().map(|(_, p)| p.as_slice()).collect();
    assert_eq!(pieces, [&b"a"[..], &b"b"[..]]);
    // The plain call collects the same stream.
    assert_eq!(d.call(&call(), b"ok").unwrap(), b"ab");
    let a = audit(&ctx);
    assert_eq!(a.matches("2 bytes out").count(), 2, "{a}");
    assert!(a.contains("\"status\":\"exit 3\""), "{a}");
}

/// `caller = ["project:ro"]`: the broker names the mount of the caller's
/// project in a header, and the supervisor gives it to the program as
/// `PADDOCK_CALLER_MOUNT`. Without the profile entry, the program sees
/// no mount.
#[test]
fn the_caller_mount_reaches_the_program_of_a_device_that_mounts_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "echo \"mount=$PADDOCK_CALLER_MOUNT caller=$PADDOCK_CALLER\"",
    );
    let o = opts(dir.path(), Shape::Exec, std::slice::from_ref(&s));
    let (d, ctx) = device(
        dir.path(),
        def(
            "exec",
            "[devices.tool.sandbox]\ncaller = [\"project:ro\"]\n",
        ),
        o,
        Answer::Session,
    );
    let out = String::from_utf8(d.call(&call(), b"").unwrap()).unwrap();
    // bwrap mounts the project at one place; on macOS the program reads
    // it at its real host path (/tmp is /private/tmp there).
    let at = crate::devices::program::caller_mount(cfg!(target_os = "macos"), &ctx.project);
    assert!(
        out.ends_with(&format!("\n\nmount={at} caller=box1\n")),
        "{out}"
    );
    let dir = tempfile::tempdir().unwrap();
    let s = script(
        dir.path(),
        "tool",
        "echo \"mount=$PADDOCK_CALLER_MOUNT caller=$PADDOCK_CALLER\"",
    );
    let o = opts(dir.path(), Shape::Exec, &[s]);
    let (d, _) = device(dir.path(), def("exec", ""), o, Answer::Session);
    let out = String::from_utf8(d.call(&call(), b"").unwrap()).unwrap();
    assert!(out.ends_with("\n\nmount= caller=box1\n"), "{out}");
}

/// A `kind = "command"` table of 0.2.1 through the device sandbox source
/// (`DeviceDef::build`), with the supervisor in a thread: the three tests
/// of command.rs, with the same assertions.
mod migration {
    use super::*;
    use crate::devices::grant::GrantHit;
    use crate::devices::DevicesCfg;

    fn dev(
        dir: &std::path::Path,
        cmd: &str,
        extra: &str,
        answer: Answer,
    ) -> (Arc<dyn Device>, DeviceCtx) {
        let cfg = DevicesCfg::parse(&format!(
            "[devices.tool]\nkind = \"command\"\nenabled = true\ncommand = {cmd}\n{extra}"
        ))
        .unwrap();
        let (ctx, _) = ctx(dir, answer);
        (cfg.user[0].build(&ctx, Ok(GrantHit::NoKey)), ctx)
    }

    #[test]
    fn the_request_is_stdin_and_never_an_argument() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(
            dir.path(),
            "fake-tool",
            "echo \"argc=$# args=$*\"\ncat\necho oops >&2",
        );
        let (d, ctx) = dev(
            dir.path(),
            &format!("[\"{s}\", \"-c\", \".\"]"),
            "max_body = \"1K\"",
            Answer::Session,
        );
        let out = d.call(&call(), b"--evil; rm -rf /\n$(id)").unwrap();
        let out = String::from_utf8(out).unwrap();
        let (head, body) = out.split_once("\n\n").unwrap();
        assert_eq!(head, "status: exit 0\nstderr: oops");
        let mut lines = body.lines();
        assert_eq!(lines.next(), Some("argc=2 args=-c ."));
        assert_eq!(lines.next(), Some("--evil; rm -rf /"));
        assert_eq!(lines.next(), Some("$(id)"));
        let a = audit(&ctx);
        assert!(
            a.contains("bytes in") && a.contains("\"status\":\"exit 0\""),
            "{a}"
        );
        assert!(
            a.contains("\"run\":\"sandbox\"")
                && a.contains("\"grant\":\"all (no grant key)\"")
                && a.contains("\"instance\":\"box1\""),
            "{a}"
        );
        assert!(!a.contains("evil"), "{a}");
        assert_eq!(d.details().unwrap()["kind"], "command");
    }

    #[test]
    fn exit_codes_stderr_and_limits_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let fail = script(
            dir.path(),
            "fail",
            "for i in 1 2 3 4 5 6 7; do echo \"line $i\" >&2; done\nexit 3",
        );
        let loud = script(dir.path(), "loud", "head -c 5000 /dev/zero");
        let (d, _) = dev(dir.path(), &format!("[\"{fail}\"]"), "", Answer::Session);
        let out = String::from_utf8(d.call(&call(), b"").unwrap()).unwrap();
        assert!(out.starts_with("status: exit 3\nstderr: line 1\n"), "{out}");
        assert!(
            out.contains("stderr: line 5\nstderr: (2 more lines)\n\n"),
            "{out}"
        );
        assert!(!out.contains("line 6"), "{out}");
        let (d, _) = dev(
            dir.path(),
            &format!("[\"{loud}\"]"),
            "max_body = \"1K\"",
            Answer::Session,
        );
        assert_eq!(d.call(&call(), b"").unwrap_err(), DevError::TooLarge);
        let mut big = vec![b'x'; 1025];
        big.push(b'\n');
        assert_eq!(d.call(&call(), &big).unwrap_err(), DevError::TooLarge);
        let (d, _) = dev(dir.path(), "[\"no-such-tool\"]", "", Answer::Session);
        let out = String::from_utf8(d.call(&call(), b"").unwrap()).unwrap();
        assert!(
            out.starts_with("status: error\nmessage: no-such-tool is not on the PATH"),
            "{out}"
        );
    }

    #[test]
    fn the_timeout_stops_a_sleeping_command() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(dir.path(), "sleeper", "echo started\nsleep 30\necho never");
        let (d, _) = dev(
            dir.path(),
            &format!("[\"{s}\"]"),
            "timeout = 1",
            Answer::Session,
        );
        let t = Instant::now();
        let out = String::from_utf8(d.call(&call(), b"").unwrap()).unwrap();
        assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
        assert!(
            out.starts_with("status: timeout\nmessage: the command did not finish in 1 seconds"),
            "{out}"
        );
        assert!(!out.contains("never"), "{out}");
    }

    #[test]
    fn a_declined_device_runs_nothing_and_mode_allow_skips_the_card() {
        let dir = tempfile::tempdir().unwrap();
        let mark = dir.path().join("ran");
        let s = script(dir.path(), "marker", &format!("touch {}", mark.display()));
        let (d, c0) = dev(dir.path(), &format!("[\"{s}\"]"), "", Answer::Deny);
        assert_eq!(d.call(&call(), b"x").unwrap_err(), DevError::Denied);
        assert!(!mark.exists());
        assert_eq!(d.details().unwrap()["instance"], "stopped", "never started");
        drop(c0);
        let dir = tempfile::tempdir().unwrap();
        let up = script(dir.path(), "up", "tr a-z A-Z");
        let cfg = DevicesCfg::parse(&format!(
            "[devices.tool]\nshape = \"exec\"\nenabled = true\ncommand = [\"{up}\"]\nmode = \"allow\"\ngrant = [\"pasture:workers\"]\n"
        ))
        .unwrap();
        let (c, approver) = ctx(dir.path(), Answer::Deny);
        let d = cfg.user[0].build(&c, Ok(GrantHit::Pasture("workers".into())));
        let out = String::from_utf8(d.call(&call(), b"hi").unwrap()).unwrap();
        assert!(out.ends_with("\n\nHI"), "{out}");
        assert_eq!(approver.1.load(Ordering::SeqCst), 0, "no card");
        let a = audit(&c);
        assert!(
            a.contains("\"decision\":\"allowed by grant pasture workers\""),
            "{a}"
        );
    }

    /// `stream = true` on a `kind = "command"` table: the device sandbox
    /// starts its supervisor with `--stream`, and each piece reaches the
    /// sink as the command writes it.
    #[test]
    fn a_streamed_command_delivers_each_piece_through_the_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(
            dir.path(),
            "tool",
            "printf a\nsleep 0.2\nprintf b\n[ \"$(cat)\" = fail ] && exit 3\nexit 0",
        );
        let (d, ctx) = dev(
            dir.path(),
            &format!("[\"{s}\"]"),
            "stream = true",
            Answer::Session,
        );
        let info = d.info();
        assert!(info.stream);
        assert!(
            info.response
                .starts_with("the standard output as the command writes it"),
            "{}",
            info.response
        );
        assert_eq!(d.details().unwrap()["stream"], true);
        let mut sink = Stamps(Vec::new());
        d.call_stream(&call(), b"ok", &mut sink).unwrap();
        let pieces: Vec<&[u8]> = sink.0.iter().map(|(_, p)| p.as_slice()).collect();
        assert_eq!(pieces, [&b"a"[..], &b"b"[..]], "raw output, piece by piece");
        let gap = sink.0[1].0.duration_since(sink.0[0].0);
        assert!(gap >= Duration::from_millis(150), "{gap:?}");
        // A non-zero exit after the first piece: the pieces, then the error.
        let mut sink = Stamps(Vec::new());
        assert_eq!(
            d.call_stream(&call(), b"fail", &mut sink).unwrap_err(),
            DevError::Failed
        );
        let pieces: Vec<&[u8]> = sink.0.iter().map(|(_, p)| p.as_slice()).collect();
        assert_eq!(pieces, [&b"a"[..], &b"b"[..]]);
        // The plain call collects the same stream, without an envelope.
        assert_eq!(d.call(&call(), b"ok").unwrap(), b"ab");
        let a = audit(&ctx);
        assert_eq!(a.matches("2 bytes out").count(), 2, "{a}");
        assert!(
            a.contains("\"status\":\"exit 3\"")
                && a.contains("\"run\":\"sandbox\"")
                && a.contains("\"instance\":\"box1\""),
            "{a}"
        );
    }
}
