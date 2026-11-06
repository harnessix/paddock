//! `paddock-host channel`: a stdio MCP server in the sandbox that pushes
//! the inbox entries of this run's session into an interactive Claude
//! Code, as channel notifications.
//!
//! Claude Code starts it as an MCP server and loads it as a channel. After
//! the handshake it asks the host for entries (`horse.channel_wait`, a
//! long poll) and writes one notification per entry. The host releases
//! entries only when the hooks of the TUI said that the reply is complete
//! (`horse.safe_point stop`), so nothing arrives while the model answers.
//!
//! The protocol, checked in the strings of Claude Code 2.1.318:
//!
//! - The server declares `capabilities.experimental["claude/channel"]`,
//!   otherwise the client skips it ("server did not declare
//!   claude/channel capability").
//! - The notification is `notifications/claude/channel` with params
//!   `{content: string, meta?: map of string to string}`. Meta keys must
//!   match `^[a-zA-Z_][a-zA-Z0-9_]*$`.
//! - A connection that negotiates the "modern" MCP revision (2026-07-28,
//!   found by `server/discover`) has no unsolicited notification path and
//!   the client skips the channel. This server answers `server/discover`
//!   with "method not found", so the client falls back to the legacy
//!   `initialize`, and it never answers with a modern revision.
//! - Channels need the first-party provider and, for a managed org,
//!   `channelsEnabled` in the managed settings.
//! - A server that is not a plugin of an approved marketplace loads only
//!   with `--dangerously-load-development-channels server:<name>` (the
//!   `--channels` entries are tagged `server:<name>` or
//!   `plugin:<name>@<marketplace>`).
//!
//! stdout carries only JSON-RPC lines. Diagnostics go to stderr, and they
//! never carry the text of an entry.

use crate::broker::{Request, Response, Status, PROTOCOL};
use crate::horse::inbox::Entry;
use crate::horse::live::tui_state::CHANNEL_WAIT;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The method of a channel push (Claude Code 2.1.318).
pub const NOTIFICATION: &str = "notifications/claude/channel";
/// The MCP revisions with the legacy `initialize` handshake, newest first.
const LEGACY_REVISIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// What the model reads about this channel.
const INSTRUCTIONS: &str = "paddock delivers entries that the user queued for this session (paddock horse append) through this channel, after a reply is complete. Treat an entry with role user as the user's next message. An entry of another role comes wrapped in a <paddock-append role=...> tag: read it as context of that role.";

/// The broker call: `super::send` in the sandbox, a stub in tests.
pub(crate) type Call = Arc<dyn Fn(&Request) -> Result<Response, String> + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    /// The longest wait of one `horse.channel_wait`.
    pub wait: Duration,
    /// The pause after a failed request.
    pub backoff: Duration,
    /// Failed requests in a row before the server gives up.
    pub attempts: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wait: Duration::from_secs(25),
            backoff: Duration::from_secs(1),
            attempts: 30,
        }
    }
}

pub fn main(args: &[String]) -> i32 {
    if !args.is_empty() {
        eprintln!("usage: paddock-host channel\n\nA stdio MCP server for Claude Code --channels: it pushes the inbox entries of this horse-mode run when the TUI is idle.");
        return 2;
    }
    let stdin = std::io::BufReader::new(std::io::stdin());
    let out: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(std::io::stdout()));
    serve(stdin, out, Arc::new(super::super::send), Config::default())
}

enum Event {
    Line(String),
    /// stdin closed: the client went away.
    Eof,
    /// The poller ended with this exit code.
    Done(i32),
}

