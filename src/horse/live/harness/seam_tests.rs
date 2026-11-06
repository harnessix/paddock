//! The three custodians behind the one seam: each one with its fake, as a
//! `Box<dyn Live>`, under the same driver and the same checks. No
//! harness, no vendor.

use super::*;
use crate::horse::live::{claude, codex, custody, fake as dsh_fake, rpc};
use crate::horse::{Binding, Store};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const TICK: Duration = Duration::from_millis(10);
const RUN: &str = "sbx-seam";
const NOTE: &str = "from the seam";

type Make = Box<dyn FnOnce() -> Result<Box<dyn Live>> + Send>;

/// One custodian with its fake.
struct Case {
    name: &'static str,
    _t: tempfile::TempDir,
    st: Store,
    /// The store session of the main session, once it is known.
    sid: Box<dyn Fn() -> Option<String>>,
    /// The inputs that the fake got, and whether one came during a turn.
    inputs: Box<dyn Fn() -> (usize, bool)>,
    /// Let a held turn of the fake end.
    release: Box<dyn Fn()>,
    make: Option<Make>,
}

fn store(t: &tempfile::TempDir) -> (Store, String) {
    let st = Store::open(&t.path().join("store")).unwrap();
    let h = st.new_default_horse().unwrap().id;
    (st, h)
}

fn dsh_case() -> Case {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(&t);
    let root = t.path().join("sessions");
    let main = "session-5eab0000-2222-4333-8444-555555555555";
    let seen = Arc::new(Mutex::new(dsh_fake::Seen::default()));
    let set = custody::Setup {
        store: st.clone(),
        horse,
        main: main.into(),
        resume: false,
        sessions_root: Some(root.clone()),
        cwd: dsh_fake::CWD.into(),
        provider: crate::horse::live::PROVIDER.into(),
        model: crate::horse::live::MODEL.into(),
        effort: None,
        run: RUN.into(),
        tick: TICK,
    };
    let f = dsh_fake::Fake {
        root: Some(root),
        existing: vec![],
        step: Duration::from_millis(5),
        seen: seen.clone(),
    };
    let make: Make = Box::new(move || {
        let (pad, end) = UnixStream::pair()?;
        let input = end.try_clone()?;
        std::thread::spawn(move || f.serve(input, end));
        let (tx, rx) = channel();
        rpc::read_runtime(pad.try_clone()?, tx);
        let c = custody::Custodian::start(set, pad, rx, std::io::sink())?;
        Ok(Box::new(c) as Box<dyn Live>)
    });
    let s2 = st.clone();
    Case {
        name: "dsh",
        _t: t,
        sid: Box::new(move || s2.session_by_native("dsh", main).unwrap().map(|s| s.id)),
        inputs: Box::new(move || {
            let s = seen.lock().unwrap();
            (s.prompts.len(), s.prompts.iter().any(|p| p.2))
        }),
        release: Box::new(|| {}),
        st,
        make: Some(make),
    }
}

fn claude_case(hold: bool, init_id: Option<&str>) -> Case {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(&t);
    let start = claude::Start::fresh().unwrap();
    let native = start.native.clone();
    let config_dir = t.path().join("claude");
    let mut f = claude::fake::Fake::new(config_dir.clone(), &native);
    f.hold = hold;
    f.init_id = init_id.map(String::from);
    let claude::fake::Running {
        stdin,
        stdout,
        release,
        seen,
        ..
    } = f.spawn();
    let ctx = claude::CustodyCtx {
        store: st.clone(),
        horse,
        start,
        config_dir,
        cwd: claude::fake::CWD.into(),
        version: crate::horse::claude::VERSION.into(),
        run: RUN.into(),
        tick: TICK,
        out: Box::new(std::io::sink()),
        wait: None,
    };
    let make: Make = Box::new(move || {
        let c = claude::Custodian::start(stdout, stdin, ctx)?;
        Ok(Box::new(c) as Box<dyn Live>)
    });
    let s2 = st.clone();
    let release = Mutex::new(release);
    Case {
        name: "claude",
        _t: t,
        sid: Box::new(move || {
            s2.session_by_native(crate::horse::claude::HARNESS, &native)
                .unwrap()
                .map(|s| s.id)
        }),
        inputs: Box::new(move || {
            let s = seen.lock().unwrap();
            (s.users.len(), s.users.iter().any(|u| u.1))
        }),
        release: Box::new(move || {
            let _ = release.lock().unwrap().send(());
        }),
        st,
        make: Some(make),
    }
}

