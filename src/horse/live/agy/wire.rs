//! The stream-json lines of `agy --input-format stream-json
//! --output-format stream-json`: one JSON object per `\n`-terminated line.
//!
//! The Antigravity CLI documentation ("Drive a session programmatically")
//! names the event of a line in its `event` member. paddock also takes a
//! `type` member, because the documentation does not show a whole output
//! line. paddock reads:
//! - `init`: the conversation starts, with its `conversation_id`;
//! - `step_update`: a step of the running turn, with `conversation_id`;
//! - `result`: the turn is over (`status`, `response`,
//!   `finished_reason`). This is the safe point.
//!
//! A line of another event is kept as it came and ignored. A line that is
//! not a JSON object with a text event is bad. paddock writes only `user`
//! lines: agy ends a session with exit code 2 on any other input
//! (`control_request`, a slash command that the CLI answers), so the
//! interrupt is a signal (live/agy/signal.rs), never a line.

use serde_json::{json, Value};

/// What a line of agy said.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    /// `init`.
    Init { conversation: Option<String> },
    /// `step_update`.
    Step { conversation: Option<String> },
    /// `result`: the turn ended.
    Result(TurnResult),
    /// A JSON object of an event that paddock does not read.
    Unknown { event: String },
    /// Not a JSON object with an event.
    Bad { why: String },
}

/// The members of a `result` line that paddock reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnResult {
    pub conversation: Option<String>,
    pub status: String,
    pub response: Option<String>,
    pub finished_reason: Option<String>,
}

impl TurnResult {
    /// SIGINT ended the turn.
    pub fn interrupted(&self) -> bool {
        self.finished_reason.as_deref() == Some("interrupted") || self.status == "interrupted"
    }

    /// The turn ended as a normal turn does.
    pub fn ok(&self) -> bool {
        self.status == "success" && !self.interrupted()
    }
}

fn text(v: &Value, a: &str, b: &str) -> Option<String> {
    v.get(a)
        .or_else(|| v.get(b))
        .and_then(Value::as_str)
        .map(String::from)
}

fn conversation(v: &Value) -> Option<String> {
    text(v, "conversation_id", "conversationId")
}

/// Parse one line, without its newline.
pub fn parse(raw: &[u8]) -> Line {
    let v: Value = match serde_json::from_slice(raw) {
        Ok(v @ Value::Object(_)) => v,
        Ok(_) => {
            return Line::Bad {
                why: "not a JSON object".into(),
            }
        }
        Err(e) => {
            return Line::Bad {
                why: format!("not JSON: {e}"),
            }
        }
    };
    let Some(event) = text(&v, "event", "type") else {
        return Line::Bad {
            why: "no event".into(),
        };
    };
    match event.as_str() {
        "init" => Line::Init {
            conversation: conversation(&v),
        },
        "step_update" => Line::Step {
            conversation: conversation(&v),
        },
        "result" => Line::Result(TurnResult {
            conversation: conversation(&v),
            status: text(&v, "status", "subtype").unwrap_or_default(),
            response: text(&v, "response", "result"),
            finished_reason: text(&v, "finished_reason", "finishedReason"),
        }),
        _ => Line::Unknown { event },
    }
}

/// A user line, with its newline: the documented input of stream-json.
pub fn user_line(text: &str) -> Vec<u8> {
    let mut b = serde_json::to_vec(&json!({
        "event": "user",
        "message": { "content": text },
    }))
    .expect("a user line serializes");
    b.push(b'\n');
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_of_agy_parse() {
        let id = "7c0e1f2a-3b4c-4d5e-8f60-0a1b2c3d4e5f";
        assert_eq!(
            parse(format!(r#"{{"event":"init","conversation_id":"{id}","cwd":"/w"}}"#).as_bytes()),
            Line::Init {
                conversation: Some(id.into())
            }
        );
        assert_eq!(
            parse(br#"{"type":"step_update","step_index":2,"state":"ACTIVE"}"#),
            Line::Step { conversation: None }
        );
        let r = parse(
            br#"{"event":"result","status":"success","response":"ok","num_turns":1,"finished_reason":"interrupted"}"#,
        );
        let Line::Result(r) = r else { panic!("{r:?}") };
        assert!(r.interrupted() && !r.ok());
        assert_eq!(r.response.as_deref(), Some("ok"));
        assert_eq!(
            parse(br#"{"event":"usage"}"#),
            Line::Unknown {
                event: "usage".into()
            }
        );
        assert!(matches!(parse(b"[1]"), Line::Bad { .. }));
        assert!(matches!(parse(b"{"), Line::Bad { .. }));
        assert!(matches!(parse(br#"{"x":1}"#), Line::Bad { .. }));
    }

    #[test]
    fn a_user_line_is_one_line_of_the_documented_shape() {
        let l = user_line("two\nlines \"quoted\"");
        assert_eq!(l.iter().filter(|b| **b == b'\n').count(), 1);
        let v: Value = serde_json::from_slice(&l).unwrap();
        assert_eq!(v["event"], "user");
        assert_eq!(v["message"]["content"], "two\nlines \"quoted\"");
    }
}
