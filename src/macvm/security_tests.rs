//! The 21 items of the security checklist, one test each, in the order and
//! with the names of the table. `s00_table_is_complete` counts them, so a
//! removed test fails the count. Items 5, 10, 11 and 16 have a Mac script
//! next to their test (`packaging/macos/checks/security.sh`).

use super::*;
use crate::env::HostEnv;
use crate::macvm::backup;
use crate::macvm::clipboard::{self, Action, Item};
use crate::macvm::crop::{self, Rect};
use crate::macvm::events::Events;
use crate::macvm::files_port::{self, Limiter, PER_MINUTE, TRANSFER_MAX};
use crate::macvm::firewall::{self, Conn, Decision, Mode, Port, Rules, Scope, Verdict};
use crate::macvm::golden::{Manifest, State};
use crate::macvm::host::{self, Session};
use crate::macvm::layout::{self, Layout};
use crate::macvm::lock::{self, Slot, SlotError};
use crate::macvm::proto::{
    self, Call, ErrorCode, Event, FileHeader, FileKind, Frame, Identity, Level, Outcome, PasteItem,
    ProxyHeader, Request, Version, LINE_MAX, VERSION,
};
use crate::macvm::report;
use crate::macvm::session::{self, Display, Now, Reason, Stamp, Start};
use crate::macvm::shares::{self, Root, Share, RESERVED};
use crate::macvm::stubnet::{self, Dhcp, Dropped, Drops, Proto, Reply, Stub, GUEST_MAC};
use crate::macvm::text::{self, Holder, SessionEnd, ShareWhy, TransferWhy};
use crate::proxy::allow::Allowlist;
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const PASSWORD: &str = "hunter2-planted-7Q";
const CLIP: &str = "CLIPBOARD-MARKER-9f3e";
const PLANT: &str = "TOKEN-PLANT-5c1d9e0a7b3f4a2e8d6c1b0f9e8d7c6b";

