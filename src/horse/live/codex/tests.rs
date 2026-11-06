//! Codex custody with the fake app-server of fake.rs on socket pairs. No
//! codex, no vendor.

use super::fake::{self, Fake, Seen, NEW_THREAD, OLD_THREAD};
use super::wire::{self, Incoming, Note};
use super::{args, find_rollout, ledger, policy, run, Answer, CustodyCtx, Policy, Report};
use crate::horse::codex::{self as codec, VERSION};
use crate::horse::live::harness::{ChannelIo, Control, Ends};
use crate::horse::{Binding, Store};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    _t: tempfile::TempDir,
    st: Store,
    horse: String,
    root: PathBuf,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap().id;
    let root = t.path().join("home/.codex/sessions");
    std::fs::create_dir_all(&root).unwrap();
    World {
        _t: t,
        st,
        horse,
        root,
    }
}

impl World {
    /// A session without a thread.
    fn fresh(&self) -> String {
        let b = Binding {
            harness: codec::HARNESS.into(),
            native_id: None,
            version: VERSION.into(),
            container: codec::CONTAINER.into(),
        };
        self.st.new_session(&self.horse, b).unwrap().id
    }

    /// A session of an old thread whose rollout is projected.
    fn projected(&self) -> String {
        let p = fake::write_rollout(
            &self.root,
            OLD_THREAD,
            &[
                fake::meta(OLD_THREAD),
                fake::row(
                    "response_item",
                    fake::message("user", "input_text", "old question"),
                ),
            ],
        );
        codec::ingest(&self.st, &p, VERSION, Some(&self.horse))
            .unwrap()
            .session
    }
}

struct Live {
    ctl: Sender<Control>,
    join: std::thread::JoinHandle<anyhow::Result<Report>>,
    out: Out,
    seen: Arc<Mutex<Seen>>,
}

fn bypass() -> Policy {
    Policy {
        approval: "never".into(),
        sandbox: "danger-full-access".into(),
    }
}

fn start_with(w: &World, session: &str, f: impl FnOnce(&mut Fake), pol: Policy) -> Live {
    let (pad_w, fake_r) = UnixStream::pair().unwrap();
    let (fake_w, pad_r) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut fk = Fake {
        root: w.root.clone(),
        resume_as: None,
        approval: wire::APPROVE_COMMAND,
        step: Duration::from_millis(60),
        unsubscribe: true,
        persist_forks: false,
        raw_items: true,
        seen: seen.clone(),
    };
    f(&mut fk);
    std::thread::spawn(move || fk.serve(fake_r, fake_w));
    let (ctl, control) = std::sync::mpsc::channel();
    let out = Out::default();
    let ctx = CustodyCtx {
        store: w.st.clone(),
        horse: w.horse.clone(),
        session: session.into(),
        cwd: fake::CWD.into(),
        sessions_root: w.root.clone(),
        run: "sbx-test".into(),
        model: None,
        effort: None,
        tick: Duration::from_millis(10),
        reply_timeout: Duration::from_secs(10),
        close_timeout: Duration::from_secs(5),
        out: Box::new(out.clone()),
    };
    let io = ChannelIo {
        from: pad_r,
        to: pad_w,
        control,
    };
    let join = std::thread::spawn(move || run(io, ctx, pol, Ends::ByRun));
    Live {
        ctl,
        join,
        out,
        seen,
    }
}

fn start(w: &World, session: &str) -> Live {
    start_with(w, session, |_| {}, bypass())
}

fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn count(seen: &Arc<Mutex<Seen>>, method: &str) -> usize {
    seen.lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.0 == method)
        .count()
}

impl Live {
    fn close(self) -> Report {
        self.ctl.send(Control::Close).unwrap();
        self.join.join().unwrap().unwrap()
    }
}

