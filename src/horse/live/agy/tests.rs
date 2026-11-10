//! agy custody with the fake agy of `tests/fixtures/horse/agy-stream` as a
//! real process on pipes, and with scripted lines. No agy, no vendor.

use super::{args, db_path, run, signal, user_text, CustodyCtx, Report, Start};
use crate::horse::agy::fixture::{Fixture, ID};
use crate::horse::agy::{self as codec, VERSION};
use crate::horse::live::harness::{ChannelIo, Control, Ends};
use crate::horse::Store;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The conversation id of the fake.
const FAKE_ID: &str = "7c0e1f2a-3b4c-4d5e-8f60-0a1b2c3d4e5f";
const OTHER: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";

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

struct World {
    t: tempfile::TempDir,
    st: Store,
    horse: String,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap().id;
    std::fs::create_dir_all(t.path().join("home/project")).unwrap();
    World { t, st, horse }
}

pub(crate) fn python() -> bool {
    Command::new("python3")
        .args(["-c", "import sqlite3"])
        .output()
        .is_ok_and(|o| o.status.success())
}

pub(crate) fn fake() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/horse/agy-stream")
}

impl World {
    fn home(&self) -> PathBuf {
        self.t.path().join("home")
    }

    fn agy_home(&self) -> PathBuf {
        self.home().join(".gemini/antigravity-cli")
    }

    fn fresh(&self) -> String {
        self.st
            .new_session(&self.horse, codec::binding(None))
            .unwrap()
            .id
    }

    /// A session of a conversation whose database the sandbox has.
    fn resumed(&self) -> String {
        let mut f = Fixture::new(ID);
        f.user("old question").reply("old answer");
        let p = f.write(&self.agy_home());
        codec::ingest(&self.st, &p, VERSION, Some(&self.horse))
            .unwrap()
            .session
    }

    fn ctx(&self, session: &str, start: Start, out: &Out) -> CustodyCtx {
        CustodyCtx {
            store: self.st.clone(),
            horse: self.horse.clone(),
            session: session.into(),
            start,
            agy_home: self.agy_home(),
            run: "agy-run".into(),
            tick: Duration::from_millis(20),
            close_timeout: Duration::from_secs(10),
            out: Box::new(out.clone()),
            interrupt: None,
        }
    }

