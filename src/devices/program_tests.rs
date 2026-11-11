use super::*;
use crate::devices::DevicesCfg;

fn def(text: &str) -> DeviceDef {
    DevicesCfg::parse(&format!("[devices.tool]\n{text}"))
        .unwrap()
        .user
        .remove(0)
}

fn exec_def() -> DeviceDef {
    def("shape = \"exec\"\ncommand = [\"x\"]\nmax_response = \"1K\"\ntimeout = 5\n")
}

fn serve_def() -> DeviceDef {
    def("shape = \"serve\"\ncommand = [\"x\"]\nmax_response = \"1K\"\ntimeout = 5\n")
}

/// Feed `raw` one byte at a time, then finish: partial reads are the rule.
fn parse(raw: &[u8], def: &DeviceDef) -> (Result<Outcome, Failure>, Vec<u8>) {
    let limit = match def.shape {
        Shape::Exec => def.limits.max_response + HEAD_ROOM,
        _ => def.limits.max_response,
    };
    let mut r = Response::new(limit);
    let mut sink = Collect::default();
    for b in raw {
        match r.feed(&[*b], &mut sink) {
            Ok(Progress::More) => {}
            Ok(Progress::Done) => break,
            Err(e) => return (Err(r.fail(e, r.failure())), sink.0),
        }
    }
    (r.finish(&mut sink, def), sink.0)
}

#[test]
fn header_values_are_clean_and_short() {
    assert_eq!(header_value("a\u{200B}b\nc\td"), "abcd");
    let long = "é".repeat(1000);
    let v = header_value(&long);
    assert!(v.len() <= MAX_HEADER_VALUE && v.len() >= MAX_HEADER_VALUE - 1);
    assert!(v.chars().all(|c| c == 'é'));
}

#[test]
fn the_headers_come_from_the_caller_and_the_def() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _) = crate::devices::user::tests::ctx(dir.path(), crate::broker::Answer::Session);
    let mut ctx = ctx;
    ctx.started_by = vec!["root1".into(), "mid2".into()];
    let p = Program::connect(&dir.path().join("dev.sock"))
        .unwrap()
        .with_caller(Caller::new("tool", &ctx).with_mount("/run/paddock/caller/project"));
    let call = Call::for_test("box1", Duration::from_secs(600));
    let h = p.headers(&call, &exec_def(), 42);
    let get = |n: &str| h.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
    assert_eq!(get("Paddock-Protocol"), Some("1"));
    assert_eq!(get("Paddock-Device"), Some("tool"));
    assert_eq!(get("Paddock-Caller"), Some("box1"));
    assert_eq!(get("Paddock-Caller-Harness"), Some("fake"));
    assert_eq!(
        get("Paddock-Caller-Project"),
        Some(dir.path().join("project").display().to_string().as_str())
    );
    assert_eq!(
        get("Paddock-Caller-Mount"),
        Some("/run/paddock/caller/project")
    );
    assert_eq!(get("Paddock-Caller-Chain"), Some("root1,mid2"));
    assert_eq!(get("Paddock-Request"), Some("1"));
    // The deadline is the timeout of the table, not the read deadline.
    assert_eq!(get("Paddock-Deadline"), Some("5"));
    assert_eq!(get("Paddock-Max-Response"), Some("1024"));
    assert_eq!(get("Content-Length"), Some("42"));
    assert_eq!(get("Connection"), Some("close"));
    let h2 = p.headers(&call, &exec_def(), 0);
    assert_eq!(
        h2.iter().find(|(k, _)| k == "Paddock-Request").unwrap().1,
        "2",
        "request ids count up per instance"
    );
    assert!(Program::connect(&dir.path().join("no/dev.sock")).is_err());
}

#[test]
fn a_content_length_body_is_collected_and_the_envelope_status_read() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nstatus: exit 0\n\nABC\n";
    let (r, body) = parse(raw, &exec_def());
    let o = r.unwrap();
    assert_eq!(o.http, 200);
    assert_eq!(o.status, "exit 0");
    assert_eq!(o.bytes, 20);
    assert_eq!(body, b"status: exit 0\n\nABC\n");
    // A serve body is bytes: the status is the code.
    let (r, _) = parse(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nABC",
        &serve_def(),
    );
    assert_eq!(r.unwrap().status, "200");
    let (r, body) = parse(b"HTTP/1.1 204 No Content\r\n\r\n", &serve_def());
    assert_eq!(r.unwrap().bytes, 0);
    assert!(body.is_empty());
}

