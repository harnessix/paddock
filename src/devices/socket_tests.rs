use super::*;
use crate::credentials::secret::Secret;
use crate::devices::{blocking, DevError, Echo, Info, Limits};

/// Sleeps until it is cancelled, and says when it saw the cancel.
struct Sleeper {
    saw_cancel: Arc<Mutex<Option<Instant>>>,
}

use std::sync::Mutex;

impl Device for Sleeper {
    fn name(&self) -> &str {
        "sleeper"
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: "any".into(),
            response: "nothing".into(),
            limits: Limits {
                max_request: 16,
                max_response: 16,
                deadline_seconds: 30,
            },
        }
    }
    fn call(&self, call: &Call, _request: &[u8]) -> Result<Vec<u8>, DevError> {
        while !call.cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
        *self.saw_cancel.lock().unwrap() = Some(Instant::now());
        Err(DevError::Cancelled)
    }
}

/// Answers with more bytes than its limit.
struct Big;

impl Device for Big {
    fn name(&self) -> &str {
        "big"
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: "any".into(),
            response: "too much".into(),
            limits: Limits {
                max_request: 16,
                max_response: 8,
                deadline_seconds: 5,
            },
        }
    }
    fn call(&self, _call: &Call, _request: &[u8]) -> Result<Vec<u8>, DevError> {
        Ok(vec![b'x'; 9])
    }
}

/// The server goes before the folder of its socket.
struct Fixture {
    _serving: Serving,
    _dir: tempfile::TempDir,
    path: PathBuf,
    secrets: Arc<SecretFiles>,
    revoked: PathBuf,
    reads: Arc<Mutex<Vec<String>>>,
    saw_cancel: Arc<Mutex<Option<Instant>>>,
    devices: Vec<Arc<dyn Device>>,
}

fn fixture(extra: Vec<Arc<dyn Device>>) -> Fixture {
    // The product raises the limit of open files in main (macOS starts
    // with 256); the test process has no main of paddock, so it does it
    // here, before 64 connections of each of many tests at once.
    crate::platform::nofile::raise();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("devices.sock");
    let revoked = dir.path().join("revoked");
    let reads = Arc::new(Mutex::new(Vec::new()));
    let r2 = reads.clone();
    let secrets = SecretFiles::new(
        Some(revoked.clone()),
        Some(Arc::new(move |n: &str| r2.lock().unwrap().push(n.into()))),
    );
    secrets.insert("T", Secret::from("tok-123"));
    let saw_cancel = Arc::new(Mutex::new(None));
    let mut devices: Vec<Arc<dyn Device>> = vec![
        Arc::new(Echo),
        Arc::new(blocking::Wait),
        Arc::new(Sleeper {
            saw_cancel: saw_cancel.clone(),
        }),
        Arc::new(Big),
    ];
    devices.extend(extra);
    let serving = serve(&path, Server::new("box1", devices.clone(), secrets.clone())).unwrap();
    Fixture {
        path,
        secrets,
        revoked,
        reads,
        saw_cancel,
        devices,
        _serving: serving,
        _dir: dir,
    }
}

/// Send raw bytes and read the whole answer: the header and the rest.
fn raw(path: &Path, bytes: &[u8]) -> (Reply, Vec<u8>) {
    let mut c = UnixStream::connect(path).unwrap();
    let _ = c.write_all(bytes);
    let mut r = BufReader::new(c);
    let line = read_line(&mut r).expect("an answer line");
    let reply: Reply = serde_json::from_slice(&line).unwrap();
    let mut rest = Vec::new();
    let _ = r.read_to_end(&mut rest);
    (reply, rest)
}

#[test]
fn echo_round_trip() {
    let f = fixture(vec![]);
    let out = request_at(&f.path, &Request::call("echo", 5), b"hello").unwrap();
    assert_eq!(out, b"hello");
    // An empty request is a request too.
    assert_eq!(
        request_at(&f.path, &Request::call("echo", 0), b"").unwrap(),
        b""
    );
}

