//! The ACP channel of `grok agent stdio`: JSON-RPC 2.0, one message per
//! `\n`-terminated line over stdio (Agent Client Protocol, version 1, as
//! the Grok Build 1.0.70 user guide "Agent mode" describes it). A frame
//! with `id` and `method` is a request of the agent, `id` alone a
//! response, `method` alone a notification.

use anyhow::{Context as _, Result};
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::mpsc::Sender;

/// The ACP protocol version that paddock speaks.
pub const PROTOCOL_VERSION: u64 = 1;

pub const INITIALIZE: &str = "initialize";
pub const SESSION_NEW: &str = "session/new";
pub const SESSION_LOAD: &str = "session/load";
/// One turn. The answer comes when the turn ended: the safe point.
pub const SESSION_PROMPT: &str = "session/prompt";
/// A notification: stop the running turn. Its `session/prompt` then
/// answers with the stop reason `cancelled`.
pub const SESSION_CANCEL: &str = "session/cancel";

pub const SESSION_UPDATE: &str = "session/update";
/// Grok's live copy of every persisted xAI update. paddock reads the
/// session files instead.
pub const XAI_NOTIFICATION: &str = "x.ai/session_notification";

pub const REQUEST_PERMISSION: &str = "session/request_permission";

/// JSON-RPC "method not found".
pub const NO_METHOD: i64 = -32601;

/// A message of the agent.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    Response {
        id: Value,
        result: std::result::Result<Value, String>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Note {
        method: String,
        params: Value,
    },
    /// A line that is not a JSON-RPC message.
    Malformed,
}

/// What the reader thread passes on.
#[derive(Clone, Debug, PartialEq)]
pub enum In {
    Msg(Incoming),
    /// The agent closed its stdout.
    Closed,
}

/// Parse one line of the agent.
pub fn parse(line: &[u8]) -> Incoming {
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return Incoming::Malformed;
    };
    let method = v.get("method").and_then(Value::as_str);
    let id = v
        .get("id")
        .filter(|i| i.is_u64() || i.is_i64() || i.is_string());
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    match (method, id) {
        (Some(m), Some(id)) => Incoming::Request {
            id: id.clone(),
            method: m.into(),
            params,
        },
        (Some(m), None) => Incoming::Note {
            method: m.into(),
            params,
        },
        (None, Some(id)) => {
            let result = match v.get("error") {
                Some(e) => Err(e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("an error without a message")
                    .to_string()),
                None => match v.get("result") {
                    Some(r) => Ok(r.clone()),
                    None => return Incoming::Malformed,
                },
            };
            Incoming::Response {
                id: id.clone(),
                result,
            }
        }
        (None, None) => Incoming::Malformed,
    }
}

/// Read the agent's stdout into the channel until it closes. Empty lines
/// are skipped.
pub fn read_agent(r: impl Read + Send + 'static, tx: Sender<In>) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(r);
        let mut line = Vec::new();
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let l = line.strip_suffix(b"\n").unwrap_or(&line);
                    let l = l.strip_suffix(b"\r").unwrap_or(l);
                    if l.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    if tx.send(In::Msg(parse(l))).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(In::Closed);
    });
}

/// The writing side of the channel.
pub struct Client<W: Write> {
    w: Option<W>,
    next: i64,
}

impl<W: Write> Client<W> {
    pub fn new(w: W) -> Self {
        Client {
            w: Some(w),
            next: 1,
        }
    }

    fn send(&mut self, mut m: Value, what: &str) -> Result<()> {
        m["jsonrpc"] = json!("2.0");
        let w = self
            .w
            .as_mut()
            .with_context(|| format!("cannot send {what}: the input of grok is closed"))?;
        let mut line = serde_json::to_vec(&m)?;
        line.push(b'\n');
        w.write_all(&line)
            .and_then(|_| w.flush())
            .with_context(|| format!("cannot send {what} to grok"))
    }

    /// Send a request and return its id.
    pub fn request(&mut self, method: &str, params: Value) -> Result<i64> {
        let id = self.next;
        self.next += 1;
        self.send(
            json!({ "id": id, "method": method, "params": params }),
            method,
        )?;
        Ok(id)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({ "method": method, "params": params }), method)
    }

    pub fn respond(&mut self, id: &Value, result: Value) -> Result<()> {
        self.send(json!({ "id": id, "result": result }), "an answer")
    }

    pub fn respond_error(&mut self, id: &Value, code: i64, message: &str) -> Result<()> {
        self.send(
            json!({ "id": id, "error": { "code": code, "message": message } }),
            "an answer",
        )
    }

    /// Close the input of the agent: it exits at EOF.
    pub fn close(&mut self) {
        self.w = None;
    }
}

/// `initialize`: who paddock is. paddock offers no file system and no
/// terminal: the agent uses its own tools in the sandbox.
pub fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
        },
        "clientInfo": { "name": "paddock", "title": "paddock", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// `session/new` in `cwd`, with no MCP server of paddock.
pub fn session_new(cwd: &str) -> Value {
    json!({ "cwd": cwd, "mcpServers": [] })
}

/// `session/load` of session `id` in `cwd`.
pub fn session_load(id: &str, cwd: &str) -> Value {
    json!({ "sessionId": id, "cwd": cwd, "mcpServers": [] })
}

/// `session/prompt`: one text block.
pub fn session_prompt(id: &str, text: &str) -> Value {
    json!({ "sessionId": id, "prompt": [{ "type": "text", "text": text }] })
}

pub fn session_cancel(id: &str) -> Value {
    json!({ "sessionId": id })
}

/// The version of the agent in the `initialize` result, when it says one.
pub fn agent_version(result: &Value) -> Option<String> {
    result
        .pointer("/agentInfo/version")
        .and_then(Value::as_str)
        .map(String::from)
}

/// The session id of a result or of the params of a notification.
pub fn session_id(v: &Value) -> Option<String> {
    v.get("sessionId").and_then(Value::as_str).map(String::from)
}

/// The stop reason of a `session/prompt` result.
pub fn stop_reason(result: &Value) -> String {
    result
        .get("stopReason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn")
        .to_string()
}
