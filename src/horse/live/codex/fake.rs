//! A fake `codex app-server` for the tests. It speaks the channel as the
//! pinned schema describes it, and contacts nothing.
//!
//! - `initialize` answers the server identity; a request before it is
//!   refused.
//! - `thread/start` creates a thread and its rollout file (`session_meta`
//!   first); `thread/resume` reads the thread id from the rollout file.
//! - `turn/start` answers the turn, sends `turn/started`, writes the user
//!   rows, and ends the turn one step later: an assistant row, a token
//!   count, `thread/tokenUsage/updated`, then `turn/completed`. A text
//!   with `[long]` runs until `turn/interrupt`; a text with `[approve]`
//!   asks a command approval first and waits for the answer.
//! - `thread/inject_items` appends the items to the rollout.
//! - An open thread holds the writer lock of the thread
//!   (`thread-writer-locks/<id>.lock` next to the sessions folder), as
//!   Codex does. `thread/unsubscribe` unloads the thread: the lock goes,
//!   then `thread/closed`. A `thread/resume` of a loaded thread attaches
//!   to it and does not read the file.
//! - A `turn/start` or `thread/inject_items` during a turn counts as a
//!   steer: paddock must never send one then.
//! - The first turn also sends a notification that paddock does not read,
//!   an empty line and a line that is not JSON.
//! - `thread/fork {ephemeral: true}` of the open thread makes a thread
//!   with no rollout file whose cache key is the key of the parent; its
//!   items go out as `rawResponseItem/completed` (with `raw_items`). With
//!   `persist_forks`, every fork is persisted: a new key and a rollout
//!   file. The cache key of each `turn/start` is in [`Seen::keys`].
//! - EOF on stdin ends the server.

use super::find_rollout;
use super::wire;
use crate::horse::codex::VERSION;
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CWD: &str = "/home/demo/project";
pub const NEW_THREAD: &str = "019a5f00-0000-7000-8000-000000000001";
pub const OLD_THREAD: &str = "019a5f00-0000-7000-8000-0000000000aa";
pub const SIDE_THREAD: &str = "019a5f00-0000-7000-8000-0000000000f1";

/// What the fake saw.
#[derive(Default, Debug)]
pub struct Seen {
    /// Each request of paddock: the method, the params, and whether a
    /// turn ran when it came.
    pub requests: Vec<(String, Value, bool)>,
    /// The answers of paddock to the requests of the fake.
    pub answers: Vec<(String, Value)>,
    pub steers: usize,
    /// The rollout bytes that each `thread/resume` read from disk.
    pub resumed: Vec<Vec<u8>>,
    /// `thread/resume` calls that attached to a loaded thread.
    pub attached: usize,
    /// The thread and the `prompt_cache_key` of each `turn/start`.
    pub keys: Vec<(String, String)>,
}

impl Seen {
    pub fn methods(&self) -> Vec<String> {
        self.requests.iter().map(|r| r.0.clone()).collect()
    }
}

pub struct Fake {
    /// The sessions folder.
    pub root: PathBuf,
    /// Answer `thread/resume` with this id instead of the asked one.
    pub resume_as: Option<String>,
    /// The method of the approval that an `[approve]` turn asks.
    pub approval: &'static str,
    pub step: Duration,
    /// The server knows `thread/unsubscribe`.
    pub unsubscribe: bool,
    /// Make every `thread/fork` persisted, also an ephemeral one.
    pub persist_forks: bool,
    /// Send the items of an ephemeral thread as `rawResponseItem/completed`.
    pub raw_items: bool,
    pub server_version: &'static str,
    pub seen: Arc<Mutex<Seen>>,
}

/// A rollout row.
pub fn row(ty: &str, payload: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "timestamp": "2026-11-06T00:00:00.000Z",
        "type": ty,
        "payload": payload,
    }))
    .unwrap()
}

pub fn meta(id: &str) -> Vec<u8> {
    row(
        "session_meta",
        json!({ "id": id, "timestamp": "2026-11-06T00:00:00.000Z", "cwd": CWD,
                "originator": "codex_cli_rs", "cli_version": VERSION, "source": "vscode" }),
    )
}

pub fn message(role: &str, kind: &str, text: &str) -> Value {
    json!({ "type": "message", "role": role, "content": [{ "type": kind, "text": text }] })
}

/// The path of the rollout of a thread under a sessions folder.
pub fn rollout_path(root: &Path, id: &str) -> PathBuf {
    root.join(format!("2026/11/06/rollout-2026-11-06T00-00-00-{id}.jsonl"))
}

