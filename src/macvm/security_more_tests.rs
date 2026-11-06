//! Items 8 to 21 of the security checklist (the first file holds items 1 to
//! 7 and the helpers). One module, two files, so each stays under the size
//! limit; `s00_table_is_complete` counts both.

use super::*;

#[test]
fn s08_stub_drop() {
    // TCP SYN to 1.1.1.1:443, UDP to 10.0.2.3:53 (not the gateway), IPv6, a DISCOVER from another MAC.
    let tcp = hex("52 54 00 a0 00 02 52 54 00 a0 00 15 08 00 45 00 00 28 00 00 40 00 40 06 2c c0 0a 00 02 0f 01 01 01 01 c0 00 01 bb 00 00 00 01 00 00 00 00 50 02 ff ff 00 00 00 00");
    let udp = hex("52 54 00 a0 00 02 52 54 00 a0 00 15 08 00 45 00 00 20 00 00 40 00 40 11 22 bc 0a 00 02 0f 0a 00 02 03 14 e9 00 35 00 0c 00 00 de ad be ef");
    let ipv6 = hex("33 33 00 00 00 01 52 54 00 a0 00 15 86 dd 60 00 00 00 00 00 11 40 fe 80 00 00 00 00 00 00 00 00 00 00 00 00 00 01 ff 02 00 00 00 00 00 00 00 00 00 00 00 00 00 01");
    let other = discover([0x02, 0, 0, 0, 0, 0x99]);
    let mut stub = Stub::default();
    let mut drops = Drops::default();
    let t0 = Instant::now();
    let want = [
        (
            &tcp,
            Dropped {
                proto: Proto::Tcp,
                dst: Some([1, 1, 1, 1]),
                port: Some(443),
            },
            "tcp 1.1.1.1:443",
        ),
        (
            &udp,
            Dropped {
                proto: Proto::Udp,
                dst: Some([10, 0, 2, 3]),
                port: Some(53),
            },
            "udp 10.0.2.3:53",
        ),
        (
            &ipv6,
            Dropped {
                proto: Proto::Ipv6,
                dst: None,
                port: None,
            },
            "ipv6",
        ),
        (
            &other,
            Dropped {
                proto: Proto::Dhcp,
                dst: None,
                port: None,
            },
            "dhcp",
        ),
    ];
    for (frame, dropped, target) in want {
        let replies = stub.handle(frame, t0);
        assert_eq!(replies.len(), 1, "{target}");
        let Reply::Dropped { what, first, count } = &replies[0] else {
            panic!("{target}: {replies:?}")
        };
        assert_eq!(what, &dropped);
        assert!(
            *first && *count == 1,
            "{target}: first={first} count={count}"
        );
        assert_eq!(what.target(), target);
        // The drop goes to the boundary log: one line per target per window.
        assert!(drops.note(what, t0));
        assert!(!drops.note(what, t0), "the second within a minute merges");
    }
    assert_eq!(drops.total(), 8);
    assert_eq!(drops.lines().len(), 4);
    // The guest's own DISCOVER is answered, so the drop is not a blanket refusal.
    assert!(matches!(
        stub.handle(&discover(GUEST_MAC), t0)[0],
        Reply::Send(_)
    ));
}

#[test]
fn s09_clipboard_key() {
    let mut s = clipboard::Sync::new(vec![]);
    assert!(!s.is_key());
    // Not key: nothing moves either way.
    assert_eq!(
        s.on_host_change(1, vec![Item::text("host secret")]),
        [Action::Nothing]
    );
    assert_eq!(
        s.on_guest_change(1, vec![Item::text("guest secret")]),
        [Action::Nothing]
    );
    // Key: the host content goes to the guest once, a guest change comes back.
    let a = s.on_key(true);
    assert!(
        matches!(&a[0], Action::SendToGuest(p) if p.items == [Item::text("host secret")]),
        "{a:?}"
    );
    assert!(
        matches!(&s.on_guest_change(2, vec![Item::text("from guest")])[0], Action::SetHost(p) if p.items == [Item::text("from guest")])
    );
    // Resigned: both ways stop again, and the next key sends only the latest.
    assert_eq!(s.on_key(false), [Action::Nothing]);
    assert_eq!(
        s.on_host_change(2, vec![Item::text("later")]),
        [Action::Nothing]
    );
    assert_eq!(
        s.on_host_change(3, vec![Item::text("latest")]),
        [Action::Nothing]
    );
    assert_eq!(
        s.on_guest_change(3, vec![Item::text("hidden")]),
        [Action::Nothing]
    );
    let a = s.on_key(true);
    assert!(
        matches!(&a[0], Action::SendToGuest(p) if p.items == [Item::text("latest")]),
        "{a:?}"
    );
}

