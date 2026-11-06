//! The device file system through a real mount. Skipped on a host
//! without FUSE.

use super::*;
use crate::devices::{blocking, fuse_available, DevError, Echo, Info, Limits};
use crate::env::HostEnv;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

/// A device that works until it is cancelled, and counts its calls.
struct Slow {
    calls: Arc<AtomicUsize>,
    saw_cancel: Arc<AtomicBool>,
    deadline: u64,
}

impl Device for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: "any".into(),
            response: "never".into(),
            limits: Limits {
                max_request: 16,
                max_response: 16,
                deadline_seconds: self.deadline,
            },
        }
    }
    fn call(&self, call: &Call, _request: &[u8]) -> Result<Vec<u8>, DevError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        while !call.cancelled() {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.saw_cancel.store(true, Ordering::SeqCst);
        Err(DevError::Cancelled)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    _m: Mounted,
    calls: Arc<AtomicUsize>,
    saw_cancel: Arc<AtomicBool>,
}

/// Mount a test file system, or None when this host has no FUSE.
fn mounted(deadline: u64) -> Option<Fixture> {
    let env = HostEnv::detect().ok()?;
    if let Err(why) = fuse_available(&env) {
        eprintln!("skipped: {why}");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    let calls = Arc::new(AtomicUsize::new(0));
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let slow = Slow {
        calls: calls.clone(),
        saw_cancel: saw_cancel.clone(),
        deadline,
    };
    let fs = DeviceFs::new(
        "test-sandbox",
        vec![Arc::new(Echo), Arc::new(slow), Arc::new(blocking::Wait)],
    );
    let m = match mount(&root, fs) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e:#}");
            return None;
        }
    };
    Some(Fixture {
        _dir: dir,
        root,
        _m: m,
        calls,
        saw_cancel,
    })
}

fn open_rw(p: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(true).open(p)
}

#[test]
fn echo_round_trip_and_one_request_per_open() {
    let Some(f) = mounted(5) else { return };
    let mut h = open_rw(&f.root.join("echo")).unwrap();
    h.write_all(b"hello ").unwrap();
    h.write_all(b"world").unwrap();
    let mut out = Vec::new();
    h.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"hello world");
    // The request was sent: no second one on the same open.
    assert!(h.write_all(b"again").is_err());
    drop(h);
    // A new open is a new request.
    let mut h = open_rw(&f.root.join("echo")).unwrap();
    h.write_all(b"2").unwrap();
    let mut s = String::new();
    h.read_to_string(&mut s).unwrap();
    assert_eq!(s, "2");
    // Shell style: open with O_TRUNC, as `3<>` and `>` do.
    let mut h = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(true)
        .open(f.root.join("echo"))
        .unwrap();
    h.write_all(b"t").unwrap();
    let mut s = String::new();
    h.read_to_string(&mut s).unwrap();
    assert_eq!(s, "t");
}

#[test]
fn catalog_is_read_only_and_the_folder_fixed() {
    let Some(f) = mounted(5) else { return };
    let text = std::fs::read_to_string(f.root.join(CATALOG)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let names: Vec<&str> = v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    // The devices that are on, then the built-in ones that are off.
    assert_eq!(names, ["echo", "slow", "wait", "ask"]);
    assert!(open_rw(&f.root.join(CATALOG)).is_err());
    assert!(std::fs::write(f.root.join(CATALOG), "x").is_err());
    assert!(std::fs::write(f.root.join("new"), "x").is_err());
    assert!(std::fs::create_dir(f.root.join("d")).is_err());
    assert!(std::fs::remove_file(f.root.join("echo")).is_err());
    assert!(std::fs::rename(f.root.join("echo"), f.root.join("e2")).is_err());
    let mut list: Vec<String> = std::fs::read_dir(&f.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    list.sort();
    assert_eq!(list, [".catalog", "echo", "slow", "wait"]);
}

#[test]
fn a_request_over_the_limit_fails() {
    let Some(f) = mounted(5) else { return };
    let mut h = open_rw(&f.root.join("slow")).unwrap();
    let e = h.write_all(&[0u8; 17]).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EFBIG));
}

