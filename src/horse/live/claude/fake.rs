//! A fake `claude -p --input-format stream-json --output-format
//! stream-json --verbose` for the tests. It runs in a thread on two pipes
//! and contacts nothing.
//!
//! - Each turn starts with a `system/init` line. Then the user row goes
//!   into the transcript, an `assistant` line goes out, the assistant row
//!   goes into the transcript, and a `result` line ends the turn.
//! - A user line that comes during a turn waits for the end of the turn,
//!   as Claude Code queues it.
//! - An interrupt during a turn answers `control_response`, writes the
//!   interrupt marker row, and ends the turn with an error `result`.
//! - EOF on stdin ends the process: it closes stdout and exits with 0.
//!
//! The rows have the shape that Claude Code 2.1.317 writes
//! (`insertMessageChain`): the linkage, the message, then the session
//! fields.

use crate::horse::claude::{transcript_path, VERSION};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

pub const CWD: &str = "/home/demo/project";
pub const MODEL: &str = "claude-opus-5-5";
pub const INTERRUPTED: &str = "[Request interrupted by user]";

/// A main-chain row as `insertMessageChain` builds it.
pub fn chain(ty: &str, uuid: &str, parent: Option<&str>, message: &Value, sid: &str) -> Vec<u8> {
    let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
    format!(
        r#"{{"parentUuid":{parent},"isSidechain":false,"type":"{ty}","message":{message},"uuid":"{uuid}","timestamp":"2026-11-06T10:00:00.000Z","userType":"external","entrypoint":"sdk-cli","cwd":"{CWD}","sessionId":"{sid}","version":"{VERSION}","gitBranch":"main"}}"#
    )
    .into_bytes()
}

pub fn user_message(text: &str) -> Value {
    json!({"role": "user", "content": [{"type": "text", "text": text}]})
}

pub fn assistant_message(n: usize, text: &str) -> Value {
    json!({"id": format!("msg_{n:02}"), "type": "message", "role": "assistant", "model": MODEL,
        "content": [{"type": "text", "text": text}], "stop_reason": "end_turn",
        "usage": {"input_tokens": 4, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 2}})
}

/// What the fake read and did, in order.
#[derive(Default, Debug)]
pub struct Seen {
    /// `user:<text>` (with ` (busy)` when a turn ran), `interrupt`,
    /// `out:<type>` for each line written.
    pub log: Vec<String>,
    /// The user texts, and whether a turn ran when each came.
    pub users: Vec<(String, bool)>,
}

/// The script of a fake.
#[derive(Clone)]
pub struct Fake {
    pub config_dir: PathBuf,
    /// The session id of the process (from `--session-id` or `--resume`).
    pub native: String,
    /// The id that `system/init` reports; `None` is `native`.
    pub init_id: Option<String>,
    /// Each `result` waits for a release.
    pub hold: bool,
    /// Each turn also writes a line of an unknown type and a bad line.
    pub noise: bool,
    /// The assistant row of the first turn stays torn until the next turn.
    pub torn: bool,
    /// The uuid of the last row of a resumed transcript.
    pub last_uuid: Option<String>,
}

/// A running fake.
pub struct Running {
    pub stdin: std::io::PipeWriter,
    pub stdout: std::io::PipeReader,
    pub release: Sender<()>,
    pub seen: Arc<Mutex<Seen>>,
    pub join: std::thread::JoinHandle<i32>,
}

enum In {
    Line(Value),
    Release,
    Eof,
}

struct State {
    f: Fake,
    out: std::io::PipeWriter,
    seen: Arc<Mutex<Seen>>,
    busy: bool,
    queue: VecDeque<String>,
    turns: usize,
    last: Option<String>,
    /// The rest of a torn row.
    tail: Option<Vec<u8>>,
}

impl Fake {
    pub fn new(config_dir: PathBuf, native: &str) -> Fake {
        Fake {
            config_dir,
            native: native.into(),
            init_id: None,
            hold: false,
            noise: false,
            torn: false,
            last_uuid: None,
        }
    }

    pub fn path(&self) -> PathBuf {
        transcript_path(&self.config_dir, CWD, &self.native)
    }

    pub fn spawn(self) -> Running {
        let (in_r, in_w) = std::io::pipe().unwrap();
        let (out_r, out_w) = std::io::pipe().unwrap();
        let (tx, rx) = channel();
        let (release, rel_rx) = channel::<()>();
        let t = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(in_r).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let _ = t.send(In::Line(v));
                }
            }
            let _ = t.send(In::Eof);
        });
        std::thread::spawn(move || {
            while rel_rx.recv().is_ok() {
                if tx.send(In::Release).is_err() {
                    break;
                }
            }
        });
        let seen = Arc::new(Mutex::new(Seen::default()));
        let mut s = State {
            last: self.last_uuid.clone(),
            f: self,
            out: out_w,
            seen: seen.clone(),
            busy: false,
            queue: VecDeque::new(),
            turns: 0,
            tail: None,
        };
        let join = std::thread::spawn(move || s.serve(rx));
        Running {
            stdin: in_w,
            stdout: out_r,
            release,
            seen,
            join,
        }
    }
}