#[test]
fn a_projected_session_resumes_its_thread() {
    let w = world();
    let sid = w.projected();
    let head = w.st.session(&sid).unwrap().head;
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "hello again").unwrap();
    until("the turn", || count(&l.seen, wire::TURN_START) == 1);
    let seen = l.seen.clone();
    let r = l.close();
    assert_eq!(count(&seen, wire::THREAD_START), 0);
    let s = seen.lock().unwrap();
    let resume = s.requests.iter().find(|x| x.0 == wire::THREAD_RESUME);
    assert_eq!(resume.unwrap().1["threadId"], OLD_THREAD);
    assert_eq!(r.native_id.as_deref(), Some(OLD_THREAD));
    // The new turn went on top of the projected rows.
    let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
    assert!(nodes.iter().any(|n| Some(&n.hash) == head.as_ref()));
    assert!(nodes.len() > 2);
}

#[test]
fn a_resumed_thread_with_another_id_ends_custody_without_a_store_write() {
    let w = world();
    let sid = w.projected();
    w.st.queue_append(&sid, "user", "never sent").unwrap();
    let before = std::fs::read(w.st.root().join(format!("sessions/{sid}.json"))).unwrap();
    let l = start_with(
        &w,
        &sid,
        |f| f.resume_as = Some(NEW_THREAD.into()),
        bypass(),
    );
    let e = l.join.join().unwrap().unwrap_err();
    assert!(format!("{e:#}").contains("instead of"), "{e:#}");
    let after = std::fs::read(w.st.root().join(format!("sessions/{sid}.json"))).unwrap();
    assert_eq!(before, after);
    assert_eq!(w.st.inbox(&sid).unwrap().len(), 1);
    assert!(w.st.deliveries(&sid).unwrap().is_empty());
    assert!(!w.st.root().join(format!("guard/{sid}.json")).exists());
    assert_eq!(count(&l.seen, wire::TURN_START), 0);
}

#[test]
fn a_fresh_session_starts_a_thread_and_binds_its_id() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    until("the binding", || {
        w.st.session(&sid).unwrap().binding.native_id.as_deref() == Some(NEW_THREAD)
    });
    let seen = l.seen.clone();
    let r = l.close();
    assert_eq!(r.native_id.as_deref(), Some(NEW_THREAD));
    let s = seen.lock().unwrap();
    assert_eq!(s.methods()[..2], [wire::INITIALIZE, wire::THREAD_START]);
    let p = &s.requests[1].1;
    assert_eq!(p["approvalPolicy"], "never");
    assert_eq!(p["sandbox"], "danger-full-access");
    assert_eq!(p["cwd"], fake::CWD);
}

#[test]
fn an_entry_queued_while_busy_goes_out_after_turn_completed() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "first").unwrap();
    until("the first turn", || count(&l.seen, wire::TURN_START) == 1);
    w.st.queue_append(&sid, "user", "second").unwrap();
    w.st.queue_append(&sid, "user", "third").unwrap();
    until("three turns", || count(&l.seen, wire::TURN_START) == 3);
    until("three landed deliveries", || {
        let d = w.st.deliveries(&sid).unwrap();
        d.len() == 3 && d.iter().all(|d| d.landed_seq.is_some())
    });
    let seen = l.seen.clone();
    let r = l.close();
    let s = seen.lock().unwrap();
    assert_eq!(s.steers, 0);
    let turns: Vec<_> = s
        .requests
        .iter()
        .filter(|x| x.0 == wire::TURN_START)
        .collect();
    assert!(
        turns.iter().all(|t| !t.2),
        "a turn/start came during a turn"
    );
    let texts: Vec<_> = turns
        .iter()
        .map(|t| t.1["input"][0]["text"].clone())
        .collect();
    assert_eq!(texts, [json!("first"), json!("second"), json!("third")]);
    assert_eq!(r.turns, 3);
    assert_eq!(r.deliveries, 3);
}