fn codex_case(resume_as: Option<&str>) -> Case {
    let t = tempfile::tempdir().unwrap();
    let (st, horse) = store(&t);
    let root = t.path().join("home/.codex/sessions");
    std::fs::create_dir_all(&root).unwrap();
    let sid = if resume_as.is_some() {
        let p = codex::fake::write_rollout(
            &root,
            codex::fake::OLD_THREAD,
            &[codex::fake::meta(codex::fake::OLD_THREAD)],
        );
        crate::horse::codex::ingest(&st, &p, crate::horse::codex::VERSION, Some(&horse))
            .unwrap()
            .session
    } else {
        let b = Binding {
            harness: crate::horse::codex::HARNESS.into(),
            native_id: None,
            version: crate::horse::codex::VERSION.into(),
            container: crate::horse::codex::CONTAINER.into(),
        };
        st.new_session(&horse, b).unwrap().id
    };
    let seen = Arc::new(Mutex::new(codex::fake::Seen::default()));
    let f = codex::fake::Fake {
        root: root.clone(),
        resume_as: resume_as.map(String::from),
        approval: codex::wire::APPROVE_COMMAND,
        step: Duration::from_millis(20),
        unsubscribe: true,
        persist_forks: false,
        raw_items: true,
        seen: seen.clone(),
    };
    let ctx = codex::CustodyCtx {
        store: st.clone(),
        horse,
        session: sid.clone(),
        cwd: codex::fake::CWD.into(),
        sessions_root: root,
        run: RUN.into(),
        model: None,
        effort: None,
        tick: TICK,
        reply_timeout: Duration::from_secs(10),
        close_timeout: Duration::from_secs(5),
        out: Box::new(std::io::sink()),
    };
    let make: Make = Box::new(move || {
        let (pad_w, fake_r) = UnixStream::pair()?;
        let (fake_w, pad_r) = UnixStream::pair()?;
        std::thread::spawn(move || f.serve(fake_r, fake_w));
        let (tx, rx) = channel();
        codex::wire::read_server(pad_r, tx);
        let policy = codex::Policy {
            approval: "never".into(),
            sandbox: "danger-full-access".into(),
        };
        let c = codex::Codex::start(pad_w, rx, ctx, policy)?;
        Ok(Box::new(c) as Box<dyn Live>)
    });
    Case {
        name: "codex",
        _t: t,
        sid: Box::new(move || Some(sid.clone())),
        inputs: Box::new(move || {
            let s = seen.lock().unwrap();
            let sent = |r: &&(String, serde_json::Value, bool)| {
                r.0 == codex::wire::TURN_START || r.0 == codex::wire::INJECT_ITEMS
            };
            let all: Vec<_> = s.requests.iter().filter(sent).collect();
            (all.len(), s.steers > 0 || all.iter().any(|r| r.2))
        }),
        release: Box::new(|| {}),
        st,
        make: Some(make),
    }
}

struct Running {
    ctl: Sender<Control>,
    join: JoinHandle<Result<End>>,
}

impl Case {
    fn drive(&mut self) -> Running {
        let make = self.make.take().unwrap();
        let st = self.st.clone();
        let (ctl, control) = channel();
        let join = std::thread::spawn(move || {
            let mut live = make()?;
            let horse = st.horses()?.remove(0).id;
            Driver::new(st, &horse, RUN, TICK).drive(&mut *live, &control)
        });
        Running { ctl, join }
    }

    fn sid(&self) -> String {
        (self.sid)().unwrap()
    }

