//! The stream-json lines of `claude -p --input-format stream-json
//! --output-format stream-json --verbose`: one JSON object per
//! `\n`-terminated line, with a `type` member.
//!
//! paddock reads `system` (the `init` subtype confirms the session id),
//! `assistant`, `user` (tool results that Claude Code echoes), `result`
//! (the end of a turn, whatever its subtype) and `control_response`.
//! paddock writes `user` messages and `control_request` with the
//! `interrupt` subtype. A line of another type is kept as it came and
//! ignored. A line that is not a JSON object with a text `type` is bad.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `system`. Only the `init` subtype matters to paddock.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct System {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// `assistant` and `user`: a message of the running turn.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub message: Value,
}

/// `result`: the turn is over.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TurnResult {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub usage: Option<Value>,
}

/// The `response` member of a `control_response`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ControlAnswer {
    pub subtype: String,
    pub request_id: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ControlResponse {
    pub response: ControlAnswer,
}

/// A line that paddock read.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    System(System),
    Assistant(Message),
    User(Message),
    Result(TurnResult),
    Control(ControlResponse),
    /// A JSON object of a type that paddock does not read, as it came.
    Unknown {
        ty: String,
        raw: Vec<u8>,
    },
    /// Not a JSON object with a `type`, or a known type in another shape.
    Bad {
        raw: Vec<u8>,
        why: String,
    },
}

/// Parse one line, without its newline.
pub fn parse(raw: &[u8]) -> Line {
    let bad = |why: String| Line::Bad {
        raw: raw.to_vec(),
        why,
    };
    let v: Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => return bad(format!("not JSON: {e}")),
    };
    let Some(ty) = v.get("type").and_then(Value::as_str).map(str::to_string) else {
        return bad("no type".into());
    };
    fn take<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
    let r = match ty.as_str() {
        "system" => take(v).map(Line::System),
        "assistant" => take(v).map(Line::Assistant),
        "user" => take(v).map(Line::User),
        "result" => take(v).map(Line::Result),
        "control_response" => take(v).map(Line::Control),
        _ => {
            return Line::Unknown {
                ty,
                raw: raw.to_vec(),
            }
        }
    };
    r.unwrap_or_else(|e| bad(format!("a {ty} line in another shape: {e}")))
}

#[derive(Serialize)]
struct Text<'a> {
    #[serde(rename = "type")]
    ty: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct UserMessage<'a> {
    role: &'static str,
    content: [Text<'a>; 1],
}

#[derive(Serialize)]
struct UserLine<'a> {
    #[serde(rename = "type")]
    ty: &'static str,
    message: UserMessage<'a>,
    parent_tool_use_id: Option<()>,
    session_id: &'a str,
}

#[derive(Serialize)]
struct Interrupt {
    subtype: &'static str,
}

#[derive(Serialize)]
struct ControlRequest<'a> {
    #[serde(rename = "type")]
    ty: &'static str,
    request_id: &'a str,
    request: Interrupt,
}

/// A user message line, with its newline.
pub fn user_line(session_id: &str, text: &str) -> Vec<u8> {
    let mut b = serde_json::to_vec(&UserLine {
        ty: "user",
        message: UserMessage {
            role: "user",
            content: [Text { ty: "text", text }],
        },
        parent_tool_use_id: None,
        session_id,
    })
    .expect("a user line serializes");
    b.push(b'\n');
    b
}

/// An interrupt request line, with its newline.
pub fn interrupt_line(request_id: &str) -> Vec<u8> {
    let mut b = serde_json::to_vec(&ControlRequest {
        ty: "control_request",
        request_id,
        request: Interrupt {
            subtype: "interrupt",
        },
    })
    .expect("a control request serializes");
    b.push(b'\n');
    b
}

/// The text of a message: a text content, or its text blocks joined.
/// `None` for a message with other blocks, for example tool results.
pub fn text_of(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let mut out = String::new();
            for b in blocks {
                if b.get("type").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                out.push_str(b.get("text").and_then(Value::as_str)?);
            }
            Some(out)
        }
        _ => None,
    }
}
