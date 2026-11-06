//! The Claude Code pty custody on paddock's real relay, with no sandbox:
//! the fake TUI of `tests/fixtures/horse/claude-tui-pty` runs on the pty
//! of a [`Relay`] whose tap is the horse tap, with the command line that
//! `channel::prepare_hooks` gives. Its hooks call a fake `paddock-host`
//! that writes each report into a file; the test hands each one to the
//! TUI state as the broker does. An entry queued while the TUI "thinks"
//! is typed only after the Stop of that turn, and it lands.

use super::claude::{run, CustodyCtx};
use super::{PtyIdle, PtyTap};
use crate::horse::claude as codec;
use crate::horse::live::channel::{self, CustodyCtx as TuiCtx, Timing};
use crate::horse::live::harness::Control;
use crate::horse::live::tui_state::{run_key, Runs, SafePoint};
use crate::horse::store::Store;
use crate::relay::{take_controlling_tty, Relay, Taps};
use std::os::fd::{FromRawFd as _, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const NATIVE: &str = "01934b20-1111-7777-8888-12345678abcd";

fn fixture() -> &'static str {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/horse/claude-tui-pty"
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

fn wait(what: &str, child: &mut std::process::Child, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !f() {
        if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            panic!("{what}: {:?}", child.wait());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn an_entry_queued_during_a_turn_is_typed_through_the_relay_after_its_stop() {
    let _relay = crate::relay::winch::TEST_SIGNAL
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (home, project, bin) = (
        dir.path().join("home"),
        dir.path().join("project"),
        dir.path().join("bin"),
    );
    for d in [&home, &project, &bin] {
        std::fs::create_dir_all(d).unwrap();
    }
    let hooks = dir.path().join("hooks.log");
    // `paddock-host horse safe-point <kind>`: one line per report.
    std::fs::write(
        bin.join("paddock-host"),
        format!("#!/bin/sh\necho \"$3\" >> '{}'\n", hooks.display()),
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

    let store = Store::open(&dir.path().join("store")).unwrap();
    let h = store.new_default_horse().unwrap();
    let s = store
        .new_session(&h.id, codec::binding(Some(NATIVE.into())))
        .unwrap();
    let runs: &'static Runs = Box::leak(Box::default());
    let key = run_key(&h.id, &s.id);
    let signals = runs.subscribe(&key);
    let tap = PtyTap::new();
    let p = crate::profile::builtin("claude").unwrap();
    let mut idle = PtyIdle::of(&p, "claude", codec::VERSION).unwrap();
    idle.quiet = Duration::from_millis(300);

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
    let mut command = vec![fixture().to_string()];
    channel::apply(&mut command, &channel::prepare_hooks(NATIVE)).unwrap();
    let (i, o, e) = relay.child_stdio().unwrap();
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..])
        .current_dir(&project)
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

    let (close, control) = std::sync::mpsc::channel();
    let ctx = CustodyCtx {
        tui: TuiCtx {
            store: store.clone(),
            horse: h.id.clone(),
            session: s.id.clone(),
            native: NATIVE.into(),
            config_dir: home.join(".claude"),
            cwd: project.display().to_string(),
            version: codec::VERSION.into(),
            run: "relay-run".into(),
            runs,
            ledger: Box::new(|_| {}),
            control,
            timing: Timing {
                tick: Duration::from_millis(20),
                ..Timing::default()
            },
        },
        idle,
        tap,
    };
    let custody = std::thread::spawn(move || run(ctx, signals));

    // The broker: each report of a hook goes to the TUI state, in order.
    let bridge_stop = Arc::new(AtomicBool::new(false));
    let bridge = {
        let (hooks, stop) = (hooks.clone(), bridge_stop.clone());
        let key = key.clone();
        std::thread::spawn(move || {
            let mut done = 0;
            while !stop.load(Ordering::SeqCst) {
                let l = lines(&hooks);
                for k in &l[done..] {
                    runs.mark(&key, k.parse::<SafePoint>().unwrap());
                }
                done = l.len();
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    // Turn 1 thinks for 3 s: the entry is queued while it runs.
    wait("turn 1 did not start", &mut child, || {
        lines(&hooks).first().is_some_and(|l| l == "prompt")
    });
    let entry = store.queue_append(&s.id, "user", "from the test").unwrap();
    wait("the entry did not land", &mut child, || {
        store
            .deliveries(&s.id)
            .unwrap()
            .iter()
            .any(|d| d.entry.id == entry.id && d.landed_seq.is_some())
    });
    let log = lines(&project.join("tui.log"));
    let at = |l: &str| log.iter().position(|x| x == l);
    let stop1 = at("stop 1").unwrap_or_else(|| panic!("no stop 1: {log:?}"));
    let got = at("got 2 from the test").unwrap_or_else(|| panic!("no entry: {log:?}"));
    assert!(stop1 < got, "{log:?}");

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
    assert_eq!((r.tui.released, r.tui.landed), (1, 1), "{r:?}");
    assert!(r.tui.not_landed.is_empty() && !r.broken, "{r:?}");
    assert!(r.tui.stops >= 2, "{r:?}");
    // SAFETY: close the caller terminal; the reader ends with it.
    unsafe {
        libc::close(cm);
        libc::close(cs);
    }
    reader.join().unwrap();
}
