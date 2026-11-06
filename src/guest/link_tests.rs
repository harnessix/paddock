use super::*;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::Mutex;

#[test]
fn the_helper_is_known_by_its_executable_in_a_bundle() {
    let b = bundle_path(Path::new("/Library/Application Support/paddock"));
    assert_eq!(
        b,
        Path::new("/Library/Application Support/paddock/link/Paddock Links.app")
    );
    assert!(is_helper(&executable(&b)));
    assert!(!is_helper(Path::new(
        "/Library/Application Support/paddock/current/paddock-guest"
    )));
    assert!(!is_helper(Path::new("/tmp/paddock-link")));
}

#[test]
fn the_info_plist_claims_http_and_https_as_a_background_app() {
    let p = info_plist("1.2.3");
    assert!(p.starts_with("<?xml"));
    for want in [
        "<string>dev.paddock.link</string>",
        "<string>paddock-link</string>",
        "<key>LSUIElement</key>\n\t<true/>",
        "<string>http</string>",
        "<string>https</string>",
        "<key>CFBundleURLSchemes</key>",
        "<string>APPL</string>",
        "<string>1.2.3</string>",
    ] {
        assert!(p.contains(want), "{want}");
    }
    assert_eq!(p.matches("<dict>").count(), p.matches("</dict>").count());
    assert_eq!(p.matches("<array>").count(), p.matches("</array>").count());
}

#[test]
fn ensure_bundle_copies_the_agent_only_when_it_changed() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let exe = t.path().join("paddock-guest");
    std::fs::write(&exe, b"agent one").unwrap();
    let b = bundle_path(t.path());
    assert!(ensure_bundle(&b, &exe, "1.0.0").unwrap());
    assert_eq!(std::fs::read(executable(&b)).unwrap(), b"agent one");
    let mode = std::fs::metadata(executable(&b))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert!(
        !ensure_bundle(&b, &exe, "1.0.0").unwrap(),
        "nothing changed"
    );
    std::fs::write(&exe, b"agent two").unwrap();
    assert!(ensure_bundle(&b, &exe, "1.0.0").unwrap());
    assert_eq!(std::fs::read(executable(&b)).unwrap(), b"agent two");
    assert!(ensure_bundle(&b, &exe, "1.0.1").unwrap(), "a new version");
    assert!(!b.join("Contents/MacOS/paddock-link.partial").exists());
}

#[test]
fn only_web_links_of_a_limited_length_pass() {
    assert!(check_url("https://auth.example.com/authorize?x=1").is_ok());
    assert!(check_url("http://127.0.0.1:1455/start").is_ok());
    for bad in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "x-apple-systempreferences:",
        "https://a.example/\nsecond",
        "",
        "HTTPS://a.example/",
    ] {
        assert!(check_url(bad).is_err(), "{bad:?}");
    }
    let long = format!("https://a.example/?q={}", "a".repeat(URL_MAX));
    assert!(check_url(&long).unwrap_err().contains("bytes"));
}

#[test]
fn the_socket_hands_each_link_to_the_sink_and_answers() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("link.sock");
    let listener = bind(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let got = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = {
        let got = got.clone();
        Arc::new(move |u: &str| -> Result<(), String> {
            if u.contains("refuse") {
                return Err("no host".into());
            }
            got.lock().unwrap().push(u.to_string());
            Ok(())
        })
    };
    std::thread::spawn(move || serve(listener, sink));
    send(&path, "https://auth.example.com/a?b=c").unwrap();
    assert_eq!(
        send(&path, "file:///etc/passwd").unwrap_err(),
        text::link_not_web()
    );
    assert_eq!(
        send(&path, "https://refuse.example/").unwrap_err(),
        "no host"
    );
    assert_eq!(
        *got.lock().unwrap(),
        vec!["https://auth.example.com/a?b=c".to_string()]
    );
}