#[test]
fn chunked_bodies_and_trailers() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: Paddock-Exit\r\n\r\n3\r\nabc\r\n2;ext=1\r\nde\r\n0\r\nPaddock-Exit: 0\r\n\r\n";
    let (r, body) = parse(raw, &serve_def());
    assert_eq!(body, b"abcde");
    assert_eq!(r.unwrap().bytes, 5);
    // A non-zero exit fails the stream after the bytes.
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\nPaddock-Exit: 3\r\n\r\n";
    let (r, body) = parse(raw, &serve_def());
    assert_eq!(body, b"abc");
    let f = r.unwrap_err();
    assert_eq!(
        (f.error, f.status.as_str(), f.http),
        (DevError::Failed, "exit 3", Some(200))
    );
    let raw =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nPaddock-Exit: timeout\r\n\r\n";
    assert_eq!(parse(raw, &serve_def()).0.unwrap_err().status, "timeout");
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nPaddock-Exit: 137\r\nPaddock-Status: too large\r\n\r\n";
    assert_eq!(
        parse(raw, &serve_def()).0.unwrap_err().error,
        DevError::TooLarge
    );
    // Broken chunk framing is a bad response.
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n";
    assert_eq!(
        parse(raw, &serve_def()).0.unwrap_err().status,
        "bad response"
    );
    // An LF-only server works too.
    let raw = b"HTTP/1.1 200 OK\nTransfer-Encoding: chunked\n\n2\nab\n0\n\n";
    assert_eq!(parse(raw, &serve_def()).1, b"ab");
}