#[test]
fn s10_crash_record() {
    // A dead pid reads as stopped.
    assert!(!host::alive(0));
    assert!(!host::alive(i32::MAX as u32));
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    let p = prepared(&layout, "crashy");
    session::begin(&p, &now(), Start::Fresh).unwrap();
    std::fs::create_dir_all(&layout.sessions).unwrap();
    record(&layout.sessions, "crashy", i32::MAX as u32, &p.dir, "");
    let dead = host::sweep(&layout.sessions, &host::alive, &mut |_| {});
    assert_eq!(dead.len(), 1);
    assert_eq!(host::last_end(&p), SessionEnd::Crashed);
    assert!(host::running(&layout, "crashy", "default").is_none());
    assert!(std::fs::read_to_string(p.events_log())
        .unwrap()
        .contains("\"kind\":\"crashed\""));
    // The stamp of a session that saved, then a state whose id does not match: discarded.
    let mut stamp = Stamp::read(&p.stamp()).unwrap();
    stamp.clean = true;
    stamp.state_id = Some("another-id-9".into());
    stamp.state_len = Some(5);
    stamp.write(&p.stamp()).unwrap();
    std::fs::write(p.state(), b"state").unwrap();
    assert_eq!(
        session::decide_files(&p, &now()),
        Ok(Start::Stale(Reason::StateDamaged))
    );
    assert!(
        session::line(Start::Stale(Reason::StateDamaged), Some(&stamp), &now())
            .contains("discarded")
    );
    // A matching id but a crash (not clean) is discarded too.
    stamp.state_id = Some(stamp.data_id.clone());
    stamp.clean = false;
    stamp.write(&p.stamp()).unwrap();
    assert_eq!(
        session::decide_files(&p, &now()),
        Ok(Start::Stale(Reason::StateDamaged))
    );
    assert_eq!(
        session::removals(Start::Stale(Reason::StateDamaged)),
        vec![session::What::State]
    );
}

#[test]
fn s11_doctor_permissions() {
    let l = crate::doctor::macvm::lines(&view());
    let line = l
        .iter()
        .find(|x| x.contains("host permission"))
        .expect("the permission line");
    assert_eq!(line, "  ok       no host permission needed (Accessibility: not needed, Screen Recording: not needed)");
    assert!(
        !l.iter()
            .any(|x| x.contains("tccutil") || x.contains("grant")),
        "{l:?}"
    );
}