#[test]
fn a_read_fails_at_the_deadline() {
    let Some(f) = mounted(1) else { return };
    let mut h = open_rw(&f.root.join("slow")).unwrap();
    h.write_all(b"x").unwrap();
    let start = Instant::now();
    let mut out = Vec::new();
    let e = h.read_to_end(&mut out).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::ETIMEDOUT));
    assert!(start.elapsed() < Duration::from_secs(4));
    let wait = Instant::now();
    while !f.saw_cancel.load(Ordering::SeqCst) && wait.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f.saw_cancel.load(Ordering::SeqCst));
}

#[test]
fn a_waiting_read_reports_still_waiting_with_etime_through_the_mount() {
    let Some(f) = mounted(5) else { return };
    let mut h = open_rw(&f.root.join("wait")).unwrap();
    h.write_all(br#"{"until": "1h", "timeout": 1}"#).unwrap();
    let start = Instant::now();
    let mut out = Vec::new();
    let e = h.read_to_end(&mut out).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(crate::waiting::ERRNO_STILL_WAITING));
    assert!(start.elapsed() < Duration::from_secs(4));
    // A time that has come returns the line.
    let mut h = open_rw(&f.root.join("wait")).unwrap();
    h.write_all(b"0s").unwrap();
    let mut s = String::new();
    h.read_to_string(&mut s).unwrap();
    assert!(s.starts_with("reached "), "{s}");
}

#[test]
fn closing_before_the_read_sends_nothing() {
    let Some(f) = mounted(5) else { return };
    let mut h = open_rw(&f.root.join("slow")).unwrap();
    h.write_all(b"x").unwrap();
    drop(h);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_killed_reader_cancels_the_request() {
    let Some(f) = mounted(30) else { return };
    let dev = f.root.join("slow");
    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("exec 3<>\"$1\"; printf x >&3; exec cat <&3")
        .arg("sh")
        .arg(&dev)
        .spawn()
        .unwrap();
    let wait = Instant::now();
    while f.calls.load(Ordering::SeqCst) == 0 && wait.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    let killed = Instant::now();
    child.kill().unwrap();
    child.wait().unwrap();
    // Far before the deadline of 30 seconds.
    assert!(killed.elapsed() < Duration::from_secs(5));
    let wait = Instant::now();
    while !f.saw_cancel.load(Ordering::SeqCst) && wait.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(f.saw_cancel.load(Ordering::SeqCst));
}

#[test]
fn unmount_leaves_an_empty_folder() {
    let Some(f) = mounted(5) else { return };
    let root = f.root.clone();
    let Fixture { _m, _dir, .. } = f;
    _m.unmount();
    assert!(!root.join(CATALOG).exists());
}

#[test]
fn a_secret_file_returns_the_value_counts_a_use_and_fails_after_revoke() {
    use crate::credentials::secret::{Secret, SecretFiles};
    let Ok(env) = HostEnv::detect() else { return };
    if let Err(why) = fuse_available(&env) {
        eprintln!("skipped: {why}");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let revoked = dir.path().join("revoked");
    let reads = Arc::new(Mutex::new(Vec::<String>::new()));
    let r2 = reads.clone();
    let secrets = SecretFiles::new(
        Some(revoked.clone()),
        Some(Arc::new(move |n: &str| {
            r2.lock().unwrap().push(n.to_string())
        })),
    );
    let fs = DeviceFs::with_secrets("test-sandbox", vec![Arc::new(Echo)], secrets.clone());
    let root = dir.path().join("dev");
    let _m = match mount(&root, fs) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e:#}");
            return;
        }
    };
    // Before the start card: no folder.
    assert!(!root.join(SECRET_DIR).exists());
    secrets.insert("GH_TOKEN", Secret::from("ghp_FILEvalue"));
    let file = root.join("secret/GH_TOKEN");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ghp_FILEvalue");
    assert_eq!(*reads.lock().unwrap(), ["GH_TOKEN"]);
    let list: Vec<String> = std::fs::read_dir(root.join(SECRET_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(list, ["GH_TOKEN"]);
    let text = std::fs::read_to_string(root.join(CATALOG)).unwrap();
    assert!(text.contains("/dev/paddock/secret/GH_TOKEN") && !text.contains("FILEvalue"));
    // Read-only.
    assert!(open_rw(&file).is_err());
    assert!(std::fs::write(&file, "x").is_err());
    // A file that is open when the user revokes fails at its next read.
    let mut open = std::fs::File::open(&file).unwrap();
    assert_eq!(reads.lock().unwrap().len(), 2);
    std::fs::write(&revoked, "GH_TOKEN\n").unwrap();
    let mut out = Vec::new();
    let e = open.read_to_end(&mut out).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EACCES));
    assert!(out.is_empty());
    assert!(std::fs::read(&file).is_err());
    assert!(secrets.names().is_empty());
    assert_eq!(reads.lock().unwrap().len(), 2);
}