#[test]
fn the_socket_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture(vec![]);
    let mode = std::fs::metadata(&f.path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn catalog_bytes_equal_catalog_with() {
    let f = fixture(vec![]);
    let out = request_at(&f.path, &Request::catalog(), b"").unwrap();
    assert_eq!(out, catalog_with(&f.devices, &f.secrets.names()));
}

#[test]
fn a_request_over_the_limit_is_refused_before_its_body() {
    let f = fixture(vec![]);
    // The header claims 100 bytes; the body never comes. The answer comes
    // at once, so the server did not wait for the body.
    let start = Instant::now();
    let (reply, _) = raw(
        &f.path,
        b"{\"v\":1,\"op\":\"call\",\"device\":\"sleeper\",\"size\":100}\n",
    );
    assert_eq!(reply.errno, Some(libc::EFBIG));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(
        request_at(&f.path, &Request::call("sleeper", 17), &[0u8; 17]),
        Err(libc::EFBIG)
    );
}

#[test]
fn a_response_over_the_limit_is_emsgsize() {
    let f = fixture(vec![]);
    assert_eq!(
        request_at(&f.path, &Request::call("big", 0), b""),
        Err(libc::EMSGSIZE)
    );
}

#[test]
fn the_65th_connection_is_busy() {
    let f = fixture(vec![]);
    let mut held = Vec::new();
    for _ in 0..MAX_OPEN {
        held.push(UnixStream::connect(&f.path).unwrap());
    }
    // The server counts a connection when it accepts it: wait until it has
    // all of them.
    let start = Instant::now();
    loop {
        let (reply, _) = raw(&f.path, b"");
        if reply.errno == Some(libc::EBUSY) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "{reply:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(held);
    // The slots come back.
    let start = Instant::now();
    while request_at(&f.path, &Request::call("echo", 1), b"x").is_err() {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_closed_client_cancels_the_request() {
    if crate::storage::package_cache::isolated_test(
        "devices::socket::tests::a_closed_client_cancels_the_request",
    ) {
        return;
    }
    let f = fixture(vec![]);
    let mut c = UnixStream::connect(&f.path).unwrap();
    c.write_all(b"{\"v\":1,\"op\":\"call\",\"device\":\"sleeper\",\"size\":0}\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(f.saw_cancel.lock().unwrap().is_none());
    let closed = Instant::now();
    drop(c);
    while f.saw_cancel.lock().unwrap().is_none() {
        assert!(closed.elapsed() < Duration::from_secs(5), "no cancel");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_bad_header_is_einval() {
    let f = fixture(vec![]);
    for bad in [
        &b"not json\n"[..],
        b"{\"v\":2,\"op\":\"catalog\"}\n",
        b"{\"v\":1,\"op\":\"format\"}\n",
        b"{\"v\":1,\"op\":\"call\"}\n",
        b"{\"v\":1,\"op\":\"secret\"}\n",
    ] {
        let (reply, rest) = raw(&f.path, bad);
        assert_eq!(
            reply.errno,
            Some(libc::EINVAL),
            "{}",
            String::from_utf8_lossy(bad)
        );
        assert!(!reply.ok);
        assert!(rest.is_empty());
    }
    let mut long = vec![b' '; MAX_HEADER + 10];
    long.push(b'\n');
    let (reply, _) = raw(&f.path, &long);
    assert_eq!(reply.errno, Some(libc::EINVAL));
}

#[test]
fn an_unknown_device_is_enoent() {
    let f = fixture(vec![]);
    assert_eq!(
        request_at(&f.path, &Request::call("nope", 0), b""),
        Err(libc::ENOENT)
    );
}

#[test]
fn a_waiting_device_answers_still_waiting() {
    let f = fixture(vec![]);
    let until = crate::waiting::format_rfc3339(crate::state::now() + 3600);
    let body = serde_json::json!({ "until": until, "timeout": 1 }).to_string();
    let r = request_at(&f.path, &Request::call("wait", body.len()), body.as_bytes());
    assert_eq!(r, Err(crate::waiting::ERRNO_STILL_WAITING));
    assert_eq!(
        crate::hostcli::device::exit_for_errno(r.unwrap_err()),
        crate::waiting::EXIT_STILL_WAITING
    );
}

#[test]
fn an_off_user_device_answers_its_disabled_status() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = crate::devices::user::tests::ctx(dir.path(), crate::broker::Answer::Deny);
    let cfg = crate::devices::DevicesCfg::parse(
        "[devices.svc]\nkind = \"http\"\nenabled = false\ntarget = \"127.0.0.1:9\"\nmethods = [\"GET\"]\npaths = [\"/api/\"]\n",
    )
    .unwrap();
    let off = cfg.user[0].build(&ctx, Err(crate::devices::grant::Why::Off));
    let f = fixture(vec![off]);
    let req = b"GET /api/x HTTP/1.1\n\n";
    let out = request_at(&f.path, &Request::call("svc", req.len()), req).unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("disabled"), "{text}");
}

#[test]
fn a_secret_counts_one_use_and_fails_after_revoke() {
    let f = fixture(vec![]);
    assert_eq!(
        request_at(&f.path, &Request::secret("T"), b"").unwrap(),
        b"tok-123"
    );
    assert_eq!(*f.reads.lock().unwrap(), ["T"]);
    assert_eq!(
        request_at(&f.path, &Request::secret("NOPE"), b""),
        Err(libc::ENOENT)
    );
    std::fs::write(&f.revoked, "T\n").unwrap();
    assert_eq!(
        request_at(&f.path, &Request::secret("T"), b""),
        Err(libc::EACCES)
    );
    assert_eq!(*f.reads.lock().unwrap(), ["T"]);
}

#[test]
fn dropping_the_server_removes_the_socket() {
    let f = fixture(vec![]);
    let path = f.path.clone();
    assert!(path.exists());
    drop(f._serving);
    assert!(!path.exists());
}

// --- Streams -------------------------------------------------------------

use crate::devices::tests::streaming::{Ab, After, Forever, Pieces};
use crate::hostcli::device::exit_for;

/// A stream fixture: the chunked devices beside the usual ones.
fn stream_fixture() -> (Fixture, Arc<Mutex<Option<Instant>>>) {
    let forever = Forever::new("forever", 64, 64 << 20);
    let saw = forever.saw_cancel.clone();
    let f = fixture(vec![
        Arc::new(Pieces {
            piece: 3,
            pieces: 3,
        }),
        Arc::new(Ab::new("ab", After::B)),
        Arc::new(Ab::new("ab-fail", After::Fail)),
        Arc::new(Ab::new("ab-wait", After::Wait)),
        Arc::new(forever),
        Arc::new(Forever::new("over", 1024, 4096)),
    ]);
    (f, saw)
}

/// Read the raw frames of a stream answer: the pieces, and whether the 0
/// chunk came before the close.
fn frames(r: &mut BufReader<UnixStream>) -> (Vec<Vec<u8>>, bool) {
    let mut pieces = Vec::new();
    loop {
        let mut len = [0u8; 4];
        if r.read_exact(&mut len).is_err() {
            return (pieces, false);
        }
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 {
            return (pieces, true);
        }
        let mut p = vec![0u8; len];
        r.read_exact(&mut p).unwrap();
        pieces.push(p);
    }
}

fn stream_raw(path: &Path, device: &str) -> (Reply, Vec<Vec<u8>>, bool) {
    let mut c = UnixStream::connect(path).unwrap();
    c.write_all(
        format!("{{\"v\":1,\"op\":\"call\",\"device\":\"{device}\",\"size\":0,\"stream\":true}}\n")
            .as_bytes(),
    )
    .unwrap();
    let mut r = BufReader::new(c);
    let line = read_line(&mut r).expect("an answer line");
    let reply: Reply = serde_json::from_slice(&line).unwrap();
    let (pieces, clean) = frames(&mut r);
    (reply, pieces, clean)
}

#[test]
fn a_stream_comes_in_chunks_with_a_clean_end() {
    let (f, _) = stream_fixture();
    let (reply, pieces, clean) = stream_raw(&f.path, "pieces");
    assert!(
        reply.ok && reply.stream && reply.size.is_none(),
        "{reply:?}"
    );
    assert_eq!(pieces, [b"000".to_vec(), b"111".to_vec(), b"222".to_vec()]);
    assert!(clean, "the 0 chunk ends it");
    // The client: each piece as it comes, and the count.
    let mut got = Vec::new();
    let mut times = Vec::new();
    let conn = UnixStream::connect(&f.path).unwrap();
    let n = request_into(conn, &Request::call_stream("pieces", 0), b"", &mut |b| {
        got.extend_from_slice(b);
        times.push(Instant::now());
        Ok(())
    })
    .unwrap();
    assert_eq!((n, got.as_slice()), (9, &b"000111222"[..]));
    assert_eq!(times.len(), 3, "one call per piece");
    assert!(times[2].duration_since(times[0]) >= Duration::from_millis(30));
    // An empty stream: the answer line and the 0 chunk.
    let (reply, pieces, clean) = stream_raw(&f.path, "echo");
    assert!(reply.ok && reply.stream, "{reply:?}");
    assert!(pieces.is_empty() && clean);
}

#[test]
fn a_failure_after_the_first_chunk_is_a_close_without_the_0_chunk() {
    let (f, _) = stream_fixture();
    let (reply, pieces, clean) = stream_raw(&f.path, "ab-fail");
    assert!(reply.ok && reply.stream, "{reply:?}");
    assert_eq!(pieces, [b"a".to_vec()]);
    assert!(!clean, "no 0 chunk");
    let mut got = Vec::new();
    let conn = UnixStream::connect(&f.path).unwrap();
    let p = request_into(conn, &Request::call_stream("ab-fail", 0), b"", &mut |b| {
        got.extend_from_slice(b);
        Ok(())
    })
    .unwrap_err();
    assert_eq!(got, b"a");
    assert_eq!(
        p,
        Partial {
            errno: libc::EIO,
            after: 1
        }
    );
    assert_eq!(exit_for(p), 7);
    // A failure before the first byte is the answer line, as before.
    let conn = UnixStream::connect(&f.path).unwrap();
    let p = request_into(conn, &Request::call_stream("big", 0), b"", &mut |_| Ok(())).unwrap_err();
    assert_eq!(
        p,
        Partial {
            errno: libc::EMSGSIZE,
            after: 0
        }
    );
    // Over the limit after bytes: the bytes within it, then the close.
    let (reply, pieces, clean) = stream_raw(&f.path, "over");
    assert!(reply.ok && reply.stream, "{reply:?}");
    assert_eq!(pieces.iter().map(Vec::len).sum::<usize>(), 4096);
    assert!(!clean);
}

#[test]
fn an_old_client_gets_a_collected_response_with_a_size() {
    let (f, _) = stream_fixture();
    // No `stream` in the header: the answer has a size and no chunks.
    let (reply, rest) = raw(
        &f.path,
        b"{\"v\":1,\"op\":\"call\",\"device\":\"ab\",\"size\":0}\n",
    );
    assert_eq!(reply.size, Some(2));
    assert!(!reply.stream);
    assert_eq!(rest, b"ab");
    assert_eq!(
        request_at(&f.path, &Request::call("pieces", 0), b"").unwrap(),
        b"000111222"
    );
    // A failure of a stream device is one errno for an old client.
    assert_eq!(
        request_at(&f.path, &Request::call("ab-fail", 0), b""),
        Err(libc::EIO)
    );
    // A new client reads a collected answer too (an older server).
    let mut got = Vec::new();
    let conn = UnixStream::connect(&f.path).unwrap();
    let n = request_into(conn, &Request::call("ab", 0), b"", &mut |b| {
        got.extend_from_slice(b);
        Ok(())
    })
    .unwrap();
    assert_eq!((n, got.as_slice()), (2, &b"ab"[..]));
    // The header of a new client serializes `stream` only when it is set.
    let line = serde_json::to_string(&Request::call("ab", 0)).unwrap();
    assert!(!line.contains("stream"), "{line}");
    let line = serde_json::to_string(&Request::call_stream("ab", 0)).unwrap();
    assert!(line.contains("\"stream\":true"), "{line}");
}

#[test]
fn a_client_that_closes_a_stream_cancels_the_device() {
    let (f, saw) = stream_fixture();
    let mut c = UnixStream::connect(&f.path).unwrap();
    c.write_all(b"{\"v\":1,\"op\":\"call\",\"device\":\"forever\",\"size\":0,\"stream\":true}\n")
        .unwrap();
    let mut r = BufReader::new(c);
    let line = read_line(&mut r).expect("an answer line");
    let reply: Reply = serde_json::from_slice(&line).unwrap();
    assert!(reply.ok && reply.stream, "{reply:?}");
    let mut got = 0;
    while got < 1024 {
        let mut len = [0u8; 4];
        r.read_exact(&mut len).unwrap();
        let mut p = vec![0u8; u32::from_be_bytes(len) as usize];
        r.read_exact(&mut p).unwrap();
        got += p.len();
    }
    assert!(saw.lock().unwrap().is_none());
    let closed = Instant::now();
    drop(r);
    while saw.lock().unwrap().is_none() {
        assert!(closed.elapsed() < Duration::from_secs(5), "no cancel");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A device whose calls wait for the user.
struct Asking;

impl Device for Asking {
    fn name(&self) -> &str {
        "asking"
    }
    fn info(&self) -> Info {
        Echo.info()
    }
    fn asks(&self, _call: &Call) -> bool {
        true
    }
    fn call(&self, _call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        Ok(request.to_vec())
    }
}

/// Each call is one `device_call` line on the bus: the sandbox, the
/// device and `ask`. The request and the response never go there.
#[test]
fn each_call_is_a_device_call_line_without_its_data() {
    use crate::eventstream::bus::{queue, Follower};
    use crate::eventstream::{Bus, Event, Line};
    let dir = tempfile::tempdir().unwrap();
    let bus = Bus::at(dir.path().join("events"));
    let follower = Follower::bind(&bus, std::process::id()).unwrap();
    let (tx, rx) = queue();
    follower.serve(tx).unwrap();
    let path = dir.path().join("devices.sock");
    let devices: Vec<Arc<dyn Device>> = vec![Arc::new(Echo), Arc::new(Asking)];
    let server = Server::with_events(
        "claude-proj-1a2b3c",
        devices,
        SecretFiles::new(None, None),
        Some(bus),
    );
    let _serving = serve(&path, server).unwrap();
    let secret = b"secret-argument-42";
    for dev in ["echo", "asking"] {
        let req = Request::call(dev, secret.len());
        assert_eq!(request_at(&path, &req, secret).unwrap(), secret);
    }
    let mut got = Vec::new();
    for _ in 0..2 {
        let text = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!text.contains("secret-argument"), "{text}");
        let line: Line = serde_json::from_str(&text).unwrap();
        let Event::DeviceCall(c) = line.event else {
            panic!("{text}")
        };
        assert_eq!(c.run, "claude-proj-1a2b3c");
        got.push((c.device, c.ask));
    }
    assert_eq!(
        got,
        [("echo".to_string(), false), ("asking".to_string(), true)]
    );
    // Without a bus a call publishes nothing and still works.
    let quiet = dir.path().join("quiet.sock");
    let server = Server::with_events(
        "claude-proj-1a2b3c",
        vec![Arc::new(Echo)],
        SecretFiles::new(None, None),
        None,
    );
    let _quiet = serve(&quiet, server).unwrap();
    assert_eq!(
        request_at(&quiet, &Request::call("echo", 2), b"hi").unwrap(),
        b"hi"
    );
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
}
