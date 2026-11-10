//! A fork through ctl of a session that the live custody holds, with a
//! peer that does what the fake dsh of the sandbox test does: it answers
//! the handshake, sends one user message with an old time, goes idle and
//! answers nothing more.

use super::tests::{give_to, stub};
use super::*;
use crate::horse::live::custody::{run as custody, Setup};
use crate::horse::live::harness::Control;
use crate::horse::live::{fake, rpc};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

const SBX: &str = "sbx-ctl-live";

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_held_session_with_one_old_message_forks_through_ctl() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    // As a run in horse mode begins: a new horse, its session with the
    // DNA entry queued, bound to the dsh id of the main session.
    let rh = crate::horse::run::begin(&st, "dsh", crate::horse::dsh::VERSION, None, None).unwrap();
    let main = "session-11111111-2222-4333-8444-777777777777";
    st.lock().unwrap().bind_native(&rh.session, main).unwrap();
    let set = Setup {
        store: st.clone(),
        horse: rh.horse.clone(),
        main: main.into(),
        resume: false,
        sessions_root: Some(t.path().join("sessions")),
        cwd: fake::CWD.into(),
        provider: crate::horse::live::PROVIDER.into(),
        model: crate::horse::live::MODEL.into(),
        effort: None,
        run: SBX.into(),
        tick: Duration::from_millis(20),
    };
    let (pad, peer) = UnixStream::pair().unwrap();
    let mut out = peer.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(peer).lines();
        let first = lines.next().unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&first).unwrap();
        let id = v["id"].clone();
        let msg = |s: String| writeln!(&out, "{s}").unwrap();
        msg(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{{"serverInfo":{{"name":"fake","version":"1"}}}}}}"#
        ));
        msg(format!(
            r#"{{"jsonrpc":"2.0","method":"session.event","params":{{"sessionId":"{main}","event":{{"type":"user/message","seq":0,"time":1793000000200,"data":{{"content":[{{"type":"text","text":"horse test"}}],"source":{{"kind":"user"}},"role":"user","id":"m1"}},"surfaceOp":"append"}}}}}}"#
        ));
        msg(format!(
            r#"{{"jsonrpc":"2.0","method":"session.status","params":{{"sessionId":"{main}","status":"idle"}}}}"#
        ));
        out.flush().unwrap();
        // Read and drop the rest, as a peer that answers nothing more.
        for _ in lines {}
    });
    let (rt, rx) = std::sync::mpsc::channel();
    rpc::read_runtime(pad.try_clone().unwrap(), rt);
    let (tx, control) = std::sync::mpsc::channel::<Control>();
    let _join = std::thread::spawn(move || custody(set, pad, rx, &control, std::io::sink()));
    let mut sid = None;
    wait_for("the message in the store", || {
        sid = st.session_by_native("dsh", main).unwrap().map(|s| s.id);
        sid.as_ref()
            .is_some_and(|s| st.session(s).unwrap().head.is_some())
    });
    let sid = sid.unwrap();
    assert_eq!(sid, rh.session);
    std::thread::sleep(Duration::from_millis(300));
    let access = Access {
        store: st.clone(),
        horse: rh.horse.clone(),
        session: rh.session.clone(),
    };
    give_to(&st, SBX, &["fork"], 1, 0);
    let host = stub(Answer::Denied);
    let r = run(&access, SBX, &format!("fork {sid}\n"), &host);
    let fork = r.unwrap_or_else(|f| panic!("{f:?}; cards {:?}", host.cards()));
    let fork = st.session(fork.trim()).unwrap();
    assert_eq!(fork.fork_of.unwrap().session, sid);
    drop(tx);
}