#[test]
fn a_user_entry_starts_a_turn_and_a_note_is_injected() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    let u = w.st.queue_append(&sid, "user", "do the thing").unwrap();
    let n = w.st.queue_append(&sid, "note", "the build is red").unwrap();
    until("the inject", || count(&l.seen, wire::INJECT_ITEMS) == 1);
    let seen = l.seen.clone();
    let r = l.close();
    let s = seen.lock().unwrap();
    let t = s.requests.iter().find(|x| x.0 == wire::TURN_START).unwrap();
    assert_eq!(t.1["input"][0]["text"], "do the thing");
    assert_eq!(t.1["clientUserMessageId"], u.id.as_str());
    let i = s
        .requests
        .iter()
        .find(|x| x.0 == wire::INJECT_ITEMS)
        .unwrap();
    assert!(!i.2, "the inject came during a turn");
    let item = &i.1["items"][0];
    assert_eq!(item["type"], "message");
    assert_eq!(item["role"], "developer");
    assert_eq!(item["content"][0]["text"], n.wire_text().as_str());
    assert_eq!(r.deliveries, 2);
    // Both rows came into the store, so both deliveries landed.
    assert_eq!(r.landed, 2);
    let d = w.st.deliveries(&sid).unwrap();
    assert!(d.iter().all(|d| d.landed_seq.is_some()), "{d:?}");
    let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
    for x in &d {
        let row: Value =
            serde_json::from_slice(&nodes[x.landed_seq.unwrap() as usize].raw).unwrap();
        assert_eq!(
            row["payload"]["content"][0]["text"],
            x.entry.wire_text().as_str()
        );
    }
}

#[test]
fn an_interrupt_is_a_safe_point_for_the_next_entry() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "[long] count to a million")
        .unwrap();
    until("the long turn", || count(&l.seen, wire::TURN_START) == 1);
    w.st.queue_append(&sid, "user", "stop and say hi").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        count(&l.seen, wire::TURN_START),
        1,
        "sent during the long turn"
    );
    l.ctl.send(Control::Interrupt).unwrap();
    until("the next turn", || count(&l.seen, wire::TURN_START) == 2);
    let seen = l.seen.clone();
    let out = l.out.clone();
    let r = l.close();
    let s = seen.lock().unwrap();
    let m: Vec<_> = s
        .methods()
        .into_iter()
        .filter(|m| m.starts_with("turn/"))
        .collect();
    assert_eq!(
        m,
        [wire::TURN_START, wire::TURN_INTERRUPT, wire::TURN_START]
    );
    assert_eq!(s.steers, 0);
    let ti = s
        .requests
        .iter()
        .find(|x| x.0 == wire::TURN_INTERRUPT)
        .unwrap();
    assert_eq!(ti.1["threadId"], NEW_THREAD);
    assert!(ti.1["turnId"].as_str().unwrap().starts_with("019a5f00"));
    assert_eq!(r.interrupts, 1);
    assert!(out.text().contains("interrupted"), "{}", out.text());
}

#[test]
fn an_approval_follows_the_plain_run_and_is_logged() {
    for (pol, want) in [
        (bypass(), "accept"),
        (
            Policy::of(&["exec".into(), "--sandbox".into(), "workspace-write".into()]).unwrap(),
            "decline",
        ),
    ] {
        let w = world();
        let sid = w.fresh();
        let l = start_with(&w, &sid, |_| {}, pol.clone());
        w.st.queue_append(&sid, "user", "[approve] clean up")
            .unwrap();
        until("the answer", || !l.seen.lock().unwrap().answers.is_empty());
        until("the turn end", || w.st.inbox(&sid).unwrap().is_empty());
        let seen = l.seen.clone();
        let r = l.close();
        let a = &seen.lock().unwrap().answers[0];
        assert_eq!(a.0, wire::APPROVE_COMMAND);
        assert_eq!(a.1, json!({ "decision": want }));
        assert_eq!(r.answers, 1);
        let lines = ledger(&w.st, &sid).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["decision"], want);
        assert_eq!(lines[0]["command"], "rm -rf build");
        assert_eq!(lines[0]["approval_policy"], pol.approval.as_str());
    }
}

