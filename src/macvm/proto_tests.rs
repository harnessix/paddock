use super::*;

const TOKEN: &str = "plantedtoken0123plantedtoken0123";
const CLIP: &str = "planted clipboard text";
const PASSWORD: &str = "planted-password";

/// One text item, as the clipboard module encodes it.
fn paste(text: &str) -> PasteItem {
    crate::macvm::clipboard::Item::text(text).to_wire()
}

fn window(id: u32) -> Window {
    Window {
        id,
        pid: 501,
        app: "com.openai.chat".into(),
        title: "ChatGPT".into(),
        frame: Frame::new(10.0, 20.0, 800.0, 600.0),
        level: Level::Normal,
        parent: None,
        minimized: false,
        focused: true,
        resizable: true,
        movable: true,
        ..Window::default()
    }
}

fn hello() -> Hello {
    Hello {
        v: VERSION,
        agent: "0.2.1".into(),
        macos: "15.2".into(),
        build: "24C101".into(),
        scale: 2.0,
        display: Display { w: 1728, h: 1117 },
        can_update: true,
        sha256: None,
    }
}

/// One of every event, with the planted secrets where a payload goes.
fn events() -> Vec<Event> {
    vec![
        Event::Hello(hello()),
        Event::Health(Health {
            uptime: 12,
            load: 0.5,
            mem_free: 1 << 30,
            data_disk_free: Some(5 << 30),
            app_running: true,
            input_source: Some("com.apple.keylayout.US".into()),
            data_verify: None,
            accessibility: true,
            locked: false,
        }),
        Event::Windows(WindowsEvent {
            order: vec![1, 2],
            changed: vec![window(1)],
            removed: vec![3],
        }),
        Event::Focus {
            app: "com.openai.chat".into(),
            window: Some(1),
        },
        Event::Clipboard {
            count: 4,
            items: vec![paste(CLIP)],
            transfer: None,
        },
        Event::Menu {
            app: "com.openai.chat".into(),
            items: vec![MenuItem {
                title: "File".into(),
                path: vec![2],
                enabled: true,
                children: vec![MenuItem {
                    title: "New Chat".into(),
                    path: vec![2, 0],
                    shortcut: super::super::menu::Shortcut::parse("Cmd-N"),
                    enabled: true,
                    ..MenuItem::default()
                }],
                ..MenuItem::default()
            }],
            cut: true,
        },
        Event::AppExited {
            app: "com.openai.chat".into(),
            code: 11,
            crash_report: Some(PathBuf::from(
                "/Users/paddock/Library/Logs/DiagnosticReports/ChatGPT-2026-11-04-100000.ips",
            )),
            relaunch: false,
        },
        Event::Pong { t: 1234 },
        Event::AxValue {
            window: 1,
            text: CLIP.into(),
        },
        Event::AxTree {
            window: 1,
            nodes: vec![crate::macvm::axmirror::Node {
                role: "AXButton".into(),
                title: "Bold".into(),
                value: Some("1".into()),
                frame: Frame::new(1.0, 2.0, 3.0, 4.0),
                path: vec![0],
                enabled: true,
                focused: false,
                children: Vec::new(),
            }],
            cut: false,
        },
        Event::Disk {
            data_free: 1 << 30,
            warn: true,
        },
        Event::DataUnmountable {
            text: "diskutil: the volume did not mount".into(),
        },
        Event::DataProgress {
            text: "copying the home: 3 of 9 parts".into(),
            wait_secs: 160,
        },
        Event::Dialog {
            owner: "DiskArbitrationAgent".into(),
            text: "[Initialize… / Ignore / Eject]: pressed Ignore".into(),
            dismissed: true,
        },
        Event::ScreenLock {
            locked: true,
            uptime: 3600,
            idle_secs: Some(42),
        },
        // The state of a login is a secret of the log as the token is.
        Event::LinkOpen {
            url: format!("https://auth.example.com/authorize?state={TOKEN}&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcb"),
        },
        Event::Reply(Reply::ok(7)),
        Event::Reply(Reply::data(
            8,
            ReplyData::InputSet {
                missing: vec!["com.sogou.inputmethod.sogou.pinyin".into()],
            },
        )),
        Event::Reply(Reply::error(
            9,
            ErrorCode::Timeout,
            "AX did not answer\nin 1 s",
        )),
    ]
}

