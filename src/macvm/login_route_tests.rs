use super::*;
use crate::broker::Answer;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;

const TOKEN: &str = "tok-of-the-session";

fn fast() -> Timing {
    Timing {
        window: Duration::from_secs(5),
        grace: Duration::from_millis(300),
        dial_wait: Duration::from_millis(400),
    }
}

/// A free port of the loopback, for a callback.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn login_url(port: u16) -> String {
    format!(
        "https://auth.example.com/oauth/authorize?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Fauth%2Fcallback&state=s"
    )
}

// ---- The decision --------------------------------------------------------

#[test]
fn the_decision_follows_the_mode_of_open_url() {
    let u = "https://auth.example.com/authorize";
    assert_eq!(decide(u, Mode::Allow, false, false), Decision::Open);
    assert_eq!(decide(u, Mode::Ask, false, false), Decision::Ask);
    assert_eq!(decide(u, Mode::Ask, true, false), Decision::Open);
    assert_eq!(
        decide(u, Mode::Deny, true, false),
        Decision::Refuse(text::link_mode_deny())
    );
    assert_eq!(
        decide(u, Mode::Allow, false, true),
        Decision::Refuse(text::link_denied_for_session())
    );
    // Only http and https, whatever the mode.
    assert!(matches!(
        decide("file:///etc/passwd", Mode::Allow, true, false),
        Decision::Refuse(_)
    ));
}

#[test]
fn the_card_is_the_open_url_card_of_the_linux_route() {
    let who = Asker {
        profile: "sample-notes".into(),
        label: "Sample Notes".into(),
        dir: "/p".into(),
    };
    let u = login_url(1455);
    let p = card("id1", &u, &who, 100, 120);
    assert_eq!(p.capability, "open-url");
    assert_eq!(p.summary, format!("open {u}"));
    assert!(p.detail.ends_with(&u));
    assert_eq!(p.sandbox, "macvm-sample-notes");
    assert!(p.reason.starts_with("Sample Notes"));
    assert!(p.session_allowed);
    assert_eq!((p.created, p.expires), (100, 220));
}

// ---- The windows ---------------------------------------------------------

#[test]
fn a_window_opens_moves_and_ends() {
    let t = Instant::now();
    let mut w = Windows::new(fast());
    assert!(!w.active(1455, t));
    assert!(w.allow(1455, t));
    assert!(w.active(1455, t + Duration::from_secs(4)));
    assert!(!w.active(1456, t), "never another port");
    // A second link of the same port moves the end, it is not new.
    assert!(!w.allow(1455, t + Duration::from_secs(3)));
    assert!(w.active(1455, t + Duration::from_secs(7)));
    assert!(!w.active(1455, t + Duration::from_secs(8)));
    assert_eq!(w.ports(t), vec![1455]);
    assert!(!w.expire(1455, t), "not over yet");
    assert!(w.expire(1455, t + Duration::from_secs(9)));
    assert!(!w.expire(1455, t + Duration::from_secs(9)), "once");
    assert!(w.ports(t).is_empty());
}

#[test]
fn the_first_completed_callback_ends_the_window_after_the_grace() {
    let t = Instant::now();
    let mut w = Windows::new(fast());
    w.allow(1455, t);
    assert!(w.completed(1455, t + Duration::from_secs(1)));
    assert!(!w.completed(1455, t + Duration::from_secs(1)), "once");
    assert!(w.active(1455, t + Duration::from_millis(1200)));
    assert!(!w.active(1455, t + Duration::from_millis(1400)));
    // A new link of the port in the grace does not open it again.
    assert!(!w.allow(1455, t + Duration::from_millis(1100)));
    assert!(!w.active(1455, t + Duration::from_millis(1400)));
    assert!(w.was_completed(1455));
    // After the end, a new link opens a new window.
    assert!(w.expire(1455, t + Duration::from_secs(2)));
    assert!(w.allow(1455, t + Duration::from_secs(3)));
    assert!(!w.was_completed(1455));
    assert!(w.active(1455, t + Duration::from_secs(7)));
}

#[test]
fn a_port_that_is_not_open_never_completes() {
    let mut w = Windows::new(fast());
    assert!(!w.completed(1455, Instant::now()));
}

// ---- The forwarder -------------------------------------------------------

/// The guest of a test: a server on its own loopback port that answers
/// with the request line it got, and an agent that forwards each ask to
/// it with the right token, as `guest/link.rs` does.
struct Guest {
    server_port: u16,
    got: mpsc::Receiver<String>,
}

