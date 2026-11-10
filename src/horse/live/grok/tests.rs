//! Grok custody with the fake ACP agent of fake.rs on socket pairs. No
//! grok, no vendor.

use super::fake::{self, Fake, Seen, NEW_ID, OLD_ID};
use super::{args, ledger_line, policy, run, wire, CustodyCtx, Policy, Report};
use crate::horse::grok::fixture::Fixture;
use crate::horse::grok::{self as codec, VERSION};
use crate::horse::live::codex::ledger;
use crate::horse::live::harness::{ChannelIo, Control, Ends};
use crate::horse::{Binding, Store};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
    home: PathBuf,
}

fn world() -> World {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap().id;
    let home = t.path().join("home/.grok");
    std::fs::create_dir_all(&home).unwrap();
    World {
        _t: t,
        st,
        horse,
        home,
    }
}

impl World {
    /// A session without a Grok session.
    fn fresh(&self) -> String {
        let b = Binding {
            harness: codec::HARNESS.into(),
            native_id: None,
            version: VERSION.into(),
            container: codec::CONTAINER.into(),
        };
        self.st.new_session(&self.horse, b).unwrap().id
    }

    /// A session of an old Grok session whose folder is there.
    fn projected(&self) -> String {
        let mut f = Fixture::new(OLD_ID, fake::CWD);
        f.prompt(0, "old question").reply("old answer");
        let dir = f.write(&self.home);
        codec::ingest(&self.st, &dir, VERSION, Some(&self.horse))
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

fn start_with(w: &World, session: &str, f: impl FnOnce(&mut Fake), pol: Policy) -> Live {
    let (pad_w, fake_r) = UnixStream::pair().unwrap();
    let (fake_w, pad_r) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut fk = Fake::new(w.home.clone(), seen.clone());
    f(&mut fk);
    std::thread::spawn(move || fk.serve(fake_r, fake_w));
    let (ctl, control) = std::sync::mpsc::channel();
    let out = Out::default();
    let ctx = CustodyCtx {
        store: w.st.clone(),
        horse: w.horse.clone(),
        session: session.into(),
        cwd: fake::CWD.into(),
        grok_home: w.home.clone(),
        run: "sbx-test".into(),
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

const YES: Policy = Policy {
    always_approve: true,
};

fn start(w: &World, session: &str) -> Live {
    start_with(w, session, |_| {}, YES)
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
fn a_fresh_session_starts_a_grok_session_and_binds_its_id() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    until("the binding", || {
        w.st.session(&sid).unwrap().binding.native_id.as_deref() == Some(NEW_ID)
    });
    let seen = l.seen.clone();
    let out = l.out.clone();
    let r = l.close();
    assert_eq!(r.native_id.as_deref(), Some(NEW_ID));
    let s = seen.lock().unwrap();
    assert_eq!(s.methods()[..2], [wire::INITIALIZE, wire::SESSION_NEW]);
    assert_eq!(s.requests[0].1["protocolVersion"], 1);
    assert_eq!(s.requests[1].1["cwd"], fake::CWD);
    assert!(out
        .text()
        .contains("grok session 019a5f00-1111-7000-8000-000000000001 in custody"));
}

#[test]
fn a_projected_session_is_loaded_under_its_id_and_grows_on_top() {
    let w = world();
    let sid = w.projected();
    let head = w.st.session(&sid).unwrap().head;
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "hello again").unwrap();
    until("the turn", || {
        w.st.deliveries(&sid)
            .unwrap()
            .iter()
            .any(|d| d.landed_seq.is_some())
    });
    let seen = l.seen.clone();
    let r = l.close();
    assert_eq!(count(&seen, wire::SESSION_NEW), 0);
    let s = seen.lock().unwrap();
    let load = s
        .requests
        .iter()
        .find(|x| x.0 == wire::SESSION_LOAD)
        .unwrap();
    assert_eq!(load.1["sessionId"], OLD_ID);
    assert_eq!(load.1["cwd"], fake::CWD);
    assert_eq!(r.native_id.as_deref(), Some(OLD_ID));
    assert_eq!((r.turns, r.deliveries, r.landed), (1, 1, 1));
    let nodes = w.st.session_nodes(&w.st.session(&sid).unwrap()).unwrap();
    assert!(nodes.iter().any(|n| Some(&n.hash) == head.as_ref()));
    assert!(nodes
        .iter()
        .any(|n| String::from_utf8_lossy(&n.raw).contains("hello again")));
}

#[test]
fn a_loaded_session_with_another_id_ends_custody_without_a_store_write() {
    let w = world();
    let sid = w.projected();
    w.st.queue_append(&sid, "user", "never sent").unwrap();
    let before = std::fs::read(w.st.root().join(format!("sessions/{sid}.json"))).unwrap();
    let l = start_with(&w, &sid, |f| f.load_as = Some(NEW_ID.into()), YES);
    let e = l.join.join().unwrap().unwrap_err();
    assert!(format!("{e:#}").contains("for a load of"), "{e:#}");
    let after = std::fs::read(w.st.root().join(format!("sessions/{sid}.json"))).unwrap();
    assert_eq!(before, after);
    assert_eq!(w.st.inbox(&sid).unwrap().len(), 1);
    assert!(w.st.deliveries(&sid).unwrap().is_empty());
    assert!(!w.st.root().join(format!("guard/{sid}.json")).exists());
    assert_eq!(count(&l.seen, wire::SESSION_PROMPT), 0);
}

#[test]
fn entries_go_out_only_after_the_prompt_answer_in_order_and_by_role() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    until("the session", || {
        w.st.session(&sid).unwrap().binding.native_id.is_some()
    });
    // All three wait while the first turn runs.
    w.st.queue_append(&sid, "user", "first").unwrap();
    until("the first turn", || {
        count(&l.seen, wire::SESSION_PROMPT) == 1
    });
    w.st.queue_append(&sid, "system", "the rules").unwrap();
    w.st.queue_append(&sid, "note", "a note").unwrap();
    until("three turns", || {
        w.st.deliveries(&sid)
            .unwrap()
            .iter()
            .filter(|d| d.landed_seq.is_some())
            .count()
            == 3
    });
    let seen = l.seen.clone();
    let r = l.close();
    let s = seen.lock().unwrap();
    assert_eq!(s.steers, 0, "a prompt went out while a turn ran");
    assert!(s
        .requests
        .iter()
        .filter(|x| x.0 == wire::SESSION_PROMPT)
        .all(|x| !x.2));
    assert_eq!(s.prompts[0], "first");
    assert_eq!(
        s.prompts[1],
        "<paddock-append role=\"system\">\nthe rules\n</paddock-append>"
    );
    assert_eq!(
        s.prompts[2],
        "<paddock-append role=\"note\">\na note\n</paddock-append>"
    );
    assert_eq!((r.turns, r.deliveries, r.landed), (3, 3, 3));
    assert!(w.st.inbox(&sid).unwrap().is_empty());
    // The non-JSON line and the unknown note were seen and skipped.
    assert_eq!((r.malformed, r.unknown), (1, 1));
}

#[test]
fn an_interrupt_sends_session_cancel_and_the_next_entry_waits_for_the_answer() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "[long] count to a million")
        .unwrap();
    until("the long turn", || {
        count(&l.seen, wire::SESSION_PROMPT) == 1
    });
    w.st.queue_append(&sid, "user", "after the stop").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(count(&l.seen, wire::SESSION_PROMPT), 1, "the entry waits");
    l.ctl.send(Control::Interrupt).unwrap();
    until("the next turn", || {
        count(&l.seen, wire::SESSION_PROMPT) == 2
    });
    until("its end", || w.st.inbox(&sid).unwrap().is_empty());
    let seen = l.seen.clone();
    let r = l.close();
    let s = seen.lock().unwrap();
    let cancels: Vec<&Value> = s
        .notes
        .iter()
        .filter(|n| n.0 == wire::SESSION_CANCEL)
        .map(|n| &n.1)
        .collect();
    assert_eq!(cancels, [&json!({"sessionId": NEW_ID})]);
    assert_eq!(s.steers, 0);
    assert_eq!(r.interrupts, 1);
    assert_eq!(r.cancelled, 1);
}

