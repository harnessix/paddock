//! A fake `grok agent stdio` for the tests. It speaks ACP as the Grok
//! Build 1.0.70 user guide describes it, writes the session folder in the
//! shape of the codec fixture, and contacts nothing.
//!
//! - `initialize` answers the agent identity; a request before it is
//!   refused.
//! - `session/new` makes session [`NEW_ID`] in the asked folder;
//!   `session/load` reads the folder of the asked id and sends its
//!   history as `session/update` notifications first. With `load_as`, the
//!   history comes for another session id.
//! - `session/prompt` writes the user rows, streams a chunk and ends the
//!   turn one step later: the reply rows, then the answer with
//!   `stopReason`. A text with `[long]` runs until `session/cancel`; a
//!   text with `[approve]` asks `session/request_permission` first and
//!   waits for the answer. A text with `[refuse]` gets an error.
//! - A `session/prompt` while a turn runs counts as a steer: paddock must
//!   never send one then.
//! - The first turn also sends a notification that paddock does not read,
//!   an empty line and a line that is not JSON.
//! - EOF on stdin ends the agent.

use super::wire;
use crate::horse::grok::fixture::Fixture;
use crate::horse::grok::VERSION;
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CWD: &str = "/home/demo/project";
pub const NEW_ID: &str = "019a5f00-1111-7000-8000-000000000001";
pub const OLD_ID: &str = "019a5f00-1111-7000-8000-0000000000aa";

/// What the fake saw.
#[derive(Default, Debug)]
pub struct Seen {
    /// Each request of paddock: the method, the params, and whether a
    /// turn ran when it came.
    pub requests: Vec<(String, Value, bool)>,
    /// The notifications of paddock.
    pub notes: Vec<(String, Value)>,
    /// The answers of paddock to the requests of the fake.
    pub answers: Vec<Value>,
    pub steers: usize,
    /// The prompt texts, in order.
    pub prompts: Vec<String>,
}

impl Seen {
    pub fn methods(&self) -> Vec<String> {
        self.requests.iter().map(|r| r.0.clone()).collect()
    }
}

pub struct Fake {
    /// The Grok home.
    pub home: PathBuf,
    /// Send the history of a load for this id instead of the asked one.
    pub load_as: Option<String>,
    /// Say this version in `initialize`.
    pub version: String,
    pub step: Duration,
    pub seen: Arc<Mutex<Seen>>,
}

impl Fake {
    pub fn new(home: PathBuf, seen: Arc<Mutex<Seen>>) -> Fake {
        Fake {
            home,
            load_as: None,
            version: VERSION.into(),
            step: Duration::from_millis(60),
            seen,
        }
    }
}

struct Turn {
    id: Value,
    prompt: u64,
    long: bool,
    /// The id of the permission request that waits for its answer.
    asking: Option<i64>,
    until: Instant,
}

struct Agent<W: Write> {
    fake: Fake,
    out: W,
    init: bool,
    session: Option<Fixture>,
    turn: Option<Turn>,
    prompts: u64,
    next: i64,
}

