//! The host side of the guest proxy route: the header line in front of
//! the request, the token check, the line bound, and the tunnel close at
//! wake.

use super::super::tests::{connect, echo_server, request, roundtrip, start};
use super::super::*;
use crate::macvm::proto::{Identity, ProxyHeader, LINE_MAX};
use std::io::Write;
use std::sync::Mutex;

const TOKEN: &str = "plantedtoken0123plantedtoken0123";

fn header(host: &str, port: u16, token: &str) -> String {
    ProxyHeader::new(
        token,
        host,
        port,
        Identity {
            pid: Some(42),
            app: Some("com.example.app".into()),
            team: Some("TEAM123456".into()),
            path: Some("/Applications/Example.app/Contents/MacOS/Example".into()),
            parent_app: None,
        },
    )
    .line()
}

fn seen_cfg(port: u16) -> (HostProxy, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let cfg = HostProxy {
        allow_loopback: vec![port],
        token: Some(TOKEN.into()),
        egress: Some(Egress(Arc::new(
            move |host: &str, id: Option<&Identity>| {
                s2.lock().unwrap().push(format!(
                    "{host} {}",
                    id.map(|i| i.to_string()).unwrap_or_else(|| "none".into())
                ));
            },
        ))),
        ..Default::default()
    };
    (cfg, seen)
}

#[test]
fn host_header_and_bare_reach_same_upstream() {
    let port = echo_server();
    let (cfg, seen) = seen_cfg(port);
    let (_d, sock) = start(cfg);
    let target = format!("localhost:{port}");
    // With the header line: the identity reaches the egress sink.
    let (code, mut s) = request(
        &sock,
        &format!(
            "{}CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n",
            header("localhost", port, TOKEN)
        ),
    );
    assert_eq!(code, 200);
    roundtrip(&mut s, b"with header");
    // Bare: the same tunnel, no identity.
    let (code, mut s) = connect(&sock, &target);
    assert_eq!(code, 200);
    roundtrip(&mut s, b"bare");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(
        seen[0],
        "localhost app = com.example.app, team = TEAM123456, pid = 42"
    );
    assert_eq!(seen[1], "localhost none");
}

#[test]
fn host_header_identity_reaches_the_boundary_sink() {
    let hits = Arc::new(Mutex::new(Vec::new()));
    let h2 = hits.clone();
    let (_d, sock) = start(HostProxy {
        token: Some(TOKEN.into()),
        boundary: Some(Boundary(Arc::new(
            move |t: &str, r: &str, id: Option<&Identity>| {
                h2.lock().unwrap().push(format!(
                    "{t} | {r} | {}",
                    id.map(|i| i.app_name()).unwrap_or_default()
                ));
            },
        ))),
        ..Default::default()
    });
    let (code, _) = request(
        &sock,
        &format!(
            "{}CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n",
            header("127.0.0.1", 1, TOKEN)
        ),
    );
    assert_eq!(code, 403);
    let hits = hits.lock().unwrap().clone();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].starts_with("127.0.0.1:1 | host loopback port"),
        "{hits:?}"
    );
    assert!(hits[0].ends_with("| com.example.app"), "{hits:?}");
}

#[test]
fn host_bad_token_refused() {
    let port = echo_server();
    let (cfg, seen) = seen_cfg(port);
    let (_d, sock) = start(cfg);
    let target = format!("localhost:{port}");
    let mut s = UnixStream::connect(&sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(
        format!(
            "{}CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n",
            header("localhost", port, "othertoken")
        )
        .as_bytes(),
    )
    .unwrap();
    let mut buf = Vec::new();
    let n = s.read_to_end(&mut buf).unwrap();
    assert_eq!(n, 0, "closed without an answer: {buf:?}");
    assert!(seen.lock().unwrap().is_empty(), "nothing was opened");
    // Without a session token the host takes any header.
    let (_d2, sock2) = start(HostProxy {
        allow_loopback: vec![port],
        ..Default::default()
    });
    let (code, _) = request(
        &sock2,
        &format!(
            "{}CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n",
            header("localhost", port, "any")
        ),
    );
    assert_eq!(code, 200);
}

#[test]
fn host_long_first_line_closes() {
    let port = echo_server();
    let (cfg, _) = seen_cfg(port);
    let (_d, sock) = start(cfg);
    let mut s = UnixStream::connect(&sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut line = b"{\"v\":1,\"token\":\"".to_vec();
    line.resize(LINE_MAX + 1, b'x');
    line.push(b'\n');
    // The proxy may close before the whole line is written.
    let _ = s.write_all(&line);
    let mut buf = Vec::new();
    let n = s.read_to_end(&mut buf).unwrap_or(0);
    assert_eq!(n, 0, "closed without an answer: {buf:?}");
    // A line that is JSON but not a header closes too.
    let mut s = UnixStream::connect(&sock).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"{\"not\":\"a header\"}\nCONNECT a:1 HTTP/1.1\r\n\r\n")
        .unwrap();
    let mut buf = Vec::new();
    assert_eq!(s.read_to_end(&mut buf).unwrap(), 0);
}

#[test]
fn close_idle_tunnels_ends_the_open_ones() {
    let port = echo_server();
    let (cfg, _) = seen_cfg(port);
    let (_d, sock) = start(cfg);
    let (code, mut s) = connect(&sock, &format!("localhost:{port}"));
    assert_eq!(code, 200);
    roundtrip(&mut s, b"alive");
    assert!(open_tunnels() >= 1);
    assert_eq!(
        close_tunnels_idle_for(Duration::from_secs(3600)),
        0,
        "it is not idle that long"
    );
    std::thread::sleep(Duration::from_millis(1100));
    assert!(
        close_idle_tunnels() >= 1,
        "idle for a second: closed at wake"
    );
    let mut buf = [0u8; 8];
    let end = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => panic!("unexpected bytes"),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if std::time::Instant::now() < end => {
                let _ = e;
            }
            Err(e) => panic!("the tunnel did not close: {e}"),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn refusals_reach_the_boundary_log() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("run").join(crate::boundary::FILE);
    let l2 = log.clone();
    let sink = Boundary(Arc::new(
        move |target: &str, rule: &str, _: Option<&Identity>| {
            let e = crate::boundary::Entry::new(crate::boundary::NETWORK, target, rule);
            crate::boundary::record_at(&l2, e).unwrap();
        },
    ));
    let port = echo_server();
    let (_d, sock) = start(HostProxy {
        boundary: Some(sink.clone()),
        ..Default::default()
    });
    assert_eq!(connect(&sock, &format!("127.0.0.1:{port}")).0, 403);
    assert_eq!(connect(&sock, &format!("127.0.0.1:{port}")).0, 403);
    let cfg = HostProxy {
        boundary: Some(sink),
        ..Default::default()
    };
    assert!(super::super::check_target(&cfg, "192.168.1.1", 22, None).is_err());
    let lines = crate::boundary::read_at(&log);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].target, format!("127.0.0.1:{port}"));
    assert_eq!(lines[0].count, 2);
    assert!(lines[0].rule.contains("loopback"));
    assert_eq!(lines[1].target, "192.168.1.1:22");
    assert!(lines[1].rule.contains("lan"));
}