#[test]
fn a_permission_gets_the_answer_of_the_plain_run_and_is_logged() {
    for (pol, option) in [
        (YES, "once"),
        (
            Policy {
                always_approve: false,
            },
            "no",
        ),
    ] {
        let w = world();
        let sid = w.fresh();
        let l = start_with(&w, &sid, |_| {}, pol);
        w.st.queue_append(&sid, "user", "[approve] clean up")
            .unwrap();
        until("the answer", || !l.seen.lock().unwrap().answers.is_empty());
        until("the turn", || {
            w.st.deliveries(&sid)
                .unwrap()
                .iter()
                .any(|d| d.landed_seq.is_some())
        });
        let seen = l.seen.clone();
        let r = l.close();
        let a = &seen.lock().unwrap().answers[0];
        assert_eq!(a["result"]["outcome"]["optionId"], option, "{a}");
        assert_eq!(r.answers, 1);
        let lines = ledger(&w.st, &sid).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["method"], wire::REQUEST_PERMISSION);
        assert_eq!(lines[0]["always_approve"], pol.always_approve);
    }
    // A request with no option of the wanted kind is cancelled.
    let (v, d) = YES.answer(&json!({"options": [{"optionId": "a", "kind": "allow_always"}]}));
    assert_eq!(v, json!({"outcome": {"outcome": "cancelled"}}));
    assert_eq!(d, "cancelled");
    let l = ledger_line(&json!({"sessionId": "s"}), &d, &YES);
    assert_eq!(l["decision"], "cancelled");
}