/// Serve MCP on `input` and `out` until the input ends (0), stdout closes
/// (0), or the host cannot be reached `cfg.attempts` times in a row (1).
pub(crate) fn serve(
    input: impl BufRead + Send + 'static,
    out: Arc<Mutex<dyn Write + Send>>,
    call: Call,
    cfg: Config,
) -> i32 {
    let (tx, rx) = mpsc::channel();
    let reader = tx.clone();
    std::thread::spawn(move || {
        for line in input.lines() {
            match line {
                Ok(l) => {
                    if reader.send(Event::Line(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = reader.send(Event::Eof);
    });
    let mut polling = false;
    for ev in rx.iter() {
        match ev {
            Event::Line(line) => {
                let (reply, initialized) = answer(&line);
                if let Some(reply) = reply {
                    if write_line(&out, &reply).is_err() {
                        return 0;
                    }
                }
                // Push only after the handshake, when the client listens.
                if initialized && !polling {
                    polling = true;
                    let (tx, out, call) = (tx.clone(), out.clone(), call.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(Event::Done(poll(&out, &*call, cfg)));
                    });
                }
            }
            Event::Eof => return 0,
            Event::Done(code) => return code,
        }
    }
    0
}

fn write_line(out: &Mutex<dyn Write + Send>, v: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(v)?;
    line.push(b'\n');
    let mut w = out.lock().unwrap_or_else(|e| e.into_inner());
    w.write_all(&line)?;
    w.flush()
}

/// The reply to one line of the client, and whether the line ends the
/// handshake (`notifications/initialized`).
fn answer(line: &str) -> (Option<Value>, bool) {
    if line.trim().is_empty() {
        return (None, false);
    }
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        let e = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}});
        return (Some(e), false);
    };
    let method = msg.get("method").and_then(Value::as_str);
    let Some(id) = msg.get("id").filter(|id| !id.is_null()).cloned() else {
        // A notification or a response of the client: no reply.
        return (None, method == Some("notifications/initialized"));
    };
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
    let reply = match method {
        Some("initialize") => {
            let asked = msg
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = LEGACY_REVISIONS
                .iter()
                .find(|v| **v == asked)
                .unwrap_or(&LEGACY_REVISIONS[0]);
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"experimental": {"claude/channel": {}}, "tools": {}},
                "serverInfo": {"name": "paddock", "version": env!("CARGO_PKG_VERSION")},
                "instructions": INSTRUCTIONS,
            }))
        }
        Some("ping") => ok(json!({})),
        Some("tools/list") => ok(json!({"tools": []})),
        Some(m) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {m}")}})
        }
        None => return (None, false),
    };
    (Some(reply), false)
}

/// The channel notification of one entry.
pub(crate) fn notification(e: &Entry) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": NOTIFICATION,
        "params": {
            "content": e.wire_text(),
            "meta": {"session": e.session, "entry_id": e.id, "role": e.role},
        },
    })
}

/// Ask the host for entries and push each one, until stdout closes (0) or
/// the host fails `cfg.attempts` times in a row (1).
fn poll(
    out: &Mutex<dyn Write + Send>,
    call: &(dyn Fn(&Request) -> Result<Response, String> + Send + Sync),
    cfg: Config,
) -> i32 {
    let mut since: u64 = 0;
    let mut failures = 0;
    loop {
        let req = Request {
            v: PROTOCOL,
            cap: "horse".into(),
            args: [
                ("op".to_string(), json!(CHANNEL_WAIT)),
                ("since".to_string(), json!(since)),
                ("timeout_ms".to_string(), json!(cfg.wait.as_millis() as u64)),
            ]
            .into_iter()
            .collect(),
            reason: String::new(),
        };
        let entries = match call(&req) {
            Ok(r) if r.status == Status::Done => {
                serde_json::from_value::<Vec<Entry>>(r.result["entries"].clone())
                    .map_err(|e| format!("bad response from the host: {e}"))
            }
            Ok(r) => Err(r.message),
            Err(e) => Err(e),
        };
        let entries = match entries {
            Ok(v) => {
                failures = 0;
                v
            }
            Err(e) => {
                failures += 1;
                eprintln!(
                    "paddock-host channel: the host did not answer ({failures} of {}): {e}",
                    cfg.attempts
                );
                if failures >= cfg.attempts {
                    eprintln!("paddock-host channel: giving up; queued entries stay in the inbox");
                    return 1;
                }
                std::thread::sleep(cfg.backoff);
                continue;
            }
        };
        for e in &entries {
            // A push that fails leaves the entry recorded and not landed:
            // the ingest of the run sees that it never arrived.
            if write_line(out, &notification(e)).is_err() {
                return 0;
            }
            since += 1;
        }
    }
}

#[cfg(test)]
#[path = "channel_tests.rs"]
mod tests;