/// The `op` of every request, by hand. A second test keeps this list
/// equal to the examples, and `timeout` has no wildcard arm, so a new
/// request needs an entry in all three places.
const OPS: &[&str] = &[
    "token",
    "window.move",
    "window.raise",
    "window.minimize",
    "window.close",
    "window.focus",
    "app.launch",
    "app.quit",
    "app.copy",
    "clipboard.set",
    "clipboard.watch",
    "drop",
    "display.set",
    "data.prepare",
    "configure",
    "ping",
    "agent.update",
    "agent.restart",
    "input.set",
    "log.fetch",
    "disk.low",
    "menu",
    "menu.press",
    "ax.watch",
    "ax.tree",
    "ax.press",
    "ax.set",
    "shutdown",
    "keychain.unlock",
    "login.forward",
];

/// One of every request, with the planted secrets where a payload goes.
pub(super) fn requests() -> Vec<Request> {
    vec![
        Request::Token {
            token: TOKEN.into(),
            v: VERSION,
            bench: false,
        },
        Request::WindowMove {
            id: 1,
            frame: Frame::new(100.0, 100.0, 800.0, 600.0),
        },
        Request::WindowRaise { id: 1 },
        Request::WindowMinimize { id: 1 },
        Request::WindowClose { id: 1 },
        Request::WindowFocus { id: 1 },
        Request::AppLaunch {
            path: PathBuf::from("/Applications/ChatGPT.app"),
            args: vec!["--x".into()],
            env: [("SECRET".to_string(), TOKEN.to_string())].into(),
        },
        Request::AppQuit {
            app: "com.openai.chat".into(),
            force: false,
        },
        Request::AppCopy {
            from: PathBuf::from("/Volumes/My Shared Files/apps/ChatGPT.app"),
            to: PathBuf::from("/Volumes/paddock/Applications/ChatGPT.app"),
            version: "1.2025.300".into(),
            unquarantine: true,
        },
        Request::ClipboardSet {
            count: 7,
            items: vec![paste(CLIP)],
            transfer: None,
        },
        Request::ClipboardWatch { on: true },
        Request::Drop {
            window: 1,
            point: Point { x: 10.0, y: 10.0 },
            paths: vec![PathBuf::from("/Volumes/My Shared Files/drops/a.txt")],
        },
        Request::DisplaySet {
            w: 1728,
            h: 1117,
            scale: 2.0,
        },
        Request::DataPrepare {
            password: PASSWORD.into(),
            data_id: "d1".into(),
        },
        Request::Configure {
            password: PASSWORD.into(),
            steps: vec![ConfigureStep {
                name: "proxy".into(),
                as_root: true,
                argv: vec!["/usr/sbin/networksetup".into(), "-setsecurewebproxy".into()],
                verify: vec!["/usr/sbin/networksetup".into(), "-getsecurewebproxy".into()],
            }],
        },
        Request::Ping { t: 5 },
        Request::AgentUpdate {
            path: PathBuf::from("/Volumes/My Shared Files/agent/paddock-guest"),
            sha256: "ab".repeat(32),
            version: "0.3.0".into(),
        },
        Request::AgentRestart,
        Request::InputSet {
            sources: vec!["com.apple.keylayout.US".into()],
            current: "com.apple.keylayout.US".into(),
            reduce_motion: true,
            increase_contrast: false,
        },
        Request::LogFetch,
        Request::DiskLow,
        Request::Menu {
            app: "com.openai.chat".into(),
        },
        Request::MenuPress { path: vec![2, 0] },
        Request::AxWatch { on: true },
        Request::AxTree { window: 1 },
        Request::AxPress {
            window: 1,
            path: vec![0, 2],
            role: "AXButton".into(),
        },
        Request::AxSet {
            window: 1,
            path: vec![1, 0],
            role: "AXTextField".into(),
            value: "hello".into(),
        },
        Request::Shutdown {
            password: PASSWORD.into(),
        },
        Request::KeychainUnlock {
            password: PASSWORD.into(),
        },
        Request::LoginForward {
            port: 1455,
            ticket: "00112233445566778899aabbccddeeff".into(),
        },
    ]
}

