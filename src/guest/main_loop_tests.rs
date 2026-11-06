use super::*;
use crate::guest::platform::fake::{Fake, FakeState};
use crate::guest::windows::RawWindow;
use crate::macvm::proto::{read_line, ConfigureStep, Frame, Outcome, Version};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const APP: &str = "com.apple.TextEdit";

/// The host side of the control line.
struct Host {
    s: UnixStream,
    req: u64,
    seen: Vec<Event>,
}

impl Host {
    fn new(s: UnixStream) -> Self {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        Host {
            s,
            req: 0,
            seen: Vec::new(),
        }
    }

    fn next(&mut self) -> Event {
        let ev: Event = read_line(&mut self.s).expect("a line").expect("not closed");
        self.seen.push(ev.clone());
        ev
    }

    /// The next event that `pred` picks, skipping the rest.
    fn until(&mut self, pred: impl Fn(&Event) -> bool) -> Event {
        let start = Instant::now();
        loop {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "no event: {:?}",
                self.seen
            );
            let ev = self.next();
            if pred(&ev) {
                return ev;
            }
        }
    }

    fn send(&mut self, request: Request) -> u64 {
        self.req += 1;
        let call = Call {
            req: self.req,
            request,
        };
        write_line(&mut self.s, &call).unwrap();
        self.req
    }

    fn reply(&mut self, req: u64) -> Outcome {
        match self.until(|e| matches!(e, Event::Reply(r) if r.req == req)) {
            Event::Reply(r) => r.outcome,
            _ => unreachable!(),
        }
    }

    fn handshake(&mut self) {
        assert!(matches!(self.next(), Event::Hello(h) if h.v == VERSION && h.can_update));
        let req = self.send(Request::Token {
            token: TOKEN.into(),
            v: VERSION,
            bench: false,
        });
        assert_eq!(self.reply(req), Outcome::Ok { data: None });
    }
}

/// The agent in a thread, with a queue of connections to dial.
fn start(fake: Fake, streams: Vec<UnixStream>) -> std::thread::JoinHandle<i32> {
    let queue = Arc::new(Mutex::new(VecDeque::from(streams)));
    std::thread::spawn(move || {
        let mut fake = fake;
        run(&mut fake, || {
            queue
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| std::io::Error::other("no more connections"))
        })
    })
}

fn raw(id: u32, pid: u32) -> RawWindow {
    RawWindow {
        id,
        pid,
        owner: "TextEdit".into(),
        bundle: Some(APP.into()),
        layer: 0,
        bounds: Frame::new(10.0, 20.0, 800.0, 600.0),
        alpha: 1.0,
    }
}

fn with_app(st: &Arc<Mutex<FakeState>>) {
    let mut st = st.lock().unwrap();
    st.windows = vec![raw(1, 100), raw(2, 100), raw(3, 200)];
    st.procs = [(1, 0), (200, 1)].into();
    st.front = Some(100);
    st.titles.insert(1, "Untitled".into());
}

#[test]
fn handshake_ping_restart() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, _st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    // The full (empty) list, the focus and the first health follow.
    assert!(
        matches!(host.until(|e| matches!(e, Event::Windows(_))), Event::Windows(w) if w.order.is_empty())
    );
    assert!(
        matches!(host.until(|e| matches!(e, Event::Health(_))), Event::Health(h) if !h.app_running)
    );
    let req = host.send(Request::Ping { t: 7 });
    assert!(matches!(
        host.until(|e| matches!(e, Event::Pong { .. })),
        Event::Pong { .. }
    ));
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    // A second token is refused.
    let req = host.send(Request::Token {
        token: "x".into(),
        v: VERSION,
        bench: false,
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::BadRequest,
            ..
        }
    ));
    // The menu bar of an app that does not run.
    let req = host.send(Request::Menu { app: APP.into() });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchApp,
            ..
        }
    ));
    let req = host.send(Request::AgentRestart);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(agent.join().unwrap(), 0);
}