    /// The fake on pipes, as the sandbox starts the rewritten command.
    fn spawn(&self, start: &Start, env: &[(&str, &str)]) -> Child {
        let mut a = vec!["--dangerously-skip-permissions".to_string()];
        a.extend(args(start));
        Command::new(fake())
            .args(&a)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home())
            .envs(env.iter().copied())
            .current_dir(self.home().join("project"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn input(&self) -> Vec<String> {
        std::fs::read_to_string(self.home().join("agy-input.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn users(&self, session: &str) -> Vec<String> {
        let s = self.st.session(session).unwrap();
        self.st
            .session_nodes(&s)
            .unwrap()
            .iter()
            .filter_map(|n| user_text(&n.raw))
            .collect()
    }
}

fn headless(c: &mut Child) -> ChannelIo<std::process::ChildStdout, std::process::ChildStdin> {
    crate::horse::live::headless::io(c.stdout.take().unwrap(), c.stdin.take().unwrap())
}

#[test]
fn the_fake_gets_queued_entries_only_after_a_result_in_order_and_by_role() {
    if !python() {
        eprintln!("skipped: needs python3");
        return;
    }
    let w = world();
    let s = w.fresh();
    w.st.queue_append(&s, "user", "first task").unwrap();
    w.st.queue_append(&s, "system", "be brief").unwrap();
    w.st.queue_append(&s, "note", "a note").unwrap();
    let start = Start::of(&w.st, &s, &w.agy_home()).unwrap();
    assert_eq!(start, Start::default(), "a session without rows is new");
    // Turn 2 holds until the test lets it go, and the fake fails when a
    // line comes during it.
    let mut c = w.spawn(&start, &[("FAKE_AGY_HOLD", "1"), ("FAKE_AGY_STEP", "0.2")]);
    let go = w.home().join("go");
    let late = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(800));
        std::fs::write(go, "").unwrap();
    });
    let out = Out::default();
    let r = run(headless(&mut c), w.ctx(&s, start, &out), Ends::WhenDone).unwrap();
    late.join().unwrap();
    assert!(c.wait().unwrap().success(), "{}", out.text());
    assert!(
        !w.home().join("early").exists(),
        "a line came during a turn"
    );
    let input = w.input();
    assert_eq!(input.len(), 3, "{input:?}");
    let v: Vec<serde_json::Value> = input
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(v[0]["message"]["content"], "first task");
    assert_eq!(
        v[1]["message"]["content"],
        "<paddock-append role=\"system\">\nbe brief\n</paddock-append>"
    );
    assert_eq!(
        v[2]["message"]["content"],
        "<paddock-append role=\"note\">\na note\n</paddock-append>"
    );
    assert_eq!(
        r,
        Report {
            native_id: Some(FAKE_ID.into()),
            turns: 3,
            deliveries: 3,
            landed: 3,
            ..Report::default()
        },
        "{}",
        out.text()
    );
    let text = out.text();
    assert!(
        text.contains(&format!("paddock: agy conversation {FAKE_ID} in custody")),
        "{text}"
    );
    assert!(text.contains("agy: turn 3 done"), "{text}");
    let sess = w.st.session(&s).unwrap();
    assert_eq!(sess.binding.native_id.as_deref(), Some(FAKE_ID));
    assert_eq!(w.users(&s).len(), 3);
    assert!(w.st.inbox(&s).unwrap().is_empty());
    assert!(crate::horse::live::records(&w.st).unwrap().is_empty());
}

#[test]
fn an_interrupt_sends_sigint_and_the_interrupted_result_is_a_safe_point() {
    if !python() {
        eprintln!("skipped: needs python3");
        return;
    }
    let w = world();
    let s = w.fresh();
    w.st.queue_append(&s, "user", "a long task").unwrap();
    let mut c = w.spawn(&Start::default(), &[("FAKE_AGY_STEP", "30")]);
    let pipe = signal::pipe_of(c.stdin.as_ref().unwrap()).unwrap();
    let pid = c.id();
    let out = Out::default();
    let mut ctx = w.ctx(&s, Start::default(), &out);
    ctx.interrupt = Some(Box::new(move || signal::interrupt(pid, pipe).map(|_| ())));
    let (tx, control) = std::sync::mpsc::channel();
    let io = ChannelIo {
        from: c.stdout.take().unwrap(),
        to: c.stdin.take().unwrap(),
        control,
    };
    let custody = std::thread::spawn(move || run(io, ctx, Ends::ByRun));
    let deadline = Instant::now() + Duration::from_secs(30);
    while w.input().is_empty() {
        assert!(Instant::now() < deadline, "the task did not go out");
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    tx.send(Control::Interrupt).unwrap();
    // The interrupted result is a safe point: the next entry goes out.
    w.st.queue_append(&s, "user", "after the stop").unwrap();
    while w.input().len() < 2 {
        assert!(Instant::now() < deadline, "no entry after the interrupt");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "SIGINT did not end the turn"
    );
    tx.send(Control::Interrupt).unwrap();
    tx.send(Control::Close).unwrap();
    let r = custody.join().unwrap().unwrap();
    assert!(c.wait().unwrap().success());
    assert_eq!((r.interrupts, r.interrupted, r.turns), (2, 2, 2), "{r:?}");
    assert_eq!(r.deliveries, 2);
    assert!(
        out.text().contains("paddock: sent SIGINT to agy"),
        "{}",
        out.text()
    );
    // Both user steps are in the store.
    assert_eq!(w.users(&s), ["a long task", "after the stop"]);
}

#[test]
fn another_conversation_id_ends_the_custody_with_no_store_write() {
    if !python() {
        eprintln!("skipped: needs python3");
        return;
    }
    let w = world();
    let s = w.resumed();
    let before = w.st.session(&s).unwrap();
    w.st.queue_append(&s, "user", "go on").unwrap();
    let start = Start::of(&w.st, &s, &w.agy_home()).unwrap();
    assert_eq!(start.conversation.as_deref(), Some(ID));
    let mut c = w.spawn(&start, &[("FAKE_AGY_REPORT", OTHER)]);
    let out = Out::default();
    let e = run(headless(&mut c), w.ctx(&s, start, &out), Ends::WhenDone).unwrap_err();
    let e = format!("{e:#}");
    assert!(
        e.contains(&format!("agy reports conversation {OTHER} in a init line")),
        "{e}"
    );
    assert!(e.contains(&format!("holds conversation {ID}")), "{e}");
    let _ = c.kill();
    let _ = c.wait();
    let after = w.st.session(&s).unwrap();
    assert_eq!(after.head, before.head, "the store took a row");
    assert_eq!(after.binding.native_id.as_deref(), Some(ID));
    assert!(w
        .st
        .session_by_native(codec::HARNESS, OTHER)
        .unwrap()
        .is_none());
}

#[test]
fn a_resumed_conversation_continues_its_database_and_a_projected_one_is_written_first() {
    if !python() {
        eprintln!("skipped: needs python3");
        return;
    }
    let w = world();
    let s = w.resumed();
    w.st.queue_append(&s, "user", "go on").unwrap();
    let start = Start::of(&w.st, &s, &w.agy_home()).unwrap();
    assert_eq!(args(&start)[4..], ["--conversation".to_string(), ID.into()]);
    let mut c = w.spawn(&start, &[]);
    let out = Out::default();
    let r = run(headless(&mut c), w.ctx(&s, start, &out), Ends::WhenDone).unwrap();
    assert!(c.wait().unwrap().success());
    assert_eq!((r.turns, r.landed), (1, 1), "{r:?}");
    assert_eq!(r.native_id.as_deref(), Some(ID));
    assert_eq!(w.users(&s), ["old question", "go on"]);
    // Another sandbox has no database: the session is projected into it.
    let other = w.t.path().join("other/.gemini/antigravity-cli");
    let p = Start::of(&w.st, &s, &other).unwrap();
    assert_eq!(p.conversation.as_deref(), Some(ID));
    assert!(db_path(&other, ID).is_file());
}

/// A custody of scripted agy lines, with what paddock wrote.
fn scripted(w: &World, s: &str, lines: &[&str]) -> (anyhow::Result<Report>, String, Out) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let to = Out::default();
    let out = Out::default();
    let io = crate::horse::live::headless::io(std::io::Cursor::new(text.into_bytes()), to.clone());
    let r = run(io, w.ctx(s, Start::default(), &out), Ends::WhenDone);
    (r, to.text(), out)
}

#[test]
fn a_result_before_the_conversation_is_named_stops_the_custody() {
    let w = world();
    let s = w.fresh();
    w.st.queue_append(&s, "user", "task").unwrap();
    let (r, _, _) = scripted(&w, &s, &[r#"{"event":"result","status":"success"}"#]);
    let e = format!("{:#}", r.unwrap_err());
    assert!(e.contains("before it named its conversation"), "{e}");
    assert_eq!(w.st.session(&s).unwrap().binding.native_id, None);
}

#[test]
fn unknown_and_bad_lines_are_counted_and_a_bad_id_is_refused() {
    let w = world();
    let s = w.fresh();
    let (r, _, out) = scripted(
        &w,
        &s,
        &[
            r#"{"event":"heartbeat"}"#,
            r#"{"event":"heartbeat"}"#,
            "not json",
        ],
    );
    let r = r.unwrap();
    assert_eq!((r.unknown, r.bad, r.exited), (2, 1, true), "{r:?}");
    let text = out.text();
    assert_eq!(text.matches("event heartbeat").count(), 1, "{text}");
    let (r, _, _) = scripted(&w, &s, &[r#"{"event":"init","conversation_id":"../x"}"#]);
    assert!(format!("{:#}", r.unwrap_err()).contains("agy started conversation ../x"));
    // A conversation that another session holds already.
    let held = w.resumed();
    let line = format!(r#"{{"event":"init","conversation_id":"{ID}"}}"#);
    let (r, _, _) = scripted(&w, &s, &[&line]);
    let e = format!("{:#}", r.unwrap_err());
    assert!(e.contains(&format!("bound to store session {held}")), "{e}");
}

#[test]
fn a_session_with_rows_needs_its_conversation() {
    let w = world();
    let s = w.resumed();
    let out = Out::default();
    let io = crate::horse::live::headless::io(std::io::empty(), Out::default());
    let e = run(io, w.ctx(&s, Start::default(), &out), Ends::WhenDone).unwrap_err();
    assert!(format!("{e:#}").contains("agy must continue its conversation"));
    let io = crate::horse::live::headless::io(std::io::empty(), Out::default());
    let start = Start {
        conversation: Some(ID.into()),
    };
    std::fs::remove_file(db_path(&w.agy_home(), ID)).unwrap();
    let e = run(io, w.ctx(&s, start, &out), Ends::WhenDone).unwrap_err();
    assert!(format!("{e:#}").contains("project it first"), "{e:#}");
}

#[test]
fn an_early_interrupt_waits_for_the_turn_to_start() {
    use crate::horse::live::harness::Live;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let w = world();
    let s = w.fresh();
    let out = Out::default();
    let mut ctx = w.ctx(&s, Start::default(), &out);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    ctx.interrupt = Some(Box::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let mut agy = super::Agy::start(std::io::empty(), Vec::new(), ctx).unwrap();
    agy.busy = true;
    agy.interrupt().unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "signal before the turn started"
    );
    agy.line(br#"{"event":"step_update"}"#).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "pending signal was not sent"
    );
}