fn guest_server() -> Guest {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server_port = l.local_addr().unwrap().port();
    let (tx, got) = mpsc::channel();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { break };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut line = String::new();
                let _ = BufReader::new(c.try_clone().unwrap()).read_line(&mut line);
                let line = line.trim_end().to_string();
                let _ = tx.send(line.clone());
                let _ = write!(c, "HTTP/1.0 200 OK\r\n\r\ngot {line}");
            });
        }
    });
    Guest { server_port, got }
}

/// Callbacks whose agent dials with `token` and connects to the guest
/// server.
fn forwarder(token: &'static str, server_port: u16) -> (Callbacks, mpsc::Receiver<Note>) {
    let (ntx, notes) = mpsc::channel();
    let me: Arc<Mutex<Option<Callbacks>>> = Arc::new(Mutex::new(None));
    let me2 = me.clone();
    let cb = Callbacks::new(
        TOKEN.into(),
        fast(),
        move |_port, ticket| {
            let cb = me2.lock().unwrap().clone().unwrap();
            std::thread::spawn(move || {
                let (host, guest) = UnixStream::pair().unwrap();
                std::thread::spawn(move || cb.guest_dial(host));
                let local = crate::guest::link::connect_local(server_port, Duration::from_secs(1));
                let Ok(local) = local else { return };
                let mut guest = guest;
                let _ = write!(guest, "{token}\n{ticket}\n");
                crate::proxy::splice(local, guest, Duration::from_secs(5));
            });
        },
        move |n| {
            let _ = ntx.send(n);
        },
    );
    *me.lock().unwrap() = Some(cb.clone());
    (cb, notes)
}

fn next(notes: &mpsc::Receiver<Note>, want: impl Fn(&Note) -> bool) -> Note {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let n = notes.recv_timeout(left).expect("the note came");
        if want(&n) {
            return n;
        }
    }
}