#[test]
fn launch_windows_move_and_exit() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    // Before the launch nothing is followed: the list is empty.
    assert!(
        matches!(host.until(|e| matches!(e, Event::Windows(_))), Event::Windows(w) if w.order.is_empty())
    );
    let req = host.send(Request::AppLaunch {
        path: "/System/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert_eq!(
        host.reply(req),
        Outcome::Ok {
            data: Some(ReplyData::Launched { pid: 100 })
        }
    );
    let Event::Windows(w) = host.until(|e| matches!(e, Event::Windows(_))) else {
        unreachable!()
    };
    assert_eq!(w.order, vec![1, 2]);
    assert_eq!(w.changed.len(), 2);
    assert_eq!(w.changed[0].title, "Untitled");
    assert_eq!(w.changed[0].app, APP);
    assert!(w.changed[0].focused);
    assert_eq!(st.lock().unwrap().observed, [100].into());
    let Event::Focus { app, window } = host.until(|e| matches!(e, Event::Focus { .. })) else {
        unreachable!()
    };
    assert_eq!((app.as_str(), window), (APP, Some(1)));
    // A second launch while the app runs is refused.
    let req = host.send(Request::AppLaunch {
        path: "/x.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::Busy,
            ..
        }
    ));
    // The guard: configure and data.prepare while the app runs.
    let req = host.send(Request::Configure {
        password: "pw".into(),
        steps: vec![],
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NotInSession,
            ..
        }
    ));
    let req = host.send(Request::DataPrepare {
        password: "pw".into(),
        data_id: "d".into(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NotInSession,
            ..
        }
    ));
    assert!(st.lock().unwrap().data_prepares.is_empty());
    // A move: the reply, then the event with the new frame.
    let frame = Frame::new(100.0, 100.0, 800.0, 600.0);
    let req = host.send(Request::WindowMove { id: 2, frame });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    let Event::Windows(w) = host.until(|e| matches!(e, Event::Windows(_))) else {
        unreachable!()
    };
    assert_eq!(w.changed.len(), 1);
    assert_eq!((w.changed[0].id, w.changed[0].frame), (2, frame));
    assert_eq!(st.lock().unwrap().ops, vec![(2, WindowOp::Move(frame))]);
    let req = host.send(Request::WindowRaise { id: 9 });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchWindow,
            ..
        }
    ));
    // A hung app: the op fails with Timeout, and ping still answers.
    st.lock().unwrap().op_result = Some((ErrorCode::Timeout, "AX timed out".into()));
    let req = host.send(Request::WindowRaise { id: 1 });
    let ping = host.send(Request::Ping { t: 1 });
    assert!(matches!(host.reply(ping), Outcome::Ok { .. }));
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::Timeout,
            ..
        }
    ));
    // Health says the app runs.
    assert!(
        matches!(host.until(|e| matches!(e, Event::Health(_))), Event::Health(h) if h.app_running)
    );
    // The app crashes: a crash report appears, no relaunch after three.
    {
        let mut st = st.lock().unwrap();
        st.procs.remove(&100);
        st.windows.retain(|w| w.pid != 100);
        st.crash = Some("/Users/paddock/Library/Logs/DiagnosticReports/TextEdit.ips".into());
    }
    let Event::Windows(w) = host.until(|e| matches!(e, Event::Windows(_))) else {
        unreachable!()
    };
    assert_eq!(w.removed, vec![1, 2]);
    let Event::AppExited {
        app,
        code,
        crash_report,
        relaunch,
    } = host.until(|e| matches!(e, Event::AppExited { .. }))
    else {
        unreachable!()
    };
    assert_eq!((app.as_str(), code, relaunch), (APP, 1, true));
    assert!(crash_report.unwrap().ends_with("TextEdit.ips"));
    // Now configure runs: a user step through /bin/sh.
    let req = host.send(Request::Configure {
        password: "planted-pw".into(),
        steps: vec![ConfigureStep {
            name: "echo".into(),
            as_root: false,
            argv: vec!["/bin/sh".into(), "-c".into(), "echo done".into()],
            verify: vec!["/bin/sh".into(), "-c".into(), "echo verified".into()],
        }],
    });
    match host.reply(req) {
        Outcome::Ok {
            data: Some(ReplyData::Configure { steps }),
        } => {
            assert_eq!(steps.len(), 1);
            assert!(steps[0].ok);
            assert_eq!(
                (steps[0].output.as_str(), steps[0].verify.as_str()),
                ("done", "verified")
            );
        }
        other => panic!("{other:?}"),
    }
    let req = host.send(Request::DataPrepare {
        password: "pw".into(),
        data_id: "d1".into(),
    });
    assert_eq!(
        host.reply(req),
        Outcome::Ok {
            data: Some(ReplyData::DataPrepare {
                verify: "verified".into()
            })
        }
    );
    assert_eq!(st.lock().unwrap().data_prepares, vec!["d1"]);
    drop(host);
    // The connection is gone and no dial succeeds: the agent waits. Stop it.
    assert!(!agent.is_finished());
}

