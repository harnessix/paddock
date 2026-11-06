//! A fake `dsh --profile sdk` for the tests: it speaks the SDK channel as
//! the dsh 0.2.1-rc.5 source describes it (`packages/sdk/protocol`,
//! `packages/sdk/server/README.md`), and contacts nothing.
//!
//! - `initialize` answers the wire-stable server identity. `session/prompt`
//!   before it is refused.
//! - `session/prompt` answers `{messageId}` at once (a durable enqueue),
//!   then runs the queued messages as turns in order: `session.status`
//!   `running`, one `session.event` per committed row, then `idle`.
//! - `session/resume` answers `{}` and streams no old rows.
//! - `shutdown` answers `{}` and closes stdout; so does EOF on stdin.
//! - It writes the session file as the backend does (`compression: none`):
//!   the header line, then one line per row, flushed at the end of a turn.

use crate::horse::dsh::Header;
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const CWD: &str = "/home/demo/project";
pub const MODEL: &str = "deepseek-flash";
pub const T0: u64 = 1_793_000_000_000;

/// What the fake saw and wrote.
#[derive(Default, Debug)]
pub struct Seen {
    /// Every row, per session, in order.
    pub rows: Vec<(String, Vec<u8>)>,
    /// For each prompt: the session, the text, and whether the session was
    /// running a turn when it came.
    pub prompts: Vec<(String, String, bool)>,
}

pub struct Fake {
    /// The sessions folder; None writes no file.
    pub root: Option<PathBuf>,
    /// Rows that a resumed session has already: (session, rows).
    pub existing: Vec<(String, Vec<Vec<u8>>)>,
    /// The time between two events.
    pub step: Duration,
    pub seen: Arc<Mutex<Seen>>,
}

