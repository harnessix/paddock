//! `paddock-host mail hook stop|tool`: the mail hooks of Claude Code and
//! Codex (mail/deliver.rs). The harness runs them with the hook input as
//! JSON on stdin and reads the answer from stdout. Codex 0.167.0 reads the
//! same Stop and PostToolUse answers (codex-rs hooks/src/schema.rs).
//!
//! - `stop` (Stop event): with unread mail, it prints
//!   `{"decision":"block","reason":TEXT}`, so Claude Code does not end the
//!   turn and gives TEXT to the agent. Without mail, it prints nothing and
//!   the turn ends.
//! - `tool` (PostToolUse event): with an unread interrupt, it prints the
//!   messages as `additionalContext`.
//!
//! A hook never keeps the agent going without new mail. When the input has
//! `stop_hook_active: true` (this stop comes after a stop that a hook
//! blocked), when the host does not answer, or when the input is not what
//! it expects, it prints nothing and exits 0: the stop happens.

use crate::broker::{Request, Status, PROTOCOL};
use std::io::Read as _;

/// The most hook input that the hook reads.
const MAX_INPUT: u64 = 1024 * 1024;

/// What the hook prints for the harness, or None to let it go on as it
/// would. `input` is the hook input; `ask` sends the broker request.
pub fn answer(
    which: &str,
    input: &serde_json::Value,
    ask: &dyn Fn(serde_json::Value) -> Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let via = match which {
        "stop" => {
            // Claude Code's loop guard: this stop follows one that a hook
            // blocked. Let it happen, so mail never holds a turn open.
            if input["stop_hook_active"] == true {
                return None;
            }
            "stop"
        }
        "tool" => "tool",
        _ => return None,
    };
    let r = ask(serde_json::json!({ "op": "deliver", "via": via }))?;
    let text = r["text"].as_str().filter(|t| !t.is_empty())?;
    Some(match via {
        "stop" => serde_json::json!({ "decision": "block", "reason": text }),
        _ => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": input["hook_event_name"].as_str().unwrap_or("PostToolUse"),
                "additionalContext": text,
            }
        }),
    })
}

/// One broker request; None when the host does not answer with done.
fn ask(args: serde_json::Value) -> Option<serde_json::Value> {
    let req = Request {
        v: PROTOCOL,
        cap: "mail".into(),
        args: serde_json::from_value(args).unwrap_or_default(),
        reason: String::new(),
    };
    match super::send(&req) {
        Ok(r) if r.status == Status::Done => Some(r.result),
        Ok(r) => {
            // A run without a mailbox is no failure.
            if r.status != Status::Disabled {
                eprintln!("paddock-host mail hook: {}", r.message);
            }
            None
        }
        Err(e) => {
            eprintln!("paddock-host mail hook: {e}");
            None
        }
    }
}

/// `paddock-host mail hook stop|tool`. Always exits 0: a failure lets the
/// harness go on as it would without the hook.
pub fn main(args: &[String]) -> i32 {
    let [which] = args else {
        eprintln!("paddock-host mail hook: use `hook stop` or `hook tool`");
        return 0;
    };
    let mut raw = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut raw);
    let input: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_default();
    if let Some(out) = answer(which, &input, &ask) {
        println!("{out}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn a_stop_with_mail_is_blocked_with_the_messages_as_the_reason() {
        let asked = RefCell::new(Vec::new());
        let ask = |a: serde_json::Value| {
            asked.borrow_mut().push(a);
            Some(serde_json::json!({"text": "--- mail #3 say\nhello\n"}))
        };
        let input = serde_json::json!({"hook_event_name": "Stop", "stop_hook_active": false});
        let out = answer("stop", &input, &ask).unwrap();
        assert_eq!(out["decision"], "block");
        assert!(out["reason"].as_str().unwrap().contains("hello"));
        assert_eq!(asked.borrow()[0]["via"], "stop");
    }

    #[test]
    fn a_stop_goes_on_without_mail_after_a_blocked_stop_or_without_host() {
        let none = |_| Some(serde_json::json!({"text": null}));
        let input = serde_json::json!({"hook_event_name": "Stop", "stop_hook_active": false});
        assert!(answer("stop", &input, &none).is_none());
        let never = |_: serde_json::Value| -> Option<serde_json::Value> {
            panic!("no request after a blocked stop")
        };
        let active = serde_json::json!({"hook_event_name": "Stop", "stop_hook_active": true});
        assert!(answer("stop", &active, &never).is_none());
        assert!(answer("stop", &input, &|_| None).is_none());
        assert!(answer("other", &input, &never).is_none());
    }

    #[test]
    fn an_interrupt_after_a_tool_is_added_context() {
        let ask = |a: serde_json::Value| {
            assert_eq!(a["via"], "tool");
            Some(serde_json::json!({"text": "urgent"}))
        };
        let input = serde_json::json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"});
        let out = answer("tool", &input, &ask).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(out["hookSpecificOutput"]["additionalContext"], "urgent");
    }
}
