//! The Grok session files of the pty custody, and the custody on
//! paddock's real relay with no sandbox: the fake TUI of
//! `tests/fixtures/horse/grok-tui` runs on the pty of a [`Relay`] whose
//! tap is the horse tap. Its hooks file has paddock's entries from the
//! same settings as the sandbox copy (run/grok_setup.rs), and the
//! `paddock-host` of its PATH writes each call and its stdin into a file.
//! The test parses each call as `paddock-host horse safe-point --stdin`
//! does and hands it to the TUI state as the broker does. An entry queued
//! while the TUI "thinks" is typed only after the Stop hook of that turn,
//! and it lands.

use super::super::codex::{CustodyCtx, Timing};
use super::super::files::{Files, Mark};
use super::super::{PtyIdle, PtyTap};
use super::{run, Grok};
use crate::horse::grok::{self as codec, fixture::Fixture};
use crate::horse::live::harness::Control;
use crate::horse::live::tui_state::{run_key, Runs};
use crate::horse::store::Store;
use crate::horse::Binding;
use crate::hostcli::horse::safe_point;
use crate::relay::{take_controlling_tty, Relay, Taps};
use std::os::fd::{FromRawFd as _, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The session of the fixture.
const ID: &str = "019a5f00-2222-7000-8000-00000000c0de";

#[test]
fn the_files_of_a_grok_session_give_the_turns_and_the_rows() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join(".grok");
    let mut f = Fixture::new(codec::fixture::ID, codec::fixture::CWD);
    f.prompt(0, "Run the tests").reply("ok");
    let dir = f.write(&home);
    let found = Grok::find(&home, codec::fixture::ID).unwrap();
    assert_eq!(found, dir.join(codec::UPDATES_FILE));
    assert!(Grok::find(&home, ID).is_none());
    assert!(Grok::find(&home, "../x").is_none());
    assert!(!Grok::sub_agent(&found));
    let marks: Vec<Mark> = f
        .updates
        .iter()
        .filter_map(|u| Grok::mark(u.as_bytes()))
        .collect();
    assert_eq!(marks, [Mark::User, Mark::End { aborted: false }]);
    f.cancelled();
    assert_eq!(
        Grok::mark(f.updates.last().unwrap().as_bytes()),
        Some(Mark::End { aborted: true })
    );
    // Rows of the session that hold a typed entry: the user chunk and the
    // user item, not an interjection.
    assert!(Grok::user_row_holds(
        f.updates[0].as_bytes(),
        "Run the tests"
    ));
    assert!(Grok::user_row_holds(f.chat[1].as_bytes(), "Run the tests"));
    f.interject("Run the tests");
    assert!(!Grok::user_row_holds(
        f.chat.last().unwrap().as_bytes(),
        "Run the tests"
    ));
    // A sub-agent session.
    std::fs::write(
        dir.join(codec::SUMMARY_FILE),
        r#"{"info":{"id":"x","cwd":"/"},"session_kind":"subagent_explore"}"#,
    )
    .unwrap();
    assert!(Grok::sub_agent(&found));
}

#[test]
fn the_grok_profile_has_a_pty_table_for_the_codec_version() {
    let p = crate::profile::builtin("grok").unwrap();
    let idle = PtyIdle::of(&p, codec::HARNESS, codec::VERSION).unwrap();
    assert_eq!(idle.interrupt_key, [0x03]);
    assert!(idle.empty_input);
    let busy = super::super::Screen::from_bytes(
        24,
        80,
        "\x1b[2J\x1b[H\u{280b} Thinking 1s   [stop]\r\n\r\n\u{276f} ".as_bytes(),
    );
    let idle_screen =
        super::super::Screen::from_bytes(24, 80, "\x1b[2J\x1b[H\u{276f} \r\n".as_bytes());
    let long = Duration::from_secs(5);
    assert!(!super::super::is_idle(&busy, &idle, long));
    assert!(super::super::is_idle(&idle_screen, &idle, long));
    let typed = super::super::Screen::from_bytes(24, 80, "\u{276f} half a line".as_bytes());
    assert!(!super::super::is_idle(&typed, &idle, long));
}

fn fixture() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/horse/grok-tui")
}