fn send(out: &Mutex<Box<dyn Write + Send>>, v: &Value) {
    let mut o = out.lock().unwrap();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// One row, in the shape of `Session.append`.
pub fn row(ty: &str, seq: usize, data: &str, surface: bool) -> Vec<u8> {
    let s = if surface {
        r#","surfaceOp":"append""#
    } else {
        ""
    };
    format!(
        r#"{{"type":"{ty}","seq":{seq},"time":{},"data":{data}{s}}}"#,
        T0 + seq as u64 * 100
    )
    .into_bytes()
}

/// The rows of turn `turn` from `seq`. The first turn has the setup.
pub fn turn_rows(seq: usize, turn: usize, text: &str, mid: &str) -> Vec<Vec<u8>> {
    let q = |s: &str| serde_json::to_string(s).unwrap();
    let mut spec: Vec<(&str, String, bool)> = vec![
        ("turn/start", format!(r#"{{"turn":{turn}}}"#), false),
        (
            "step/start",
            format!(r#"{{"turn":{turn},"step":1}}"#),
            false,
        ),
    ];
    if turn == 1 {
        spec.push(("system/message", r#"{"turn":1,"step":1,"message":{"role":"system","content":[{"type":"text","text":"You are a fake dsh. Answer in one line."}],"source":{"kind":"system-prompt"},"id":"msg-sys"}}"#.into(), true));
    }
    spec.push((
        "user/message",
        format!(
            r#"{{"content":[{{"type":"text","text":{}}}],"source":{{"kind":"user","rpcId":"rpc-{turn}","clientTimeZone":"UTC"}},"role":"user","id":{}}}"#,
            q(text),
            q(mid)
        ),
        true,
    ));
    if turn == 1 {
        let rc = format!("Current working directory: \\\"{CWD}\\\".");
        spec.push(("user/message", format!(r#"{{"content":[{{"type":"text","text":"Current runtime context.\n\n{rc}"}}],"source":{{"kind":"runtime-context","form":"snapshot","sections":[{{"name":"working-directory:current","text":"{rc}"}}]}},"role":"user","id":"msg-rc"}}"#), true));
        spec.push(("request/header", format!(r#"{{"header":{{"config":{{"provider":"deepseek-messages","model":"{MODEL}","maxTokens":256000,"reasoningEffort":"high"}},"adapterDefaults":{{"reasoningEffort":true,"maxTokens":true}},"tools":[{{"name":"read","description":"Read a file.","parameters":{{"type":"object","properties":{{"file_path":{{"type":"string"}}}},"required":["file_path"],"additionalProperties":false}}}}]}},"reason":"initial"}}"#), false));
        spec.push(("request/context", format!(r#"{{"provider":"deepseek-messages","model":"{MODEL}","contextWindow":1000000,"systemPromptUpdate":"in-history"}}"#), false));
    }
    spec.push((
        "assistant/message",
        format!(
            r#"{{"turn":{turn},"step":1,"message":{{"role":"assistant","content":[{{"type":"text","text":{}}}],"source":{{"kind":"model","provider":"deepseek-messages","model":"{MODEL}"}},"id":"msg-a{turn}"}},"usage":{{"inputTokens":400,"outputTokens":20,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":420}}}}"#,
            q(&format!("echo: {text}"))
        ),
        true,
    ));
    spec.push(("step/end", format!(r#"{{"turn":{turn},"step":1}}"#), false));
    spec.push((
        "turn/end",
        format!(r#"{{"turn":{turn},"reason":{{"kind":"completed"}}}}"#),
        false,
    ));
    spec.iter()
        .enumerate()
        .map(|(i, (t, d, s))| row(t, seq + i, d, *s))
        .collect()
}

pub fn header(id: &str) -> Header {
    Header {
        id: id.into(),
        created_at: T0,
        cwd: Some(CWD.into()),
        parent_session: None,
        is_seeded: false,
        origin: None,
        delegation_depth: 0,
        agent_preset: None,
    }
}

/// Write a session file as dsh does with `compression: none`.
pub fn write_file(root: &std::path::Path, id: &str, rows: &[Vec<u8>]) {
    let dir = crate::horse::dsh::session_dir(root, Some(CWD), id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut b = header(id).encode();
    b.push(b'\n');
    for r in rows {
        b.extend_from_slice(r);
        b.push(b'\n');
    }
    std::fs::write(dir.join("session.v4.jsonl"), b).unwrap();
}

struct Agent {
    rows: Vec<Vec<u8>>,
    queue: Vec<(String, String)>,
    running: bool,
    turns: usize,
}

impl Fake {
    /// Serve one runtime on these streams until shutdown or EOF.
    pub fn serve(self, input: impl Read + Send + 'static, output: impl Write + Send + 'static) {
        let out: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(output)));
        let agents: Arc<Mutex<Vec<(String, Agent)>>> = Arc::new(Mutex::new(Vec::new()));
        for (id, rows) in &self.existing {
            let turns = rows
                .iter()
                .filter(|r| r.windows(10).any(|w| w == b"turn/start"))
                .count();
            agents.lock().unwrap().push((
                id.clone(),
                Agent {
                    rows: rows.clone(),
                    queue: Vec::new(),
                    running: false,
                    turns,
                },
            ));
        }
        let me = Arc::new(self);
        let mut init = false;
        let mut msg = 0;
        let mut workers = Vec::new();
        for line in BufReader::new(input).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let (Some(id), Some(method)) = (v.get("id").cloned(), v["method"].as_str()) else {
                continue;
            };
            let p = &v["params"];
            let reply = |r: Value| send(&out, &json!({"jsonrpc":"2.0","id":id,"result":r}));
            let fail = |m: &str| {
                send(
                    &out,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":m}}),
                )
            };
            match method {
                "initialize" => {
                    init = true;
                    reply(
                        json!({"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"0.0.1"}}),
                    );
                }
                "session/resume" => {
                    let sid = p["sessionId"].as_str().unwrap_or_default();
                    if agents.lock().unwrap().iter().any(|(s, _)| s == sid) {
                        reply(json!({}));
                    } else {
                        fail("SESSION_NOT_FOUND");
                    }
                }
                "session/prompt" if !init => fail("initialize first"),
                "session/prompt" => {
                    msg += 1;
                    let mid = format!("msg-u{msg}");
                    let sid = p["sessionId"].as_str().unwrap_or_default().to_string();
                    let text = p["contentBlocks"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    reply(json!({ "messageId": mid }));
                    let start = {
                        let mut a = agents.lock().unwrap();
                        if !a.iter().any(|(s, _)| *s == sid) {
                            a.push((
                                sid.clone(),
                                Agent {
                                    rows: Vec::new(),
                                    queue: Vec::new(),
                                    running: false,
                                    turns: 0,
                                },
                            ));
                        }
                        let ag = &mut a.iter_mut().find(|(s, _)| *s == sid).unwrap().1;
                        me.seen.lock().unwrap().prompts.push((
                            sid.clone(),
                            text.clone(),
                            ag.running,
                        ));
                        ag.queue.push((text, mid));
                        !std::mem::replace(&mut ag.running, true)
                    };
                    if start {
                        let (out, agents, me) = (out.clone(), agents.clone(), me.clone());
                        workers.push(std::thread::spawn(move || me.drive(&sid, &out, &agents)));
                    }
                }
                "shutdown" => {
                    reply(json!({}));
                    break;
                }
                _ => send(
                    &out,
                    &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
                ),
            }
        }
        // Disposal drains the running agents before the process exits.
        for w in workers {
            let _ = w.join();
        }
        // Dropping the last handle closes stdout.
        drop(out);
    }

    /// Run the queued messages of a session as turns.
    fn drive(
        &self,
        sid: &str,
        out: &Mutex<Box<dyn Write + Send>>,
        agents: &Mutex<Vec<(String, Agent)>>,
    ) {
        let status = |s: &str| {
            send(
                out,
                &json!({"jsonrpc":"2.0","method":"session.status","params":{"sessionId":sid,"status":s}}),
            )
        };
        status("running");
        loop {
            let next = {
                let mut a = agents.lock().unwrap();
                let ag = &mut a.iter_mut().find(|(s, _)| s == sid).unwrap().1;
                if ag.queue.is_empty() {
                    ag.running = false;
                    None
                } else {
                    ag.turns += 1;
                    let (text, mid) = ag.queue.remove(0);
                    Some(turn_rows(ag.rows.len(), ag.turns, &text, &mid))
                }
            };
            let Some(rows) = next else { break };
            for r in rows {
                std::thread::sleep(self.step);
                // `JSON.stringify` keeps the key order of the event.
                let line = format!(
                    r#"{{"jsonrpc":"2.0","method":"session.event","params":{{"sessionId":{},"event":{}}}}}"#,
                    serde_json::to_string(sid).unwrap(),
                    String::from_utf8_lossy(&r)
                );
                {
                    let mut o = out.lock().unwrap();
                    let _ = writeln!(o, "{line}");
                    let _ = o.flush();
                }
                self.seen.lock().unwrap().rows.push((sid.into(), r.clone()));
                let mut a = agents.lock().unwrap();
                a.iter_mut().find(|(s, _)| s == sid).unwrap().1.rows.push(r);
            }
            if let Some(root) = &self.root {
                let a = agents.lock().unwrap();
                let rows = &a.iter().find(|(s, _)| s == sid).unwrap().1.rows;
                write_file(root, sid, rows);
            }
        }
        status("idle");
    }
}