impl State {
    fn send(&mut self, v: &Value) {
        let ty = v["type"].as_str().unwrap_or_default().to_string();
        let _ = writeln!(self.out, "{v}");
        let _ = self.out.flush();
        self.seen.lock().unwrap().log.push(format!("out:{ty}"));
    }

    fn send_raw(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    fn append(&mut self, bytes: &[u8]) {
        let path = self.f.path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
    }

    fn row(&mut self, ty: &str, uuid: &str, message: &Value) -> Vec<u8> {
        let r = chain(ty, uuid, self.last.as_deref(), message, &self.f.native);
        self.last = Some(uuid.into());
        r
    }

    fn serve(&mut self, rx: Receiver<In>) -> i32 {
        while let Ok(m) = rx.recv() {
            match m {
                In::Line(v) => self.input(&v),
                In::Release if self.busy => self.finish(),
                In::Release => {}
                In::Eof => break,
            }
        }
        0
    }

    fn input(&mut self, v: &Value) {
        match v["type"].as_str() {
            Some("user") => {
                let text = v["message"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                {
                    let mut s = self.seen.lock().unwrap();
                    let busy = if self.busy { " (busy)" } else { "" };
                    s.log.push(format!("user:{text}{busy}"));
                    s.users.push((text.clone(), self.busy));
                }
                if self.busy {
                    self.queue.push_back(text);
                } else {
                    self.start(&text);
                }
            }
            Some("control_request") if v["request"]["subtype"] == "interrupt" => {
                self.seen.lock().unwrap().log.push("interrupt".into());
                let id = v["request_id"].clone();
                if !self.busy {
                    self.send(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": id, "error": "no turn runs"}}));
                    return;
                }
                self.send(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {}}}));
                let uuid = format!("i{}", self.turns);
                let r = self.row("user", &uuid, &user_message(INTERRUPTED));
                self.complete_tail();
                self.append(&[r.as_slice(), b"\n"].concat());
                self.result("error_during_execution", true);
            }
            _ => {}
        }
    }

    fn complete_tail(&mut self) {
        if let Some(t) = self.tail.take() {
            self.append(&t);
        }
    }

    fn start(&mut self, text: &str) {
        self.busy = true;
        self.turns += 1;
        let n = self.turns;
        let id = self.f.init_id.clone().unwrap_or(self.f.native.clone());
        self.send(
            &json!({"type": "system", "subtype": "init", "cwd": CWD, "session_id": id,
            "tools": ["Read"], "mcp_servers": [], "model": MODEL, "permissionMode": "default",
            "apiKeySource": "none", "claude_code_version": VERSION, "uuid": format!("init-{n}")}),
        );
        self.complete_tail();
        let u = self.row("user", &format!("u{n}"), &user_message(text));
        self.append(&[u.as_slice(), b"\n"].concat());
        let reply = format!("echo: {text}");
        let m = assistant_message(n, &reply);
        let native = self.f.native.clone();
        self.send(
            &json!({"type": "assistant", "message": m, "parent_tool_use_id": null,
            "session_id": native, "uuid": format!("a{n}")}),
        );
        let a = self.row("assistant", &format!("a{n}"), &m);
        let mut a = [a.as_slice(), b"\n"].concat();
        if self.f.torn && n == 1 {
            let rest = a.split_off(a.len() / 2);
            self.tail = Some(rest);
        }
        self.append(&a);
        if self.f.noise {
            self.send(
                &json!({"type": "stream_event", "event": {"type": "ping"}, "session_id": native}),
            );
            self.send_raw(r#"{"type":"assis"#);
        }
        if !self.f.hold {
            self.finish();
        }
    }

    fn result(&mut self, subtype: &str, is_error: bool) {
        let native = self.f.native.clone();
        self.send(
            &json!({"type": "result", "subtype": subtype, "is_error": is_error,
            "duration_ms": 10, "num_turns": 1, "session_id": native, "total_cost_usd": 0,
            "usage": {"input_tokens": 4, "output_tokens": 2}}),
        );
        self.busy = false;
        if let Some(next) = self.queue.pop_front() {
            self.start(&next);
        }
    }

    fn finish(&mut self) {
        self.result("success", false);
    }
}
