//! Claude Code custody over stream-json, with the fake claude of fake.rs
//! on two pipes. No claude, no vendor.

use super::fake::{self, Fake, Running, Seen, CWD};
use super::*;
use crate::horse::claude as codec;
use crate::horse::live::harness::{ChannelIo, Control};
use crate::horse::Store;
use serde_json::Value;
use std::path::Path;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(10);

#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);

impl Write for Out {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Out {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

struct Run {
    tx: Sender<Control>,
    release: Sender<()>,
    seen: Arc<Mutex<Seen>>,
    out: Out,
    join: std::thread::JoinHandle<Result<Report>>,
}

impl Run {
    fn users(&self) -> Vec<(String, bool)> {
        self.seen.lock().unwrap().users.clone()
    }
    fn log(&self) -> Vec<String> {
        self.seen.lock().unwrap().log.clone()
    }
    fn end(self) -> Result<Report> {
        self.tx.send(Control::Close).unwrap();
        self.join.join().unwrap()
    }
}

fn start(st: &Store, horse: &str, start: Start, f: Fake) -> Run {
    let config_dir = f.config_dir.clone();
    let Running {
        stdin,
        stdout,
        release,
        seen,
        join: fake_join,
    } = f.spawn();
    let (tx, control) = channel();
    let io = ChannelIo {
        from: stdout,
        to: stdin,
        control,
    };
    let out = Out::default();
    let ctx = CustodyCtx {
        store: st.clone(),
        horse: horse.into(),
        start,
        config_dir,
        cwd: CWD.into(),
        version: codec::VERSION.into(),
        run: "sbx-claude".into(),
        tick: TICK,
        out: Box::new(out.clone()),
        wait: Some(Box::new(move || fake_join.join().ok())),
    };
    let join = std::thread::spawn(move || run(io, ctx, Ends::ByRun));
    Run {
        tx,
        release,
        seen,
        out,
        join,
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Some ticks of the custodian, to show that something does not happen.
fn settle() {
    std::thread::sleep(TICK * 8);
}

fn session_of(st: &Store, native: &str) -> Option<String> {
    st.session_by_native(codec::HARNESS, native)
        .unwrap()
        .map(|s| s.id)
}

fn uuids(st: &Store, sid: &str) -> Vec<String> {
    let s = st.session(sid).unwrap();
    st.session_nodes(&s)
        .unwrap()
        .iter()
        .filter_map(|n| codec::info(&n.raw).ok()?.uuid)
        .collect()
}

fn store(t: &Path) -> (Store, String) {
    let st = Store::open(&t.join("store")).unwrap();
    let h = st.new_default_horse().unwrap();
    (st, h.id)
}

#[test]
fn args_pick_session_id_for_a_new_session_and_resume_for_a_projected_one() {
    let fresh = Start::fresh().unwrap();
    codec::check_session_id(&fresh.native).unwrap();
    let a = args(&fresh, Some("claude-opus-5-5"));
    assert_eq!(
        a,
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--session-id",
            &fresh.native,
            "--model",
            "claude-opus-5-5"
        ]
    );
    assert!(!a.iter().any(|x| x == "--include-partial-messages"));

    // A session with rows whose file is gone: resume projects it back.
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let config = t.path().join("claude");
    let native = "0b9e3f52-7a1c-4f0e-9d55-2c6a8e1b4d70";
    let path = codec::transcript_path(&config, CWD, native);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rows = [
        fake::chain("user", "u1", None, &fake::user_message("hi"), native),
        fake::chain(
            "assistant",
            "a1",
            Some("u1"),
            &fake::assistant_message(1, "ho"),
            native,
        ),
    ];
    std::fs::write(&path, [rows[0].as_slice(), b"\n", &rows[1], b"\n"].concat()).unwrap();
    let got = codec::ingest(&st, &path, codec::VERSION, Some(&horse)).unwrap();
    std::fs::remove_file(&path).unwrap();
    let s = Start::resume(&st, &got.session, &config, CWD).unwrap();
    assert_eq!(
        s,
        Start {
            native: native.into(),
            session: Some(got.session.clone()),
            resume: true
        }
    );
    assert!(path.is_file(), "resume projects the transcript");
    assert_eq!(args(&s, None)[6..], ["--resume", native]);
    // The file is there now: resume takes it as it is.
    assert_eq!(Start::resume(&st, &got.session, &config, CWD).unwrap(), s);
    // Claude Code resumes only from the project folder of its cwd.
    std::fs::remove_file(&path).unwrap();
    let e = Start::resume(&st, &got.session, &config, "/home/demo/other").unwrap_err();
    assert!(e.to_string().contains("another folder"), "{e}");
}

#[test]
fn wire_lines_have_the_stream_json_shapes() {
    let u: Value = serde_json::from_slice(&wire::user_line("s1", "hi")).unwrap();
    assert_eq!(
        u,
        serde_json::json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "hi"}]}, "parent_tool_use_id": null, "session_id": "s1"})
    );
    let i = wire::interrupt_line("r1");
    assert_eq!(i.last(), Some(&b'\n'));
    let i: Value = serde_json::from_slice(&i).unwrap();
    assert_eq!(
        i,
        serde_json::json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "interrupt"}})
    );
    assert!(
        matches!(wire::parse(br#"{"type":"stream_event"}"#), wire::Line::Unknown { ty, .. } if ty == "stream_event")
    );
    assert!(matches!(
        wire::parse(br#"{"type":"assis"#),
        wire::Line::Bad { .. }
    ));
    assert!(matches!(
        wire::parse(br#"{"type":"result"}"#),
        wire::Line::Bad { .. }
    ));
    assert!(matches!(wire::parse(b"[1]"), wire::Line::Bad { .. }));
}

#[test]
fn the_turn_state_follows_user_lines_and_results() {
    let mut c = Claude::new(Vec::new(), "s1");
    assert!(c.is_idle());
    assert!(!c.interrupt().unwrap(), "no turn runs");
    c.deliver("hi").unwrap();
    assert!(!c.is_idle());
    assert!(c.deliver("again").is_err(), "one turn at a time");
    let e = c.read(br#"{"type":"result","subtype":"success","session_id":"s1"}"#);
    assert!(e.is_err(), "a result before system/init fails closed");
    c.read(br#"{"type":"system","subtype":"init","session_id":"s1"}"#)
        .unwrap();
    assert!(c.interrupt().unwrap());
    assert!(!c.interrupt().unwrap(), "one interrupt at a time");
    let r = c
        .read(br#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"s1"}"#)
        .unwrap();
    assert!(matches!(r, Event::TurnEnd(_)));
    assert!(!c.is_idle(), "the interrupt waits for its answer");
    assert!(!c.give_up_interrupt(Duration::from_secs(60)));
    let a = c
        .read(br#"{"type":"control_response","response":{"subtype":"success","request_id":"paddock-interrupt-1"}}"#)
        .unwrap();
    assert_eq!(a, Event::Answered { error: None });
    assert!(c.is_idle());
    // An interrupt without an answer is given up after the turn ended.
    c.deliver("x").unwrap();
    c.interrupt().unwrap();
    c.read(br#"{"type":"result","subtype":"success","session_id":"s1"}"#)
        .unwrap();
    assert!(c.give_up_interrupt(Duration::ZERO));
    assert!(c.is_idle());
    c.close();
    assert!(!c.is_idle());
    assert!(c.deliver("y").is_err());
    let e = c
        .read(br#"{"type":"system","subtype":"init","session_id":"s2"}"#)
        .unwrap_err();
    assert!(e.to_string().contains("session s2"), "{e}");
    let e = c
        .read(br#"{"type":"system","subtype":"init","session_id":"s1","claude_code_version":"9.9.9"}"#)
        .unwrap_err();
    assert!(e.to_string().contains("9.9.9"), "{e}");
}

#[test]
fn an_entry_queued_during_a_turn_waits_for_the_result_and_lands_after_ingest() {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let s = Start::fresh().unwrap();
    let mut f = Fake::new(t.path().join("claude"), &s.native);
    f.hold = true;
    let native = s.native.clone();
    let r = start(&st, &horse, s, f);
    r.tx.send(Control::User("hello".into())).unwrap();
    wait_for("the first turn", || r.users().len() == 1);
    // system/init joined the session to the horse before its first row.
    wait_for("the session", || session_of(&st, &native).is_some());
    let sid = session_of(&st, &native).unwrap();
    assert!(st.session(&sid).unwrap().head.is_none());
    let e = st.queue_append(&sid, "note", "look at the logs").unwrap();
    settle();
    assert_eq!(r.users().len(), 1, "no delivery during a turn");
    assert_eq!(st.inbox(&sid).unwrap(), vec![e.clone()]);
    r.release.send(()).unwrap();
    wait_for("the delivery", || r.users().len() == 2);
    assert_eq!(r.users()[1], (e.wire_text(), false));
    // The line goes out before paddock records its delivery.
    wait_for("the delivery record", || {
        !st.deliveries(&sid).unwrap().is_empty()
    });
    // The inbox entry leaves right after the record is written.
    wait_for("the inbox to empty", || st.inbox(&sid).unwrap().is_empty());
    // The rows of turn 1 were in the store before the delivery.
    let d = &st.deliveries(&sid).unwrap()[0];
    assert_eq!(d.at_head, {
        let s = st.session(&sid).unwrap();
        st.session_nodes(&s).unwrap().get(1).map(|n| n.hash.clone())
    });
    r.release.send(()).unwrap();
    wait_for("the landing", || {
        st.deliveries(&sid).unwrap()[0].landed_seq.is_some()
    });
    let d = &st.deliveries(&sid).unwrap()[0];
    assert_eq!(d.landed_seq, Some(2), "the user row of turn 2");
    assert_eq!(uuids(&st, &sid), ["u1", "a1", "u2", "a2"]);
    let out = r.out.clone();
    let rep = r.end().unwrap();
    assert_eq!(rep.native_id, native);
    assert_eq!(rep.session.as_deref(), Some(sid.as_str()));
    assert_eq!((rep.turns, rep.deliveries, rep.landed), (2, 1, 1));
    assert_eq!(rep.exit, Some(0));
    assert!(out.text().contains("claude: echo: hello"), "{}", out.text());
    assert!(out.text().contains("delivered append"));
    assert!(!st.root().join("live/sbx-claude.json").exists());
}

#[test]
fn two_entries_go_in_queue_order_with_one_result_between_them() {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let s = Start::fresh().unwrap();
    let mut f = Fake::new(t.path().join("claude"), &s.native);
    f.hold = true;
    let native = s.native.clone();
    let r = start(&st, &horse, s, f);
    r.tx.send(Control::User("start".into())).unwrap();
    wait_for("the session", || session_of(&st, &native).is_some());
    let sid = session_of(&st, &native).unwrap();
    let a = st.queue_append(&sid, "user", "first").unwrap();
    let b = st.queue_append(&sid, "user", "second").unwrap();
    // The live record shows the running turn.
    wait_for("the live record", || {
        crate::horse::live::records(&st).unwrap().len() == 1
    });
    let rec = &crate::horse::live::records(&st).unwrap()[0];
    assert_eq!(
        (rec.main.as_str(), rec.sessions[0].idle),
        (native.as_str(), false)
    );
    r.release.send(()).unwrap();
    wait_for("the first entry", || r.users().len() == 2);
    settle();
    assert_eq!(r.users().len(), 2, "the second waits for the next result");
    r.release.send(()).unwrap();
    wait_for("the second entry", || r.users().len() == 3);
    let texts: Vec<_> = r.users().into_iter().collect();
    assert_eq!(
        texts,
        [
            ("start".to_string(), false),
            (a.wire_text(), false),
            (b.wire_text(), false)
        ]
    );
    let log = r.log();
    let first = log.iter().position(|l| l == "user:first").unwrap();
    let second = log.iter().position(|l| l == "user:second").unwrap();
    assert_eq!(
        log[first..second]
            .iter()
            .filter(|l| *l == "out:result")
            .count(),
        1
    );
    r.release.send(()).unwrap();
    let rep = r.end().unwrap();
    assert_eq!((rep.turns, rep.deliveries, rep.landed), (3, 2, 2));
    let d = st.deliveries(&sid).unwrap();
    assert_eq!(
        d.iter().map(|d| d.landed_seq).collect::<Vec<_>>(),
        [Some(2), Some(4)]
    );
}

#[test]
fn an_interrupt_waits_for_the_answer_and_the_result_then_delivers() {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let s = Start::fresh().unwrap();
    let mut f = Fake::new(t.path().join("claude"), &s.native);
    f.hold = true;
    let native = s.native.clone();
    let r = start(&st, &horse, s, f);
    r.tx.send(Control::Interrupt).unwrap();
    r.tx.send(Control::User("a long task".into())).unwrap();
    wait_for("the session", || session_of(&st, &native).is_some());
    let sid = session_of(&st, &native).unwrap();
    let e = st.queue_append(&sid, "user", "after the stop").unwrap();
    r.tx.send(Control::Interrupt).unwrap();
    wait_for("the delivery", || r.users().len() == 2);
    let log = r.log();
    let at = |x: &str| log.iter().position(|l| l == x).unwrap();
    let i = at("interrupt");
    assert_eq!(log[i + 1], "out:control_response");
    assert_eq!(log[i + 2], "out:result");
    assert!(at(&format!("user:{}", e.wire_text())) > i + 2);
    r.release.send(()).unwrap();
    let out = r.out.clone();
    let rep = r.end().unwrap();
    assert_eq!((rep.interrupts, rep.turns, rep.deliveries), (1, 2, 1));
    let text = out.text();
    assert!(text.contains("nothing to interrupt"), "{text}");
    assert!(text.contains("ended with error_during_execution"), "{text}");
    assert_eq!(uuids(&st, &sid), ["u1", "a1", "i1", "u2", "a2"]);
}

#[test]
fn unknown_and_bad_lines_and_a_torn_row_do_not_stop_the_loop() {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let s = Start::fresh().unwrap();
    let mut f = Fake::new(t.path().join("claude"), &s.native);
    f.hold = true;
    f.noise = true;
    f.torn = true;
    let native = s.native.clone();
    let r = start(&st, &horse, s, f);
    r.tx.send(Control::User("one".into())).unwrap();
    wait_for("the session", || session_of(&st, &native).is_some());
    let sid = session_of(&st, &native).unwrap();
    r.release.send(()).unwrap();
    wait_for("the ingest of turn 1", || {
        st.session(&sid).unwrap().head.is_some()
    });
    settle();
    assert_eq!(uuids(&st, &sid), ["u1"], "the torn row waits");
    r.tx.send(Control::User("two".into())).unwrap();
    wait_for("turn 2", || r.users().len() == 2);
    r.release.send(()).unwrap();
    wait_for("the ingest of turn 2", || uuids(&st, &sid).len() == 4);
    assert_eq!(uuids(&st, &sid), ["u1", "a1", "u2", "a2"]);
    let out = r.out.clone();
    let rep = r.end().unwrap();
    assert_eq!((rep.turns, rep.unknown, rep.bad), (2, 2, 2));
    let text = out.text();
    assert_eq!(text.matches("type stream_event").count(), 1, "{text}");
    assert_eq!(text.matches("cannot read").count(), 1, "{text}");
}

#[test]
fn a_resumed_session_gets_its_inbox_without_a_user_line() {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let config = t.path().join("claude");
    let native = "0b9e3f52-7a1c-4f0e-9d55-2c6a8e1b4d70";
    let path = codec::transcript_path(&config, CWD, native);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let rows = [
        fake::chain("user", "u0", None, &fake::user_message("old"), native),
        fake::chain(
            "assistant",
            "a0",
            Some("u0"),
            &fake::assistant_message(0, "x"),
            native,
        ),
    ];
    std::fs::write(&path, [rows[0].as_slice(), b"\n", &rows[1], b"\n"].concat()).unwrap();
    let sid = codec::ingest(&st, &path, codec::VERSION, Some(&horse))
        .unwrap()
        .session;
    let e = st.queue_append(&sid, "system", "the build is red").unwrap();
    let s = Start::resume(&st, &sid, &config, CWD).unwrap();
    assert!(s.resume);
    let mut f = Fake::new(config, native);
    f.last_uuid = Some("a0".into());
    let r = start(&st, &horse, s, f);
    wait_for("the delivery", || r.users().len() == 1);
    assert_eq!(r.users()[0].0, e.wire_text());
    wait_for("the landing", || {
        st.deliveries(&sid)
            .unwrap()
            .first()
            .is_some_and(|d| d.landed_seq.is_some())
    });
    let rep = r.end().unwrap();
    assert_eq!(rep.session.as_deref(), Some(sid.as_str()));
    assert_eq!(uuids(&st, &sid), ["u0", "a0", "u1", "a1"]);
    assert_eq!(st.deliveries(&sid).unwrap()[0].landed_seq, Some(2));
}

#[test]
fn a_mismatching_init_session_id_ends_custody_with_no_store_write() {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(t.path());
    let before = std::fs::read_dir(st.root()).unwrap().count();
    let s = Start::fresh().unwrap();
    let mut f = Fake::new(t.path().join("claude"), &s.native);
    f.init_id = Some("11111111-2222-4333-8444-555555555555".into());
    let r = start(&st, &horse, s, f);
    r.tx.send(Control::User("hi".into())).unwrap();
    let e = r.join.join().unwrap().unwrap_err();
    assert!(
        e.to_string()
            .contains("claude reports session 11111111-2222-4333-8444-555555555555"),
        "{e}"
    );
    assert!(st.sessions().unwrap().is_empty());
    assert!(!st.root().join("live").exists());
    assert!(!st.root().join("claude").exists(), "no cursor");
    assert_eq!(std::fs::read_dir(st.root()).unwrap().count(), before);
}