    fn wait_for(&self, what: &str, mut f: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !f() {
            assert!(
                Instant::now() < end,
                "{}: timed out waiting for {what}",
                self.name
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn note_landed(&self) -> Option<u64> {
        let d = self.st.deliveries(&self.sid()).unwrap();
        d.iter()
            .find(|d| d.entry.text == NOTE)
            .and_then(|d| d.landed_seq)
    }
}

/// The same run for every custodian: a typed line starts the first turn,
/// an inbox entry goes out at a safe point and lands after an ingest, the
/// live record shows the session, and the end comes at a safe point.
fn the_same_custody(mut c: Case) {
    let r = c.drive();
    r.ctl.send(Control::User("hello".into())).unwrap();
    c.wait_for("the main session", || (c.sid)().is_some());
    let sid = c.sid();
    c.wait_for("the first turn in the store", || {
        c.st.session(&sid).unwrap().head.is_some()
    });
    c.wait_for("the live record", || {
        crate::horse::live::records(&c.st)
            .unwrap()
            .iter()
            .any(|x| x.run == RUN && x.sessions[0].session == sid)
    });
    let before = c.st.session(&sid).unwrap().head;
    c.st.queue_append(&sid, "note", NOTE).unwrap();
    c.wait_for("the landed note", || c.note_landed().is_some());
    r.ctl.send(Control::Close).unwrap();
    let end = r.join.join().unwrap().unwrap();
    assert_eq!(end, End::Closed, "{}", c.name);
    let (n, during_a_turn) = (c.inputs)();
    assert!(n >= 2, "{}: {n} inputs", c.name);
    assert!(!during_a_turn, "{}: an input came during a turn", c.name);
    assert!(c.st.inbox(&sid).unwrap().is_empty(), "{}", c.name);
    let s = c.st.session(&sid).unwrap();
    assert_ne!(s.head, before, "{}", c.name);
    let row = &c.st.session_nodes(&s).unwrap()[c.note_landed().unwrap() as usize];
    assert!(
        String::from_utf8_lossy(&row.raw).contains(NOTE),
        "{}: the landed row is not the note",
        c.name
    );
    assert!(
        crate::horse::live::records(&c.st).unwrap().is_empty(),
        "{}: a record after the end",
        c.name
    );
}

#[test]
fn dsh_custody_through_the_seam() {
    the_same_custody(dsh_case());
}

#[test]
fn claude_custody_through_the_seam() {
    the_same_custody(claude_case(false, None));
}

#[test]
fn codex_custody_through_the_seam() {
    the_same_custody(codex_case(None));
}

/// An interrupt stops a long turn, and the entry that waited goes out
/// after the end of that turn, never during it.
fn an_interrupt_frees_the_safe_point(mut c: Case, long: &str) {
    let r = c.drive();
    r.ctl.send(Control::User(long.into())).unwrap();
    c.wait_for("the long turn", || (c.inputs)().0 == 1);
    c.wait_for("the main session", || (c.sid)().is_some());
    let sid = c.sid();
    c.st.queue_append(&sid, "user", "after the stop").unwrap();
    std::thread::sleep(TICK * 10);
    assert_eq!((c.inputs)().0, 1, "{}: sent during the long turn", c.name);
    r.ctl.send(Control::Interrupt).unwrap();
    c.wait_for("the entry", || (c.inputs)().0 == 2);
    r.ctl.send(Control::Close).unwrap();
    (c.release)();
    assert_eq!(r.join.join().unwrap().unwrap(), End::Closed, "{}", c.name);
    assert!(!(c.inputs)().1, "{}: an input came during a turn", c.name);
    assert!(c.st.inbox(&sid).unwrap().is_empty(), "{}", c.name);
}

#[test]
fn claude_interrupt_through_the_seam() {
    an_interrupt_frees_the_safe_point(claude_case(true, None), "a long task");
}

#[test]
fn codex_interrupt_through_the_seam() {
    an_interrupt_frees_the_safe_point(codex_case(None), "[long] count to a million");
}

#[test]
fn a_wrong_session_id_fails_closed_through_the_seam() {
    let wrong = "11111111-2222-4333-8444-555555555555";
    for mut c in [claude_case(false, Some(wrong)), codex_case(Some(wrong))] {
        let sessions = c.st.sessions().unwrap();
        let r = c.drive();
        r.ctl.send(Control::User("hi".into())).unwrap();
        let e = r.join.join().unwrap().unwrap_err();
        assert!(format!("{e:#}").contains(wrong), "{}: {e:#}", c.name);
        assert_eq!(c.st.sessions().unwrap(), sessions, "{}", c.name);
        assert!(
            c.st.read_ref(&format!("live/{RUN}.json"))
                .unwrap()
                .is_none(),
            "{}",
            c.name
        );
        if let Some(sid) = sessions.first().map(|s| s.id.clone()) {
            assert!(c.st.deliveries(&sid).unwrap().is_empty(), "{}", c.name);
        }
    }
}