#[test]
fn s12_golden_flags() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    std::fs::create_dir_all(&layout.golden.dir).unwrap();
    std::fs::write(&layout.golden.disk, b"disk").unwrap();
    layout::seal_file(&layout.golden.disk).unwrap();
    assert!(crate::macvm::manage::sealed_ok(&layout.golden.disk));
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open(&layout.golden.disk)
            .is_err(),
        "0444 refuses a write"
    );
    // A lost seal. On a Mac, `chmod` of a `uchg` file fails with EPERM,
    // so the flag goes first, as it does when a user clears it.
    layout::unseal_file(&layout.golden.disk).unwrap();
    assert!(!layout::is_immutable(&layout.golden.disk));
    std::fs::set_permissions(&layout.golden.disk, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!crate::macvm::manage::sealed_ok(&layout.golden.disk));
    // The doctor says so.
    let mut m = Manifest::new("15.2", "24C101", "0.9.0");
    m.sealed = Some("2026-11-01T10:00:00Z".into());
    m.at(State::Sealed);
    let mut v = view();
    v.golden = crate::macvm::manage::GoldenView::Sealed {
        manifest: m,
        size: 1 << 30,
        flags_ok: false,
    };
    let l = crate::doctor::macvm::lines(&v);
    assert!(
        l[1].starts_with("  warn     golden") && l[1].contains("lost their 0444 uchg seal"),
        "{}",
        l[1]
    );
    // The code: nothing but the builder opens or writes a golden path; the session clones it.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::new();
    for f in walkdir(&src.join("macvm"))
        .into_iter()
        .chain(walkdir(&src.join("guest")))
    {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name.contains("_tests") || name == "golden.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let opens = ["File::open", "OpenOptions", "fs::write", "File::create"]
                .iter()
                .any(|o| line.contains(o));
            if opens && line.contains("golden") && !line.contains("clonefile") {
                hits.push(format!("{name}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a golden path is opened outside the builder:\n{}",
        hits.join("\n")
    );
}

#[test]
fn s13_control_single() {
    // One control connection per session: the second is refused until the first ends.
    let gate = ControlGate::new();
    assert!(gate.open());
    assert!(gate.is_open());
    assert!(!gate.open(), "the second connection on 1030 is refused");
    assert!(!gate.clone().open(), "the clone shares the state");
    gate.close();
    assert!(gate.open());
    // A forwarded port accepts only after the token line.
    let line = |text: &[u8]| {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        // Before the peer closes: macOS fails setsockopt with EINVAL on a
        // socket whose peer is gone.
        b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        a.write_all(text).unwrap();
        drop(a);
        token_line_ok(&mut b, TOKEN)
    };
    assert!(line(
        format!("{TOKEN}\nCONNECT a:1 HTTP/1.1\r\n").as_bytes()
    ));
    assert!(!line(
        b"0000000000000000000000000000000f\nCONNECT a:1 HTTP/1.1\r\n"
    ));
    assert!(!line(b"CONNECT a:1 HTTP/1.1\r\n\r\n"));
    assert!(!line(b""));
    assert!(!line(
        format!("{}\n", "x".repeat(TOKEN_LINE_MAX + 1)).as_bytes()
    ));
    assert!(!line(TOKEN.as_bytes()), "no newline: not a token line");
}

#[test]
fn s14_files_limits() {
    let t0 = Instant::now();
    let mut l = Limiter::new();
    assert_eq!(
        l.admit(t0, TRANSFER_MAX + 1),
        Err(TransferWhy::TooLarge {
            size: TRANSFER_MAX + 1
        })
    );
    assert_eq!(TRANSFER_MAX, 16 << 20);
    assert_eq!(PER_MINUTE, 64);
    for i in 0..PER_MINUTE {
        assert_eq!(
            l.admit(t0 + Duration::from_millis(i as u64), TRANSFER_MAX),
            Ok(()),
            "{i}"
        );
    }
    assert_eq!(
        l.admit(t0 + Duration::from_secs(59), 1),
        Err(TransferWhy::TooMany)
    );
    assert_eq!(l.recent(), 64);
    assert_eq!(
        l.admit(t0 + Duration::from_secs(60), 1),
        Ok(()),
        "the first one fell out of the minute"
    );
    assert_eq!(files_port::IDLE, Duration::from_secs(10));
    assert!(!files_port::idle_closes(
        t0,
        t0 + Duration::from_millis(9_999)
    ));
    assert!(files_port::idle_closes(t0, t0 + Duration::from_secs(10)));
    assert!(
        !files_port::plain_name("../x")
            && !files_port::plain_name("a/b")
            && files_port::plain_name("clip-1.json")
    );
}

#[test]
fn s15_header_bounds() {
    assert_eq!(LINE_MAX, 256 * 1024);
    let mut long = format!("{{\"v\":1,\"token\":\"{TOKEN}\",\"host\":\"a\",\"port\":1,\"path\":\"");
    long.push_str(&"p".repeat(LINE_MAX));
    long.push_str("\"}");
    let e = ProxyHeader::parse(&long).unwrap_err();
    assert!(e.contains(&LINE_MAX.to_string()), "{e}");
    assert!(ProxyHeader::parse("CONNECT a:1 HTTP/1.1").is_err());
    assert!(ProxyHeader::parse("{\"not\":\"a header\"}").is_err());
    let good = ProxyHeader::new(TOKEN, "a", 1, Identity::default());
    assert_eq!(ProxyHeader::parse(good.line().trim_end()).unwrap(), good);
    // The proxy front: a longer line closes, a non JSON first byte is a plain request.
    let cfg = crate::proxy::HostProxy {
        token: Some(TOKEN.into()),
        ..Default::default()
    };
    let mut line = b"{\"v\":1,\"token\":\"".to_vec();
    line.resize(LINE_MAX + 1, b'x');
    line.extend_from_slice(b"\nCONNECT a:1 HTTP/1.1\r\n\r\n");
    let e = crate::proxy::front::read_front(&cfg, &mut Cursor::new(line))
        .err()
        .expect("closed");
    assert!(e.to_string().contains("longer than"), "{e}");
    let f = crate::proxy::front::read_front(
        &cfg,
        &mut Cursor::new(b"CONNECT a:1 HTTP/1.1\r\nHost: a\r\n\r\n".to_vec()),
    )
    .unwrap();
    assert!(f.identity.is_none() && f.target.is_none());
    assert!(f.head.starts_with(b"CONNECT a:1"));
    let wrong = ProxyHeader::new(
        "0000000000000000000000000000000f",
        "a",
        1,
        Identity::default(),
    )
    .line();
    let e = crate::proxy::front::read_front(
        &cfg,
        &mut Cursor::new(format!("{wrong}CONNECT a:1 HTTP/1.1\r\n\r\n").into_bytes()),
    )
    .err()
    .expect("closed");
    assert!(e.to_string().contains("another token"), "{e}");
}

#[test]
fn s16_slots() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("missing").join("dev.paddock.macvm");
    let a = Slot::take_in(&dir, "u", "chatgpt-desktop", 812).unwrap();
    assert_eq!(
        slots_dir_mode(&dir),
        Some(SLOTS_DIR_MODE),
        "made with the sticky bit"
    );
    assert_eq!(ls_mode(SLOTS_DIR_MODE), "drwxrwxrwt");
    assert_eq!(ls_mode(0o755), "drwxr-xr-x");
    assert_eq!(
        std::fs::metadata(&a.path).unwrap().permissions().mode() & 0o777,
        0o666
    );
    let b = Slot::take_in(&dir, "u", "vscode", 813).unwrap();
    assert_eq!((a.index, b.index), (0, 1));
    // A holder's text is not trusted beyond display: 10 KiB of junk is one cut line.
    let junk = format!("{}\n{}", "x".repeat(10 * 1024), "y".repeat(100));
    std::fs::write(&b.path, &junk).unwrap();
    let Err(SlotError::Full(full)) = Slot::take_in(&dir, "u", "third", 814) else {
        panic!("a third slot")
    };
    assert_eq!(
        full.holders[0],
        Holder::Mine {
            app: "chatgpt-desktop".into(),
            pid: 812
        }
    );
    let Holder::User { name } = &full.holders[1] else {
        panic!("{:?}", full.holders[1])
    };
    assert_eq!(name.len(), 80);
    let line = text::third_session(&full.holders);
    assert!(
        line.contains(&"x".repeat(80)) && !line.contains(&"x".repeat(81)) && !line.contains("yy"),
        "{line}"
    );
    assert!(line.len() < 400, "{}", line.len());
    assert_eq!(lock::holder_line("\x1b[31mred\tline\nnext"), "[31mred line");
    assert_eq!(lock::holder_line("   \n"), "unknown");
    // The doctor line of the folder.
    let mut v = view();
    v.slots_dir = Some(0o755);
    let l = crate::doctor::macvm::lines(&v);
    assert!(
        l.iter().any(|x| x.starts_with("  warn     slots folder")
            && x.contains("drwxr-xr-x")
            && x.contains("chmod 1777")),
        "{l:?}"
    );
    v.slots_dir = None;
    assert!(crate::doctor::macvm::lines(&v)
        .iter()
        .any(|x| x.starts_with("  ok       slots folder") && x.contains("not made yet")));
    v.slots_dir = Some(SLOTS_DIR_MODE);
    assert!(crate::doctor::macvm::lines(&v).iter().any(|x| x
        == &format!(
            "  ok       slots folder {}: drwxrwxrwt, slot files 0666",
            lock::SLOT_DIR
        )));
}

#[test]
fn s17_agent_update_hash() {
    use crate::guest::update::{self, CURRENT, NEXT, PREV};
    use crate::guest::BINARY;
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join(CURRENT)).unwrap();
    std::fs::write(d.path().join(CURRENT).join(BINARY), b"old agent").unwrap();
    std::fs::write(d.path().join(update::LAUNCH), b"#!/bin/sh\n").unwrap();
    let src = d.path().join("share").join(BINARY);
    std::fs::create_dir(src.parent().unwrap()).unwrap();
    std::fs::write(&src, b"new agent").unwrap();
    let right = update::sha256_file(&src).unwrap();
    let mut wrong = right.clone();
    wrong.replace_range(0..1, if right.starts_with('0') { "1" } else { "0" });
    let e = update::apply(d.path(), &src, &wrong, update::copy_executable).unwrap_err();
    assert_eq!(e.0, ErrorCode::BadHash);
    assert_eq!(
        std::fs::read(d.path().join(CURRENT).join(BINARY)).unwrap(),
        b"old agent"
    );
    assert!(!d.path().join(NEXT).exists() && !d.path().join(PREV).exists());
    // The right hash replaces it, and the old one stays as prev.
    let bin = update::apply(d.path(), &src, &right, update::copy_executable).unwrap();
    assert_eq!(std::fs::read(bin).unwrap(), b"new agent");
    assert_eq!(
        std::fs::read(d.path().join(PREV).join(BINARY)).unwrap(),
        b"old agent"
    );
}