/// A rejected request arrives as a 400 whose body is the envelope of the
/// exit 65 (the status line, the stderr of the program). The reader must
/// not see those bytes: with them, a stream reader would fail after bytes
/// (exit 7 of `paddock-host device`) instead of EINVAL (exit 8).
#[test]
fn the_body_of_a_failing_status_stays_out_of_the_sink() {
    let d = def(
        "shape = \"exec\"\ncommand = [\"x\"]\nstream = true\nmax_response = \"1K\"\ntimeout = 5\n",
    );
    let body = b"status: exit 65\nstderr: request must be test or check\n\n";
    let raw = format!(
        "HTTP/1.1 400 Bad Request\r\nPaddock-Status: rejected\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut raw = raw.into_bytes();
    raw.extend_from_slice(body);
    let (r, out) = parse(&raw, &d);
    let f = r.unwrap_err();
    assert_eq!(
        (f.error, f.status.as_str()),
        (DevError::Invalid, "rejected")
    );
    assert!(
        out.is_empty(),
        "the reader got {:?}",
        String::from_utf8_lossy(&out)
    );
    // A failure of the supervisor itself: the same, with its message held.
    let raw = b"HTTP/1.1 503 Down\r\nPaddock-Status: failed\r\nContent-Length: 4\r\n\r\ndead";
    let (r, out) = parse(raw, &d);
    assert_eq!(r.unwrap_err().error, DevError::Failed);
    assert!(out.is_empty());
    // A 200 stream delivers every byte, also when its trailer fails it.
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\nPaddock-Exit: 3\r\n\r\n";
    let (r, out) = parse(raw, &d);
    assert_eq!(r.unwrap_err().status, "exit 3");
    assert_eq!(out, b"hello");
}

#[test]
fn statuses_map_to_errnos() {
    let d = serve_def();
    let cases: &[(&[u8], DevError, &str)] = &[
        (
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n",
            DevError::Waiting,
            "waiting",
        ),
        (
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
            DevError::Invalid,
            "rejected",
        ),
        (
            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n",
            DevError::Denied,
            "declined",
        ),
        (
            b"HTTP/1.1 413 Too Large\r\nContent-Length: 0\r\n\r\n",
            DevError::RequestTooLarge,
            "request too large",
        ),
        (
            b"HTTP/1.1 429 Busy\r\nContent-Length: 0\r\n\r\n",
            DevError::Busy,
            "busy",
        ),
        (
            b"HTTP/1.1 503 Down\r\nContent-Length: 0\r\n\r\n",
            DevError::Failed,
            "503",
        ),
        (
            b"HTTP/1.1 500 Oops\r\nPaddock-Status: failed\r\nContent-Length: 0\r\n\r\n",
            DevError::Failed,
            "failed",
        ),
        (
            b"HTTP/1.1 500 Oops\r\nPaddock-Status: too large\r\nContent-Length: 0\r\n\r\n",
            DevError::TooLarge,
            "too large",
        ),
        (
            b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n",
            DevError::Failed,
            "3xx",
        ),
    ];
    for (raw, err, status) in cases {
        let f = parse(raw, &d).0.unwrap_err();
        assert_eq!(
            (f.error, f.status.as_str()),
            (*err, *status),
            "{}",
            String::from_utf8_lossy(raw)
        );
        assert!(f.http.is_some());
    }
    assert_eq!(map_status(200, None), Ok(()));
    assert_eq!(map_status(204, Some("x")), Ok(()));
    assert_eq!(DevError::Busy.errno(), libc::EBUSY);
    assert_eq!(DevError::RequestTooLarge.errno(), libc::EFBIG);
}

#[test]
fn the_cost_header_is_parsed() {
    let raw = b"HTTP/1.1 200 OK\r\nPaddock-Cost: 0.002\r\nContent-Length: 1\r\n\r\nx";
    assert_eq!(parse(raw, &serve_def()).0.unwrap().cost, Some(2000));
    let raw = b"HTTP/1.1 200 OK\r\nPaddock-Cost: lots\r\nContent-Length: 1\r\n\r\nx";
    assert_eq!(parse(raw, &serve_def()).0.unwrap().cost, None);
}

#[test]
fn too_much_body_and_broken_responses_fail() {
    let d = serve_def();
    let big = vec![b'x'; 1025];
    let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: 1025\r\n\r\n".to_vec();
    raw.extend_from_slice(&big);
    let (r, body) = parse(&raw, &d);
    assert_eq!(r.unwrap_err().error, DevError::TooLarge);
    assert!(body.len() <= 1025);
    // Exactly the limit passes.
    let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\n".to_vec();
    raw.extend_from_slice(&big[..1024]);
    assert!(parse(&raw, &d).0.is_ok());
    let (r, _) = parse(b"not http at all\r\n\r\n", &d);
    assert_eq!(r.unwrap_err().status, "bad response");
    let (r, _) = parse(b"", &d);
    assert_eq!(r.unwrap_err().status, "closed");
    // A short body is a bad response, never a partial success.
    let (r, _) = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc", &d);
    let f = r.unwrap_err();
    assert_eq!(
        (f.error, f.status.as_str()),
        (DevError::Failed, "bad response")
    );
    let (r, _) = parse(b"HTTP/1.1 200 OK\r\nContent-Length: ten\r\n\r\nabc", &d);
    assert_eq!(r.unwrap_err().status, "bad response");
    // Without a length the body ends with the connection.
    let (r, body) = parse(b"HTTP/1.1 200 OK\r\n\r\nabc", &d);
    assert_eq!(r.unwrap().bytes, 3);
    assert_eq!(body, b"abc");
}

#[test]
fn the_example_script_is_the_one_of_the_design() {
    assert!(EXAMPLE.starts_with("#!/usr/bin/env python3\n"));
    assert!(EXAMPLE.contains("PADDOCK_DEVICE_SOCKET"));
    assert!(EXAMPLE.contains("socketserver.UnixStreamServer"));
    assert!(EXAMPLE.lines().count() <= 20, "a 20-line device");
}

#[test]
fn a_body_that_arrives_in_the_same_read_as_the_head_is_fine() {
    // A fast program's first 64 KiB come in one read: the head is a few
    // lines, the rest is body. Only a head longer than HEAD_ROOM is bad.
    let d = def("shape = \"exec\"\ncommand = [\"x\"]\nmax_response = \"1M\"\ntimeout = 5\n");
    let body = vec![b'x'; 200 * 1024];
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(&body);
    let mut r = Response::new(d.limits.max_response + HEAD_ROOM);
    let mut sink = Collect::default();
    for chunk in raw.chunks(64 * 1024) {
        assert!(r.feed(chunk, &mut sink).is_ok(), "{:?}", r.failure());
    }
    let o = r.finish(&mut sink, &d).unwrap();
    assert_eq!(o.http, 200);
    assert_eq!(sink.0.len(), body.len());
    // A head that never ends within HEAD_ROOM is still refused.
    let mut r = Response::new(d.limits.max_response + HEAD_ROOM);
    let long = format!("HTTP/1.1 200 OK\r\nX: {}\r\n", "y".repeat(HEAD_ROOM));
    let e = r.feed(long.as_bytes(), &mut sink).unwrap_err();
    assert_eq!(e, DevError::Failed);
    assert_eq!(r.failure(), "bad response");
}
