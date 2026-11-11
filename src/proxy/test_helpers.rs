use super::*;

pub(crate) fn echo_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut r = c.try_clone().unwrap();
                let mut w = c;
                let _ = std::io::copy(&mut r, &mut w);
            });
        }
    });
    port
}

pub(crate) fn start(cfg: HostProxy) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("p.sock");
    serve(&sock, cfg).unwrap();
    (dir, sock)
}

/// Send a request and return the status code and the open stream.
pub(crate) fn request(sock: &Path, text: &str) -> (u16, UnixStream) {
    request_on_stream(UnixStream::connect(sock).unwrap(), text)
}

fn request_on_stream(mut s: UnixStream, text: &str) -> (u16, UnixStream) {
    if let Err(error) = s.set_read_timeout(Some(Duration::from_secs(10))) {
        // macOS can report EINVAL after the peer closes. Use nonblocking reads
        // in that case so a missing response still fails without hanging.
        assert!(
            cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EINVAL),
            "failed to set read timeout: {error}"
        );
        s.set_nonblocking(true).unwrap();
    }
    // A proxy at its connection limit answers 503 and closes without reading,
    // so the write can fail. The answer is still there to read.
    let _ = s.write_all(text.as_bytes());
    let (head, _) = read_head(&mut s).unwrap();
    let mut h = [httparse::EMPTY_HEADER; 16];
    let mut r = httparse::Response::new(&mut h);
    r.parse(&head).unwrap();
    (r.code.unwrap(), s)
}

#[test]
fn reads_a_response_after_the_peer_closes() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("p.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let client = UnixStream::connect(&sock).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
    drop(peer);

    let (code, _) = request_on_stream(client, "GET / HTTP/1.1\r\n\r\n");
    assert_eq!(code, 503);
}

pub(crate) fn connect(sock: &Path, target: &str) -> (u16, UnixStream) {
    request(
        sock,
        &format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"),
    )
}

pub(crate) fn roundtrip(s: &mut UnixStream, msg: &[u8]) {
    s.write_all(msg).unwrap();
    let mut buf = vec![0u8; msg.len()];
    s.read_exact(&mut buf).unwrap();
    assert_eq!(buf, msg);
}
