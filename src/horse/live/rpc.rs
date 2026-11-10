//! The dsh SDK channel: JSON-RPC 2.0, one message per `\n`-terminated line
//! over the stdio of `dsh --profile sdk` (`packages/sdk/protocol`,
//! `src/transport.ts` and `src/types.ts` at the tag `dsh-v0.2.1-rc.5`).
//! A frame with `id` and `method` is a request, `id` alone a response,
//! `method` alone a notification. Malformed lines are ignored, as the dsh
//! transport does. stdout of the runtime carries only these frames.

use crate::horse::dsh::request::J;
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::mpsc::Sender;

/// A message from the runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Response {
        id: u64,
        result: std::result::Result<Value, String>,
    },
    /// `session.event`: the event envelope, written as `JSON.stringify`
    /// writes it (key order kept).
    Event {
        session: String,
        ty: String,
        seq: u64,
        raw: Vec<u8>,
    },
    /// `session.status`.
    Status { session: String, idle: bool },
    /// Another notification, for example `subagent.started`.
    Other(String),
    /// The runtime closed its stdout.
    Closed,
}

/// Parse one line of the runtime. `None` for a line to ignore.
pub fn parse(line: &[u8]) -> Option<Msg> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let method = v.get("method").and_then(Value::as_str);
    let id = v.get("id").and_then(Value::as_u64);
    match (method, id) {
        (None, Some(id)) => {
            let result = match v.get("error") {
                Some(e) => Err(e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("error")
                    .to_string()),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(Msg::Response { id, result })
        }
        (Some("session.event"), None) => {
            let p = v.get("params")?;
            let session = p.get("sessionId")?.as_str()?.to_string();
            let ev = p.get("event")?;
            let ty = ev.get("type")?.as_str()?.to_string();
            let seq = ev.get("seq")?.as_u64()?;
            let j = J::parse(line).ok()?;
            let raw = j.get("params")?.get("event")?.text().into_bytes();
            Some(Msg::Event {
                session,
                ty,
                seq,
                raw,
            })
        }
        (Some("session.status"), None) => {
            let p = v.get("params")?;
            Some(Msg::Status {
                session: p.get("sessionId")?.as_str()?.to_string(),
                idle: p.get("status")?.as_str()? == "idle",
            })
        }
        (Some(m), None) => Some(Msg::Other(m.to_string())),
        // The server never sends requests (README, Known Limitations).
        (Some(_), Some(_)) => None,
        (None, None) => None,
    }
}

/// Read the runtime's stdout into the channel until it closes.
pub fn read_runtime(r: impl Read + Send + 'static, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(r);
        let mut line = Vec::new();
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(m) = parse(line.strip_suffix(b"\n").unwrap_or(&line)) {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                }
            }
        }
        let _ = tx.send(Msg::Closed);
    });
}

/// The request side of the channel.
pub struct Client<W: Write> {
    w: W,
    next: u64,
}

impl<W: Write> Client<W> {
    pub fn new(w: W) -> Self {
        Client { w, next: 1 }
    }

    /// Send a request and return its id.
    pub fn request(&mut self, method: &str, params: Option<Value>) -> Result<u64> {
        let id = self.next;
        self.next += 1;
        let mut m = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            m["params"] = p;
        }
        let mut line = serde_json::to_vec(&m)?;
        line.push(b'\n');
        self.w
            .write_all(&line)
            .and_then(|_| self.w.flush())
            .with_context(|| format!("cannot send {method} to dsh"))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_parse_as_the_dsh_transport_reads_them() {
        let ev = br#"{"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"session-a","event":{"type":"turn/start","seq":3,"time":5,"data":{"turn":1}}}}"#;
        assert_eq!(
            parse(ev),
            Some(Msg::Event {
                session: "session-a".into(),
                ty: "turn/start".into(),
                seq: 3,
                raw: br#"{"type":"turn/start","seq":3,"time":5,"data":{"turn":1}}"#.to_vec(),
            })
        );
        let st = br#"{"jsonrpc":"2.0","method":"session.status","params":{"sessionId":"session-a","status":"idle"}}"#;
        assert_eq!(
            parse(st),
            Some(Msg::Status {
                session: "session-a".into(),
                idle: true
            })
        );
        let ok = br#"{"jsonrpc":"2.0","id":2,"result":{"messageId":"m"}}"#;
        assert!(matches!(
            parse(ok),
            Some(Msg::Response {
                id: 2,
                result: Ok(_)
            })
        ));
        let err = br#"{"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"no"}}"#;
        assert_eq!(
            parse(err),
            Some(Msg::Response {
                id: 3,
                result: Err("no".into())
            })
        );
        assert_eq!(parse(b"not json"), None);
        assert_eq!(parse(br#"{"jsonrpc":"2.0","id":1,"method":"x"}"#), None);
    }
}