#[test]
fn a_refused_prompt_stays_in_the_inbox() {
    let w = world();
    let sid = w.fresh();
    let l = start(&w, &sid);
    w.st.queue_append(&sid, "user", "[refuse] this").unwrap();
    until("the refusal", || l.out.text().contains("did not take"));
    let r = l.close();
    assert_eq!(r.deliveries, 0);
    assert_eq!(w.st.inbox(&sid).unwrap().len(), 1);
    assert!(w.st.deliveries(&sid).unwrap().is_empty());
}

#[test]
fn another_agent_version_is_refused() {
    let w = world();
    let sid = w.fresh();
    let l = start_with(&w, &sid, |f| f.version = "1.0.71".into(), YES);
    let e = l.join.join().unwrap().unwrap_err();
    assert!(
        format!("{e:#}").contains("Grok Build 1.0.71 is not supported"),
        "{e:#}"
    );
    assert!(w.st.session(&sid).unwrap().binding.native_id.is_none());
}

#[test]
fn a_session_that_cannot_be_held_is_refused_before_the_agent_starts() {
    let w = world();
    // A session id with no session folder.
    let sid = w.fresh();
    w.st.update_session(&sid, |s| s.binding.native_id = Some(OLD_ID.into()))
        .unwrap();
    let l = start(&w, &sid);
    let e = l.join.join().unwrap().unwrap_err();
    assert!(format!("{e:#}").contains("project it first"), "{e:#}");
    assert!(l.seen.lock().unwrap().requests.is_empty());
}

#[test]
fn the_agent_exit_ends_the_custody_with_a_last_ingest() {
    let w = world();
    let sid = w.fresh();
    let (pad_w, fake_r) = UnixStream::pair().unwrap();
    let (fake_w, pad_r) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let fk = Fake::new(w.home.clone(), seen.clone());
    let agent = std::thread::spawn(move || fk.serve(fake_r, fake_w));
    let (_ctl, control) = std::sync::mpsc::channel();
    let ctx = CustodyCtx {
        store: w.st.clone(),
        horse: w.horse.clone(),
        session: sid.clone(),
        cwd: fake::CWD.into(),
        grok_home: w.home.clone(),
        run: "sbx-exit".into(),
        tick: Duration::from_millis(10),
        reply_timeout: Duration::from_secs(10),
        close_timeout: Duration::from_secs(5),
        out: Box::new(Out::default()),
    };
    w.st.queue_append(&sid, "user", "the task").unwrap();
    let io = ChannelIo {
        from: pad_r,
        to: pad_w,
        control,
    };
    // A headless run: it closes once the task is done.
    let r = run(io, ctx, YES, Ends::WhenDone).unwrap();
    agent.join().unwrap();
    assert_eq!((r.turns, r.deliveries, r.landed), (1, 1, 1));
    assert!(w.st.inbox(&sid).unwrap().is_empty());
    assert!(crate::horse::live::records(&w.st).unwrap().is_empty());
}

#[test]
fn the_plain_run_settings_come_from_the_profile() {
    let p = crate::profile::builtin("grok").unwrap();
    let pol = policy(&p).unwrap();
    assert!(pol.always_approve);
    assert_eq!(
        args(&pol),
        ["agent", "--always-approve", "--no-leader", "stdio"]
    );
    let ask = Policy::of(&["--prompt-file".into(), "x".into()]);
    assert!(!ask.always_approve);
    assert_eq!(args(&ask), ["agent", "--no-leader", "stdio"]);
}

#[test]
fn the_wire_parses_as_acp_says() {
    use wire::{parse, Incoming};
    assert_eq!(
        parse(br#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"cancelled"}}"#),
        Incoming::Response {
            id: json!(3),
            result: Ok(json!({"stopReason": "cancelled"}))
        }
    );
    assert!(matches!(
        parse(br#"{"jsonrpc":"2.0","id":"a","method":"session/request_permission","params":{}}"#),
        Incoming::Request { .. }
    ));
    assert!(matches!(
        parse(br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s"}}"#),
        Incoming::Note { .. }
    ));
    assert_eq!(parse(b"[1]"), Incoming::Malformed);
    assert_eq!(wire::stop_reason(&json!({})), "end_turn");
    assert_eq!(
        wire::agent_version(&json!({"agentInfo": {"version": "1.0.70"}})).as_deref(),
        Some("1.0.70")
    );
}
