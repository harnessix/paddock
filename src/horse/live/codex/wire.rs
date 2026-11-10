//! The `codex app-server` channel: JSON-RPC 2.0 without the `jsonrpc`
//! member, one message per `\n`-terminated line over stdio. The names and
//! shapes are pinned to the schema that `codex app-server
//! generate-json-schema` prints for codex-cli 0.166.0 (`ClientRequest`,
//! `ServerNotification`, `ServerRequest`, `v2/*`). A request id is a
//! number or a text. A frame with `id` and `method` is a request of the
//! server, `id` alone a response, `method` alone a notification.

use anyhow::{Context as _, Result};
use serde_json::{json, Value};
use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::mpsc::Sender;

pub const INITIALIZE: &str = "initialize";
pub const INITIALIZED: &str = "initialized";
pub const THREAD_START: &str = "thread/start";
pub const THREAD_RESUME: &str = "thread/resume";
pub const TURN_START: &str = "turn/start";
pub const TURN_INTERRUPT: &str = "turn/interrupt";
pub const INJECT_ITEMS: &str = "thread/inject_items";
/// Drop the subscription of this connection to a thread. The server
/// unloads a thread that has no subscriber left and says `thread/closed`.
pub const THREAD_UNSUBSCRIBE: &str = "thread/unsubscribe";
/// Fork a thread. With `ephemeral: true` the fork has no rollout file and
/// keeps the cache key of its parent.
pub const THREAD_FORK: &str = "thread/fork";

pub const TURN_STARTED: &str = "turn/started";
pub const TURN_COMPLETED: &str = "turn/completed";
pub const TOKEN_USAGE: &str = "thread/tokenUsage/updated";
pub const THREAD_STARTED: &str = "thread/started";
pub const THREAD_CLOSED: &str = "thread/closed";
pub const ERROR: &str = "error";
/// One Responses API item of a turn, as the model wrote it.
pub const RAW_ITEM: &str = "rawResponseItem/completed";

pub const APPROVE_COMMAND: &str = "item/commandExecution/requestApproval";
pub const APPROVE_FILE_CHANGE: &str = "item/fileChange/requestApproval";
pub const APPROVE_PERMISSIONS: &str = "item/permissions/requestApproval";
pub const LEGACY_EXEC: &str = "execCommandApproval";
pub const LEGACY_PATCH: &str = "applyPatchApproval";
pub const USER_INPUT: &str = "item/tool/requestUserInput";
pub const ELICITATION: &str = "mcpServer/elicitation/request";

/// Streaming notifications that paddock does not read: the server does
/// not send them on this connection (`optOutNotificationMethods`).
pub const OPT_OUT: [&str; 9] = [
    "item/agentMessage/delta",
    "item/plan/delta",
    "item/reasoning/summaryTextDelta",
    "item/reasoning/summaryPartAdded",
    "item/reasoning/textDelta",
    "item/commandExecution/outputDelta",
    "item/fileChange/outputDelta",
    "command/exec/outputDelta",
    "process/outputDelta",
];

/// JSON-RPC "method not found".
pub const NO_METHOD: i64 = -32601;

/// A turn as `turn/started`, `turn/completed` and `turn/start` carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub id: String,
    /// `completed`, `interrupted`, `failed` or `inProgress`.
    pub status: String,
    pub error: Option<String>,
}

impl Turn {
    pub fn from(v: &Value) -> Option<Turn> {
        Some(Turn {
            id: v.get("id")?.as_str()?.to_string(),
            status: v
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("inProgress")
                .to_string(),
            error: v
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(String::from),
        })
    }
}

/// A notification of the server.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    TurnStarted {
        thread: String,
        turn: Turn,
    },
    TurnCompleted {
        thread: String,
        turn: Turn,
    },
    TokenUsage {
        thread: String,
        total: Option<u64>,
    },
    ThreadStarted {
        thread: String,
    },
    ThreadClosed {
        thread: String,
    },
    /// A Responses API item of a turn, raw.
    RawItem {
        thread: String,
        item: Value,
    },
    Error {
        message: String,
    },
    Other(String),
}

/// A message of the server.
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
    Note(Note),
    /// A line that is not a JSON-RPC message.
    Malformed,
}

/// What the reader thread passes on.
#[derive(Clone, Debug, PartialEq)]
pub enum In {
    Msg(Incoming),
    /// The server closed its stdout.
    Closed,
}

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

fn note(method: &str, p: &Value) -> Note {
    let thread = || text(p, "threadId");
    let turn = || p.get("turn").and_then(Turn::from);
    let known = match method {
        TURN_STARTED => thread()
            .zip(turn())
            .map(|(thread, turn)| Note::TurnStarted { thread, turn }),
        TURN_COMPLETED => thread()
            .zip(turn())
            .map(|(thread, turn)| Note::TurnCompleted { thread, turn }),
        TOKEN_USAGE => thread().map(|thread| Note::TokenUsage {
            thread,
            total: p
                .pointer("/tokenUsage/total/totalTokens")
                .and_then(Value::as_u64),
        }),
        THREAD_STARTED => p
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(|t| Note::ThreadStarted { thread: t.into() }),
        THREAD_CLOSED => thread().map(|thread| Note::ThreadClosed { thread }),
        RAW_ITEM => thread()
            .zip(p.get("item").filter(|i| i.is_object()).cloned())
            .map(|(thread, item)| Note::RawItem { thread, item }),
        ERROR => Some(Note::Error {
            message: p
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("an error without a message")
                .to_string(),
        }),
        _ => None,
    };
    known.unwrap_or_else(|| Note::Other(method.to_string()))
}

