//! The guest proxy over socket pairs: the client side and the vsock side
//! are both `UnixStream` pairs, so the tests see every byte the proxy
//! writes to the host.

use super::*;
use std::io::{BufRead, BufReader, Read, Write};

const TOKEN: &str = "plantedtoken0123plantedtoken0123";

fn id() -> Identity {
    Identity {
        pid: Some(612),
        app: Some("com.openai.chat".into()),
        team: Some("2DC432GLL2".into()),
        path: Some("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT".into()),
        parent_app: None,
    }
}

/// Run `serve_one` in a thread with a fresh pair for the host side.
/// Returns the client end, the host end and the thread.
fn run(
    request: &[u8],
    token: Option<&'static str>,
    cap: Option<&'static Tunnels>,
    dial_ok: bool,
) -> (
    UnixStream,
    Option<UnixStream>,
    std::thread::JoinHandle<Outcome>,
) {
    let (mut client, server_side) = UnixStream::pair().unwrap();
    let (host_near, host_far) = UnixStream::pair().unwrap();
    client.write_all(request).unwrap();
    let tunnels = cap.unwrap_or_else(|| Box::leak(Box::new(Tunnels::default())));
    let slot = tunnels.take();
    let far = dial_ok.then_some(host_far);
    let t = std::thread::spawn(move || {
        serve_one(
            server_side,
            40_000,
            slot,
            token,
            |port| {
                assert_eq!(port, 40_000);
                id()
            },
            move || {
                if dial_ok {
                    Ok(host_near)
                } else {
                    Err(io::Error::new(io::ErrorKind::TimedOut, "no host"))
                }
            },
        )
    });
    (client, far, t)
}

fn lines(host: &mut BufReader<UnixStream>, n: usize) -> Vec<String> {
    (0..n)
        .map(|_| {
            let mut l = String::new();
            host.read_line(&mut l).unwrap();
            l
        })
        .collect()
}

fn status(client: &mut UnixStream) -> u16 {
    status_rest(client).0
}

/// The status and the bytes that came after the head.
fn status_rest(client: &mut UnixStream) -> (u16, Vec<u8>) {
    let (head, rest) = read_head(client).unwrap();
    let mut h = [httparse::EMPTY_HEADER; 16];
    let mut r = httparse::Response::new(&mut h);
    r.parse(&head).unwrap();
    (r.code.unwrap(), rest)
}

#[test]
fn guest_connect_header_then_connect_line() {
    let (mut client, host, t) = run(
        b"CONNECT chatgpt.com:443 HTTP/1.1\r\nHost: chatgpt.com:443\r\nProxy-Connection: keep-alive\r\n\r\n\x16\x03\x01early",
        Some(TOKEN),
        None,
        true,
    );
    let mut host = BufReader::new(host.unwrap());
    let l = lines(&mut host, 3);
    assert_eq!(l[0], format!("{TOKEN}\n"), "the token line of the route");
    let h = ProxyHeader::parse(l[1].trim_end()).unwrap();
    assert_eq!(
        (h.host.as_str(), h.port, h.token.as_str()),
        ("chatgpt.com", 443, TOKEN)
    );
    assert_eq!(h.id, id());
    assert_eq!(
        l[2], "CONNECT chatgpt.com:443 HTTP/1.1\r\n",
        "then the CONNECT line"
    );
    let l = lines(&mut host, 2);
    assert_eq!(
        l,
        ["Host: chatgpt.com:443\r\n", "\r\n"],
        "no client header is forwarded"
    );
    let mut early = [0u8; 8];
    host.read_exact(&mut early).unwrap();
    assert_eq!(
        &early, b"\x16\x03\x01early",
        "bytes after the head follow the CONNECT"
    );
    // The 200 comes from the host through the copy.
    let mut inner = host.into_inner();
    inner
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\nhello from upstream")
        .unwrap();
    let (code, mut got) = status_rest(&mut client);
    assert_eq!(code, 200);
    while got.len() < 19 {
        let mut buf = [0u8; 19];
        let n = client.read(&mut buf).unwrap();
        assert!(n > 0);
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got, b"hello from upstream");
    client.write_all(b"back").unwrap();
    let mut buf = [0u8; 4];
    inner.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"back");
    drop(inner);
    drop(client);
    assert_eq!(t.join().unwrap(), Outcome::Tunneled);
}

