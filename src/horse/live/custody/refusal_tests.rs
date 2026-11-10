//! dsh refuses a prompt: the session is idle again, the first close ends
//! the run, and an entry that dsh keeps refusing goes out only after a
//! wait, and leaves the inbox after a few refusals.

use super::refusal::LIMIT;
use super::*;
use crate::horse::live::harness::End;
use crate::horse::live::{fake as dsh_fake, rpc};
use crate::horse::{dsh, run};
use std::io::{BufRead as _, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const TICK: Duration = Duration::from_millis(10);
const RUN: &str = "sbx-refusal";
const MAIN: &str = "session-5eab0000-5555-4444-8444-777777777777";
/// The longest wait for a run that must end.
const LIMIT_WAIT: Duration = Duration::from_secs(15);

/// What the scripted dsh read and sent, with the time of each prompt.
#[derive(Default)]
struct Seen {
    trace: Vec<String>,
    prompts: Vec<Instant>,
}

type Shared = Arc<Mutex<Seen>>;

fn send(out: &Mutex<UnixStream>, v: &Value) {
    let mut o = out.lock().unwrap();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// A dsh that answers the first `refuse` prompts with an error, and runs
/// each later prompt as a short turn with no rows.
fn serve(input: UnixStream, out: UnixStream, refuse: usize, seen: Shared) {
    let out = Arc::new(Mutex::new(out));
    for line in BufReader::new(input).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = v["id"].clone();
        match v["method"].as_str().unwrap_or_default() {
            "initialize" => send(
                &out,
                &json!({"jsonrpc":"2.0","id":id,"result":{"serverInfo":{"name":"fake","version":"0"}}}),
            ),
            "session/prompt" => {
                let n = {
                    let mut s = seen.lock().unwrap();
                    s.prompts.push(Instant::now());
                    s.prompts.len()
                };
                if n <= refuse {
                    seen.lock().unwrap().trace.push("prompt refused".into());
                    send(
                        &out,
                        &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"the session is closed"}}),
                    );
                    continue;
                }
                seen.lock().unwrap().trace.push("prompt".into());
                send(
                    &out,
                    &json!({"jsonrpc":"2.0","id":id,"result":{"messageId": format!("msg-{n}")}}),
                );
                let (out, seen) = (out.clone(), seen.clone());
                std::thread::spawn(move || {
                    for s in ["running", "idle"] {
                        std::thread::sleep(TICK * 3);
                        seen.lock().unwrap().trace.push(s.into());
                        send(
                            &out,
                            &json!({"jsonrpc":"2.0","method":"session.status","params":{"sessionId":MAIN,"status":s}}),
                        );
                    }
                });
            }
            "shutdown" => {
                seen.lock().unwrap().trace.push("shutdown".into());
                send(&out, &json!({"jsonrpc":"2.0","id":id,"result":{}}));
                break;
            }
            _ => {}
        }
    }
}

struct Rig {
    st: Store,
    session: String,
    horse: String,
    seen: Shared,
    ctl: Sender<Control>,
    ended: std::sync::mpsc::Receiver<End>,
}

/// A run over the real custody and driver; dsh refuses `refuse` prompts.
fn rig(refuse: usize, retry: Duration, close_now: bool) -> (Rig, tempfile::TempDir) {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let parent = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    st.lock()
        .unwrap()
        .bind_native(&parent.session, MAIN)
        .unwrap();
    assert_eq!(st.inbox(&parent.session).unwrap().len(), 1);
    let set = Setup {
        store: st.clone(),
        horse: parent.horse.clone(),
        main: MAIN.into(),
        resume: false,
        sessions_root: None,
        cwd: dsh_fake::CWD.into(),
        provider: crate::horse::live::PROVIDER.into(),
        model: crate::horse::live::MODEL.into(),
        effort: None,
        run: RUN.into(),
        tick: TICK,
    };
    let seen: Shared = Arc::default();
    let (ctl, control) = channel();
    if close_now {
        ctl.send(Control::Close).unwrap();
    }
    let (pad, end) = UnixStream::pair().unwrap();
    let fake_in = end.try_clone().unwrap();
    let s = seen.clone();
    std::thread::spawn(move || serve(fake_in, end, refuse, s));
    let (tx, rx) = channel();
    rpc::read_runtime(pad.try_clone().unwrap(), tx);
    let (done, ended) = channel();
    let (st2, horse) = (st.clone(), parent.horse.clone());
    std::thread::spawn(move || {
        let mut c = Custodian::start(set, pad, rx, std::io::sink())
            .unwrap()
            .retry_wait(retry);
        let e = Driver::new(st2, &horse, RUN, TICK)
            .close_wait(Duration::from_secs(10))
            .drive(&mut c, &control)
            .unwrap();
        let _ = done.send(e);
    });
    let r = Rig {
        st,
        session: parent.session,
        horse: parent.horse,
        seen,
        ctl,
        ended,
    };
    (r, t)
}

fn trace(r: &Rig) -> Vec<String> {
    r.seen.lock().unwrap().trace.clone()
}

#[test]
fn a_refused_prompt_leaves_the_session_idle_and_the_first_close_ends_the_run() {
    let (r, _t) = rig(usize::MAX, Duration::from_secs(30), true);
    let ended = r.ended.recv_timeout(LIMIT_WAIT);
    assert_eq!(
        ended,
        Ok(End::Closed),
        "the run did not end after a refused prompt: {:?}",
        trace(&r)
    );
    assert_eq!(trace(&r), ["prompt refused", "shutdown"]);
    // One refusal: the entry waits for the next run.
    assert_eq!(r.st.inbox(&r.session).unwrap().len(), 1);
}

#[test]
fn an_entry_that_dsh_keeps_refusing_waits_between_tries_and_leaves_the_inbox() {
    let retry = Duration::from_millis(150);
    let (r, _t) = rig(LIMIT as usize, retry, false);
    let next =
        r.st.queue_append(&r.session, "user", "the next entry")
            .unwrap();
    let start = Instant::now();
    while !r.st.inbox(&r.session).unwrap().is_empty() {
        assert!(
            start.elapsed() < LIMIT_WAIT,
            "the inbox did not empty: {:?}",
            trace(&r)
        );
        std::thread::sleep(TICK);
    }
    r.ctl.send(Control::Close).unwrap();
    assert_eq!(r.ended.recv_timeout(LIMIT_WAIT), Ok(End::Closed));
    let mut want = vec!["prompt refused"; LIMIT as usize];
    want.extend(["prompt", "running", "idle", "shutdown"]);
    assert_eq!(trace(&r), want);
    let prompts = r.seen.lock().unwrap().prompts.clone();
    for w in prompts[..LIMIT as usize].windows(2) {
        assert!(
            w[1] - w[0] >= retry,
            "a refused entry went out again after {:?}",
            w[1] - w[0]
        );
    }
    // The entry after the dropped one does not wait for the retry.
    assert!(prompts[LIMIT as usize] - prompts[LIMIT as usize - 1] < retry);
    let events = run::events(&r.st, &r.horse).unwrap();
    let dropped: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "append.dropped")
        .collect();
    assert_eq!(dropped.len(), 1, "{events:?}");
    assert_eq!(dropped[0]["refusals"], LIMIT);
    assert_eq!(dropped[0]["why"], "the session is closed");
    assert_eq!(dropped[0]["entry"]["role"], "system");
    let delivered = r.st.deliveries(&r.session).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].entry.id, next.id);
}