#[test]
fn a_stale_socket_goes_but_another_file_stays() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("link.sock");
    drop(bind(&path).unwrap());
    assert!(bind(&path).is_ok(), "the socket of an earlier agent goes");
    let file = t.path().join("plain");
    std::fs::write(&file, "x").unwrap();
    let e = bind(&file).unwrap_err();
    assert!(e.to_string().contains("is not a socket"), "{e}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn the_helper_says_when_the_agent_is_gone() {
    let t = tempfile::tempdir().unwrap();
    let e = send(&t.path().join("none.sock"), "https://a.example/").unwrap_err();
    assert!(e.contains("cannot reach the paddock agent"), "{e}");
}

#[test]
fn tickets_are_hex_and_never_break_a_line() {
    assert!(valid_ticket("0123456789abcdef0123456789abcdef"));
    assert!(!valid_ticket("0123456789abcde"), "too short");
    assert!(!valid_ticket("0123456789ABCDEF0123"), "upper case");
    assert!(!valid_ticket("0123456789abcdef\n0123"));
    assert!(!valid_ticket(&"a".repeat(65)));
}

/// A guest server on a loopback port, the host side as a socket pair: the
/// dial gets the token line and the ticket line first, then the bytes of
/// the guest server.
#[test]
fn a_forward_dials_only_when_a_guest_program_listens() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut c, _) = server.accept().unwrap();
        let mut line = String::new();
        BufReader::new(c.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        write!(c, "HTTP/1.0 200 OK\r\n\r\ngot {}", line.trim_end()).unwrap();
    });
    let (host, guest) = UnixStream::pair().unwrap();
    let ticket = "00112233445566778899aabbccddeeff";
    let (local, up) = forward_start(
        port,
        ticket,
        Some("tok"),
        |p| connect_local(p, LOCAL_CONNECT),
        || Ok(guest),
    )
    .unwrap();
    std::thread::spawn(move || crate::proxy::splice(local, up, FORWARD_IDLE));
    let mut host = host;
    host.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(host.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    assert_eq!(line, "tok\n");
    line.clear();
    r.read_line(&mut line).unwrap();
    assert_eq!(line, format!("{ticket}\n"));
    host.write_all(b"GET /callback?code=x HTTP/1.1\r\n\r\n")
        .unwrap();
    let mut rest = String::new();
    let _ = r.read_to_string(&mut rest);
    assert!(
        rest.ends_with("got GET /callback?code=x HTTP/1.1"),
        "{rest:?}"
    );
}

#[test]
fn a_forward_to_a_port_where_nothing_listens_never_dials() {
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    let dialed = std::cell::Cell::new(false);
    let e = forward_start(
        port,
        "00112233445566778899aabbccddeeff",
        Some("tok"),
        |p| connect_local(p, LOCAL_CONNECT),
        || {
            dialed.set(true);
            UnixStream::pair().map(|p| p.0)
        },
    )
    .unwrap_err();
    assert!(e.contains("no guest program listens"), "{e}");
    assert!(!dialed.get());
}

#[test]
fn a_forward_needs_the_token_a_port_and_a_ticket() {
    let never = |_: u16| -> io::Result<TcpStream> { panic!("no connect") };
    let no_dial = || -> io::Result<UnixStream> { panic!("no dial") };
    let e = forward_start(
        1455,
        "00112233445566778899aabbccddeeff",
        None,
        never,
        no_dial,
    )
    .unwrap_err();
    assert_eq!(e, text::login_forward_no_token());
    let e = forward_start(
        0,
        "00112233445566778899aabbccddeeff",
        Some("t"),
        never,
        no_dial,
    )
    .unwrap_err();
    assert_eq!(e, text::login_forward_bad());
    let e = forward_start(1455, "x\ny", Some("t"), never, no_dial).unwrap_err();
    assert_eq!(e, text::login_forward_bad());
}

/// The Info.plist of the helper is a valid property list for `plutil`.
#[cfg(target_os = "macos")]
#[test]
fn the_info_plist_lints_on_a_mac() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("Info.plist");
    std::fs::write(&p, info_plist("1.0.0")).unwrap();
    let out = std::process::Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(&p)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}