/// A caller terminal of 24 x 80: (master, slave).
fn caller_pty() -> (RawFd, RawFd) {
    let (mut m, mut s) = (0, 0);
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two new fds into the locals.
    let rc = unsafe {
        libc::openpty(
            &mut m,
            &mut s,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    (m, s)
}

fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

/// paddock's hooks file as the sandbox copy has it.
fn hooks_file(home: &Path) {
    let into = ".grok/hooks/paddock-compaction.json";
    let mut doc = serde_json::json!({});
    for s in crate::horse::run::grok_setup::hook_settings(into) {
        crate::state::json_set(&mut doc, &s.path, s.value.clone());
    }
    let path = home.join(into);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
}

#[test]
fn an_entry_queued_during_a_grok_turn_is_typed_through_the_relay_after_its_stop_hook() {
    let _relay = crate::relay::winch::TEST_SIGNAL
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (home, project, bin) = (
        dir.path().join("home"),
        dir.path().join("project"),
        dir.path().join("bin"),
    );
    for d in [&home.join(".grok"), &project, &bin] {
        std::fs::create_dir_all(d).unwrap();
    }
    hooks_file(&home);
    // `paddock-host horse safe-point KIND --stdin`: one line per call, the
    // arguments after `horse`, a tab, and the event from stdin.
    let calls = dir.path().join("calls.log");
    std::fs::write(
        bin.join("paddock-host"),
        format!(
            "#!/bin/sh\nshift\nev=$(cat)\nprintf '%s\\t%s\\n' \"$*\" \"$ev\" >> '{}'\n",
            calls.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            bin.join("paddock-host"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    let (mail_env, store) = super::super::mail_test_support::setup(dir.path());
    let h = store.new_default_horse().unwrap();
    let s = store
        .new_session(
            &h.id,
            Binding {
                harness: codec::HARNESS.into(),
                native_id: None,
                version: codec::VERSION.into(),
                container: codec::CONTAINER.into(),
            },
        )
        .unwrap();
    super::super::mail_test_support::link(&mail_env, &s.id);
    let runs: &'static Runs = Box::leak(Box::default());
    let key = run_key(&h.id, &s.id);
    let signals = runs.subscribe(&key);
    let tap = PtyTap::with_mail(&mail_env, super::super::mail_test_support::RUN);
    let p = crate::profile::builtin("grok").unwrap();
    let idle = PtyIdle::of(&p, codec::HARNESS, codec::VERSION).unwrap();

    let (cm, cs) = caller_pty();
    let gate = Taps::of(None, None, Some(tap.clone()));
    let mut relay = Relay::start(cs, cs, |_| {}, gate).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        // SAFETY: a dup of the caller master, owned by the reader.
        let mut m = unsafe { std::fs::File::from_raw_fd(libc::dup(cm)) };
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = [0u8; 4096];
            while !stop.load(Ordering::SeqCst) {
                match m.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        })
    };
    let (i, o, e) = relay.child_stdio().unwrap();
    let mut cmd = std::process::Command::new(fixture());
    cmd.current_dir(&project)
        .env("HOME", &home)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .stdin(i)
        .stdout(o)
        .stderr(e);
    // SAFETY: take_controlling_tty uses setsid and ioctl, which are safe
    // in pre_exec. The slave is fd 0.
    unsafe {
        cmd.pre_exec(take_controlling_tty);
    }
    let mut child = cmd.spawn().unwrap();
    relay.close_slave();

    let ledger = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let l = ledger.clone();
    let (close, control) = std::sync::mpsc::channel();
    let ctx = CustodyCtx {
        store: store.clone(),
        horse: h.id.clone(),
        session: s.id.clone(),
        sessions_root: home.join(".grok"),
        version: codec::VERSION.into(),
        run: "relay-run".into(),
        runs,
        ledger: Box::new(move |line: &str| l.lock().unwrap().push(line.into())),
        control,
        timing: Timing {
            tick: Duration::from_millis(20),
            ..Timing::default()
        },
        idle,
        tap,
    };
    let custody = std::thread::spawn(move || run(ctx, signals));

    // The broker: each call goes to the TUI state, in order, as
    // `paddock-host horse safe-point --stdin` sends it.
    let bridge_stop = Arc::new(AtomicBool::new(false));
    let bridge = {
        let (calls, stop) = (calls.clone(), bridge_stop.clone());
        let key = key.clone();
        std::thread::spawn(move || {
            let mut done = 0;
            while !stop.load(Ordering::SeqCst) {
                let l = lines(&calls);
                for c in &l[done..] {
                    let (args, event) = c.split_once('\t').unwrap_or((c, ""));
                    let args: Vec<String> = args.split(' ').map(String::from).collect();
                    assert_eq!(args[0], "safe-point", "{c}");
                    let mut call =
                        safe_point::parse(&args[1..]).unwrap_or_else(|| panic!("not a call: {c}"));
                    assert!(call.stdin, "{c}");
                    safe_point::from_hook(&mut call, event);
                    if call.tell {
                        runs.mark_turn(
                            &key,
                            call.kind.parse().unwrap(),
                            call.thread.as_deref(),
                            call.turn.as_deref(),
                        );
                    }
                }
                done = l.len();
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let log = project.join("tui.log");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut wait = |what: &str, f: &mut dyn FnMut() -> bool| {
        while !f() {
            if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                panic!(
                    "{what}: {:?}\ntui.log: {:?}\ncalls: {:?}\nledger: {:?}",
                    child.wait(),
                    lines(&log),
                    lines(&calls),
                    ledger.lock().unwrap(),
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // Turn 1 thinks for 3 s: the entry is queued while it runs.
    wait("the TUI did not start", &mut || {
        lines(&log).iter().any(|l| l == "hook prompt 1")
    });
    let message = super::super::mail_test_support::send(&mail_env, "from the test");
    assert_eq!(
        crate::mail::cursor(
            &mail_env,
            super::super::mail_test_support::RUN,
            crate::mail::Dir::Down,
            crate::mail::AGENT
        ),
        0
    );
    let entry = store
        .inbox(&s.id)
        .unwrap()
        .into_iter()
        .find(|e| e.mail_seq == Some(message.seq))
        .unwrap();
    wait("the entry did not land", &mut || {
        store
            .deliveries(&s.id)
            .unwrap()
            .iter()
            .any(|d| d.entry.id == entry.id && d.landed_seq.is_some())
    });
    let tui = lines(&log);
    let at = |l: &str| tui.iter().position(|x| x == l);
    let stop1 = at("hook stop 1").unwrap_or_else(|| panic!("no stop 1: {tui:?}"));
    let got = tui
        .iter()
        .position(|l| l.starts_with("got 2 paddock:"))
        .unwrap_or_else(|| panic!("no entry: {tui:?}"));
    assert_eq!(
        crate::mail::cursor(
            &mail_env,
            super::super::mail_test_support::RUN,
            crate::mail::Dir::Down,
            crate::mail::AGENT
        ),
        message.seq
    );
    let events = crate::mail::read(
        &mail_env,
        super::super::mail_test_support::RUN,
        crate::mail::Dir::Up,
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e.via.as_deref() == Some("relay")
                && e.seqs == [message.seq]
                && e.text.contains("typed at the idle prompt"))
            .count(),
        1
    );
    assert!(stop1 < got, "the entry came during turn 1: {tui:?}");
    wait("no stop 2", &mut || {
        lines(&log).iter().any(|l| l == "hook stop 2")
    });

    std::fs::write(project.join("done"), "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        assert!(Instant::now() < deadline, "the TUI did not end");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status:?}");
    relay.finish();
    stop.store(true, Ordering::SeqCst);
    bridge_stop.store(true, Ordering::SeqCst);
    bridge.join().unwrap();
    close.send(Control::Close).unwrap();
    close.send(Control::Close).unwrap();
    let r = custody.join().unwrap().unwrap();
    assert_eq!(r.thread.as_deref(), Some(ID), "{r:?}");
    assert_eq!((r.typed, r.landed), (1, 1), "{r:?}");
    assert!(r.not_landed.is_empty(), "{r:?}");
    assert_eq!((r.stops, r.other_stops, r.missed_stops), (2, 0, 0), "{r:?}");
    assert_eq!(
        store.session(&s.id).unwrap().binding.native_id.as_deref(),
        Some(ID)
    );
    // The store has both turns, and the typed entry as a user row.
    let nodes = store.session_nodes(&store.session(&s.id).unwrap()).unwrap();
    assert!(nodes
        .iter()
        .any(|n| Grok::user_row_holds(&n.raw, &entry.wire_text())));
    // SAFETY: close the caller terminal; the reader ends with it.
    unsafe {
        libc::close(cm);
        libc::close(cs);
    }
    reader.join().unwrap();
}

#[test]
fn a_late_stop_of_an_older_turn_does_not_free_the_tui() {
    use crate::horse::live::harness::Live as _;
    use crate::horse::live::tui_state::SafePoint;
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join(".grok");
    let mut f = Fixture::new(ID, "/home/demo/project");
    f.prompt(0, "one").reply("done");
    f.write(&home);
    let store = Store::open(&t.path().join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let s = store.new_session(&h.id, codec::binding(None)).unwrap();
    let runs: &'static Runs = Box::leak(Box::default());
    let key = run_key(&h.id, &s.id);
    let signals = runs.subscribe(&key);
    let tap = PtyTap::new();
    tap.attach_with(24, 80, |_| Ok(()));
    tap.output_bytes("\u{276f} \r\n".as_bytes());
    let p = crate::profile::builtin("grok").unwrap();
    let mut idle = PtyIdle::of(&p, codec::HARNESS, codec::VERSION).unwrap();
    idle.quiet = Duration::ZERO;
    let (_close, control) = std::sync::mpsc::channel();
    let ctx = CustodyCtx {
        store: store.clone(),
        horse: h.id.clone(),
        session: s.id.clone(),
        sessions_root: home.clone(),
        version: codec::VERSION.into(),
        run: "late".into(),
        runs,
        ledger: Box::new(|_| {}),
        control,
        timing: Timing::default(),
        idle,
        tap,
    };
    let mut c = super::super::codex::Custodian::<Grok>::new(ctx, signals).unwrap();
    runs.mark_turn(&key, SafePoint::Prompt, Some(ID), Some("p1"));
    runs.mark_turn(&key, SafePoint::Stop, Some(ID), Some("p1"));
    c.pump(Duration::from_millis(10)).unwrap();
    assert!(c.safe(None), "the stop of the newest turn frees the TUI");
    // The prompt of turn 2, then the late report of turn 1.
    runs.mark_turn(&key, SafePoint::Prompt, Some(ID), Some("p2"));
    runs.mark_turn(&key, SafePoint::Stop, Some(ID), Some("p1"));
    c.pump(Duration::from_millis(10)).unwrap();
    assert!(!c.safe(None), "a late stop freed a running turn");
    runs.mark_turn(&key, SafePoint::Stop, Some(ID), Some("p2"));
    c.pump(Duration::from_millis(10)).unwrap();
    assert!(c.safe(None));
}