#[test]
fn quit_replies_when_the_app_is_gone() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let req = host.send(Request::AppQuit {
        app: APP.into(),
        force: false,
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchApp,
            ..
        }
    ));
    let req = host.send(Request::AppLaunch {
        path: "/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    let req = host.send(Request::AppQuit {
        app: APP.into(),
        force: true,
    });
    // No reply while the app lives.
    assert!(
        matches!(host.until(|e| matches!(e, Event::Health(_))), Event::Health(h) if h.app_running)
    );
    assert!(!host
        .seen
        .iter()
        .any(|e| matches!(e, Event::Reply(r) if r.req == req)));
    assert_eq!(st.lock().unwrap().quits, vec![(100, true)]);
    st.lock().unwrap().procs.remove(&100);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    // A requested exit: code 0, relaunch allowed, no crash wait.
    let Event::AppExited {
        code,
        relaunch,
        crash_report,
        ..
    } = host.until(|e| matches!(e, Event::AppExited { .. }))
    else {
        unreachable!()
    };
    assert_eq!((code, relaunch, crash_report), (0, true, None));
    let req = host.send(Request::AgentRestart);
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent.join().unwrap(), 0);
}

#[test]
fn shutdown_needs_the_password_and_no_app() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let shutdown = |host: &mut Host, password: &str| {
        let req = host.send(Request::Shutdown {
            password: password.into(),
        });
        host.reply(req)
    };
    // An older host sends no password: an error, the host stops the VM.
    let r = shutdown(&mut host, "");
    assert!(
        matches!(&r, Outcome::Error { code: ErrorCode::Io, text } if text.contains("no guest password")),
        "{r:?}"
    );
    // sudo refused the password: the error goes back with its text.
    st.lock().unwrap().shutdown_result = Some((ErrorCode::Io, "exit 1: Sorry".into()));
    let r = shutdown(&mut host, "pw");
    assert!(
        matches!(&r, Outcome::Error { text, .. } if text == "exit 1: Sorry"),
        "{r:?}"
    );
    assert_eq!(shutdown(&mut host, "pw"), Outcome::Ok { data: None });
    assert_eq!(st.lock().unwrap().shutdowns, 3);
    // While an app runs, the password is not used at all.
    with_app(&st);
    let req = host.send(Request::AppLaunch {
        path: "/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    let r = shutdown(&mut host, "pw");
    assert!(
        matches!(
            r,
            Outcome::Error {
                code: ErrorCode::NotInSession,
                ..
            }
        ),
        "{r:?}"
    );
    assert_eq!(st.lock().unwrap().shutdowns, 3);
    let req = host.send(Request::AgentRestart);
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent.join().unwrap(), 0);
}

#[test]
fn reconnects_after_a_lost_connection_with_a_full_list() {
    let (a1, b1) = UnixStream::pair().unwrap();
    let (a2, b2) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    let agent = start(fake, vec![a1, a2]);
    let mut host = Host::new(b1);
    host.handshake();
    let req = host.send(Request::AppLaunch {
        path: "/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert!(
        matches!(host.until(|e| matches!(e, Event::Windows(_))), Event::Windows(w) if w.order == vec![1, 2])
    );
    // The host closes the line (the shim restarted).
    drop(host);
    let mut host = Host::new(b2);
    let t = Instant::now();
    host.handshake();
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "the reconnect comes within RECONNECT"
    );
    // The full list again, the app still followed.
    let Event::Windows(w) = host.until(|e| matches!(e, Event::Windows(_))) else {
        unreachable!()
    };
    assert_eq!(w.order, vec![1, 2]);
    assert_eq!(w.changed.len(), 2);
    let req = host.send(Request::AgentRestart);
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent.join().unwrap(), 0);
}

