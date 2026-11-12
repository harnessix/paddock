//! The client of the device socket: the same answers and exit codes as the
//! file protocol of `/dev/paddock`.

use super::*;
use crate::credentials::secret::{Secret, SecretFiles};
use crate::devices::socket::{serve, Server};
use crate::devices::{Call, DevError, Device, Echo, Info, Limits};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Fails with the error that the request names.
struct Fails;

const ERRORS: &[(&str, DevError)] = &[
    ("invalid", DevError::Invalid),
    ("failed", DevError::Failed),
    ("too-large", DevError::TooLarge),
    ("cancelled", DevError::Cancelled),
    ("waiting", DevError::Waiting),
    ("denied", DevError::Denied),
    ("budget", DevError::BudgetUsed),
];

impl Device for Fails {
    fn name(&self) -> &str {
        "fails"
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: "an error name".into(),
            response: "none".into(),
            limits: Limits {
                max_request: 32,
                max_response: 32,
                deadline_seconds: 5,
            },
        }
    }
    fn call(&self, _call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let r = String::from_utf8_lossy(request);
        Err(ERRORS
            .iter()
            .find(|(n, _)| *n == r)
            .map(|(_, e)| *e)
            .unwrap_or(DevError::Failed))
    }
}

/// The server, its socket and its folder, in the order they must go.
fn server() -> (
    crate::devices::socket::Serving,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("devices.sock");
    let secrets = SecretFiles::new(None, None);
    secrets.insert("T", Secret::from("tok"));
    let s = serve(
        &path,
        Server::new("box1", vec![Arc::new(Echo), Arc::new(Fails)], secrets),
    )
    .unwrap();
    (s, path, dir)
}

#[test]
fn the_client_uses_the_socket_when_there_is_one() {
    let (_s, sock, _d) = server();
    // The folder does not exist: only the socket can answer.
    let nowhere = std::path::Path::new("/nonexistent/paddock-devices");
    assert_eq!(
        call_with(Some(&sock), nowhere, "echo", b"hi").unwrap(),
        b"hi"
    );
    assert_eq!(
        call_with(Some(&sock), nowhere, "secret/T", b"").unwrap(),
        b"tok"
    );
    let cat = catalog_with(Some(&sock), nowhere).unwrap();
    assert!(String::from_utf8_lossy(&cat).contains("\"echo\""));
    // Without the socket, the folder.
    assert_eq!(call_with(None, nowhere, "echo", b"hi"), Err(libc::ENOENT));
}

#[test]
fn without_a_socket_a_secret_is_a_plain_read_of_the_file() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("secret")).unwrap();
    std::fs::write(d.path().join("secret/T"), "v").unwrap();
    assert_eq!(call_with(None, d.path(), "secret/T", b"").unwrap(), b"v");
    assert_eq!(secret_name("secret/T"), Some("T"));
    assert_eq!(secret_name("secret/"), None);
    assert_eq!(secret_name("secret/a/b"), None);
    assert_eq!(secret_name("echo"), None);
}

#[test]
fn each_errno_maps_to_the_exit_code_of_the_file_path() {
    let (_s, sock, _d) = server();
    let root = std::path::Path::new("/nonexistent");
    for (name, e) in ERRORS {
        let got = call_with(Some(&sock), root, "fails", name.as_bytes()).unwrap_err();
        assert_eq!(got, e.errno(), "{name}");
        assert_eq!(exit_for_errno(got), exit_for_errno(e.errno()), "{name}");
    }
    let still = call_with(Some(&sock), root, "fails", b"waiting").unwrap_err();
    assert_eq!(exit_for_errno(still), crate::waiting::EXIT_STILL_WAITING);
    // The errors of the server itself.
    let big = call_with(Some(&sock), root, "fails", &[b'x'; 64]).unwrap_err();
    assert_eq!((big, exit_for_errno(big)), (libc::EFBIG, 10));
    let none = call_with(Some(&sock), root, "nope", b"").unwrap_err();
    assert_eq!((none, exit_for_errno(none)), (libc::ENOENT, 4));
    let secret = call_with(Some(&sock), root, "secret/NOPE", b"").unwrap_err();
    assert_eq!(secret, libc::ENOENT);
}

#[test]
fn a_stream_over_the_socket_is_printed_as_it_comes_and_a_late_failure_is_7() {
    use crate::devices::tests::streaming::{Ab, After};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("devices.sock");
    let ab = Ab::new("ab", After::B);
    let _s = serve(
        &path,
        Server::new(
            "box1",
            vec![
                Arc::new(ab),
                Arc::new(Ab::new("ab-fail", After::Fail)),
                Arc::new(Ab::new("ab-wait", After::Wait)),
            ],
            SecretFiles::new(None, None),
        ),
    )
    .unwrap();
    let root = std::path::Path::new("/nonexistent");
    // Each piece reaches the output when the device writes it.
    struct Stamp(Vec<(Instant, Vec<u8>)>);
    impl std::io::Write for Stamp {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.push((Instant::now(), b.to_vec()));
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Stamp(Vec::new());
    assert_eq!(
        run_with(Some(&path), root, "ab", b"x", &mut out).unwrap(),
        2
    );
    assert_eq!(out.0.len(), 2);
    assert_eq!((&out.0[0].1[..], &out.0[1].1[..]), (&b"a"[..], &b"b"[..]));
    assert!(out.0[1].0.duration_since(out.0[0].0) >= Duration::from_millis(150));
    // A failure after the first byte: the byte is out, exit 7.
    let mut got = Vec::new();
    let p = run_with(Some(&path), root, "ab-fail", b"x", &mut got).unwrap_err();
    assert_eq!(got, b"a");
    assert_eq!((p.errno, p.after, exit_for(p)), (libc::EIO, 1, 7));
    // Still waiting after a byte is a close too on the socket: the client
    // cannot tell it from a failure, and says so with 7.
    let mut got = Vec::new();
    let p = run_with(Some(&path), root, "ab-wait", b"x", &mut got).unwrap_err();
    assert_eq!(got, b"a");
    assert_eq!(exit_for(p), 7);
    // The collecting call sees the whole response, or the errno.
    assert_eq!(call_with(Some(&path), root, "ab", b"x").unwrap(), b"ab");
    assert_eq!(
        call_with(Some(&path), root, "ab-fail", b"x"),
        Err(libc::EIO)
    );
}