#[test]
fn s18_bundle_redaction() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    std::fs::create_dir_all(&layout.golden.dir).unwrap();
    let mut m = Manifest::new("15.2", "24C101", "0.9.0");
    m.at(State::Sealed);
    layout.golden.write_manifest(&m).unwrap();
    let p = layout.profile("chatgpt-desktop", "default");
    std::fs::create_dir_all(p.logs()).unwrap();
    now().stamp("abcd1234-3".into()).write(&p.stamp()).unwrap();
    std::fs::write(p.state(), b"STATE-BYTES-PLANT").unwrap();
    // The clipboard text goes through the log path of each side: kind().
    let item = PasteItem {
        types: BTreeMap::from([(clipboard::TEXT.to_string(), CLIP.to_string())]),
    };
    let kinds = [
        Request::ClipboardSet {
            count: 1,
            items: vec![item.clone()],
            transfer: None,
        }
        .kind(),
        Event::Clipboard {
            count: 2,
            items: vec![item],
            transfer: Some(CLIP.to_string()),
        }
        .kind(),
        FileHeader {
            token: PLANT.into(),
            kind: FileKind::Clipboard,
            name: "clip-1.json".into(),
            size: 9,
        }
        .kind(),
    ];
    let raw = [
        format!("token={PLANT}"),
        format!("{{\"token\":\"{PLANT}\",\"v\":1}}"),
        format!("password: {PASSWORD}"),
        format!("GUEST_PASSWD={PASSWORD}"),
    ];
    for path in [p.shim_log(), p.console_log(), p.boundary_log()] {
        let mut f = std::fs::File::create(&path).unwrap();
        for l in raw.iter().chain(kinds.iter()) {
            writeln!(f, "2026-11-04T10:00:00Z INFO {l}").unwrap();
        }
    }
    let ev = Events::open(&p);
    let log = crate::guest::log::FileLog::new(p.agent_log(), log::LevelFilter::Debug);
    for l in raw.iter().chain(kinds.iter()) {
        ev.record("error", l, &[]);
        log.write_line(l).unwrap();
    }
    for which in crate::macvm::logs::Which::ALL {
        let text = std::fs::read_to_string(which.path(&p)).unwrap();
        assert!(!text.contains(CLIP), "{which:?} holds the clipboard text");
    }
    let out = report::macvm_section(&e, &report::Opts::default());
    for leak in [PLANT, PASSWORD, CLIP, "STATE-BYTES-PLANT"] {
        assert!(!out.contains(leak), "{leak} leaked:\n{out}");
    }
    assert!(
        out.contains("profile chatgpt-desktop/default (")
            && out.contains("shim.log (last 200 lines):"),
        "{out}"
    );
}