impl Fake {
    /// Serve the channel until EOF on `input`.
    pub fn serve(self, input: impl Read + Send + 'static, out: impl Write) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<Value>>();
        std::thread::spawn(move || {
            for l in BufReader::new(input).lines() {
                let Ok(l) = l else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    if tx.send(Some(v)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(None);
        });
        let mut a = Agent {
            fake: self,
            out,
            init: false,
            session: None,
            turn: None,
            prompts: 0,
            next: 1000,
        };
        loop {
            let wait = a
                .turn
                .as_ref()
                .filter(|t| !t.long && t.asking.is_none())
                .map(|t| t.until.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(50));
            match rx.recv_timeout(wait) {
                Ok(Some(v)) => a.handle(v),
                Ok(None) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {}
            }
            let due = a
                .turn
                .as_ref()
                .is_some_and(|t| !t.long && t.asking.is_none() && Instant::now() >= t.until);
            if due {
                a.finish("end_turn");
            }
        }
    }
}

impl<W: Write> Agent<W> {
    fn send(&mut self, v: Value) {
        let mut v = v;
        v["jsonrpc"] = json!("2.0");
        let _ = writeln!(self.out, "{v}");
        let _ = self.out.flush();
    }

    fn raw(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    fn ok(&mut self, id: &Value, result: Value) {
        self.send(json!({"id": id, "result": result}));
    }

    fn error(&mut self, id: &Value, m: &str) {
        self.send(json!({"id": id, "error": {"code": -32000, "message": m}}));
    }

    fn update(&mut self, sid: &str, update: Value) {
        self.send(json!({"method": wire::SESSION_UPDATE,
                         "params": {"sessionId": sid, "update": update}}));
    }

    fn write(&self) {
        if let Some(f) = &self.session {
            f.write(&self.fake.home);
        }
    }

    fn handle(&mut self, v: Value) {
        let method = v["method"].as_str().unwrap_or_default().to_string();
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let busy = self.turn.is_some();
        match v.get("id").cloned() {
            // An answer of paddock to a request of the fake.
            Some(id) if method.is_empty() => {
                self.fake.seen.lock().unwrap().answers.push(v.clone());
                let asked = self.turn.as_ref().and_then(|t| t.asking);
                if asked.is_some() && id.as_i64() == asked {
                    let step = self.fake.step;
                    if let Some(t) = self.turn.as_mut() {
                        t.asking = None;
                        t.until = Instant::now() + step;
                    }
                }
            }
            Some(id) => {
                let mut seen = self.fake.seen.lock().unwrap();
                seen.requests.push((method.clone(), params.clone(), busy));
                drop(seen);
                self.request(&id, &method, &params, busy);
            }
            None => {
                self.fake
                    .seen
                    .lock()
                    .unwrap()
                    .notes
                    .push((method.clone(), params.clone()));
                if method == wire::SESSION_CANCEL && self.turn.is_some() {
                    self.finish("cancelled");
                }
            }
        }
    }

    fn request(&mut self, id: &Value, method: &str, params: &Value, busy: bool) {
        if method != wire::INITIALIZE && !self.init {
            return self.error(id, "not initialized");
        }
        match method {
            wire::INITIALIZE => {
                self.init = true;
                let v = self.fake.version.clone();
                self.ok(
                    id,
                    json!({"protocolVersion": 1,
                           "agentCapabilities": {"loadSession": true},
                           "agentInfo": {"name": "grok", "version": v}}),
                );
            }
            wire::SESSION_NEW => {
                let cwd = params["cwd"].as_str().unwrap_or(CWD).to_string();
                self.session = Some(Fixture::new(NEW_ID, &cwd));
                self.write();
                self.ok(id, json!({"sessionId": NEW_ID}));
            }
            wire::SESSION_LOAD => {
                let want = params["sessionId"].as_str().unwrap_or_default().to_string();
                let cwd = params["cwd"].as_str().unwrap_or(CWD).to_string();
                let f = Fixture::read(&self.fake.home, &cwd, &want);
                self.prompts = f
                    .chat
                    .iter()
                    .filter(|c| c.contains("\"prompt_index\""))
                    .count() as u64;
                let sid = self.fake.load_as.clone().unwrap_or(want);
                for u in f.updates.clone() {
                    let row: Value = serde_json::from_str(&u).unwrap_or_default();
                    self.update(&sid, row["params"]["update"].clone());
                }
                self.session = Some(f);
                self.ok(id, json!({}));
            }
            wire::SESSION_PROMPT => {
                if busy {
                    self.fake.seen.lock().unwrap().steers += 1;
                    return self.error(id, "a turn runs");
                }
                let text = params["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                self.fake.seen.lock().unwrap().prompts.push(text.clone());
                if text.contains("[refuse]") {
                    return self.error(id, "the prompt is refused");
                }
                self.start(id, &text);
            }
            _ => {
                self.send(json!({"id": id, "error": {"code": -32601, "message": "unknown method"}}))
            }
        }
    }

    fn start(&mut self, id: &Value, text: &str) {
        let p = self.prompts;
        self.prompts += 1;
        let Some(f) = self.session.as_mut() else {
            return self.error(id, "no session");
        };
        f.prompt(p, text);
        let sid = f.id.clone();
        self.write();
        self.update(
            &sid,
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "working"}}),
        );
        if p == 0 {
            self.send(json!({"method": "x.ai/unknown_note", "params": {}}));
            self.raw("");
            self.raw("not json");
        }
        let mut turn = Turn {
            id: id.clone(),
            prompt: p,
            long: text.contains("[long]"),
            asking: None,
            until: Instant::now() + self.fake.step,
        };
        if text.contains("[approve]") {
            let rid = self.next;
            self.next += 1;
            turn.asking = Some(rid);
            self.send(
                json!({"id": rid, "method": wire::REQUEST_PERMISSION, "params": {
                    "sessionId": sid,
                    "toolCall": {"toolCallId": "call_1", "title": "rm -rf build"},
                    "options": [
                        {"optionId": "always", "name": "Always", "kind": "allow_always"},
                        {"optionId": "once", "name": "Allow", "kind": "allow_once"},
                        {"optionId": "no", "name": "Reject", "kind": "reject_once"}
                    ]
                }}),
            );
        }
        self.turn = Some(turn);
    }

    fn finish(&mut self, why: &str) {
        let Some(t) = self.turn.take() else { return };
        if let Some(f) = self.session.as_mut() {
            if why == "cancelled" {
                f.cancelled();
            } else {
                f.reply(&format!("done {}", t.prompt));
            }
        }
        self.write();
        self.ok(&t.id, json!({"stopReason": why}));
    }
}