/// Write a rollout file with these rows.
pub fn write_rollout(root: &Path, id: &str, rows: &[Vec<u8>]) -> PathBuf {
    let p = rollout_path(root, id);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut b = Vec::new();
    for r in rows {
        b.extend_from_slice(r);
        b.push(b'\n');
    }
    std::fs::write(&p, b).unwrap();
    p
}

/// Take the writer lock of a thread, as Codex does while it holds it.
fn writer_lock(root: &Path, thread: &str) -> std::fs::File {
    let dir = root.parent().unwrap().join("thread-writer-locks");
    std::fs::create_dir_all(&dir).unwrap();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(format!("{thread}.lock")))
        .unwrap();
    // Another thread of the test process that starts a process can hold a
    // copy of a lock that its owner closed, for a few milliseconds.
    let wait = crate::horse::codex::writer_lock::WAIT;
    assert!(
        crate::horse::codex::writer_lock::take(&f, wait),
        "the writer lock of thread {thread} is taken"
    );
    f
}

enum Stage {
    Run(Instant),
    Long,
    Approval(String),
}

struct Turn {
    id: String,
    text: String,
    stage: Stage,
}

struct Server<O: Write> {
    f: Fake,
    out: O,
    init: bool,
    thread: Option<String>,
    rollout: Option<PathBuf>,
    /// The writer lock of the open thread.
    lock: Option<std::fs::File>,
    turn: Option<Turn>,
    turns: usize,
    tokens: u64,
    /// The thread of the request or turn at hand.
    cur: String,
    /// The ephemeral threads.
    ephemeral: Vec<String>,
    /// The cache key of each thread that is not keyed by its own id.
    keys: std::collections::HashMap<String, String>,
}