/// For manual checks of a backend: mount at $PADDOCK_TEST_MOUNT for
/// $PADDOCK_TEST_SECONDS. `cargo test -- --ignored hold_a_mount`.
#[test]
#[ignore]
fn hold_a_mount() {
    let Ok(dir) = std::env::var("PADDOCK_TEST_MOUNT") else {
        return;
    };
    let secs: u64 = std::env::var("PADDOCK_TEST_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let fs = DeviceFs::new("manual", vec![Arc::new(Echo)]);
    let _m = mount(Path::new(&dir), fs).unwrap();
    std::thread::sleep(Duration::from_secs(secs));
}

// --- Streams -------------------------------------------------------------

use super::handle::{Next, Stream};
use super::KEEP;
use crate::devices::tests::streaming::{pattern, Ab, After, Big, Forever, BIG, PIECE};
use crate::hostcli::device::{exit_for, run_with, Partial};

#[test]
fn a_stream_answers_in_order_and_ends_once() {
    let mut s = Stream::new(100);
    let mut cursor = 0;
    assert!(matches!(s.next(&mut cursor, 10), Next::Wait));
    assert!(s.push(b"hello"));
    match s.next(&mut cursor, 3) {
        Next::Data(d) => assert_eq!(d, b"hel"),
        _ => panic!("bytes"),
    }
    match s.next(&mut cursor, 10) {
        Next::Data(d) => assert_eq!(d, b"lo"),
        _ => panic!("bytes"),
    }
    assert!(matches!(s.next(&mut cursor, 10), Next::Wait));
    s.finish(Ok(()));
    match s.next(&mut cursor, 10) {
        Next::Data(d) => assert!(d.is_empty(), "end of file"),
        _ => panic!("eof"),
    }
    assert_eq!(cursor, 5);
    assert!(!s.push(b"late"), "nothing after the end");
    assert_eq!(s.total(), 5);
}

#[test]
fn a_failure_after_bytes_gives_the_bytes_then_the_errno() {
    let mut s = Stream::new(100);
    let mut cursor = 0;
    s.push(b"ab");
    s.finish(Err(Errno::EIO));
    match s.next(&mut cursor, 1) {
        Next::Data(d) => assert_eq!(d, b"a"),
        _ => panic!("bytes first"),
    }
    match s.next(&mut cursor, 1) {
        Next::Data(d) => assert_eq!(d, b"b"),
        _ => panic!("bytes first"),
    }
    assert!(matches!(s.next(&mut cursor, 1), Next::Error(Errno::EIO)));
    assert!(matches!(s.next(&mut cursor, 1), Next::Error(Errno::EIO)));
}

#[test]
fn a_stream_over_its_limit_fails_with_emsgsize_and_keeps_that() {
    let mut s = Stream::new(4);
    let mut cursor = 0;
    assert!(s.push(b"abcd"));
    assert!(!s.push(b"e"), "over the limit");
    assert!(!s.open());
    // The end of the device call does not hide the limit.
    s.finish(Err(Errno::ECANCELED));
    match s.next(&mut cursor, 10) {
        Next::Data(d) => assert_eq!(d, b"abcd"),
        _ => panic!("bytes first"),
    }
    assert!(matches!(
        s.next(&mut cursor, 1),
        Next::Error(Errno::EMSGSIZE)
    ));
}

#[test]
fn the_read_front_is_dropped_and_an_old_offset_is_espipe() {
    let mut s = Stream::new(usize::MAX);
    let mut cursor = 0;
    let piece = vec![7u8; 64 << 10];
    let mut max_held = 0;
    for _ in 0..1024 {
        s.push(&piece);
        // A sequential reader sees every byte, whatever is dropped.
        while s.unread(cursor) > 0 {
            match s.next(&mut cursor, 1 << 20) {
                Next::Data(d) => assert!(d.iter().all(|b| *b == 7)),
                _ => panic!("bytes"),
            }
        }
        max_held = max_held.max(s.held());
    }
    assert_eq!(cursor, 64 << 20);
    assert_eq!(s.total(), 64 << 20);
    assert!(s.base() > 0, "the front was dropped");
    assert!(max_held <= KEEP, "held {max_held}");
    // An offset below the dropped front cannot be served.
    let mut old = s.base() - 1;
    assert!(matches!(s.next(&mut old, 1), Next::Error(Errno::ESPIPE)));
    // The first byte that is still here can.
    s.push(&piece);
    let mut at = s.base();
    assert!(matches!(s.next(&mut at, 1), Next::Data(_)));
}

/// The resident set of this process, in bytes.
fn resident() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    let pages: usize = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
    pages * 4096
}