#[test]
fn s19_rules_file() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("firewall").join("app.toml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut rules = Rules::load_at(path.clone()).unwrap();
    let all = |verdict| {
        firewall::new_rule(
            "*",
            None,
            "*",
            Port::Any,
            verdict,
            Scope::Always,
            firewall::BY_USER,
            0,
        )
    };
    let e = rules.add(all(Verdict::Allow), false).unwrap_err();
    assert!(e.to_string().contains("--yes-all-hosts"), "{e}");
    assert!(rules.file.is_empty());
    rules.add(all(Verdict::Deny), false).unwrap();
    rules.add(all(Verdict::Allow), true).unwrap();
    rules.save().unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("host = \"*\""));
    rules.seed(
        &[
            "chatgpt.com".into(),
            "*.openai.com".into(),
            "not a host".into(),
        ],
        1_700_000_000,
    );
    assert_eq!(rules.seeds.len(), 2);
    assert!(rules.seeds.iter().all(|r| r.by == firewall::BY_PROFILE
        && r.verdict == Verdict::Allow
        && r.id.starts_with("seed-")));
    assert!(
        firewall::rule_line(&rules.seeds[0]).contains("profile"),
        "{}",
        firewall::rule_line(&rules.seeds[0])
    );
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("chatgpt.com"),
        "the seeds stay out of the file"
    );
    assert!(rules
        .remove(&rules.seeds[0].id.clone(), "app")
        .unwrap_err()
        .to_string()
        .contains("seed"));
}

