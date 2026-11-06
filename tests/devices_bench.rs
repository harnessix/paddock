//! The bench of the devices. Ignored: run it with
//!
//! ```sh
//! cargo test --release --test devices_bench -- --ignored --nocapture
//! ```
//!
//! Two layers, each a table of p50 and p95 over ten runs (fifty for the
//! small requests), with the target next to each number.
//!
//! The contract layer runs on every machine: the supervisor `paddock
//! device-serve` as a plain child process, the broker's client
//! (`devices::program::Program`) as the caller, the relay through the
//! host side of `connect`. It measures what the contract costs without a
//! sandbox and without FUSE.
//!
//! The full layer needs bubblewrap and FUSE: the start of a device sandbox
//! through `paddock device check`, and an agent sandbox whose harness is
//! a Python program that times `/dev/paddock/NAME` reads: one exec and one
//! serve request, the first byte of a stream, 64 MiB collected and
//! streamed, 16 parallel readers. The RSS of the broker (the paddock
//! process of the run) is read from /proc while the stream runs.

mod common;

use common::device_world::{can_run, fuse_works, python, started_ms, wait_for, World};
use paddock::devices::config::DeviceDef;
use paddock::devices::program::{Collect, Program, Sink};
use paddock::devices::{Call, DevicesCfg};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MIB: usize = 1 << 20;
// 48 MiB: under the 64M cap of `max_response`, so the cap is not what is
// measured.
const BIG: usize = 48 * MIB;