#[test]
fn the_link_and_the_forward_have_their_wire_names() {
    let ev = Event::LinkOpen {
        url: "https://a.example/".into(),
    };
    assert_eq!(
        serde_json::to_string(&ev).unwrap(),
        r#"{"ev":"link.open","url":"https://a.example/"}"#
    );
    assert_eq!(ev.kind(), "link.open https://a.example (18 characters)");
    let r = Request::LoginForward {
        port: 1455,
        ticket: "ab".repeat(16),
    };
    assert_eq!(op_of(&r), "login.forward");
    assert_eq!(r.kind(), "login.forward port 1455");
    assert!(
        !r.kind().contains("abab"),
        "the ticket stays out of the log"
    );
    assert_eq!(timeout(&r), Duration::from_secs(10));
}

fn op_of(r: &Request) -> String {
    let v = serde_json::to_value(r).unwrap();
    v["op"].as_str().unwrap().to_string()
}

#[test]
fn round_trip_every_message() {
    for ev in events() {
        let mut buf = Vec::new();
        write_line(&mut buf, &ev).unwrap();
        assert_eq!(
            buf.iter().filter(|b| **b == b'\n').count(),
            1,
            "{}",
            ev.kind()
        );
        let back: Event = read_line(&mut buf.as_slice()).unwrap().unwrap();
        assert_eq!(back, ev);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with("{\"ev\":\""), "{text}");
    }
    for (i, r) in requests().into_iter().enumerate() {
        let call = Call {
            req: i as u64 + 1,
            request: r.clone(),
        };
        let mut buf = Vec::new();
        write_line(&mut buf, &call).unwrap();
        let text = String::from_utf8(buf.clone()).unwrap();
        assert!(
            text.starts_with(&format!("{{\"req\":{},\"op\":\"", i + 1)),
            "{text}"
        );
        let back: Call = read_line(&mut buf.as_slice()).unwrap().unwrap();
        assert_eq!(back, call);
    }
    // The reply flattens its outcome next to the id.
    let text = serde_json::to_string(&Event::Reply(Reply::error(9, ErrorCode::NotInSession, "x")))
        .unwrap();
    assert_eq!(
        text,
        r#"{"ev":"reply","req":9,"result":"error","code":"not_in_session","text":"x"}"#
    );
    assert_eq!(
        serde_json::to_string(&Event::Reply(Reply::ok(1))).unwrap(),
        r#"{"ev":"reply","req":1,"result":"ok"}"#
    );
}

#[test]
fn timeout_table_covers_every_request() {
    let reqs = requests();
    let ops: Vec<String> = reqs.iter().map(op_of).collect();
    assert_eq!(ops, OPS, "the examples and the hand list name the same ops");
    for r in &reqs {
        let t = timeout(r);
        assert!(t >= Duration::from_secs(2), "{}: {t:?}", r.kind());
        assert!(t <= Duration::from_secs(300), "{}: {t:?}", r.kind());
    }
    assert_eq!(
        timeout(&Request::AppQuit {
            app: "a".into(),
            force: true
        }),
        Duration::from_secs(10)
    );
    assert_eq!(
        timeout(&Request::Configure {
            password: String::new(),
            steps: vec![]
        }),
        Duration::from_secs(300)
    );
}

#[test]
fn hello_major_mismatch_takes_update_path() {
    let mut h = hello();
    h.v = Version {
        major: VERSION.major + 1,
        minor: 0,
    };
    h.agent = "9.0.0".into();
    assert_eq!(
        check_hello(&h, VERSION, "0.2.1").unwrap(),
        HelloAction::Update
    );
    // An older agent of the same major updates too: the host version wins.
    let mut h = hello();
    h.agent = "0.1.0".into();
    assert_eq!(
        check_hello(&h, VERSION, "0.2.1").unwrap(),
        HelloAction::Update
    );
    // The same version talks.
    assert_eq!(
        check_hello(&hello(), VERSION, "0.2.1").unwrap(),
        HelloAction::Talk
    );
    // A different minor of an agent that cannot update still talks.
    let mut h = hello();
    h.agent = "0.2.0".into();
    h.v = Version {
        major: VERSION.major,
        minor: VERSION.minor + 5,
    };
    h.can_update = false;
    assert_eq!(
        check_hello(&h, VERSION, "0.2.1").unwrap(),
        HelloAction::Talk
    );
}

