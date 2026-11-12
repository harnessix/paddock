//! The device scenarios that need real bubblewrap and FUSE, as functions:
//! tests/devices_sandbox_runs.rs, tests/devices_shared.rs and
//! tests/devices_stress.rs run them, the last one twice at once in
//! parallel threads. Each call makes a world of its own (a temporary
//! home, state and runtime folder), so two calls share nothing but the
//! host: the user manager, `/dev/fuse` and the load.
//!
//! Every wait here is for the real condition, with a deadline wide enough
//! for a busy host, and names what it waited for. The sandboxes write
//! their answers under a temporary name and rename them, so the test
//! never reads a half-written file. Each function returns false when the
//! host cannot run the scenario, after it said why.

use super::device_world::{alive, can_run, fuse_works, wait_for, World};
use paddock::devices::{Call, DevError, Device, DeviceFs, Info, Limits, Sink};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a wait for a sandbox start, a request or a stop may take on
/// a busy host.
pub const WIDE: Duration = Duration::from_secs(120);

/// The fake harness of the agent-sandbox scenario: a request, then a wait
/// for a flag file, then a request again, and a flag of its own after
/// the second, so the host side knows the answer came before it restarts
/// the instance. `$F` is the flags folder.
const AGENT: &str = r#"
F=flags
if [ ! -e /dev/paddock/up ]; then echo NODEV; exit 0; fi
echo "FIRST $(printf abc | paddock-host device up - | tail -n 1) EXIT $?"
cat /dev/paddock/.catalog > "$F/catalog1.tmp" 2>/dev/null && mv "$F/catalog1.tmp" "$F/catalog1"
while [ ! -e "$F/go" ]; do sleep 0.1; done
echo "SECOND $(printf def | paddock-host device up - | tail -n 1) EXIT $?"
: > "$F/second"
while [ ! -e "$F/go2" ]; do sleep 0.1; done
echo "THIRD $(printf ghi | paddock-host device up - | tail -n 1) EXIT $?"
"#;

/// An agent sandbox drives the instance of its run through `/dev/paddock`
/// and the host sees it: the catalog, `paddock ps`, `device ls|stop|
/// restart|logs|stats`, and the end of the instances with the run.
pub fn agent_drives_instance() -> bool {
    if !can_run() {
        return false;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return false;
    }
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
    let child = w.run("agent");
    // first-request: `up` has no state file before the first request;
    // with-sandbox: `ws` has one from the start.
    wait_for("the ws instance", WIDE, || !w.state_files("ws").is_empty());
    wait_for("the first request", WIDE, || {
        flags.join("catalog1").exists()
    });
    let catalog: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(flags.join("catalog1")).unwrap()).unwrap();
    let up = catalog["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "up")
        .unwrap();
    assert_eq!(up["instance"], "running");
    assert_eq!(up["run"], "sandbox");
    let st = w.state_files("up").remove(0);
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
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_for("the instance to stop", WIDE, || !alive(pid));
    assert!(w.state_files("up").is_empty());
    std::fs::write(flags.join("go"), "").unwrap();
    // The second answer must be in before the restart: a restart cuts a
    // request at work, and that is not what this proves.
    wait_for("the second answer", WIDE, || flags.join("second").exists());
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
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
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
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if text.contains("NODEV") {
        eprintln!("skipped: the sandbox has no /dev/paddock here");
        return false;
    }
    assert!(text.contains("FIRST ABC EXIT 0"), "{text}");
    assert!(text.contains("SECOND DEF EXIT 0"), "{text}");
    assert!(text.contains("THIRD GHI EXIT 0"), "{text}");
    // The instances end with the run.
    wait_for("the instances to end", WIDE, || {
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
    assert!(v["p50_ms"].is_u64(), "{stats}");
    true
}

/// Two agent sandboxes drive one shared instance through `/dev/paddock/up`:
/// one instance, the user count of two, both runs end, and the instance
/// exits after `idle`.
pub fn agent_uses_shared_instance() -> bool {
    if !can_run() {
        return false;
    }
    if !fuse_works() {
        eprintln!("skipped: no FUSE on this host");
        return false;
    }
    let w = World::new("");
    let dev = w.root.join("dev").display().to_string();
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.up]\nshape = \"exec\"\nenabled = true\nmode = \"allow\"\ngrant = [\"all\"]\nscope = \"shared\"\nidle = 2\ncommand = [\"{dev}/up.sh\"]\n[devices.up.sandbox]\nro = [\"{dev}\"]\n"
        ),
    )
    .unwrap();
    w.script(
        "up.sh",
        "printf 'caller=%s ' \"$PADDOCK_CALLER\"\ntr a-z A-Z",
    );
    let flags = w.root.join("project/flags");
    std::fs::create_dir_all(&flags).unwrap();
    // The name of the answer file is random: `$$` is the same in both
    // sandboxes, each of which has its own pid namespace. The answer is
    // written under a temporary name and renamed when it is whole, with
    // the exit code of the request on its last line.
    w.harness(
        "agent",
        "if [ ! -e /dev/paddock/up ]; then echo NODEV; exit 0; fi\nn=$(od -An -N4 -tu4 /dev/urandom | tr -d ' ')\nprintf abc | paddock-host device up - > \"flags/tmp.$n\" 2>&1\necho \"exit $?\" >> \"flags/tmp.$n\"\nmv \"flags/tmp.$n\" \"flags/out.$n\"\nwhile [ ! -e flags/go ]; do sleep 0.1; done",
    );
    let mut one = w.run("agent");
    let mut two = w.run("agent");
    let state = w
        .root
        .join("state/paddock/devices/instances/up/shared.json");
    let read = || -> serde_json::Value {
        std::fs::read_to_string(&state)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    let log = || {
        std::fs::read_to_string(w.root.join("state/paddock/devices/log/up/shared.log"))
            .unwrap_or_default()
    };
    let answers = || -> Vec<String> {
        std::fs::read_dir(&flags)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("out."))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .collect()
    };
    wait_for("two leases", WIDE, || read()["users"].as_u64() == Some(2));
    let st = read();
    assert_eq!(st["run"], "shared", "{st}");
    let pid = st["pid"].as_u64().unwrap();
    // One instance for both runs: one state file.
    let files: Vec<String> = std::fs::read_dir(w.root.join("state/paddock/devices/instances/up"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["shared.json"], "{files:?}");
    // Both runs got an answer, each with its own caller. A lease comes
    // before the request, so the answers are waited for on their own.
    wait_for("two answers", WIDE, || answers().len() == 2);
    let answers = answers();
    for a in &answers {
        assert!(
            a.contains("ABC") && a.contains("exit 0"),
            "{a}\nlog:\n{}",
            log()
        );
    }
    assert_ne!(
        answers[0], answers[1],
        "the caller is the same: {answers:?}"
    );
    std::fs::write(flags.join("go"), "").unwrap();
    let _ = one.wait();
    let _ = two.wait();
    wait_for("the idle exit", WIDE, || !state.exists());
    assert!(
        !paddock::state::alive(pid as u32),
        "the instance still runs:\n{}",
        log()
    );
    true
}