struct Row {
    what: &'static str,
    target: &'static str,
    p50: String,
    p95: String,
    ok: Option<bool>,
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| v[((v.len() as f64 - 1.0) * q).round() as usize];
    (p(0.5), p(0.95))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn print_table(title: &str, rows: &[Row]) {
    println!("\n{title}");
    println!(
        "| {:<52} | {:<40} | {:>10} | {:>10} | {:<6} |",
        "measure", "target", "p50", "p95", "met"
    );
    println!(
        "| {} | {} | {} | {} | {} |",
        "-".repeat(52),
        "-".repeat(40),
        "-".repeat(10),
        "-".repeat(10),
        "-".repeat(6)
    );
    for r in rows {
        println!(
            "| {:<52} | {:<40} | {:>10} | {:>10} | {:<6} |",
            r.what,
            r.target,
            r.p50,
            r.p95,
            match r.ok {
                Some(true) => "yes",
                Some(false) => "NO",
                None => "-",
            }
        );
    }
}

/// A sink that counts and keeps nothing.
struct Count(usize);
impl Sink for Count {
    fn push(&mut self, bytes: &[u8]) {
        self.0 += bytes.len();
    }
}

/// A sink that notes when the first byte came.
struct First(Option<Instant>, usize);
impl Sink for First {
    fn push(&mut self, bytes: &[u8]) {
        if self.0.is_none() {
            self.0 = Some(Instant::now());
        }
        self.1 += bytes.len();
    }
}

fn def(name: &str, table: &str) -> DeviceDef {
    DevicesCfg::parse(&format!("[devices.{name}]\n{table}"))
        .unwrap()
        .user
        .remove(0)
}

fn rss_kib(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn ctxt_switches(pid: u32) -> u64 {
    let t = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    ["voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:"]
        .iter()
        .filter_map(|k| t.lines().find_map(|l| l.strip_prefix(k)))
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .sum()
}

/// A supervisor as a child process of the test.
struct Super {
    child: Child,
    socket: PathBuf,
}

impl Super {
    fn start(dir: &Path, name: &str, d: &DeviceDef, args: &[&str], command: &[&str]) -> Self {
        let sdir = dir.join(name);
        std::fs::create_dir_all(&sdir).unwrap();
        let socket = sdir.join("dev.sock");
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        // The supervisor touches no user folder; the guard of the tests
        // wants every paddock to get temporary ones all the same.
        c.env("XDG_DATA_HOME", sdir.join("data"))
            .env("XDG_STATE_HOME", sdir.join("state"))
            .env("XDG_CONFIG_HOME", sdir.join("cfg"))
            .env("XDG_RUNTIME_DIR", &sdir);
        c.arg("device-serve")
            .arg("--socket")
            .arg(&socket)
            .args(["--shape", d.shape.name()])
            .args(["--timeout", &d.limits.timeout.to_string()])
            .args(["--max-request", &d.limits.max_request.to_string()])
            .args(["--max-response", &d.limits.max_response.to_string()])
            .args(["--concurrency", &d.limits.concurrency.to_string()])
            .arg("--log")
            .arg(sdir.join("log"));
        if d.stream {
            c.arg("--stream");
        }
        c.args(args).arg("--").args(command);
        let child = c
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let me = Self { child, socket };
        let p = me.program();
        let end = Instant::now() + Duration::from_secs(10);
        while p.health(Duration::from_millis(200)).is_err() {
            assert!(Instant::now() < end, "the supervisor {name} did not answer");
        }
        me
    }

    fn program(&self) -> Program {
        Program::connect(&self.socket).unwrap()
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Super {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn call() -> Call {
    Call::for_test("bench", Duration::from_secs(120))
}

fn one(p: &Program, d: &DeviceDef, req: &[u8]) -> (Duration, usize) {
    let mut s = Count(0);
    let t = Instant::now();
    p.call(&call(), d, req, &mut s).unwrap();
    (t.elapsed(), s.0)
}

/// A TCP echo on the loopback that copies everything back.
fn echo() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut r = s.try_clone().unwrap();
                let mut w = s;
                let mut buf = vec![0u8; 256 * 1024];
                while let Ok(n) = r.read(&mut buf) {
                    if n == 0 || w.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

#[test]
#[ignore]
fn contract_path_without_a_sandbox() {
    let tmp = common::tempdir();
    let dir = tmp.path().to_path_buf();
    let mut rows = Vec::new();
    let plain = |table: &str| def("d", table);

    // Supervisor start, exec cat, health answered.
    let d_cat = plain("shape = \"exec\"\ncommand = [\"cat\"]\n");
    let mut starts = Vec::new();
    for i in 0..10 {
        let t = Instant::now();
        let s = Super::start(&dir, &format!("s{i}"), &d_cat, &[], &["cat"]);
        starts.push(ms(t.elapsed()));
        drop(s);
    }
    let (a, b) = stats(starts);
    rows.push(Row {
        what: "supervisor start (plain process, exec, health answered)",
        target: "sandbox start: p50 < 250 ms, p95 < 500 ms",
        p50: format!("{a:.1} ms"),
        p95: format!("{b:.1} ms"),
        ok: Some(a < 250.0 && b < 500.0),
    });

    // One exec request (cat) after the start, and a plain fork of cat.
    let s = Super::start(&dir, "cat", &d_cat, &[], &["cat"]);
    let p = s.program();
    let mut v = Vec::new();
    for _ in 0..50 {
        v.push(ms(one(&p, &d_cat, b"hello").0));
    }
    let (a, b) = stats(v);
    let mut f = Vec::new();
    for _ in 0..50 {
        let t = Instant::now();
        let mut c = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(b"hello").unwrap();
        let _ = c.wait_with_output().unwrap();
        f.push(ms(t.elapsed()));
    }
    let (fa, _) = stats(f);
    rows.push(Row {
        what: "one exec request (cat), through the contract",
        target: "p50 < 15 ms, overhead above fork < 5 ms",
        p50: format!("{a:.2} ms"),
        p95: format!("{b:.2} ms"),
        ok: Some(a < 15.0 && a - fa < 5.0),
    });
    rows.push(Row {
        what: "  a plain fork of cat, for the overhead",
        target: "-",
        p50: format!("{fa:.2} ms"),
        p95: String::new(),
        ok: None,
    });
    drop(s);

    // One serve request: the python script of the example.
    if let Some(py) = python() {
        let script = dir.join("upper.py");
        std::fs::write(&script, paddock::devices::program::EXAMPLE).unwrap();
        let d = plain("shape = \"serve\"\ncommand = [\"x\"]\n");
        let s = Super::start(
            &dir,
            "upper",
            &d,
            &[],
            &[py, "-I", &script.display().to_string()],
        );
        let p = s.program();
        let mut v = Vec::new();
        for _ in 0..50 {
            let (t, n) = one(&p, &d, b"hello");
            assert_eq!(n, 5);
            v.push(ms(t));
        }
        let (a, b) = stats(v);
        rows.push(Row {
            what: "one serve request (python upper.py)",
            target: "p50 < 4 ms",
            p50: format!("{a:.2} ms"),
            p95: format!("{b:.2} ms"),
            ok: Some(a < 4.0),
        });
    } else {
        println!("no /usr/bin/python3: the serve request is not measured");
    }

    // First byte of a stream: the program sleeps 200 ms, writes, sleeps.
    let d = plain("shape = \"exec\"\nstream = true\ncommand = [\"x\"]\n");
    let s = Super::start(
        &dir,
        "first",
        &d,
        &[],
        &["sh", "-c", "sleep 0.2; echo first; sleep 0.2; echo last"],
    );
    let p = s.program();
    let mut v = Vec::new();
    for _ in 0..10 {
        let mut sink = First(None, 0);
        let t = Instant::now();
        p.call(&call(), &d, b"", &mut sink).unwrap();
        let first = sink.0.unwrap() - t;
        v.push(ms(first.saturating_sub(Duration::from_millis(200))));
    }
    let (a, b) = stats(v);
    rows.push(Row {
        what: "first byte of a stream, after the program wrote it",
        target: "< 20 ms",
        p50: format!("{a:.1} ms"),
        p95: format!("{b:.1} ms"),
        ok: Some(b < 20.0),
    });
    drop(s);

    // 64 MiB collected, then streamed with the RSS of the client.
    let big = format!("head -c {BIG} /dev/zero");
    for stream in [false, true] {
        let d = plain(&format!(
            "shape = \"exec\"\nstream = {stream}\nmax_response = \"64M\"\ntimeout = 120\ncommand = [\"x\"]\n"
        ));
        let s = Super::start(
            &dir,
            if stream { "bigs" } else { "bigc" },
            &d,
            &[],
            &["sh", "-c", &big],
        );
        let p = s.program();
        let mut v = Vec::new();
        let before = rss_kib(std::process::id());
        let mut peak = before;
        for _ in 0..10 {
            let t = Instant::now();
            let n = if stream {
                let mut sink = Count(0);
                p.call(&call(), &d, b"", &mut sink).unwrap();
                sink.0
            } else {
                let mut sink = Collect::default();
                p.call(&call(), &d, b"", &mut sink).unwrap();
                sink.0.len()
            };
            // A collected exec response carries the envelope before the body.
            assert!((BIG..BIG + 1024).contains(&n), "{n}");
            v.push(BIG as f64 / MIB as f64 / t.elapsed().as_secs_f64());
            peak = peak.max(rss_kib(std::process::id()));
        }
        let (a, b) = stats(v.clone());
        let (lo, _) = (v.iter().cloned().fold(f64::MAX, f64::min), 0);
        let mem = (peak.saturating_sub(before)) as f64 / 1024.0;
        rows.push(Row {
            what: if stream {
                "48 MiB response, streamed (MiB/s; p95 is the slowest)"
            } else {
                "48 MiB response, collected (MiB/s; p95 is the slowest)"
            },
            target: if stream {
                "> 200 MiB/s, client memory < 16 MiB above idle"
            } else {
                "> 200 MiB/s"
            },
            p50: format!("{a:.0} MiB/s"),
            p95: format!("{lo:.0} MiB/s"),
            ok: Some(a > 200.0 && (!stream || mem < 16.0)),
        });
        let _ = b;
        if stream {
            rows.push(Row {
                what: "  client RSS above idle while streaming",
                target: "< 16 MiB",
                p50: format!("{mem:.1} MiB"),
                p95: String::new(),
                ok: Some(mem < 16.0),
            });
        }
    }

    // The connect relay: supervisor listener, host side, TCP echo.
    {
        use paddock::devices::sandbox::connect::{start, Who};
        let port = echo();
        let entries =
            vec![
                paddock::devices::config::Connect::parse(&format!("2222:127.0.0.1:{port}"), true)
                    .unwrap(),
            ];
        let cdir = dir.join("connect");
        let env = paddock::env::HostEnv::from_parts(
            dir.join("home"),
            "bench".into(),
            1000,
            1000,
            [(
                "XDG_STATE_HOME".to_string(),
                dir.join("state").display().to_string(),
            )]
            .into_iter()
            .collect(),
        );
        let who = Who {
            env,
            device: "relay".into(),
            caller: "bench".into(),
        };
        let _listeners = start(who, &cdir, &entries).unwrap();
        let d = plain("shape = \"exec\"\ncommand = [\"cat\"]\n");
        let sock = cdir.join("0.sock").display().to_string();
        let relay_port = free_port();
        let s = Super::start(
            &dir,
            "relay",
            &d,
            &["--connect", &format!("{relay_port}:{sock}")],
            &["cat"],
        );
        let mut v = Vec::new();
        for _ in 0..10 {
            let t = Instant::now();
            let mut w = std::net::TcpStream::connect(("127.0.0.1", relay_port)).unwrap();
            let mut r = w.try_clone().unwrap();
            let reader = std::thread::spawn(move || {
                let mut buf = vec![0u8; 256 * 1024];
                let mut got = 0;
                while got < BIG {
                    let n = r.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    got += n;
                }
                got
            });
            let chunk = vec![1u8; MIB];
            for _ in 0..48 {
                w.write_all(&chunk).unwrap();
            }
            let got = reader.join().unwrap();
            assert_eq!(got, BIG);
            v.push(BIG as f64 / MIB as f64 / t.elapsed().as_secs_f64());
        }
        let (a, _) = stats(v.clone());
        let lo = v.iter().cloned().fold(f64::MAX, f64::min);
        rows.push(Row {
            what: "connect relay, one connection, 48 MiB there and back",
            target: "> 100 MiB/s",
            p50: format!("{a:.0} MiB/s"),
            p95: format!("{lo:.0} MiB/s"),
            ok: Some(a > 100.0),
        });
        drop(s);
    }

    // Idle: RSS and wakeups of a supervisor that gets no request.
    {
        let s = Super::start(&dir, "idle", &d_cat, &[], &["cat"]);
        std::thread::sleep(Duration::from_secs(1));
        let c0 = ctxt_switches(s.pid());
        let t = Instant::now();
        std::thread::sleep(Duration::from_secs(3));
        let wake = (ctxt_switches(s.pid()) - c0) as f64 / t.elapsed().as_secs_f64();
        let rss = rss_kib(s.pid()) as f64 / 1024.0;
        rows.push(Row {
            what: "idle supervisor: RSS (p50) and wakeups per second (p95)",
            target: "RSS < 10 MiB, 0 wakeups/s",
            p50: format!("{rss:.1} MiB"),
            p95: format!("{wake:.1}/s"),
            ok: Some(rss < 10.0 && wake < 1.0),
        });
    }

    // 16 parallel requests of one instance: none waits more than the
    // longest single request plus 50 ms.
    {
        let d = plain("shape = \"exec\"\nconcurrency = 16\ncommand = [\"x\"]\n");
        let s = Super::start(&dir, "par", &d, &[], &["sh", "-c", "sleep 0.2; cat"]);
        let p = Arc::new(s.program());
        let single = {
            let mut v = Vec::new();
            for _ in 0..5 {
                v.push(one(&p, &d, b"x").0);
            }
            v.into_iter().max().unwrap()
        };
        let mut v = Vec::new();
        for _ in 0..10 {
            let times = Arc::new(Mutex::new(Vec::new()));
            let hs: Vec<_> = (0..16)
                .map(|_| {
                    let (p, d, times) = (p.clone(), d.clone(), times.clone());
                    std::thread::spawn(move || {
                        let t = one(&p, &d, b"x").0;
                        times.lock().unwrap().push(t);
                    })
                })
                .collect();
            for h in hs {
                h.join().unwrap();
            }
            let slowest = *times.lock().unwrap().iter().max().unwrap();
            v.push(ms(slowest.saturating_sub(single)));
        }
        let (a, b) = stats(v);
        rows.push(Row {
            what: "16 parallel requests: wait above the longest single one",
            target: "< 50 ms",
            p50: format!("{a:.1} ms"),
            p95: format!("{b:.1} ms"),
            ok: Some(b < 50.0),
        });
    }
    print_table(
        "devices bench, the contract path (supervisor as a plain process, the broker's client, no sandbox, no FUSE)",
        &rows,
    );
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The harness of the full layer: times the reads of `/dev/paddock` and
/// writes the numbers as JSON into `flags/result.json`. `BIG` and `N` are
/// filled in.
const BENCH_PY: &str = r#"
import json, os, time, threading
BIG = __BIG__
def req(name, data, stream=False):
    fd = os.open("/dev/paddock/" + name, os.O_RDWR)
    t = time.perf_counter()
    os.write(fd, data)
    first = None
    got = 0
    while True:
        b = os.read(fd, 1 << 20)
        if not b:
            break
        if first is None:
            first = time.perf_counter()
        got += len(b)
    os.close(fd)
    return time.perf_counter() - t, first - t if first else None, got
r = {}
def series(name, data, n):
    v = []
    for _ in range(n):
        v.append(req(name, data)[0] * 1000)
    return v
r["exec_ms"] = series("cat", b"hello", 50)
if os.path.exists("/dev/paddock/upper"):
    r["serve_ms"] = series("upper", b"hello", 50)
r["first_ms"] = []
for _ in range(10):
    t, first, _ = req("first", b"")
    r["first_ms"].append(max(0.0, first * 1000 - 200))
for name, key in (("bigc", "collected_mibs"), ("bigs", "streamed_mibs")):
    v = []
    if key == "streamed_mibs":
        open("flags/stream-start", "w").close()
    for _ in range(10):
        t, _, got = req(name, b"")
        assert got == BIG, got
        v.append(BIG / (1 << 20) / t)
    if key == "streamed_mibs":
        open("flags/stream-end", "w").close()
    r[key] = v
single = max(req("par", b"x")[0] for _ in range(5))
waits = []
for _ in range(10):
    times = []
    def one():
        times.append(req("par", b"x")[0])
    ts = [threading.Thread(target=one) for _ in range(16)]
    for t in ts: t.start()
    for t in ts: t.join()
    waits.append(max(0.0, (max(times) - single) * 1000))
r["parallel_wait_ms"] = waits
json.dump(r, open("flags/result.json", "w"))
"#;

#[test]
#[ignore]
fn full_path_through_bwrap_and_fuse() {
    if !can_run() {
        return;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return;
    }
    let Some(py) = python() else {
        eprintln!("skipped: no /usr/bin/python3");
        return;
    };
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("dev/upper.py"),
        paddock::devices::program::EXAMPLE,
    )
    .unwrap();
    w.script("first.sh", "sleep 0.2; echo first; sleep 0.2; echo last");
    w.script("big.sh", &format!("head -c {BIG} /dev/zero"));
    w.script("par.sh", "sleep 0.2; cat");
    let common = "enabled = true\nmode = \"allow\"\ngrant = [\"all\"]\n";
    let table = |name: &str, body: &str| {
        format!("[devices.{name}]\n{common}{body}[devices.{name}.sandbox]\nro = [\"{dev}\"]\n\n")
    };
    let cfg = table("cat", "shape = \"exec\"\ncommand = [\"cat\"]\n")
        + &table("upper", &format!("shape = \"serve\"\ncommand = [\"{py}\", \"-I\", \"{dev}/upper.py\"]\n"))
        + &table("first", &format!("shape = \"exec\"\nstream = true\ncommand = [\"{dev}/first.sh\"]\n"))
        + &table("bigc", &format!("shape = \"exec\"\nmax_response = \"64M\"\ntimeout = 120\ncommand = [\"{dev}/big.sh\"]\n"))
        + &table("bigs", &format!("shape = \"exec\"\nstream = true\nmax_response = \"64M\"\ntimeout = 120\ncommand = [\"{dev}/big.sh\"]\n"))
        + &table("par", &format!("shape = \"exec\"\nconcurrency = 16\ncommand = [\"{dev}/par.sh\"]\n"));
    std::fs::write(w.root.join("cfg/paddock/config.toml"), cfg).unwrap();
    let mut rows = Vec::new();
    // The start of a device sandbox: ten checks.
    let mut starts = Vec::new();
    for _ in 0..10 {
        starts.push(started_ms(&w.check("cat", "abc")) as f64);
    }
    let (a, b) = stats(starts);
    rows.push(Row {
        what: "start of a device sandbox (bwrap, exec, health answered)",
        target: "p50 < 250 ms, p95 < 500 ms",
        p50: format!("{a:.0} ms"),
        p95: format!("{b:.0} ms"),
        ok: Some(a < 250.0 && b < 500.0),
    });
    // The agent sandbox runs the Python bench.
    let flags = w.root.join("project/flags");
    std::fs::create_dir_all(&flags).unwrap();
    std::fs::write(
        w.root.join("project/bench.py"),
        BENCH_PY.replace("__BIG__", &BIG.to_string()),
    )
    .unwrap();
    w.harness(
        "bench",
        "if [ ! -e /dev/paddock/cat ]; then echo NODEV; exit 0; fi\n/usr/bin/python3 -I bench.py",
    );
    let child = w.run("bench");
    let pid = child.id();
    // The broker's RSS while the stream runs: sampled from the host.
    let before = rss_kib(pid);
    wait_for("the stream to start", Duration::from_secs(600), || {
        flags.join("stream-start").exists()
    });
    let mut peak = before;
    while !flags.join("stream-end").exists() {
        peak = peak.max(rss_kib(pid));
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if text.contains("NODEV") {
        eprintln!("skipped: the sandbox has no /dev/paddock here");
        return;
    }
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(flags.join("result.json")).unwrap()).unwrap();
    let series = |k: &str| -> Option<Vec<f64>> {
        r[k].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
    };
    let mut push = |what: &'static str,
                    target: &'static str,
                    key: &str,
                    unit: &str,
                    ok: &dyn Fn(f64, f64) -> bool,
                    low_is_p95: bool| {
        if let Some(v) = series(key) {
            let (a, b) = stats(v.clone());
            let b = if low_is_p95 {
                v.iter().cloned().fold(f64::MAX, f64::min)
            } else {
                b
            };
            rows.push(Row {
                what,
                target,
                p50: format!("{a:.2} {unit}"),
                p95: format!("{b:.2} {unit}"),
                ok: Some(ok(a, b)),
            });
        }
    };
    push(
        "one exec request (cat) through FUSE",
        "p50 < 15 ms",
        "exec_ms",
        "ms",
        &|a, _| a < 15.0,
        false,
    );
    push(
        "one serve request (python upper.py) through FUSE",
        "p50 < 4 ms",
        "serve_ms",
        "ms",
        &|a, _| a < 4.0,
        false,
    );
    push(
        "first byte of a stream, after the program wrote it",
        "< 20 ms",
        "first_ms",
        "ms",
        &|_, b| b < 20.0,
        false,
    );
    push(
        "48 MiB response, collected (p95 is the slowest)",
        "> 200 MiB/s",
        "collected_mibs",
        "MiB/s",
        &|a, _| a > 200.0,
        true,
    );
    push(
        "48 MiB response, streamed (p95 is the slowest)",
        "> 200 MiB/s",
        "streamed_mibs",
        "MiB/s",
        &|a, _| a > 200.0,
        true,
    );
    push(
        "16 parallel requests: wait above the longest single one",
        "< 50 ms",
        "parallel_wait_ms",
        "ms",
        &|_, b| b < 50.0,
        false,
    );
    let mem = peak.saturating_sub(before) as f64 / 1024.0;
    rows.push(Row {
        what: "  broker RSS above idle while streaming 48 MiB",
        target: "< 16 MiB",
        p50: format!("{mem:.1} MiB"),
        p95: String::new(),
        ok: Some(mem < 16.0),
    });
    print_table(
        "devices bench, the full path (bwrap device sandbox, FUSE, the broker of a run)",
        &rows,
    );
}