fn env(t: &Path) -> HostEnv {
    HostEnv::from_parts(
        t.join("home"),
        "u".into(),
        501,
        20,
        [
            (
                "XDG_DATA_HOME".to_string(),
                t.join("data").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_string(),
                t.join("config").display().to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

fn now() -> Now {
    Now {
        golden: "15.2 (24C101)".into(),
        host_build: "24C101".into(),
        vz: "vz-1".into(),
        protocol: Version { major: 1, minor: 3 },
        paddock: "0.9.0".into(),
        agent: "0.9.0".into(),
        display: Display {
            w: 2560,
            h: 1600,
            scale: 2,
        },
    }
}

/// A profile as `prepare` makes it from a small golden.
fn prepared(layout: &Layout, app: &str) -> Profile {
    std::fs::create_dir_all(&layout.golden.dir).unwrap();
    std::fs::write(&layout.golden.disk, b"golden disk").unwrap();
    std::fs::write(&layout.golden.aux, b"golden aux").unwrap();
    let p = layout.profile(app, "default");
    let mut backup = |_: &Profile, _: &str| Ok(());
    let mut hooks = session::Hooks {
        clone: &session::copy_clone,
        machine_id: &|| Ok(vec![7u8; 16]),
        backup: &mut backup,
        format_data: &|_: &Path| anyhow::bail!("blank in the tests"),
    };
    session::prepare(&p, &layout.golden, &now(), Start::Fresh, &mut hooks).unwrap();
    p
}

fn record(dir: &Path, name: &str, pid: u32, profile: &Path, input: &str) -> PathBuf {
    let s = Session {
        bench: false,
        pid,
        app: name.into(),
        home: host::DEFAULT_HOME.into(),
        profile: profile.to_path_buf(),
        slot: Some(0),
        input_source: input.into(),
        started: 1,
        control: "/c.sock".into(),
        badge: None,
        health: None,
        start: None,
        app_info: None,
        network: None,
        network_mode: String::new(),
        shares: vec![],
        menu: None,
        ax: None,
        ..Default::default()
    };
    let path = dir.join(format!("{name}--default.json"));
    std::fs::write(&path, serde_json::to_vec(&s).unwrap()).unwrap();
    path
}

#[test]
fn s00_table_is_complete() {
    let src = concat!(
        include_str!("security_tests.rs"),
        include_str!("security_more_tests.rs")
    );
    let names: Vec<&str> = src
        .split("\n#[test]\nfn ")
        .skip(1)
        .map(|rest| rest.split('(').next().unwrap())
        .filter(|n| n.starts_with('s') && n[1..3].bytes().all(|b| b.is_ascii_digit()))
        .collect();
    let table = [
        "s01_shares_refuse_roots",
        "s02_shares_only_named",
        "s03_password_guard",
        "s04_token",
        "s05_modes",
        "s06_forged_identity",
        "s07_forwards",
        "s08_stub_drop",
        "s09_clipboard_key",
        "s10_crash_record",
        "s11_doctor_permissions",
        "s12_golden_flags",
        "s13_control_single",
        "s14_files_limits",
        "s15_header_bounds",
        "s16_slots",
        "s17_agent_update_hash",
        "s18_bundle_redaction",
        "s19_rules_file",
        "s20_window_size_levels",
        "s21_input_restore",
    ];
    let items: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| *n != "s00_table_is_complete")
        .collect();
    assert_eq!(items.len(), 21, "{items:?}");
    assert_eq!(items, table);
}

#[test]
fn s01_shares_refuse_roots() {
    let home = Path::new("/Users/ana");
    let roots = [
        "/",
        "/Users/ana",
        "/Users",
        "/Volumes",
        "/private",
        "/tmp",
        "/var",
        "/etc",
        "/Library",
        "/System",
        "/Applications",
    ];
    for r in roots {
        assert!(
            shares::refusal(Path::new(r), home).is_some(),
            "{r} must be refused"
        );
    }
    // A parent of one of them, and a child of a system folder.
    assert_eq!(
        shares::refusal(Path::new("/Users"), home),
        Some(ShareWhy::AllHomes)
    );
    assert_eq!(
        shares::refusal(
            Path::new("/Volumes/Data/Users"),
            Path::new("/Volumes/Data/Users/ana")
        ),
        Some(ShareWhy::ContainsHome)
    );
    assert_eq!(shares::refusal(Path::new("/"), home), Some(ShareWhy::Root));
    assert_eq!(
        shares::refusal(Path::new("/private/etc"), home),
        Some(ShareWhy::SystemFiles)
    );
    assert_eq!(
        shares::refusal(Path::new("/Users/ana/work/../.."), home),
        Some(ShareWhy::AllHomes)
    );
    assert_eq!(
        shares::refusal(Path::new("/Users/ana/.ssh"), home),
        Some(ShareWhy::Protected)
    );
    assert_eq!(shares::refusal(Path::new("/Users/ana/work"), home), None);
}

#[test]
fn s02_shares_only_named() {
    // The scratch folder is the home of the test: the temporary folder of
    // the system is a system folder, but a folder in the home is not.
    let t = crate::macvm::shares::tests::scratch();
    let p = Profile::new(t.path().join("p"));
    let agent = t.path().join("agent");
    let b = shares::build("app", &[], &p, Some(&agent), t.home(), &[], false).unwrap();
    let names: Vec<&str> = b.shares.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, RESERVED);
    assert!(b.shares.iter().all(|s| !s.is_workspace()));
    assert!(b
        .shares
        .iter()
        .filter(|s| s.name == "apps" || s.name == "agent")
        .all(|s| s.read_only));
    // A named folder is `ws-1`, after it; a name of the user never shadows a reserved one.
    let ws = t.path().join("drop");
    std::fs::create_dir(&ws).unwrap();
    let roots = [Root {
        path: ws.clone(),
        read_only: true,
        unprotected: false,
    }];
    let b = shares::build("app", &roots, &p, None, t.home(), &[], false).unwrap();
    assert_eq!(
        b.shares[0],
        Share {
            name: "ws-1".into(),
            host: ws,
            read_only: true
        }
    );
    assert_eq!(b.shares.len(), 4);
}

/// The guest agent with the fake platform, one control connection.
fn agent_pair() -> (
    UnixStream,
    Arc<Mutex<crate::guest::platform::fake::FakeState>>,
    std::thread::JoinHandle<i32>,
) {
    use crate::guest::platform::fake::Fake;
    use crate::guest::windows::RawWindow;
    let (a, b) = UnixStream::pair().unwrap();
    b.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let (mut fake, st) = Fake::new();
    {
        let mut s = st.lock().unwrap();
        s.windows = vec![RawWindow {
            id: 1,
            pid: 100,
            owner: "TextEdit".into(),
            bundle: Some("com.apple.TextEdit".into()),
            layer: 0,
            bounds: Frame::new(10.0, 20.0, 800.0, 600.0),
            alpha: 1.0,
        }];
        s.procs = [(1, 0)].into();
        s.front = Some(100);
        s.launch_pid = 100;
    }
    let queue = Mutex::new(Some(a));
    let agent = std::thread::spawn(move || {
        crate::guest::main_loop::run(&mut fake, || {
            queue
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| std::io::Error::other("done"))
        })
    });
    (b, st, agent)
}

fn send(s: &mut UnixStream, req: u64, request: Request) -> Outcome {
    proto::write_line(s, &Call { req, request }).unwrap();
    loop {
        let ev: Event = proto::read_line(s).unwrap().expect("open");
        if let Event::Reply(r) = ev {
            if r.req == req {
                return r.outcome;
            }
        }
    }
}

#[test]
fn s03_password_guard() {
    let (mut host, st, agent) = agent_pair();
    let Event::Hello(_) = proto::read_line::<Event>(&mut host).unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(
        send(
            &mut host,
            1,
            Request::Token {
                bench: false,
                now_ms: None,
                token: TOKEN.into(),
                v: VERSION
            }
        ),
        Outcome::Ok { data: None }
    );
    assert!(matches!(
        send(
            &mut host,
            2,
            Request::AppLaunch {
                path: "/x.app".into(),
                args: vec![],
                env: Default::default()
            }
        ),
        Outcome::Ok { .. }
    ));
    // The guard: an app pid is present, so password requests are refused.
    for (req, r) in [
        (
            6,
            Request::ClockSet {
                unix_ms: 1,
                password: PASSWORD.into(),
            },
        ),
        (
            3,
            Request::Configure {
                password: PASSWORD.into(),
                steps: vec![],
            },
        ),
        (
            4,
            Request::DataPrepare {
                password: PASSWORD.into(),
                data_id: "d-1".into(),
            },
        ),
    ] {
        assert!(
            matches!(
                send(&mut host, req, r),
                Outcome::Error {
                    code: ErrorCode::NotInSession,
                    ..
                }
            ),
            "request {req} must be refused while the app runs"
        );
    }
    assert!(st.lock().unwrap().data_prepares.is_empty());
    // `agent.restart` ends the loop with exit 0; `shutdown` would reconnect.
    assert_eq!(
        send(&mut host, 5, Request::AgentRestart),
        Outcome::Ok { data: None }
    );
    drop(host);
    assert_eq!(agent.join().unwrap(), 0);
    // The password is in no other session request: only these five carry
    // the field. `keychain.unlock` gives it to the Security framework of
    // the guest, never to a command line (`guest/keychain.rs`).
    for r in every_request() {
        let json = serde_json::to_string(&r).unwrap();
        let guarded = matches!(
            r,
            Request::Configure { .. }
                | Request::ClockSet { .. }
                | Request::DataPrepare { .. }
                | Request::Shutdown { .. }
                | Request::KeychainUnlock { .. }
        );
        assert_eq!(json.contains("\"password\""), guarded, "{json}");
        assert!(!r.kind().contains(PASSWORD), "{}", r.kind());
    }
}

#[path = "security_requests_tests.rs"]
mod requests;
use requests::every_request;

#[test]
fn s04_token() {
    let a = session::random_hex(16);
    let b = session::random_hex(16);
    assert_ne!(a, b);
    for t in [&a, &b] {
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|c| c.is_ascii_hexdigit()));
    }
    // kind() and Debug strip it.
    let req = Request::Token {
        bench: false,
        now_ms: None,
        token: a.clone(),
        v: VERSION,
    };
    assert!(!req.kind().contains(&a), "{}", req.kind());
    let header = ProxyHeader::new(&a, "notes.example.com", 443, Identity::default());
    assert!(!format!("{header:?}").contains(&a));
    assert!(header.line().contains(&a), "the wire line carries it");
    let fh = FileHeader {
        token: a.clone(),
        kind: FileKind::Log,
        name: "agent.log".into(),
        size: 1,
    };
    assert!(!fh.kind().contains(&a));
    // The event log of a fake session, written through the log path, has none.
    let t = tempfile::tempdir().unwrap();
    let p = Profile::new(t.path().join("p"));
    let ev = Events::open(&p);
    ev.record("start", &req.kind(), &[]);
    ev.record("files", &fh.kind(), &[]);
    ev.record("proxy", &format!("{header:?}"), &[]);
    let log = std::fs::read_to_string(p.events_log()).unwrap();
    assert_eq!(log.lines().count(), 3);
    assert!(!log.contains(&a), "{log}");
    // Never on disk: no file of the profile holds it.
    for e in walkdir(&p.dir) {
        let bytes = std::fs::read(&e).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(&a),
            "{}",
            e.display()
        );
    }
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn s05_modes() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    let p = prepared(&layout, "sample-notes");
    // The copy of the backup is plain on Linux: a small data disk.
    std::fs::File::options()
        .write(true)
        .open(p.data())
        .unwrap()
        .set_len(4096)
        .unwrap();
    std::fs::write(p.state(), b"state").unwrap();
    std::fs::set_permissions(p.state(), std::fs::Permissions::from_mode(0o600)).unwrap();
    let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(p.dir.clone()), 0o700);
    assert_eq!(mode(p.state()), 0o600);
    assert_eq!(mode(p.data()), 0o600);
    assert_eq!(mode(p.system()), 0o600);
    // A backup of the pair: 0600 files in a 0700 folder.
    backup::backup(
        &p,
        "manual",
        true,
        1_700_000_000,
        u64::MAX,
        &session::copy_clone,
    )
    .unwrap();
    let b = backup::newest(&p).expect("a backup");
    assert_eq!(mode(b.dir.clone()), 0o700);
    for name in backup::FILES {
        assert_eq!(mode(b.dir.join(name)), 0o600, "{name}");
    }
    assert!(mode_issues(&p).is_empty(), "{:?}", mode_issues(&p));
    // The golden: 0444 (and `uchg` on a Mac, `check-05-golden-flags.sh`).
    layout::seal_file(&layout.golden.disk).unwrap();
    assert_eq!(mode(layout.golden.disk.clone()), 0o444);
    assert!(crate::macvm::manage::sealed_ok(&layout.golden.disk));
    // A wrong mode is an issue that the doctor names.
    std::fs::set_permissions(p.data(), std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(
        b.dir.join("data.img"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let issues = mode_issues(&p);
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert_eq!(issues[0], "data.img 0644 (expected 0600)");
    assert!(
        issues[1].ends_with("/data.img 0640 (expected 0600)"),
        "{}",
        issues[1]
    );
}

#[test]
fn s06_forged_identity() {
    let (list, bad) = Allowlist::parse(&["api.example.com"]);
    assert!(bad.is_empty());
    let rule = firewall::new_rule(
        "com.evil.app",
        None,
        "evil.example",
        Port::Num(443),
        Verdict::Allow,
        Scope::Always,
        firewall::BY_USER,
        0,
    );
    let conn = Conn::new("com.evil.app", "evil.example", 443);
    // The forged identity matches the rule, and the mode still denies.
    assert!(firewall::matches(&rule, &conn));
    for mode in [Mode::Api, Mode::Allowlist] {
        assert_eq!(
            firewall::decide(mode, Some(&list), std::slice::from_ref(&rule), &conn),
            Decision::Deny(None),
            "{mode:?}"
        );
        assert_eq!(
            firewall::decide(mode, None, std::slice::from_ref(&rule), &conn),
            Decision::Deny(None)
        );
    }
    assert_eq!(
        firewall::decide(Mode::None, Some(&list), std::slice::from_ref(&rule), &conn),
        Decision::Deny(None)
    );
    // A host of the list passes the mode, and then the rules decide.
    let ok = Conn::new("com.evil.app", "api.example.com", 443);
    assert_eq!(
        firewall::decide(Mode::Api, Some(&list), &[], &ok),
        Decision::Allow(None)
    );
}

#[test]
fn s07_forwards() {
    use crate::vm::helper::{Cli, Cmd};
    use clap::Parser;
    let parse = |forward: &str| {
        Cli::try_parse_from([
            "paddock-vm",
            "app",
            "--profile",
            "/p/app/home",
            "--golden",
            "/g",
            "--app",
            "a",
            "--token-fd",
            "3",
            "--display",
            "2560x1600@2",
            "--forward",
            forward,
        ])
    };
    let ok = parse("guest-proxy=/tmp/p.sock").unwrap();
    let Cmd::App(a) = ok.cmd else { panic!("app") };
    assert_eq!(
        a.forwards,
        vec![(crate::vm::ports::GUEST_PROXY, PathBuf::from("/tmp/p.sock"))]
    );
    for other in [
        "broker",
        "1025",
        "kv",
        "devices",
        "gateway",
        "control",
        "guest-control",
        "1030",
        "guest-files",
        "1032",
        "proxy",
        "pty",
    ] {
        let e = parse(&format!("{other}=/tmp/x.sock"))
            .err()
            .unwrap_or_else(|| panic!("{other} must be refused"));
        assert!(
            e.to_string().contains("guest-proxy")
                || e.to_string().contains("not a known port")
                || e.to_string().contains("port"),
            "{other}: {e}"
        );
    }
}

/// A DHCP DISCOVER from `mac` (the shape of the stub's own tests).
fn discover(mac: [u8; 6]) -> Vec<u8> {
    let d = Dhcp {
        op: 1,
        xid: [1, 2, 3, 4],
        flags: 0x8000,
        ciaddr: [0; 4],
        yiaddr: [0; 4],
        siaddr: [0; 4],
        chaddr: mac,
        options: vec![(53, vec![1])],
    };
    stubnet::Ethernet {
        dst: [0xff; 6],
        src: mac,
        ethertype: stubnet::ETH_IPV4,
        payload: stubnet::Ipv4 {
            src: [0; 4],
            dst: [255; 4],
            proto: stubnet::IP_UDP,
            payload: stubnet::Udp {
                src_port: 68,
                dst_port: 67,
                payload: d.build(),
            }
            .build([0; 4], [255; 4]),
        }
        .build(),
    }
    .build()
}

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn view() -> crate::doctor::macvm::View {
    crate::doctor::macvm::View {
        golden: crate::macvm::manage::GoldenView::Missing,
        host_version: Some("15.2".into()),
        apple_silicon: true,
        entitlement: Some(true),
        profiles: vec![],
        sessions: vec![],
        slots: vec![None, None],
        slots_dir: Some(SLOTS_DIR_MODE),
        free: 120 << 30,
        prev: crate::macvm::golden::Prev::None,
    }
}

#[path = "security_more_tests.rs"]
mod more;