#[test]
fn hello_without_update_names_macvm_update() {
    let mut h = hello();
    h.v = Version {
        major: VERSION.major + 1,
        minor: 0,
    };
    h.agent = "9.0.0".into();
    h.can_update = false;
    let e = check_hello(&h, VERSION, "0.2.1").unwrap_err().to_string();
    assert!(e.contains("paddock macvm update"), "{e}");
    assert!(e.contains("9.0.0") && e.contains("0.2.1"), "{e}");
}

#[test]
fn kind_strips_token_and_payloads() {
    for ev in events() {
        let k = ev.kind();
        assert!(!k.contains(TOKEN), "{k}");
        assert!(!k.contains(CLIP), "{k}");
        assert!(!k.contains(PASSWORD), "{k}");
    }
    for r in requests() {
        let k = Call { req: 3, request: r }.kind();
        assert!(!k.contains(TOKEN), "{k}");
        assert!(!k.contains(CLIP), "{k}");
        assert!(!k.contains(PASSWORD), "{k}");
        assert!(k.starts_with("req 3 "), "{k}");
    }
    let h = FileHeader {
        token: TOKEN.into(),
        kind: FileKind::Log,
        name: "paddock-guest.log".into(),
        size: 10,
    };
    assert!(!h.kind().contains(TOKEN));
    // Error text stays on one line.
    let r = Reply::error(1, ErrorCode::Io, "a\nb");
    assert_eq!(r.kind(), "reply 1 error Io: a b");
}

#[test]
fn versions_compare_by_major() {
    let a = Version { major: 1, minor: 0 };
    let b = Version { major: 1, minor: 7 };
    let c = Version { major: 2, minor: 0 };
    assert!(a.talks(b));
    assert!(!a.talks(c));
    assert!(a < b && b < c);
    assert_eq!(c.to_string(), "2.0");
}

#[test]
fn a_long_line_is_refused() {
    let mut line = vec![b'x'; LINE_MAX + 1];
    line.push(b'\n');
    let e = read_line::<Event>(&mut line.as_slice())
        .unwrap_err()
        .to_string();
    assert!(e.contains("longer than"), "{e}");
}

#[test]
fn levels_are_seven_distinct_names() {
    let names: std::collections::BTreeSet<String> = Level::ALL
        .iter()
        .map(|l| serde_json::to_string(l).unwrap())
        .collect();
    assert_eq!(names.len(), 7);
    assert!(names.contains("\"popover\""));
}

#[test]
fn a_shutdown_of_an_older_host_has_no_password_and_the_log_shows_none() {
    let call: Call = serde_json::from_str(r#"{"req":3,"op":"shutdown"}"#).unwrap();
    assert_eq!(
        call.request,
        Request::Shutdown {
            password: String::new()
        }
    );
    let r = Request::Shutdown {
        password: PASSWORD.into(),
    };
    assert_eq!(r.kind(), "shutdown");
    assert_eq!(timeout(&r).as_secs(), 30);
    let json = serde_json::to_string(&r).unwrap();
    assert_eq!(
        json,
        format!(r#"{{"op":"shutdown","password":"{PASSWORD}"}}"#)
    );
}

#[test]
fn an_agent_of_the_same_version_but_another_build_updates() {
    let mut h = hello();
    h.agent = "0.2.1".into();
    let host = Some("aa".repeat(32));
    // No hash from the agent (an older build): the version decides.
    assert_eq!(
        check_hello_sha(&h, VERSION, "0.2.1", host.as_deref()).unwrap(),
        HelloAction::Talk
    );
    h.sha256 = Some("AA".repeat(32));
    assert_eq!(
        check_hello_sha(&h, VERSION, "0.2.1", host.as_deref()).unwrap(),
        HelloAction::Talk,
        "the same binary talks, whatever the case of the hash"
    );
    h.sha256 = Some("bb".repeat(32));
    assert_eq!(
        check_hello_sha(&h, VERSION, "0.2.1", host.as_deref()).unwrap(),
        HelloAction::Update
    );
    // An agent that cannot update still talks.
    h.can_update = false;
    assert_eq!(
        check_hello_sha(&h, VERSION, "0.2.1", host.as_deref()).unwrap(),
        HelloAction::Talk
    );
}