#[test]
fn every_turn_is_ingested_and_the_guard_follows() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "one").unwrap();
    until("the first ingest", || {
        w.st.deliveries(&sid)
            .unwrap()
            .first()
            .is_some_and(|d| d.landed_seq.is_some())
    });
    let h1 = w.st.session(&sid).unwrap().head.unwrap();
    let g = w.st.guard_state(&sid).unwrap().tuple;
    assert_eq!(g.native_id.as_deref(), Some(NEW_THREAD));
    assert_eq!(g.head.as_ref(), Some(&h1));
    w.st.queue_append(&sid, "user", "two").unwrap();
    until("the second ingest", || {
        w.st.deliveries(&sid)
            .unwrap()
            .get(1)
            .is_some_and(|d| d.landed_seq.is_some())
    });
    let h2 = w.st.session(&sid).unwrap().head.unwrap();
    assert_ne!(h1, h2);
    let r = l.close();
    // The store holds the rollout byte for byte, keyed by record identity.
    let p = find_rollout(&w.root, NEW_THREAD).unwrap();
    let file = std::fs::read(&p).unwrap();
    let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
    let mut joined = Vec::new();
    for n in &nodes {
        joined.extend_from_slice(&n.raw);
        joined.push(b'\n');
    }
    assert_eq!(joined, file);
    assert!(nodes.iter().any(|n| n
        .meta
        .key
        .as_deref()
        .is_some_and(|k| k.starts_with("response:msg_"))));
    assert_eq!(r.turns, 2);
    assert_eq!(r.landed, 2);
    assert_eq!(r.tokens, Some(240));
    // The first turn sent a notification paddock does not read, an empty
    // line and a line that is not JSON: counted, not fatal.
    assert_eq!(r.unknown, 1);
    assert_eq!(r.malformed, 1);
    assert!(r.exited);
}

#[test]
fn a_user_line_waits_in_the_inbox_like_an_append() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    l.ctl.send(Control::User("typed".into())).unwrap();
    l.ctl.send(Control::User("   ".into())).unwrap();
    until("the turn", || count(&l.seen, wire::TURN_START) == 1);
    let r = l.close();
    assert_eq!(r.deliveries, 1);
    assert_eq!(w.st.deliveries(&sid).unwrap()[0].entry.text, "typed");
}

#[test]
fn a_session_that_cannot_be_held_is_refused_before_the_server_starts() {
    let w = world();
    // Rows but no thread: an unprojected fork.
    let sid = w.projected();
    w.st.update_session(&sid, |s| s.binding.native_id = None)
        .unwrap();
    let l = start(&w, &sid);
    let e = l.join.join().unwrap().unwrap_err();
    assert!(format!("{e:#}").contains("project it first"), "{e:#}");
    // A thread whose rollout is gone.
    let sid = w.fresh();
    w.st.update_session(&sid, |s| s.binding.native_id = Some(NEW_THREAD.into()))
        .unwrap();
    let l = start(&w, &sid);
    let e = l.join.join().unwrap().unwrap_err();
    assert!(format!("{e:#}").contains("no rollout file"), "{e:#}");
    assert!(l.seen.lock().unwrap().requests.is_empty());
}

#[test]
fn the_server_exit_ends_the_custody() {
    let w = world();
    let sid = w.fresh();
    let (pad_w, fake_r) = UnixStream::pair().unwrap();
    let (fake_w, pad_r) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fk = Fake {
        root: w.root.clone(),
        resume_as: None,
        approval: wire::APPROVE_COMMAND,
        step: Duration::from_millis(20),
        unsubscribe: true,
        persist_forks: false,
        raw_items: true,
        seen: seen.clone(),
    };
    let server = std::thread::spawn(move || fk.serve(fake_r, fake_w));
    let (ctl, control) = std::sync::mpsc::channel();
    let ctx = CustodyCtx {
        store: w.st.clone(),
        horse: w.horse.clone(),
        session: sid.clone(),
        cwd: fake::CWD.into(),
        sessions_root: w.root.clone(),
        run: "sbx-test".into(),
        model: None,
        effort: None,
        tick: Duration::from_millis(10),
        reply_timeout: Duration::from_secs(10),
        close_timeout: Duration::from_secs(5),
        out: Box::new(Out::default()),
    };
    let join = std::thread::spawn(move || {
        run(
            ChannelIo {
                from: pad_r,
                to: pad_w,
                control,
            },
            ctx,
            bypass(),
            Ends::ByRun,
        )
    });
    until("the thread", || {
        w.st.session(&sid).unwrap().binding.native_id.is_some()
    });
    // The control side closes: paddock closes the input, the server exits.
    drop(ctl);
    let r = join.join().unwrap().unwrap();
    server.join().unwrap();
    assert!(r.exited);
}