fn get(port: u16, path: &str) -> std::io::Result<String> {
    let mut c = TcpStream::connect(("127.0.0.1", port))?;
    c.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(c, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")?;
    let mut out = String::new();
    c.read_to_string(&mut out)?;
    Ok(out)
}

#[test]
fn a_callback_reaches_the_guest_and_closes_the_port_after_the_grace() {
    let guest = guest_server();
    let (cb, notes) = forwarder(TOKEN, guest.server_port);
    let port = free_port();
    assert_eq!(cb.open_url(&login_url(port)), vec![port]);
    let Note::Listening { addrs, .. } = next(&notes, |n| matches!(n, Note::Listening { .. }))
    else {
        unreachable!()
    };
    assert!(!addrs.is_empty());
    assert!(
        addrs
            .iter()
            .all(|a| a.ip().is_loopback() && a.port() == port),
        "{addrs:?}"
    );
    let body = get(port, "/auth/callback?code=x").unwrap();
    assert!(
        body.ends_with("got GET /auth/callback?code=x HTTP/1.1"),
        "{body:?}"
    );
    assert_eq!(
        guest.got.recv_timeout(Duration::from_secs(5)).unwrap(),
        "GET /auth/callback?code=x HTTP/1.1"
    );
    let Note::Forwarded { up, down, .. } = next(&notes, |n| matches!(n, Note::Forwarded { .. }))
    else {
        unreachable!()
    };
    assert!(up > 0 && down > 0);
    assert_eq!(
        next(&notes, |n| matches!(n, Note::Completed { .. })),
        Note::Completed { port }
    );
    assert_eq!(
        next(&notes, |n| matches!(n, Note::Closed { .. })),
        Note::Closed {
            port,
            completed: true
        }
    );
    // The port is closed on the host now.
    std::thread::sleep(Duration::from_millis(300));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    assert!(cb.ports().is_empty());
}

#[test]
fn a_link_without_a_loopback_callback_opens_no_port() {
    let (cb, _notes) = forwarder(TOKEN, 1);
    assert!(cb.open_url("https://github.com/login/device").is_empty());
    assert!(cb
        .open_url("https://a.example/?redirect_uri=https%3A%2F%2Fevil.example%3A9%2F")
        .is_empty());
    assert!(cb.ports().is_empty());
}

#[test]
fn a_dial_with_a_wrong_token_gets_nothing() {
    let guest = guest_server();
    let (cb, notes) = forwarder("not-the-token", guest.server_port);
    let port = free_port();
    cb.open_url(&login_url(port));
    next(&notes, |n| matches!(n, Note::Listening { .. }));
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(c, "GET /auth/callback?code=x HTTP/1.1\r\n\r\n").unwrap();
    // The host connection waits in vain and closes after the dial wait.
    assert_eq!(
        next(&notes, |n| matches!(n, Note::NoDial { .. })),
        Note::NoDial { port }
    );
    let mut out = Vec::new();
    let _ = c.read_to_end(&mut out);
    assert!(out.is_empty(), "{out:?}");
    // The fake agent connects to the guest server before the host checks
    // the token, as the real one does; the server gets no byte.
    while let Ok(line) = guest.got.recv_timeout(Duration::from_millis(500)) {
        assert!(line.is_empty(), "the guest server got {line:?}");
    }
    assert_eq!(cb.waiting(), 0);
}

#[test]
fn an_unknown_ticket_gets_nothing() {
    let (cb, _notes) = forwarder(TOKEN, 1);
    let (host, mut guest) = UnixStream::pair().unwrap();
    guest
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let t = std::thread::spawn(move || cb.guest_dial(host));
    write!(guest, "{TOKEN}\n00112233445566778899aabbccddeeff\n").unwrap();
    t.join().unwrap();
    let mut out = Vec::new();
    let _ = guest.read_to_end(&mut out);
    assert!(out.is_empty());
}

#[test]
fn a_window_ends_without_a_callback() {
    let (ntx, notes) = mpsc::channel();
    let timing = Timing {
        window: Duration::from_millis(300),
        ..fast()
    };
    let cb = Callbacks::new(
        TOKEN.into(),
        timing,
        |_, _| {},
        move |n| {
            let _ = ntx.send(n);
        },
    );
    let port = free_port();
    cb.listen(port);
    next(&notes, |n| matches!(n, Note::Listening { .. }));
    assert_eq!(
        next(&notes, |n| matches!(n, Note::Closed { .. })),
        Note::Closed {
            port,
            completed: false
        }
    );
}

#[test]
fn a_port_that_a_host_program_holds_is_named() {
    let (ntx, notes) = mpsc::channel();
    let cb = Callbacks::new(
        TOKEN.into(),
        fast(),
        |_, _| {},
        move |n| {
            let _ = ntx.send(n);
        },
    );
    let v4 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = v4.local_addr().unwrap().port();
    let v6 = std::net::TcpListener::bind(("::1", port)).ok();
    cb.listen(port);
    let n = next(&notes, |n| {
        matches!(n, Note::BindFailed { .. } | Note::Listening { .. })
    });
    match (&v6, n) {
        // Both loopback addresses are taken: the port cannot listen.
        (Some(_), Note::BindFailed { port: p, why }) => {
            assert_eq!(p, port);
            assert!(why.contains("127.0.0.1"), "{why}");
            assert!(cb.ports().is_empty());
        }
        // No IPv6 here: only [::1] was free, the listener takes it.
        (None, Note::Listening { addrs, .. }) => {
            assert!(addrs.iter().all(|a| a.is_ipv6()), "{addrs:?}");
        }
        (_, n) => panic!("{n:?}"),
    }
    drop(v4);
}

// ---- The links -----------------------------------------------------------

struct Fixed {
    answer: Option<Answer>,
    asked: Mutex<Vec<Pending>>,
}

impl Approver for Fixed {
    fn ask(&self, _: &HostEnv, p: &Pending, _: Duration) -> Option<Answer> {
        self.asked.lock().unwrap().push(p.clone());
        self.answer
    }
}

fn links(mode: Mode, answer: Option<Answer>) -> (Links, Arc<Fixed>) {
    let t = tempfile::tempdir().unwrap();
    let env = HostEnv::from_parts(t.path().into(), "u".into(), 501, 20, Default::default());
    let mut policy = HostPolicy::default();
    policy.modes.insert("open-url".into(), mode);
    let fixed = Arc::new(Fixed {
        answer,
        asked: Mutex::new(Vec::new()),
    });
    let who = Asker {
        profile: "sample-notes".into(),
        label: "Sample Notes".into(),
        dir: "/p".into(),
    };
    (Links::new(env, Ok(policy), fixed.clone(), who), fixed)
}

fn quiet() -> Callbacks {
    Callbacks::new(TOKEN.into(), fast(), |_, _| {}, |_| {})
}

#[test]
fn an_allowed_link_opens_and_its_callback_port_listens() {
    let (l, fixed) = links(Mode::Allow, None);
    let opened = Mutex::new(Vec::new());
    let open = |u: &str| -> Result<(), String> {
        opened.lock().unwrap().push(u.to_string());
        Ok(())
    };
    let cb = quiet();
    let port = free_port();
    let u = login_url(port);
    assert_eq!(l.handle(&u, &open, &cb), Opened::Yes(vec![port]));
    assert_eq!(*opened.lock().unwrap(), vec![u]);
    assert!(fixed.asked.lock().unwrap().is_empty(), "allow asks nobody");
    assert_eq!(cb.ports(), vec![port]);
}

#[test]
fn ask_shows_the_card_and_follows_the_answer() {
    let open = |_: &str| -> Result<(), String> { Ok(()) };
    let cb = quiet();
    let u = "https://notes.example.com/auth/login";
    let (l, fixed) = links(Mode::Ask, Some(Answer::Once));
    assert_eq!(l.handle(u, &open, &cb), Opened::Yes(vec![]));
    assert_eq!(l.handle(u, &open, &cb), Opened::Yes(vec![]));
    assert_eq!(fixed.asked.lock().unwrap().len(), 2, "once asks each time");
    assert_eq!(fixed.asked.lock().unwrap()[0].capability, "open-url");

    let (l, fixed) = links(Mode::Ask, Some(Answer::Session));
    l.handle(u, &open, &cb);
    assert_eq!(l.handle(u, &open, &cb), Opened::Yes(vec![]));
    assert_eq!(
        fixed.asked.lock().unwrap().len(),
        1,
        "the session answer holds"
    );

    let (l, _) = links(Mode::Ask, Some(Answer::Deny));
    assert_eq!(
        l.handle(u, &open, &cb),
        Opened::No(text::link_denied(&crate::macvm::proto::link_kind(u)))
    );

    let (l, fixed) = links(Mode::Ask, Some(Answer::DenySession));
    l.handle(u, &open, &cb);
    let Opened::No(line) = l.handle(u, &open, &cb) else {
        panic!()
    };
    assert!(line.contains("for the rest of it"), "{line}");
    assert_eq!(fixed.asked.lock().unwrap().len(), 1);

    let (l, _) = links(Mode::Ask, None);
    let Opened::No(line) = l.handle(u, &open, &cb) else {
        panic!()
    };
    assert!(
        line.contains("nobody answered within 120 seconds"),
        "{line}"
    );
}

#[test]
fn a_refused_link_opens_nothing_and_no_port() {
    let never = |_: &str| -> Result<(), String> { panic!("no open") };
    let cb = quiet();
    let (l, fixed) = links(Mode::Allow, None);
    assert!(matches!(
        l.handle("file:///etc/passwd", &never, &cb),
        Opened::No(_)
    ));
    let (l, _) = links(Mode::Deny, None);
    let Opened::No(line) = l.handle(&login_url(free_port()), &never, &cb) else {
        panic!()
    };
    assert!(line.contains("open-url is deny"), "{line}");
    assert!(fixed.asked.lock().unwrap().is_empty());
    assert!(cb.ports().is_empty());
}

#[test]
fn a_failed_open_listens_on_no_port() {
    let (l, _) = links(Mode::Allow, None);
    let fail = |_: &str| -> Result<(), String> { Err("no browser".into()) };
    let cb = quiet();
    let Opened::No(line) = l.handle(&login_url(free_port()), &fail, &cb) else {
        panic!()
    };
    assert!(line.contains("no browser"), "{line}");
    assert!(cb.ports().is_empty());
}

#[test]
fn an_unreadable_config_refuses_every_link() {
    let (mut l, _) = links(Mode::Allow, None);
    l.policy = Err("[host] wait must be 5 to 3600 seconds".into());
    let never = |_: &str| -> Result<(), String> { panic!("no open") };
    let Opened::No(line) = l.handle("https://a.example/", &never, &quiet()) else {
        panic!()
    };
    assert!(line.contains("wait must be"), "{line}");
}

#[test]
fn the_link_of_the_event_log_has_no_query() {
    let u = login_url(1455);
    let k = crate::macvm::proto::link_kind(&u);
    assert_eq!(
        k,
        format!("https://auth.example.com ({} characters)", u.len())
    );
    assert!(!k.contains("state"));
}