/// Set in the process that the stream scenario starts for the
/// measurement.
pub const MEASURE_CHILD: &str = "PADDOCK_TEST_MEASURE_CHILD";

/// The size of the stream, and its pieces.
const BIG: usize = 64 << 20;
const PIECE: usize = 64 << 10;

/// The byte of the stream at `pos`.
fn pattern(pos: usize) -> u8 {
    pos as u8
}

/// A device that streams `BIG` bytes of `pattern`.
struct Big;

impl Device for Big {
    fn name(&self) -> &str {
        "big"
    }
    fn info(&self) -> Info {
        Info {
            request: "any".into(),
            response: "64 MiB of a pattern".into(),
            limits: Limits {
                max_request: 16,
                max_response: BIG,
                deadline_seconds: 300,
            },
            shape: "builtin",
            stream: true,
        }
    }
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let mut c = paddock::devices::Collect::default();
        self.call_stream(call, request, &mut c)?;
        Ok(c.0)
    }
    fn call_stream(
        &self,
        call: &Call,
        _request: &[u8],
        sink: &mut dyn Sink,
    ) -> Result<(), DevError> {
        // Every piece is the same: PIECE is a multiple of 256.
        let piece: Vec<u8> = (0..PIECE).map(pattern).collect();
        for _ in 0..BIG / PIECE {
            if call.cancelled() {
                return Err(DevError::Cancelled);
            }
            sink.push(&piece);
        }
        Ok(())
    }
}

/// The resident set of this process, in bytes.
fn resident() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    let pages: usize = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
    pages * 4096
}

/// Mount a file system with `Big`, read the whole stream, and prove the
/// bytes match and the resident set of this process grew by less than
/// 16 MiB. Call it in a process that runs nothing else: the resident set
/// is of the whole process.
pub fn measure_big_stream() -> bool {
    let Ok(env) = paddock::env::HostEnv::detect() else {
        return false;
    };
    if let Err(why) = paddock::devices::fuse_available(&env) {
        eprintln!("skipped: {why}");
        return false;
    }
    let dir = super::tempdir();
    let root = dir.path().join("dev");
    let fs = DeviceFs::new("stress", vec![Arc::new(Big)]);
    let m = match paddock::devices::mount(&root, fs) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e:#}");
            return false;
        }
    };
    let expected: Vec<u8> = (0..(1 << 20) + 256).map(pattern).collect();
    let mut h = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("big"))
        .unwrap();
    h.write_all(b"x").unwrap();
    let start_rss = resident();
    let mut peak = start_rss;
    let mut buf = vec![0u8; 256 << 10];
    let mut pos = 0usize;
    let mut reads = 0usize;
    let t = Instant::now();
    loop {
        let n = h.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        assert!(n <= 1 << 20, "a read of {n}");
        let from = pos % 256;
        assert!(
            buf[..n] == expected[from..from + n],
            "bytes differ at {pos}"
        );
        pos += n;
        reads += 1;
        if reads.is_multiple_of(64) {
            peak = peak.max(resident());
        }
    }
    let took = t.elapsed();
    peak = peak.max(resident());
    assert_eq!(pos, BIG);
    let mib_s = (BIG as f64 / (1 << 20) as f64) / took.as_secs_f64();
    eprintln!(
        "64 MiB stream through the mount: {took:?}, {mib_s:.0} MiB/s, {reads} reads, resident {} KiB over the start (peak)",
        (peak - start_rss) / 1024
    );
    assert!(
        peak - start_rss < 16 << 20,
        "resident grew by {} KiB",
        (peak - start_rss) / 1024
    );
    drop(h);
    m.unmount();
    true
}

/// Run the test `name` of this binary in a process of its own, with
/// [`MEASURE_CHILD`] set, one thread and no capture, so the measurement
/// sees that process alone. The output of the child is the message of a
/// failure. False when the child skipped.
pub fn big_stream_in_a_child(name: &str) -> bool {
    let me = std::env::current_exe().unwrap();
    let out = std::process::Command::new(&me)
        .args([
            name,
            "--exact",
            "--include-ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MEASURE_CHILD, "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{name} in {}:\n{text}", me.display());
    assert!(
        text.contains("test result: ok. 1 passed"),
        "{name} did not run in the child:\n{text}"
    );
    for l in text.lines().filter(|l| l.contains("through the mount")) {
        eprintln!("{l}");
    }
    !text.contains("skipped:")
}