#[test]
fn update_log_input_and_disk() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    {
        let mut s = st.lock().unwrap();
        s.missing = vec!["com.sogou.inputmethod.sogou.pinyin".into()];
        s.update_result = Some((ErrorCode::BadHash, "wrong hash".into()));
        s.health.data_disk_free = Some(3 << 30);
        s.data_check = Some("The volume appears to be OK".into());
    }
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    assert!(
        matches!(host.until(|e| matches!(e, Event::Health(_))), Event::Health(h) if h.data_verify.as_deref() == Some("The volume appears to be OK"))
    );
    assert!(!host.seen.iter().any(|e| matches!(e, Event::Disk { .. })));
    let req = host.send(Request::InputSet {
        sources: vec!["a".into()],
        current: "com.apple.keylayout.German".into(),
        reduce_motion: true,
        increase_contrast: false,
    });
    assert_eq!(
        host.reply(req),
        Outcome::Ok {
            data: Some(ReplyData::InputSet {
                missing: vec!["com.sogou.inputmethod.sogou.pinyin".into()]
            })
        }
    );
    assert_eq!(
        st.lock().unwrap().inputs,
        vec!["com.apple.keylayout.German"]
    );
    let req = host.send(Request::LogFetch);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(st.lock().unwrap().logs_sent, vec![TOKEN]);
    let req = host.send(Request::DiskLow);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(st.lock().unwrap().alerts, 1);
    let req = host.send(Request::AxWatch { on: true });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    st.lock().unwrap().ax_values.push((1, "typed y".into()));
    assert!(
        matches!(host.until(|e| matches!(e, Event::AxValue { .. })), Event::AxValue { window: 1, text } if text == "typed y")
    );
    // The disk crosses 2 GiB, then 1 GiB.
    st.lock().unwrap().health.data_disk_free = Some(1500 << 20);
    assert!(
        matches!(host.until(|e| matches!(e, Event::Disk { .. })), Event::Disk { warn: true, data_free } if data_free == 1500 << 20)
    );
    st.lock().unwrap().health.data_disk_free = Some(500 << 20);
    assert!(matches!(
        host.until(|e| matches!(e, Event::Disk { .. })),
        Event::Disk { warn: true, .. }
    ));
    st.lock().unwrap().health.data_disk_free = Some(10 << 30);
    assert!(matches!(
        host.until(|e| matches!(e, Event::Disk { .. })),
        Event::Disk { warn: false, .. }
    ));
    // A wrong hash keeps the agent running; a good update exits 0.
    let req = host.send(Request::AgentUpdate {
        path: "/Volumes/My Shared Files/agent/paddock-guest".into(),
        sha256: "ab".repeat(32),
        version: "0.3.0".into(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::BadHash,
            ..
        }
    ));
    st.lock().unwrap().update_result = None;
    let req = host.send(Request::AgentUpdate {
        path: "/Volumes/My Shared Files/agent/paddock-guest".into(),
        sha256: "ab".repeat(32),
        version: "0.3.0".into(),
    });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(agent.join().unwrap(), 0);
    assert_eq!(st.lock().unwrap().updates.len(), 2);
}

#[test]
fn a_wrong_first_request_ends_the_connection() {
    let (a, b) = UnixStream::pair().unwrap();
    let (a2, b2) = UnixStream::pair().unwrap();
    let (fake, _st) = Fake::new();
    let agent = start(fake, vec![a, a2]);
    let mut host = Host::new(b);
    assert!(matches!(host.next(), Event::Hello(_)));
    let req = host.send(Request::Ping { t: 1 });
    // The reader answers the ping itself; the loop still waits for the token.
    assert!(matches!(
        host.until(|e| matches!(e, Event::Pong { .. })),
        Event::Pong { .. }
    ));
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    let req = host.send(Request::Shutdown {
        password: String::new(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::BadRequest,
            ..
        }
    ));
    // The agent dials again.
    let mut host2 = Host::new(b2);
    host2.handshake();
    let mut line = Vec::new();
    line.extend_from_slice(b"not json\n");
    host2.s.write_all(&line).unwrap();
    // A bad line ends that connection too; no more connections: the agent waits.
    std::thread::sleep(Duration::from_millis(200));
    assert!(!agent.is_finished());
    let _ = Version { major: 1, minor: 0 };
}

#[test]
fn a_restarted_agent_follows_the_same_app() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("app");
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    st.lock().unwrap().state_file = Some(file.clone());
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let req = host.send(Request::AppLaunch {
        path: "/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    let Event::Windows(before) = host.until(
        |e| matches!(e, Event::Windows(_) if !matches!(e, Event::Windows(w) if w.order.is_empty())),
    ) else {
        unreachable!()
    };
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "100 TextEdit\n");
    // The agent "crashes": a restart request ends it, a new agent starts
    // with the same state file and the same process table.
    let req = host.send(Request::AgentRestart);
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent.join().unwrap(), 0);
    let (a2, b2) = UnixStream::pair().unwrap();
    let (fake2, st2) = Fake::new();
    with_app(&st2);
    {
        let mut s = st2.lock().unwrap();
        s.state_file = Some(file.clone());
        s.procs.insert(100, 1);
    }
    let agent2 = start(fake2, vec![a2]);
    let mut host2 = Host::new(b2);
    host2.handshake();
    let Event::Windows(after) = host2.until(|e| matches!(e, Event::Windows(_))) else {
        unreachable!()
    };
    assert_eq!(after.order, before.order);
    assert_eq!(
        after.changed.iter().map(|w| w.frame).collect::<Vec<_>>(),
        before.changed.iter().map(|w| w.frame).collect::<Vec<_>>()
    );
    assert!(
        matches!(host2.until(|e| matches!(e, Event::Health(_))), Event::Health(h) if h.app_running)
    );
    // The app exits: the memory goes.
    st2.lock().unwrap().procs.remove(&100);
    host2.until(|e| matches!(e, Event::AppExited { .. }));
    assert!(!file.exists());
    let req = host2.send(Request::AgentRestart);
    assert!(matches!(host2.reply(req), Outcome::Ok { .. }));
    assert_eq!(agent2.join().unwrap(), 0);
}