/// Parse one line of the server.
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
        (Some(m), None) => Incoming::Note(note(m, &params)),
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

/// Read the server's stdout into the channel until it closes. Empty
/// lines are skipped.
pub fn read_server(r: impl Read + Send + 'static, tx: Sender<In>) {
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

    fn send(&mut self, m: &Value, what: &str) -> Result<()> {
        let w = self
            .w
            .as_mut()
            .with_context(|| format!("cannot send {what}: the input of codex is closed"))?;
        let mut line = serde_json::to_vec(m)?;
        line.push(b'\n');
        w.write_all(&line)
            .and_then(|_| w.flush())
            .with_context(|| format!("cannot send {what} to codex"))
    }

    /// Send a request and return its id.
    pub fn request(&mut self, method: &str, params: Value) -> Result<i64> {
        let id = self.next;
        self.next += 1;
        self.send(
            &json!({ "id": id, "method": method, "params": params }),
            method,
        )?;
        Ok(id)
    }

    pub fn notify(&mut self, method: &str) -> Result<()> {
        self.send(&json!({ "method": method }), method)
    }

    pub fn respond(&mut self, id: &Value, result: Value) -> Result<()> {
        self.send(&json!({ "id": id, "result": result }), "an answer")
    }

    pub fn respond_error(&mut self, id: &Value, code: i64, message: &str) -> Result<()> {
        self.send(
            &json!({ "id": id, "error": { "code": code, "message": message } }),
            "an answer",
        )
    }

    /// Close the input of the server: it exits at EOF.
    pub fn close(&mut self) {
        self.w = None;
    }
}

/// `initialize`: who paddock is, and the notifications it does not read.
pub fn initialize() -> Value {
    json!({
        "clientInfo": { "name": "paddock", "title": "paddock", "version": env!("CARGO_PKG_VERSION") },
        "capabilities": { "optOutNotificationMethods": OPT_OUT },
    })
}

/// The settings that `thread/start` and `thread/resume` share.
pub fn thread_params(
    cwd: &str,
    policy: &super::Policy,
    model: Option<&str>,
    resume: Option<&str>,
) -> Value {
    let mut p = json!({
        "cwd": cwd,
        "approvalPolicy": policy.approval,
        "sandbox": policy.sandbox,
    });
    if let Some(m) = model {
        p["model"] = json!(m);
    }
    if let Some(id) = resume {
        p["threadId"] = json!(id);
    }
    p
}

/// `turn/start`: one text input. The id of the inbox entry travels as
/// `clientUserMessageId`.
pub fn turn_start(thread: &str, text: &str, message_id: &str, effort: Option<&str>) -> Value {
    let mut p = json!({
        "threadId": thread,
        "input": [{ "type": "text", "text": text, "text_elements": [] }],
        "clientUserMessageId": message_id,
    });
    if let Some(e) = effort {
        p["effort"] = json!(e);
    }
    p
}

/// `thread/fork` of the whole thread, ephemeral: no rollout file, and the
/// cache key of the parent.
pub fn thread_fork_ephemeral(thread: &str) -> Value {
    json!({ "threadId": thread, "ephemeral": true })
}

pub fn thread_unsubscribe(thread: &str) -> Value {
    json!({ "threadId": thread })
}

pub fn turn_interrupt(thread: &str, turn: &str) -> Value {
    json!({ "threadId": thread, "turnId": turn })
}

/// A Responses API message item with one input text.
pub fn input_message(role: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": "input_text", "text": text }],
    })
}

/// `thread/inject_items`: one Responses API message item with one text.
pub fn inject_items(thread: &str, role: &str, text: &str) -> Value {
    json!({ "threadId": thread, "items": [input_message(role, text)] })
}

/// The id and the CLI version of the thread in a `thread/start` or
/// `thread/resume` result.
pub fn thread_of(result: &Value) -> Option<(String, Option<String>)> {
    let t = result.get("thread")?;
    Some((text(t, "id")?, text(t, "cliVersion")))
}

/// Read the actual app-server version from its initialize response.
pub(super) fn reader_version(result: &Value) -> Result<String> {
    let agent = result
        .get("userAgent")
        .and_then(Value::as_str)
        .context("codex app-server did not identify its version")?;
    let product = agent
        .split_whitespace()
        .next()
        .context("empty codex app-server user agent")?;
    let (name, version) = product
        .split_once('/')
        .context("unrecognized codex app-server user agent")?;
    anyhow::ensure!(!name.is_empty(), "missing codex app-server user agent name");
    crate::horse::codex::check_version(version)?;
    Ok(version.into())
}
