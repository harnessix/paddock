//! The Codex pty custody on paddock's real relay, with no sandbox: the
//! fake TUI of `tests/fixtures/horse/codex-tui` runs on the pty of a
//! [`Relay`] whose tap is the horse tap. Its `config.toml` gets paddock's
//! `notify` from the same setting as the sandbox copy
//! (horse/codex/notify.rs), and the `paddock-host` of its PATH writes each
//! call into a file. The test parses each call as `paddock-host horse
//! safe-point` does and hands it to the TUI state as the broker does,
//! with the thread id. An entry queued while the TUI "thinks" is typed
//! only after the notify of that turn, and it lands.

use super::codex::{run, CustodyCtx, Timing};
use super::{PtyIdle, PtyTap};
use crate::horse::codex as codec;
use crate::horse::live::harness::Control;
use crate::horse::live::tui_state::{run_key, Runs};
use crate::horse::Binding;
use crate::relay::{take_controlling_tty, Relay, Taps};
use std::os::fd::{FromRawFd as _, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The thread of the fixture.
const THREAD: &str = "019a5f00-0000-7000-8000-00000000c0de";

fn fixture() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/horse/codex-tui"
    )
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

#[test]
fn an_entry_queued_during_a_codex_turn_is_typed_through_the_relay_after_its_notify() {
    let _relay = crate::relay::winch::TEST_SIGNAL
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (home, project, bin) = (
        dir.path().join("home"),
        dir.path().join("project"),
        dir.path().join("bin"),
    );
    for d in [&home.join(".codex"), &project, &bin] {
        std::fs::create_dir_all(d).unwrap();
    }
    let project = project.canonicalize().unwrap();
    // paddock's notify, written as in the sandbox copy.
    let cfg =
        crate::privacy::set_toml("model = \"gpt-5.5\"\n", &[&codec::notify::setting()]).unwrap();
    std::fs::write(home.join(codec::notify::CONFIG), cfg).unwrap();
    // `paddock-host horse safe-point stop JSON`: one line per call, the
    // arguments after `horse` split by a tab.
    let calls = dir.path().join("calls.log");
    std::fs::write(
        bin.join("paddock-host"),
        format!(
            "#!/bin/sh\nshift\n(IFS=\"$(printf '\\t')\"; printf '%s\\n' \"$*\") >> '{}'\n",
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

    let (mail_env, store) = super::mail_test_support::setup(dir.path());
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
    super::mail_test_support::link(&mail_env, &s.id);
    let runs: &'static Runs = Box::leak(Box::default());
    let key = run_key(&h.id, &s.id);
    let signals = runs.subscribe(&key);
    let tap = PtyTap::with_mail(&mail_env, super::mail_test_support::RUN);
    // The table of the profile as it ships.
    let p = crate::profile::builtin("codex").unwrap();
    let idle = PtyIdle::of(&p, codec::HARNESS, codec::VERSION).unwrap();

    // The relay, with the horse tap as its only tap.
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
        sessions_root: home.join(".codex/sessions"),
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
    // `paddock-host horse safe-point` sends it.
    let bridge_stop = Arc::new(AtomicBool::new(false));
    let bridge = {
        let (calls, stop) = (calls.clone(), bridge_stop.clone());
        let key = key.clone();
        std::thread::spawn(move || {
            let mut done = 0;
            while !stop.load(Ordering::SeqCst) {
                let text = std::fs::read_to_string(&calls).unwrap_or_default();
                let complete = text.rsplit_once('\n').map_or("", |(lines, _)| lines);
                let l: Vec<_> = complete.lines().collect();
                for c in &l[done..] {
                    let args: Vec<String> = c.split('\t').map(String::from).collect();
                    assert_eq!(args[0], "safe-point", "{c}");
                    let call = crate::hostcli::horse::safe_point::parse(&args[1..])
                        .unwrap_or_else(|| panic!("not a call: {c}"));
                    if call.tell {
                        runs.mark_thread(&key, call.kind.parse().unwrap(), call.thread.as_deref());
                    }
                }
                done = l.len();
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let log = project.join("tui.log");
    let mut wait = |what: &str, f: &mut dyn FnMut() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !f() {
            if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                panic!(
                    "{what}: {:?}\ntui.log: {:?}\ncalls: {:?}\nledger: {:?}\nstate: {:?}",
                    child.wait(),
                    lines(&log),
                    lines(&calls),
                    ledger.lock().unwrap(),
                    runs.state(&key),
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // Turn 1 thinks for 3 s: the entry is queued while it runs.
    wait("the TUI did not start", &mut || {
        std::fs::read_to_string(&log).is_ok()
    });
    let message = super::mail_test_support::send(&mail_env, "from the test");
    assert_eq!(
        crate::mail::cursor(
            &mail_env,
            super::mail_test_support::RUN,
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
    let notify1 = at("notify 1").unwrap_or_else(|| panic!("no notify 1: {tui:?}"));
    let got = tui
        .iter()
        .position(|l| l.starts_with("got 2 paddock:"))
        .unwrap_or_else(|| panic!("no entry: {tui:?}"));
    assert_eq!(
        crate::mail::cursor(
            &mail_env,
            super::mail_test_support::RUN,
            crate::mail::Dir::Down,
            crate::mail::AGENT
        ),
        message.seq
    );
    let events = crate::mail::read(
        &mail_env,
        super::mail_test_support::RUN,
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
    assert!(notify1 < got, "{tui:?}");
    wait("no notify 2", &mut || {
        lines(&log).iter().any(|l| l == "notify 2")
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
    assert_eq!(r.thread.as_deref(), Some(THREAD), "{r:?}");
    assert_eq!((r.typed, r.landed), (1, 1), "{r:?}");
    assert!(r.not_landed.is_empty(), "{r:?}");
    assert_eq!((r.stops, r.other_stops, r.missed_stops), (2, 0, 0), "{r:?}");
    assert_eq!(
        store.session(&s.id).unwrap().binding.native_id.as_deref(),
        Some(THREAD)
    );
    // SAFETY: close the caller terminal; the reader ends with it.
    unsafe {
        libc::close(cm);
        libc::close(cs);
    }
    reader.join().unwrap();
}