/// The clipboard in the guest: no clipboard event without the watch, the
/// content at the watch start stays in the guest, a change while watched goes
/// inline, a `clipboard.set` is written and its own change count is not
/// echoed, a concealed item never goes, and a `drop` opens the files with the
/// app without an echo either.
#[test]
fn clipboard_watch_set_and_drop() {
    use crate::macvm::clipboard::{Item, CONCEALED};
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    // A change without the watch: nothing within 300 ms.
    {
        let mut s = st.lock().unwrap();
        s.clip_count = 5;
        s.clip_items = vec![Item::text("unwatched")];
    }
    host.s
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let start_at = Instant::now();
    while start_at.elapsed() < Duration::from_millis(300) {
        match read_line::<Event>(&mut host.s) {
            Ok(Some(Event::Clipboard { .. })) => panic!("a clipboard event without the watch"),
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    host.s
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // The watch starts: the content of now is not sent.
    let req = host.send(Request::ClipboardWatch { on: true });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    {
        let mut s = st.lock().unwrap();
        s.clip_count = 6;
        s.clip_items = vec![
            Item::text("hunter2").with(CONCEALED, b""),
            Item::text("note"),
        ];
    }
    let ev = host.until(|e| matches!(e, Event::Clipboard { .. }));
    let Event::Clipboard {
        count,
        items,
        transfer,
    } = ev
    else {
        unreachable!()
    };
    assert_eq!(count, 6);
    assert!(transfer.is_none());
    assert_eq!(
        crate::macvm::clipboard::from_wire(&items),
        vec![Item::text("note")],
        "the concealed item stays in the guest"
    );
    // clipboard.set: written, and the new count is not echoed back.
    let req = host.send(Request::ClipboardSet {
        count: 9,
        items: crate::macvm::clipboard::to_wire(&[Item::text("from host")]),
        transfer: None,
    });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(
        st.lock().unwrap().clip_written,
        vec![vec![Item::text("from host")]]
    );
    // A drop: opened with the app's pid, the pasteboard write not echoed.
    let req = host.send(Request::AppLaunch {
        path: "/System/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    let req = host.send(Request::Drop {
        window: 1,
        point: crate::macvm::proto::Point { x: 1.0, y: 2.0 },
        paths: vec!["/Volumes/My Shared Files/drop/1-1/a.txt".into()],
    });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(
        st.lock().unwrap().opened,
        vec![(
            vec![PathBuf::from("/Volumes/My Shared Files/drop/1-1/a.txt")],
            Some(100)
        )]
    );
    // A transfer path outside the drop share is refused.
    let req = host.send(Request::ClipboardSet {
        count: 10,
        items: vec![],
        transfer: Some("/Volumes/My Shared Files/ws-1/x.json".into()),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::BadRequest,
            ..
        }
    ));
    // Nothing of the writes came back as a clipboard event, and the
    // health reports the Downloads check once.
    let ev = host.until(|e| matches!(e, Event::Health(_)));
    assert!(matches!(ev, Event::Health(_)));
    assert!(!host.seen[1..]
        .iter()
        .filter(|e| matches!(e, Event::Clipboard { count, .. } if *count != 6))
        .any(|_| true));
    assert!(st.lock().unwrap().downloads_checks >= 1);
    let req = host.send(Request::AgentRestart);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(agent.join().unwrap(), 0);
}

#[path = "main_loop_ax_tests.rs"]
mod ax_tree;
#[path = "main_loop_dialog_tests.rs"]
mod dialog;
#[path = "main_loop_menu_tests.rs"]
mod menu;
#[path = "main_loop_screen_tests.rs"]
mod screen_tests;

#[path = "main_loop_chrome_tests.rs"]
mod chrome_tests;
#[path = "main_loop_data_tests.rs"]
mod data_tests;