#[test]
fn s20_window_size_levels() {
    let main = Rect {
        x: 0.0,
        y: 0.0,
        w: 2560.0,
        h: 1600.0,
    };
    let full = Frame::new(0.0, 0.0, 2560.0, 1600.0);
    let over = Frame::new(0.0, 0.0, 2561.0, 1600.0);
    let quarter = Frame::new(0.0, 0.0, 1280.0, 800.0);
    let over_quarter = Frame::new(0.0, 0.0, 1281.0, 800.0);
    for level in Level::ALL {
        let (f, o, q, oq) = (
            crop::size_allowed(level, full, main),
            crop::size_allowed(level, over, main),
            crop::size_allowed(level, quarter, main),
            crop::size_allowed(level, over_quarter, main),
        );
        match level {
            Level::Normal | Level::Hidden => {
                assert_eq!((f, o, q, oq), (true, true, true, true), "{level:?}")
            }
            Level::Floating | Level::Sheet => {
                assert_eq!((f, o, q, oq), (true, false, true, true), "{level:?}")
            }
            Level::Menu | Level::Popover | Level::Tooltip => {
                assert_eq!((f, o, q, oq), (false, false, true, false), "{level:?}")
            }
        }
        let fit = crop::fit_to_limit(level, over, main);
        assert!(crop::size_allowed(level, fit, main), "{level:?}: {fit:?}");
    }
}

#[test]
fn s21_input_restore() {
    let t = tempfile::tempdir().unwrap();
    let e = env(t.path());
    let layout = Layout::new(&e);
    std::fs::create_dir_all(&layout.sessions).unwrap();
    let live = layout.profile("live", "default");
    let dead = layout.profile("dead", "default");
    std::fs::create_dir_all(&dead.dir).unwrap();
    record(
        &layout.sessions,
        "live",
        100,
        &live.dir,
        "com.apple.keylayout.US",
    );
    record(
        &layout.sessions,
        "dead",
        200,
        &dead.dir,
        "com.apple.keylayout.German",
    );
    record(&layout.sessions, "none", 300, &dead.dir, "");
    let mut restored = Vec::new();
    let gone = host::sweep(&layout.sessions, &|pid| pid == 100, &mut |src| {
        restored.push(src.to_string())
    });
    assert_eq!(gone.len(), 2);
    assert_eq!(
        restored,
        vec!["com.apple.keylayout.German"],
        "the saved source of the dead session, once, none for an empty one"
    );
    assert_eq!(host::sessions(&layout.sessions).len(), 1);
    assert!(
        host::sweep(&layout.sessions, &|pid| pid == 100, &mut |_| panic!(
            "nothing to restore"
        ))
        .is_empty()
    );
    // The shim writes the saved source to the record for this: the field round-trips.
    let s: Session = serde_json::from_str(
        &std::fs::read_to_string(layout.sessions.join("live--default.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(s.input_source, "com.apple.keylayout.US");
}
