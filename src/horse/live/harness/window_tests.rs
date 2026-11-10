//! dsh answers `session/prompt` at once (a durable enqueue) and reports
//! `session.status` `running` only later. The session is busy from the
//! prompt on: no second entry and no `shutdown` goes out before the
//! `idle` of the turn. The driver runs over the real dsh custody and a
//! scripted dsh that waits between its answer and `running`.

use super::*;
use crate::horse::live::{custody, fake as dsh_fake, rpc};
use crate::horse::{dsh, run};
use serde_json::{json, Value};
use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

const TICK: Duration = Duration::from_millis(10);
const RUN: &str = "sbx-window";
const MAIN: &str = "session-5eab0000-4444-4444-8444-666666666666";
/// The wait between the answer to a prompt and `running`.
const GAP: Duration = Duration::from_millis(300);

type Trace = Arc<Mutex<Vec<String>>>;

fn send(out: &Mutex<UnixStream>, v: &Value) {
    let mut o = out.lock().unwrap();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// A dsh that runs each prompt as a turn with no rows: it answers at once,
/// waits [`GAP`], sends `running`, waits a bit more, then `idle`. A prompt
/// during a turn queues behind it. The trace has what paddock sent and
/// the statuses, in the order that the fake sent or read them.
fn serve(input: UnixStream, out: UnixStream, trace: Trace) {
    let out = Arc::new(Mutex::new(out));
    let queue = Arc::new(Mutex::new((0usize, false)));
    for line in BufReader::new(input).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = v["id"].clone();
        let reply = |r: Value| send(&out, &json!({"jsonrpc":"2.0","id":id,"result":r}));
        match v["method"].as_str().unwrap_or_default() {
            "initialize" => reply(json!({"serverInfo":{"name":"fake","version":"0"}})),
            "session/prompt" => {
                let busy = {
                    let mut q = queue.lock().unwrap();
                    q.0 += 1;
                    let busy = q.1;
                    q.1 = true;
                    busy
                };
                trace.lock().unwrap().push(if busy {
                    "prompt during a turn".into()
                } else {
                    "prompt".into()
                });
                reply(json!({"messageId": format!("msg-{}", trace.lock().unwrap().len())}));
                if !busy {
                    let (out, queue, trace) = (out.clone(), queue.clone(), trace.clone());
                    std::thread::spawn(move || {
                        let status = |s: &str| {
                            send(
                                &out,
                                &json!({"jsonrpc":"2.0","method":"session.status","params":{"sessionId":MAIN,"status":s}}),
                            )
                        };
                        std::thread::sleep(GAP);
                        trace.lock().unwrap().push("running".into());
                        status("running");
                        loop {
                            std::thread::sleep(TICK * 5);
                            let mut q = queue.lock().unwrap();
                            q.0 -= 1;
                            if q.0 == 0 {
                                q.1 = false;
                                // Under the lock: the next prompt starts a
                                // new turn only after this `idle`.
                                trace.lock().unwrap().push("idle".into());
                                status("idle");
                                break;
                            }
                        }
                    });
                }
            }
            "shutdown" => {
                trace.lock().unwrap().push("shutdown".into());
                reply(json!({}));
                break;
            }
            _ => {}
        }
    }
}

#[test]
fn a_session_is_busy_from_the_prompt_until_its_idle() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::open(&t.path().join("store")).unwrap();
    let parent = run::begin(&st, dsh::HARNESS, dsh::VERSION, None, None).unwrap();
    st.lock()
        .unwrap()
        .bind_native(&parent.session, MAIN)
        .unwrap();
    // The DNA entry of the new session waits already; a second entry joins it.
    st.queue_append(&parent.session, "user", "second entry")
        .unwrap();
    assert_eq!(st.inbox(&parent.session).unwrap().len(), 2);
    let set = custody::Setup {
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
    let trace: Trace = Arc::default();
    let (ctl, control) = channel();
    // The input ends at once: the close falls into the window of a turn.
    ctl.send(Control::Close).unwrap();
    let (pad, end) = UnixStream::pair().unwrap();
    let fake_in = end.try_clone().unwrap();
    let tr = trace.clone();
    let fake = std::thread::spawn(move || serve(fake_in, end, tr));
    let (tx, rx) = channel();
    rpc::read_runtime(pad.try_clone().unwrap(), tx);
    let mut c = custody::Custodian::start(set, pad, rx, std::io::sink()).unwrap();
    let ended = Driver::new(st.clone(), &parent.horse, RUN, TICK)
        .close_wait(Duration::from_secs(20))
        .drive(&mut c, &control)
        .unwrap();
    fake.join().unwrap();
    let trace = trace.lock().unwrap().clone();
    assert_eq!(
        trace,
        ["prompt", "running", "idle", "prompt", "running", "idle", "shutdown"],
        "an entry or the end went out inside a turn"
    );
    assert_eq!(ended, End::Closed);
    assert!(st.inbox(&parent.session).unwrap().is_empty());
    drop(ctl);
}