impl Fake {
    /// Serve until stdin closes.
    pub fn serve(self, input: impl Read + Send + 'static, out: impl Write) {
        let (tx, rx) = std::sync::mpsc::channel::<Value>();
        std::thread::spawn(move || {
            for l in BufReader::new(input).lines() {
                let Ok(l) = l else { break };
                if let Ok(v) = serde_json::from_str(&l) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let step = self.step;
        let mut s = Server {
            f: self,
            out,
            init: false,
            thread: None,
            rollout: None,
            lock: None,
            turn: None,
            turns: 0,
            tokens: 0,
            cur: String::new(),
            ephemeral: Vec::new(),
            keys: Default::default(),
        };
        loop {
            match rx.recv_timeout(step / 2) {
                Ok(v) => s.handle(v),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Some(Turn {
                stage: Stage::Run(at),
                ..
            }) = &s.turn
            {
                if Instant::now() >= *at {
                    s.finish("completed");
                }
            }
        }
    }
}

impl<O: Write> Server<O> {
    fn send(&mut self, v: Value) {
        let _ = writeln!(self.out, "{v}");
        let _ = self.out.flush();
    }

    fn raw(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "method": method, "params": params }));
    }

    fn append(&mut self, r: Vec<u8>) {
        if self.ephemeral.contains(&self.cur) {
            // No file: the items go out raw.
            let v: Value = serde_json::from_slice(&r).unwrap();
            if self.f.raw_items && v["type"] == "response_item" {
                let turn = self.turn.as_ref().map(|t| t.id.clone());
                let p = json!({ "threadId": self.cur, "turnId": turn, "item": v["payload"] });
                self.notify(wire::RAW_ITEM, p);
            }
            return;
        }
        let p = self.rollout.clone().expect("a thread is open");
        let mut f = std::fs::OpenOptions::new().append(true).open(p).unwrap();
        let mut b = r;
        b.push(b'\n');
        f.write_all(&b).unwrap();
    }

    fn handle(&mut self, v: Value) {
        let Some(method) = v.get("method").and_then(Value::as_str).map(String::from) else {
            // An answer of paddock.
            if let (Some(id), Some(Turn { stage, .. })) = (v.get("id"), &mut self.turn) {
                if matches!(stage, Stage::Approval(a) if Some(a.as_str()) == id.as_str()) {
                    *stage = Stage::Run(Instant::now() + self.f.step);
                    let a = self.f.approval.to_string();
                    let r = v.get("result").or(v.get("error")).cloned();
                    self.f.seen.lock().unwrap().answers.push((a, r.unwrap()));
                }
            }
            return;
        };
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = v.get("id").cloned() else {
            return;
        };
        let busy = self.turn.is_some();
        if let Some(t) = params["threadId"].as_str() {
            if !busy {
                self.cur = t.to_string();
            }
        }
        self.f
            .seen
            .lock()
            .unwrap()
            .requests
            .push((method.clone(), params.clone(), busy));
        if method != wire::INITIALIZE && !self.init {
            return self.error(&id, "Not initialized");
        }
        match method.as_str() {
            wire::INITIALIZE => {
                self.init = true;
                self.ok(
                    &id,
                    json!({ "codexHome": "/home/demo/.codex", "platformFamily": "unix",
                    "platformOs": "linux", "userAgent": format!("paddock/{} (fake)", self.f.server_version) }),
                );
            }
            wire::THREAD_START => {
                let p = write_rollout(&self.f.root, NEW_THREAD, &[meta(NEW_THREAD)]);
                self.open(&id, NEW_THREAD.into(), p);
            }
            wire::THREAD_RESUME => {
                let want = params["threadId"].as_str().unwrap_or_default().to_string();
                if self.thread.as_deref() == Some(want.as_str()) {
                    self.f.seen.lock().unwrap().attached += 1;
                    let p = self.rollout.clone().unwrap();
                    return self.open(&id, want, p);
                }
                let Some(p) = find_rollout(&self.f.root, &want) else {
                    return self.error(&id, &format!("no rollout found for thread id {want}"));
                };
                let first = std::fs::read_to_string(&p).unwrap();
                self.f
                    .seen
                    .lock()
                    .unwrap()
                    .resumed
                    .push(first.clone().into_bytes());
                let first: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
                let tid = first["payload"]["id"].as_str().unwrap().to_string();
                let tid = self.f.resume_as.clone().unwrap_or(tid);
                self.open(&id, tid, p);
            }
            wire::TURN_START => {
                if busy {
                    self.f.seen.lock().unwrap().steers += 1;
                    return self.error(&id, "a turn runs");
                }
                let text = params["input"][0]["text"].as_str().unwrap_or_default();
                let th = self.cur.clone();
                let key = self.keys.get(&th).cloned().unwrap_or_else(|| th.clone());
                self.f.seen.lock().unwrap().keys.push((th, key));
                self.start_turn(&id, text.to_string());
            }
            wire::TURN_INTERRUPT => {
                self.ok(&id, json!({}));
                if self.turn.as_ref().map(|t| t.id.as_str()) == params["turnId"].as_str() {
                    self.append(row(
                        "event_msg",
                        json!({ "type": "turn_aborted", "reason": "interrupted" }),
                    ));
                    self.finish("interrupted");
                }
            }
            wire::INJECT_ITEMS => {
                if busy {
                    self.f.seen.lock().unwrap().steers += 1;
                }
                for it in params["items"].as_array().cloned().unwrap_or_default() {
                    self.append(row("response_item", it));
                }
                self.ok(&id, json!({}));
            }
            wire::THREAD_FORK => self.fork(&id, &params),
            wire::THREAD_UNSUBSCRIBE if self.f.unsubscribe => {
                let th = params["threadId"].as_str().unwrap_or_default().to_string();
                if let Some(i) = self.ephemeral.iter().position(|x| *x == th) {
                    self.ephemeral.remove(i);
                    self.ok(&id, json!({ "status": "unsubscribed" }));
                    return self.notify(wire::THREAD_CLOSED, json!({ "threadId": th }));
                }
                if self.thread.as_deref() != Some(th.as_str()) {
                    return self.ok(&id, json!({ "status": "notLoaded" }));
                }
                self.ok(&id, json!({ "status": "unsubscribed" }));
                self.thread = None;
                self.rollout = None;
                self.lock = None;
                self.notify(wire::THREAD_CLOSED, json!({ "threadId": th }));
            }
            m => self.error_code(&id, wire::NO_METHOD, &format!("unknown method {m}")),
        }
    }

    fn ok(&mut self, id: &Value, result: Value) {
        self.send(json!({ "id": id, "result": result }));
    }

    fn error(&mut self, id: &Value, m: &str) {
        self.error_code(id, -32600, m);
    }

    fn error_code(&mut self, id: &Value, code: i64, m: &str) {
        self.send(json!({ "id": id, "error": { "code": code, "message": m } }));
    }

    fn open(&mut self, id: &Value, thread: String, path: PathBuf) {
        if self.thread.as_deref() != Some(thread.as_str()) {
            self.lock = Some(writer_lock(&self.f.root, &thread));
        }
        self.rollout = Some(path);
        self.thread = Some(thread.clone());
        let t = json!({ "id": thread, "cliVersion": VERSION, "cwd": CWD, "sessionId": thread,
            "ephemeral": false, "modelProvider": "openai", "preview": "", "turns": [] });
        self.ok(
            id,
            json!({ "thread": t, "approvalPolicy": "never", "approvalsReviewer": "user",
                "cwd": CWD, "model": "gpt-5.5", "modelProvider": "openai",
                "sandbox": { "type": "dangerFullAccess" } }),
        );
        self.notify(wire::THREAD_STARTED, json!({ "thread": t }));
    }

    fn thread(&self) -> String {
        self.cur.clone()
    }

    /// `thread/fork` of the open thread.
    fn fork(&mut self, id: &Value, params: &Value) {
        let parent = self.thread.clone().unwrap_or_default();
        if params["threadId"].as_str() != Some(parent.as_str()) || self.turn.is_some() {
            return self.error(id, "the thread is not open or a turn runs");
        }
        let ephemeral = params["ephemeral"] == true && !self.f.persist_forks;
        let key = self.keys.get(&parent).cloned().unwrap_or(parent.clone());
        if ephemeral {
            self.ephemeral.push(SIDE_THREAD.into());
            self.keys.insert(SIDE_THREAD.into(), key);
        } else {
            let p = self.rollout.clone().unwrap();
            let mut rows = vec![meta(SIDE_THREAD)];
            rows.extend(
                std::fs::read(p)
                    .unwrap()
                    .split(|b| *b == b'\n')
                    .filter(|l| !l.is_empty())
                    .map(<[u8]>::to_vec),
            );
            write_rollout(&self.f.root, SIDE_THREAD, &rows);
        }
        let t = json!({ "id": SIDE_THREAD, "cliVersion": VERSION, "cwd": CWD, "sessionId": SIDE_THREAD,
            "forkedFromId": parent, "ephemeral": ephemeral, "modelProvider": "openai", "preview": "", "turns": [] });
        self.ok(
            id,
            json!({ "thread": t, "approvalPolicy": "never", "cwd": CWD, "model": "gpt-5.5" }),
        );
        self.notify(wire::THREAD_STARTED, json!({ "thread": t }));
    }

    fn start_turn(&mut self, id: &Value, text: String) {
        self.turns += 1;
        let tid = format!("019a5f00-0000-7000-9000-{:012}", self.turns);
        let turn = json!({ "id": tid, "status": "inProgress", "items": [] });
        self.ok(id, json!({ "turn": turn }));
        let th = self.thread();
        self.notify(wire::TURN_STARTED, json!({ "threadId": th, "turn": turn }));
        if self.turns == 1 {
            self.notify("item/started", json!({ "threadId": th }));
            self.raw("");
            self.raw("this is not json");
        }
        self.append(row(
            "event_msg",
            json!({ "type": "task_started", "turn_id": tid }),
        ));
        self.append(row("response_item", message("user", "input_text", &text)));
        self.append(row(
            "event_msg",
            json!({ "type": "user_message", "message": text }),
        ));
        let stage = if text.contains("[approve]") {
            let a = "ap-1".to_string();
            let p = json!({ "threadId": th, "turnId": tid, "itemId": "call_1",
                "startedAtMs": 1, "command": "rm -rf build", "reason": "clean" });
            self.send(json!({ "id": a, "method": self.f.approval, "params": p }));
            Stage::Approval(a)
        } else if text.contains("[long]") {
            Stage::Long
        } else {
            Stage::Run(Instant::now() + self.f.step)
        };
        self.turn = Some(Turn {
            id: tid,
            text,
            stage,
        });
    }

    fn finish(&mut self, status: &str) {
        let Some(t) = self.turn.take() else { return };
        let th = self.thread();
        if status == "completed" {
            let mut a = message("assistant", "output_text", &format!("echo: {}", t.text));
            a["id"] = json!(format!("msg_{}", t.id));
            self.append(row("response_item", a));
        }
        self.tokens += 120;
        let total = json!({ "totalTokens": self.tokens, "inputTokens": 100, "cachedInputTokens": 0,
            "outputTokens": 20, "reasoningOutputTokens": 0 });
        self.append(row(
            "event_msg",
            json!({ "type": "token_count", "info": { "total_token_usage": { "total_tokens": self.tokens } } }),
        ));
        self.append(row(
            "event_msg",
            json!({ "type": "task_complete", "turn_id": t.id }),
        ));
        self.notify(
            wire::TOKEN_USAGE,
            json!({ "threadId": th, "turnId": t.id, "tokenUsage": { "total": total, "last": total } }),
        );
        let turn = json!({ "id": t.id, "status": status, "items": [] });
        self.notify(
            wire::TURN_COMPLETED,
            json!({ "threadId": th, "turn": turn }),
        );
    }
}