struct StreamMount {
    _dir: tempfile::TempDir,
    root: PathBuf,
    _m: Mounted,
    fs: DeviceFs,
    ab: Arc<Mutex<Option<Instant>>>,
    forever: Arc<Mutex<Option<Instant>>>,
    over: Arc<Mutex<Option<Instant>>>,
}

/// A mount with the streaming devices, or None without FUSE.
fn stream_mount() -> Option<StreamMount> {
    let env = HostEnv::detect().ok()?;
    if let Err(why) = fuse_available(&env) {
        eprintln!("skipped: {why}");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    let ab = Ab::new("ab", After::B);
    let ab_t = ab.wrote_a.clone();
    let forever = Forever::new("forever", 64, 64 << 20);
    let forever_t = forever.saw_cancel.clone();
    let over = Forever::new("over", 1024, 4096);
    let over_t = over.saw_cancel.clone();
    let fs = DeviceFs::new(
        "test-sandbox",
        vec![
            Arc::new(ab),
            Arc::new(Ab::new("ab-fail", After::Fail)),
            Arc::new(Ab::new("ab-wait", After::Wait)),
            Arc::new(Big),
            Arc::new(forever),
            Arc::new(over),
        ],
    );
    let m = match mount(&root, fs.clone()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("skipped: {e:#}");
            return None;
        }
    };
    Some(StreamMount {
        _dir: dir,
        root,
        _m: m,
        fs,
        ab: ab_t,
        forever: forever_t,
        over: over_t,
    })
}

#[test]
fn a_stream_delivers_each_byte_as_it_is_written() {
    let Some(f) = stream_mount() else { return };
    let mut h = open_rw(&f.root.join("ab")).unwrap();
    h.write_all(b"x").unwrap();
    let mut buf = [0u8; 16];
    let n = h.read(&mut buf).unwrap();
    let got_a = Instant::now();
    assert_eq!(&buf[..n], b"a");
    let wrote_a = f.ab.lock().unwrap().expect("the device wrote a");
    let lag = got_a.duration_since(wrote_a);
    eprintln!("first byte of the stream: {lag:?} after the write");
    assert!(lag < Duration::from_millis(50), "{lag:?}");
    let n = h.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"b");
    assert!(
        got_a.elapsed() >= Duration::from_millis(150),
        "b came after the wait"
    );
    assert_eq!(h.read(&mut buf).unwrap(), 0, "end of file");
    assert_eq!(h.read(&mut buf).unwrap(), 0, "and it stays");
    assert!(h.write_all(b"again").is_err(), "one open is one request");
}

#[test]
fn a_failure_after_the_first_byte_fails_the_next_read_with_eio() {
    let Some(f) = stream_mount() else { return };
    let mut h = open_rw(&f.root.join("ab-fail")).unwrap();
    h.write_all(b"x").unwrap();
    let mut buf = [0u8; 16];
    let n = h.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"a");
    let e = h.read(&mut buf).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EIO));
    // paddock-host device: what came, then exit 7.
    let mut out = Vec::new();
    let p = run_with(None, &f.root, "ab-fail", b"x", &mut out).unwrap_err();
    assert_eq!(out, b"a");
    assert_eq!(
        p,
        Partial {
            errno: libc::EIO,
            after: 1
        }
    );
    assert_eq!(exit_for(p), 7);
    // Still waiting after bytes: exit 75, with the bytes.
    let mut out = Vec::new();
    let p = run_with(None, &f.root, "ab-wait", b"x", &mut out).unwrap_err();
    assert_eq!(out, b"a");
    assert_eq!(p.errno, crate::waiting::ERRNO_STILL_WAITING);
    assert_eq!(exit_for(p), crate::waiting::EXIT_STILL_WAITING);
    // A good stream through the same path.
    let mut out = Vec::new();
    assert_eq!(run_with(None, &f.root, "ab", b"x", &mut out).unwrap(), 2);
    assert_eq!(out, b"ab");
}

