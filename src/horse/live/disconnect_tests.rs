//! A disconnect request ends a custody of the fake dsh at its next safe
//! point, without a Close of the run.

use super::super::custody::{run, Setup};
use super::super::fake::{self, Fake, Seen};
use super::super::harness::Control;
use super::super::rpc;
use crate::horse::Store;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_disconnect_request_ends_the_custody_at_the_next_safe_point() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let root = t.path().join("sessions");
    let h = st.new_default_horse().unwrap();
    let main = "session-11111111-2222-4333-8444-666666666666";
    let set = Setup {
        store: st.clone(),
        horse: h.id.clone(),
        main: main.into(),
        resume: false,
        sessions_root: Some(root.clone()),
        cwd: fake::CWD.into(),
        provider: super::super::PROVIDER.into(),
        model: super::super::MODEL.into(),
        effort: None,
        run: "sbx-disconnect".into(),
        tick: Duration::from_millis(20),
    };
    let (pad, dsh_end) = UnixStream::pair().unwrap();
    let f = Fake {
        root: Some(root),
        existing: vec![],
        step: Duration::from_millis(5),
        seen: Arc::new(Mutex::new(Seen::default())),
    };
    let input = dsh_end.try_clone().unwrap();
    std::thread::spawn(move || f.serve(input, dsh_end));
    let (rt, rx) = std::sync::mpsc::channel();
    rpc::read_runtime(pad.try_clone().unwrap(), rt);
    let (tx, control) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || run(set, pad, rx, &control, std::io::sink()));
    tx.send(Control::User("hello".into())).unwrap();
    let mut sid = None;
    wait_for("the main session in the live record", || {
        sid = st.session_by_native("dsh", main).unwrap().map(|s| s.id);
        sid.as_ref().is_some_and(|sid| {
            super::super::records(&st)
                .unwrap()
                .iter()
                .any(|r| r.sessions.iter().any(|s| &s.session == sid))
        })
    });
    let sid = sid.unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!join.is_finished(), "the custody ended without a request");
    super::ask(&st, &sid).unwrap();
    // The run does not close; the request ends the custody.
    let end = Instant::now() + Duration::from_secs(20);
    while !join.is_finished() {
        assert!(Instant::now() < end, "the custody did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
    join.join().unwrap().unwrap();
    assert!(st.read_ref(&format!("disconnect/{sid}")).unwrap().is_none());
    assert!(super::super::records(&st).unwrap().is_empty());
    drop(tx);
}