#[test]
fn guest_plain_get() {
    let request = b"GET http://x/path?q=1 HTTP/1.1\r\nHost: x\r\nUser-Agent: t\r\n\r\nbody";
    let (mut client, host, t) = run(request, Some(TOKEN), None, true);
    let mut host = BufReader::new(host.unwrap());
    let l = lines(&mut host, 2);
    assert_eq!(l[0], format!("{TOKEN}\n"));
    let h = ProxyHeader::parse(l[1].trim_end()).unwrap();
    assert_eq!((h.host.as_str(), h.port), ("x", 80));
    let mut rest = vec![0u8; request.len()];
    host.read_exact(&mut rest).unwrap();
    assert_eq!(
        rest, request,
        "the original request, head and body, as it came"
    );
    host.get_mut()
        .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
        .unwrap();
    assert_eq!(status(&mut client), 204);
    drop(host);
    drop(client);
    assert_eq!(t.join().unwrap(), Outcome::Tunneled);
}

#[test]
fn guest_malformed_400() {
    for bad in [
        &b"not a request\r\n\r\n"[..],
        b"GET /relative HTTP/1.1\r\n\r\n",
        b"GET ftp://x/ HTTP/1.1\r\n\r\n",
        b"CONNECT nohost HTTP/1.1\r\n\r\n",
        b"\x00\x01\x02\r\n\r\n",
    ] {
        let (mut client, _host, t) = run(bad, Some(TOKEN), None, true);
        assert_eq!(status(&mut client), 400, "{bad:?}");
        assert_eq!(t.join().unwrap(), Outcome::Refused(400));
    }
    // A head that never completes: the client closes, the proxy answers
    // 400 to whoever is left.
    let (client, server_side) = UnixStream::pair().unwrap();
    client.shutdown(std::net::Shutdown::Write).unwrap();
    let tunnels = Tunnels::default();
    let out = serve_one(
        server_side,
        1,
        tunnels.take(),
        Some(TOKEN),
        |_| id(),
        || Ok::<UnixStream, io::Error>(UnixStream::pair().unwrap().0),
    );
    assert_eq!(out, Outcome::Refused(400));
    assert_eq!(
        parse_request(b"CONNECT a:443 HTTP/1.1\r\n\r\n"),
        Some((Kind::Connect, "a".into(), 443))
    );
    assert_eq!(
        parse_request(b"GET http://a/ HTTP/1.1\r\n\r\n"),
        Some((Kind::Plain, "a".into(), 80))
    );
    assert_eq!(
        parse_request(b"GET http://[::1]:8080/ HTTP/1.1\r\n\r\n"),
        Some((Kind::Plain, "::1".into(), 8080))
    );
    assert_eq!(
        parse_request(b"GET http://a/ HTTP/1.1\r\n"),
        None,
        "incomplete"
    );
}

#[test]
fn guest_dial_fail_502() {
    let started = std::time::Instant::now();
    let (mut client, _none, t) = run(
        b"CONNECT chatgpt.com:443 HTTP/1.1\r\n\r\n",
        Some(TOKEN),
        None,
        false,
    );
    assert_eq!(status(&mut client), 502);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the app sees the error at once"
    );
    assert_eq!(t.join().unwrap(), Outcome::Refused(502));
    // Before the token: 502 as well, and no dial.
    let (mut client, _none, t) = run(b"CONNECT a:443 HTTP/1.1\r\n\r\n", None, None, false);
    assert_eq!(status(&mut client), 502);
    assert_eq!(t.join().unwrap(), Outcome::Refused(502));
}

#[test]
fn guest_cap_503() {
    let tunnels: &'static Tunnels = Box::leak(Box::new(Tunnels::default()));
    let mut held = Vec::new();
    for _ in 0..MAX_TUNNELS {
        held.push(tunnels.take().expect("under the cap"));
    }
    assert_eq!(tunnels.open(), MAX_TUNNELS);
    assert!(tunnels.take().is_none(), "the 513th is refused");
    let (mut client, _host, t) = run(
        b"CONNECT a:443 HTTP/1.1\r\n\r\n",
        Some(TOKEN),
        Some(tunnels),
        true,
    );
    assert_eq!(status(&mut client), 503);
    assert_eq!(t.join().unwrap(), Outcome::Refused(503));
    held.pop();
    assert_eq!(tunnels.open(), MAX_TUNNELS - 1);
    assert!(tunnels.take().is_some(), "a closed tunnel frees its slot");
}

#[test]
fn token_is_shared_with_the_main_loop() {
    set_token("abc");
    assert_eq!(token().as_deref(), Some("abc"));
}
