//! Custody, live ingest, safe-point append and the prefix guard, with the
//! fake dsh of fake.rs on a socket pair. No dsh, no vendor.

use super::custody::{run, Report, Setup};
use super::fake::{self, Fake, Seen};
use super::harness::Control;
use super::rpc;
use crate::horse::guard::{check, Op, TTL_MS};
use crate::horse::store::Origin;
use crate::horse::{dsh, Binding, Store};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
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

fn out_has(o: &Out, s: &str) -> bool {
    o.text().contains(s)
}

struct Run {
    tx: Sender<Control>,
    join: std::thread::JoinHandle<anyhow::Result<Report>>,
    out: Out,
    seen: Arc<Mutex<Seen>>,
}

fn setup(st: &Store, horse: &str, main: &str, root: &Path, resume: bool) -> Setup {
    Setup {
        store: st.clone(),
        horse: horse.into(),
        main: main.into(),
        resume,
        sessions_root: Some(root.to_path_buf()),
        cwd: fake::CWD.into(),
        provider: super::PROVIDER.into(),
        model: super::MODEL.into(),
        effort: None,
        run: "sbx-test".into(),
        tick: Duration::from_millis(20),
    }
}

fn start(set: Setup, existing: Vec<(String, Vec<Vec<u8>>)>) -> Run {
    let (pad, dsh_end) = UnixStream::pair().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let f = Fake {
        root: set.sessions_root.clone(),
        existing,
        step: Duration::from_millis(5),
        seen: seen.clone(),
    };
    let input = dsh_end.try_clone().unwrap();
    std::thread::spawn(move || f.serve(input, dsh_end));
    let (rt, rx) = std::sync::mpsc::channel();
    rpc::read_runtime(pad.try_clone().unwrap(), rt);
    let (tx, control) = std::sync::mpsc::channel();
    let out = Out::default();
    let o = out.clone();
    let join = std::thread::spawn(move || run(set, pad, rx, &control, o));
    Run {
        tx,
        join,
        out,
        seen,
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn rows_of(st: &Store, sid: &str) -> Vec<Vec<u8>> {
    let s = st.session(sid).unwrap();
    st.session_nodes(&s)
        .unwrap()
        .into_iter()
        .map(|n| n.raw)
        .collect()
}

fn ends_turn(st: &Store, sid: &str, turns: usize) -> bool {
    rows_of(st, sid)
        .iter()
        .filter(|r| r.windows(10).any(|w| w == b"\"turn/end\""))
        .count()
        >= turns
}

#[test]
fn custody_ingests_the_stream_and_delivers_an_append_at_the_safe_point() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let root = t.path().join("sessions");
    let h = st.new_default_horse().unwrap();
    let main = "session-11111111-2222-4333-8444-555555555555";
    let r = start(setup(&st, &h.id, main, &root, false), vec![]);
    r.tx.send(Control::User("hello".into())).unwrap();
    let sid = {
        let mut sid = None;
        wait_for("the main session", || {
            sid = st.session_by_native("dsh", main).unwrap().map(|s| s.id);
            sid.is_some()
        });
        sid.unwrap()
    };
    // The live record shows the run while it runs.
    wait_for("the live record", || {
        super::records(&st).unwrap().iter().any(|x| x.main == main)
    });
    wait_for("turn 1", || ends_turn(&st, &sid, 1));
    st.queue_append(&sid, "note", "the host says hi").unwrap();
    wait_for("turn 2", || ends_turn(&st, &sid, 2));
    wait_for("the landed append", || {
        st.deliveries(&sid)
            .unwrap()
            .first()
            .is_some_and(|d| d.landed_seq.is_some())
    });
    r.tx.send(Control::Close).unwrap();
    let rep = r.join.join().unwrap().unwrap();

    // Every streamed row is in the store, byte for byte, in order.
    let seen = r.seen.lock().unwrap();
    let streamed: Vec<Vec<u8>> = seen.rows.iter().map(|(_, r)| r.clone()).collect();
    assert_eq!(rows_of(&st, &sid), streamed);
    assert_eq!(rep.events, streamed.len());
    assert_eq!(rep.delivered, 1);
    // The file agrees: the end of custody read it for the header.
    let s = st.session(&sid).unwrap();
    let hdr = st.get(s.native_header.as_ref().unwrap()).unwrap().raw;
    assert_eq!(hdr, fake::header(main).encode());

    // The append came only when the session was idle, as the newest entry.
    assert_eq!(seen.prompts.len(), 2);
    assert!(seen.prompts.iter().all(|(_, _, running)| !running));
    assert!(seen.prompts[1].1.contains("role=\"note\""));
    let d = &st.deliveries(&sid).unwrap()[0];
    let landed = &rows_of(&st, &sid)[d.landed_seq.unwrap() as usize];
    assert!(String::from_utf8_lossy(landed).contains("the host says hi"));
    let turn1_end = rows_of(&st, &sid)
        .iter()
        .position(|r| r.windows(10).any(|w| w == b"\"turn/end\""))
        .unwrap();
    assert!(d.landed_seq.unwrap() as usize > turn1_end);
    // The delivery is a node in the DAG, beside the head of that moment.
    let node = st.get(&d.node).unwrap();
    assert_eq!(node.meta.parent, d.at_head);
    assert_eq!(
        d.at_head,
        Some(st.session_nodes(&s).unwrap()[turn1_end].hash.clone())
    );
    assert!(st.inbox(&sid).unwrap().is_empty());
    let g = st.guard_state(&sid).unwrap();
    assert_eq!(g.tuple.model.as_deref(), Some(fake::MODEL));
    // The rows of the fake are from 2026-10: the guard notes the pause.
    let last = g.last.unwrap();
    assert!(
        last.reasons.iter().any(|r| r.contains("min ago")),
        "{last:?}"
    );
    assert!(out_has(&r.out, "prefix guard: an append at the end loses"));
    // The assistant text reached the user, and the run left no record.
    let out = r.out.text();
    assert!(out.contains("dsh: echo: hello"), "{out}");
    assert!(out.contains("delivered append"), "{out}");
    assert!(super::records(&st).unwrap().is_empty());
}

#[test]
fn a_resumed_session_takes_its_old_rows_from_the_file() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let root = t.path().join("sessions");
    let main = "session-aaaaaaaa-2222-4333-8444-555555555555";
    // A session of an earlier run that the store never saw.
    let old = fake::turn_rows(0, 1, "first", "msg-u0");
    fake::write_file(&root, main, &old);
    let h = st.new_default_horse().unwrap();
    let r = start(
        setup(&st, &h.id, main, &root, true),
        vec![(main.to_string(), old.clone())],
    );
    r.tx.send(Control::User("second".into())).unwrap();
    let sid = {
        let mut sid = None;
        wait_for("the main session", || {
            sid = st.session_by_native("dsh", main).unwrap().map(|s| s.id);
            sid.is_some()
        });
        sid.unwrap()
    };
    wait_for("turn 2", || ends_turn(&st, &sid, 2));
    r.tx.send(Control::Close).unwrap();
    let rep = r.join.join().unwrap().unwrap();
    let mut want = old.clone();
    want.extend(r.seen.lock().unwrap().rows.iter().map(|(_, r)| r.clone()));
    assert_eq!(rows_of(&st, &sid), want);
    assert!(rep.reconciled >= 1);
    // The rows of the file and of the stream have the same hashes.
    let log = dsh::read_file(
        &dsh::current_file(&super::custody::find_session_dir(&root, main).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(log.rows, want);
}

#[test]
fn the_sdk_command_replaces_the_web_launch() {
    let mut c: Vec<String> = ["/usr/bin/dsh", "--x", "web", "--y"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    super::sdk_command(&mut c, &["web".to_string()]).unwrap();
    assert_eq!(c, ["/usr/bin/dsh", "--x", "--profile", "sdk", "--y"]);
    assert!(super::sdk_command(&mut c, &["web".to_string()]).is_err());
}

#[test]
fn horse_settings_are_off_by_default() {
    let s = super::settings_in("").unwrap();
    assert!(!s.enabled);
    assert_eq!(s.dsh_model, super::MODEL);
    let s =
        super::settings_in("[horse]\nenabled = true\ndsh_model = \"deepseek-v4-pro\"\n").unwrap();
    assert!(s.enabled);
    assert_eq!(s.dsh_model, "deepseek-v4-pro");
    assert!(super::settings_in("[horse]\nenabled = 1\n").is_err());
    assert!(super::settings_in("[horse]\nother = true\n").is_err());
}

/// A session of two turns in the store, with its header.
fn two_turns(st: &Store) -> (String, Vec<crate::horse::Hash>) {
    const O: Origin = Origin {
        harness: dsh::HARNESS,
        container: dsh::CONTAINER,
        version: dsh::VERSION,
    };
    let mut rows = fake::turn_rows(0, 1, "read notes.txt", "msg-u1");
    rows.extend(fake::turn_rows(rows.len(), 2, "count the words", "msg-u2"));
    let h = st.new_default_horse().unwrap();
    let s = st
        .new_session(
            &h.id,
            Binding {
                harness: dsh::HARNESS.into(),
                native_id: Some("session-g".into()),
                version: dsh::VERSION.into(),
                container: dsh::CONTAINER.into(),
            },
        )
        .unwrap();
    let mut parent = None;
    let mut hashes = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let x = st
            .put_native(&O, parent.as_ref(), &format!("seq:{i}"), r)
            .unwrap();
        hashes.push(x.clone());
        parent = Some(x);
    }
    let hdr = st
        .put_blob("dsh-header", &fake::header("session-g").encode())
        .unwrap();
    st.update_session(&s.id, |s| {
        s.head = parent;
        s.native_header = Some(hdr);
    })
    .unwrap();
    (s.id, hashes)
}

#[test]
fn the_guard_judges_operations_by_the_dsh_rules() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(t.path()).unwrap();
    let (sid, h) = two_turns(&st);
    let rows = rows_of(&st, &sid);
    let ty = |i: usize| dsh::row_info(&rows[i]).unwrap().0;
    let turn1_end = (0..rows.len()).find(|&i| ty(i) == "turn/end").unwrap();
    let user2 = turn1_end + 3;
    assert_eq!(ty(user2), "user/message");
    let last = fake::T0 + (rows.len() as u64 - 3) * 100;
    let now = last + 60_000;
    let v = |op: Op| check(&st, &sid, &op, now).unwrap();

    // Appends at the end and forks at the head or at a reply end keep it.
    assert!(v(Op::Append { after: None }).keeps);
    assert!(
        v(Op::Fork {
            at: h[h.len() - 1].clone()
        })
        .keeps
    );
    let f = v(Op::Fork {
        at: h[turn1_end].clone(),
    });
    assert!(f.keeps, "{}", f.message());

    // A fork inside a turn: the cache ends at the reply before the cut.
    let f = v(Op::Fork {
        at: h[user2].clone(),
    });
    assert!(!f.keeps);
    assert!(f.lost_bytes > 0 && f.lost_bytes < f.prefix_bytes);
    assert!(f
        .suggestion
        .as_deref()
        .unwrap()
        .contains(&format!("seq:{}", rows.len() - 1)));

    // An entry before the tail: the rest of the prompt is lost.
    let a = v(Op::Append {
        after: Some(h[user2].clone()),
    });
    assert!(!a.keeps);
    assert!(a
        .suggestion
        .as_deref()
        .unwrap()
        .starts_with(&format!("paddock horse append {sid}")));
    assert!(a.message().contains("loses about"));

    // A model switch loses all; the same model loses nothing.
    let m = v(Op::Model("deepseek-v4-pro".into()));
    assert_eq!(m.lost_bytes, m.prefix_bytes);
    assert!(v(Op::Model(fake::MODEL.into())).keeps);
    // The usage of the last reply scales bytes to tokens.
    assert_eq!(m.lost_tokens, 420);

    // flash: added tools and a new system prompt are appended.
    assert!(v(Op::Tools { added_only: true }).keeps);
    let tc = v(Op::Tools { added_only: false });
    assert!(!tc.keeps && tc.lost_bytes < tc.prefix_bytes);
    assert!(v(Op::System).keeps);
    // A cwd change rewrites the text that names the old cwd.
    let c = v(Op::Cwd("/home/demo/other".into()));
    assert!(!c.keeps);
    assert!(v(Op::Cwd(fake::CWD.into())).keeps);

    // A long pause: the cache may be gone. Too soon: a note only.
    let p = check(&st, &sid, &Op::Append { after: None }, last + TTL_MS + 1).unwrap();
    assert!(!p.keeps && p.lost_bytes == p.prefix_bytes);
    let s = check(&st, &sid, &Op::Append { after: None }, last + 1_000).unwrap();
    assert!(s.keeps && !s.reasons.is_empty());

    // The verdict and the tuple are kept per session.
    st.save_verdict(&m).unwrap();
    let g = st.guard_state(&sid).unwrap();
    assert_eq!(g.last.unwrap().op, m.op);
    assert_eq!(g.tuple.cwd.as_deref(), Some(fake::CWD));
    assert_eq!(g.tuple.effort.as_deref(), Some("high"));
    assert_eq!(g.tuple.last_request_ms, Some(last));
    assert!(g.tuple.tools_hash.is_some() && g.tuple.system_hash.is_some());
}

/// A pipe to dsh whose writes fail once `broken` is set.
#[derive(Clone, Default)]
struct Pipe {
    broken: Arc<Mutex<bool>>,
}

impl Write for Pipe {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if *self.broken.lock().unwrap() {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn dsh_takes_prompts_after_shutdown_until_it_answers_or_its_pipe_breaks() {
    use super::custody::Custodian;
    use super::harness::{Input, Live};
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let horse = st.new_default_horse().unwrap().id;
    let main = "session-5eab0000-3333-4333-8444-555555555555";
    let set = setup(&st, &horse, main, &t.path().join("sessions"), false);
    let pipe = Pipe::default();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut c = Custodian::start(set, pipe.clone(), rx, std::io::sink()).unwrap();
    let ok = |id| rpc::Msg::Response {
        id,
        result: Ok(serde_json::json!({})),
    };
    tx.send(ok(1)).unwrap();
    c.pump(Duration::ZERO).unwrap();
    let sid = c.main_session().unwrap();
    assert!(c.safe(None));
    assert!(
        !c.takes_input(),
        "takes_input is about the time after the end"
    );

    // Before the end, a broken pipe ends the custody (fail closed).
    *pipe.broken.lock().unwrap() = true;
    assert!(c.deliver(Input::Typed("x".into())).is_err());
    *pipe.broken.lock().unwrap() = false;

    c.close_input().unwrap();
    assert!(c.takes_input(), "dsh was only asked to end");
    // dsh ends during the write: the entry stays in the inbox.
    *pipe.broken.lock().unwrap() = true;
    let e = st.queue_append(&sid, "system", "late").unwrap();
    c.deliver(Input::Entry(e)).unwrap();
    assert!(!c.takes_input());
    assert_eq!(st.inbox(&sid).unwrap().len(), 1);
    assert!(st.deliveries(&sid).unwrap().is_empty());

    // A dsh that answers shutdown takes no more prompts.
    let (tx, rx) = std::sync::mpsc::channel();
    let set = setup(&st, &horse, main, &t.path().join("sessions"), false);
    let mut c = Custodian::start(set, Pipe::default(), rx, std::io::sink()).unwrap();
    tx.send(ok(1)).unwrap();
    c.pump(Duration::ZERO).unwrap();
    c.close_input().unwrap();
    assert!(c.takes_input());
    tx.send(ok(2)).unwrap();
    c.pump(Duration::ZERO).unwrap();
    assert!(!c.takes_input());
}