/// Set in the process that the memory test starts for the measurement.
const MEASURE_CHILD: &str = "PADDOCK_TEST_MEASURE_CHILD";

/// Run the test `name` (its path without the crate) in a process of its
/// own: this test binary again, with one thread and no capture. The
/// output of the child is the message of a failure.
fn run_in_a_child(name: &str) {
    let me = std::env::current_exe().unwrap();
    let out = std::process::Command::new(&me)
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
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
}

/// The resident set of a process is of the whole process, and the lib
/// tests run in threads of one process, so a measurement here would see
/// what every other test allocates at the same time. The test starts
/// itself again in a process of its own, and that process measures.
#[test]
fn a_64_mib_stream_holds_little_memory_and_the_bytes_match() {
    if std::env::var_os(MEASURE_CHILD).is_none() {
        let Ok(env) = HostEnv::detect() else { return };
        if let Err(why) = fuse_available(&env) {
            eprintln!("skipped: {why}");
            return;
        }
        let path = module_path!().split_once("::").map_or("", |(_, r)| r);
        run_in_a_child(&format!(
            "{path}::a_64_mib_stream_holds_little_memory_and_the_bytes_match"
        ));
        return;
    }
    let Some(f) = stream_mount() else { return };
    let expected: Vec<u8> = (0..(1 << 20) + 256).map(pattern).collect();
    let mut h = open_rw(&f.root.join("big")).unwrap();
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
    assert_eq!(f.fs.running(), 0);
    let _ = PIECE;
}

#[test]
fn closing_a_stream_cancels_the_device_within_300_ms() {
    let Some(f) = stream_mount() else { return };
    let mut h = open_rw(&f.root.join("forever")).unwrap();
    h.write_all(b"x").unwrap();
    let mut got = 0usize;
    let mut buf = [0u8; 256];
    while got < 1024 {
        got += h.read(&mut buf).unwrap();
    }
    assert_eq!(f.fs.running(), 1);
    let closed = Instant::now();
    drop(h);
    let seen = loop {
        if let Some(t) = *f.forever.lock().unwrap() {
            break t;
        }
        assert!(closed.elapsed() < Duration::from_secs(2), "no cancel");
        std::thread::sleep(Duration::from_millis(5));
    };
    let lag = seen.duration_since(closed);
    eprintln!("the device saw the cancel {lag:?} after the close");
    assert!(lag < Duration::from_millis(300), "{lag:?}");
    let wait = Instant::now();
    while f.fs.running() != 0 {
        assert!(wait.elapsed() < Duration::from_secs(2), "still running");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_stream_past_max_response_fails_with_emsgsize_and_cancels_the_device() {
    let Some(f) = stream_mount() else { return };
    let mut h = open_rw(&f.root.join("over")).unwrap();
    h.write_all(b"x").unwrap();
    let mut out = Vec::new();
    let e = h.read_to_end(&mut out).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EMSGSIZE));
    assert_eq!(out.len(), 4096, "the bytes within the limit came");
    let wait = Instant::now();
    while f.over.lock().unwrap().is_none() {
        assert!(wait.elapsed() < Duration::from_secs(2), "not cancelled");
        std::thread::sleep(Duration::from_millis(5));
    }
    while f.fs.running() != 0 {
        assert!(wait.elapsed() < Duration::from_secs(2), "still running");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_stream_cannot_be_read_twice_or_out_of_order() {
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::FileExt;
    let Some(f) = stream_mount() else { return };
    let mut h = open_rw(&f.root.join("ab")).unwrap();
    h.write_all(b"x").unwrap();
    let mut buf = [0u8; 16];
    assert_eq!(h.read(&mut buf).unwrap(), 1);
    // The file is not seekable: the kernel refuses a seek and a positioned
    // read, so an old offset never reaches paddock.
    let e = h.seek(SeekFrom::Start(0)).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::ESPIPE));
    let e = h.read_at(&mut buf, 0).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::ESPIPE));
    // The sequential reader goes on.
    let n = h.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"b");
    assert_eq!(h.read(&mut buf).unwrap(), 0);
}