#[test]
fn the_plain_run_settings_come_from_the_profile() {
    let p = crate::profile::builtin("codex").unwrap();
    assert_eq!(policy(&p).unwrap(), bypass());
    let a = args(&p).unwrap();
    assert_eq!(a[..3], ["app-server", "--listen", "stdio://"]);
    assert!(a.contains(&"approval_policy=\"never\"".to_string()));
    assert!(a.contains(&"sandbox_mode=\"danger-full-access\"".to_string()));
    let of = |v: &[&str]| Policy::of(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(of(&["exec"]).unwrap().sandbox, "read-only");
    assert_eq!(of(&["exec", "--full-auto"]).unwrap().approval, "on-request");
    assert_eq!(
        of(&["-a", "untrusted", "--sandbox=workspace-write"])
            .unwrap()
            .sandbox,
        "workspace-write"
    );
    assert!(of(&["-a", "always"]).is_err());
    assert!(of(&["--sandbox"]).is_err());
    // Never more permissive than the plain run.
    let ro = of(&["exec"]).unwrap();
    for m in [wire::APPROVE_COMMAND, wire::APPROVE_FILE_CHANGE] {
        assert_eq!(ro.answer(m).decision(), "decline");
        assert_eq!(bypass().answer(m).decision(), "accept");
    }
    assert_eq!(ro.answer(wire::LEGACY_EXEC).decision(), "denied");
    assert!(matches!(
        bypass().answer(wire::APPROVE_PERMISSIONS),
        Answer::Result(v, _) if v["permissions"] == json!({})
    ));
    assert!(matches!(
        bypass().answer(wire::USER_INPUT),
        Answer::Error(..)
    ));
    assert!(matches!(
        bypass().answer("item/tool/call"),
        Answer::Error(..)
    ));
    let mut no = p.clone();
    no.oneshot = None;
    assert!(args(&no).is_err());
}

#[test]
fn the_wire_parses_as_the_schema_says() {
    let p = |s: &str| wire::parse(s.as_bytes());
    assert_eq!(
        p(
            r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"u","status":"interrupted","items":[],"error":{"message":"stop"}}}}"#
        ),
        Incoming::Note(Note::TurnCompleted {
            thread: "t".into(),
            turn: wire::Turn {
                id: "u".into(),
                status: "interrupted".into(),
                error: Some("stop".into())
            }
        })
    );
    assert!(matches!(
        p(r#"{"id":"ap-1","method":"item/fileChange/requestApproval","params":{}}"#),
        Incoming::Request {
            id: Value::String(_),
            ..
        }
    ));
    assert!(matches!(
        p(r#"{"id":3,"result":{}}"#),
        Incoming::Response { result: Ok(_), .. }
    ));
    assert!(matches!(
        p(r#"{"id":3,"error":{"code":-1,"message":"no"}}"#),
        Incoming::Response { result: Err(m), .. } if m == "no"
    ));
    assert_eq!(
        p(r#"{"method":"item/started","params":{}}"#),
        Incoming::Note(Note::Other("item/started".into()))
    );
    // A known method with a shape it does not have is an unknown note.
    assert_eq!(
        p(r#"{"method":"turn/started","params":{}}"#),
        Incoming::Note(Note::Other("turn/started".into()))
    );
    assert_eq!(p("nope"), Incoming::Malformed);
    assert_eq!(p(r#"{"id":4}"#), Incoming::Malformed);
    assert_eq!(p("{}"), Incoming::Malformed);
}

#[test]
fn find_rollout_takes_only_rollout_files_of_the_thread() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    assert!(find_rollout(root, NEW_THREAD).is_none());
    let p = fake::write_rollout(root, NEW_THREAD, &[fake::meta(NEW_THREAD)]);
    std::fs::write(p.with_file_name(format!("other-{NEW_THREAD}.jsonl")), "").unwrap();
    assert_eq!(find_rollout(root, NEW_THREAD), Some(p));
    assert!(find_rollout(root, OLD_THREAD).is_none());
    assert!(find_rollout(Path::new("/nonexistent"), NEW_THREAD).is_none());
}

#[path = "side_tests.rs"]
mod side;
