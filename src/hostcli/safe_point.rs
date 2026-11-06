//! `paddock-host horse safe-point stop|prompt [--then PROGRAM ARG...] [JSON]`:
//! a hook of an interactive run tells the host that a turn ended or
//! started.
//!
//! Claude Code runs it as a hook with no arguments. Codex runs it as its
//! `notify` program and adds one JSON argument after each turn
//! (`{"type": "agent-turn-complete", "thread-id": ..., "turn-id": ...}`).
//! The thread id goes into the request, so the host can tell the turns of
//! its session from the turns of another thread. A JSON argument of
//! another type is no safe point: the host hears nothing of it.
//!
//! `--then` names the user's own `notify` program, which paddock found in
//! the configuration (horse/codex/notify.rs). It runs afterwards with the
//! same JSON argument, also when the host cannot be reached.

use crate::broker::{Request, PROTOCOL};
use serde_json::{json, Value};

/// The JSON type of Codex's `notify` after a turn.
pub const TURN_COMPLETE: &str = "agent-turn-complete";

/// One call, as the arguments give it.
#[derive(Debug, PartialEq, Eq)]
pub struct Call {
    pub kind: String,
    /// Tell the host: false for a JSON argument of another type.
    pub tell: bool,
    pub thread: Option<String>,
    /// The user's program and its arguments, empty for none.
    pub then: Vec<String>,
    /// The JSON argument as Codex passed it.
    pub payload: Option<String>,
}

/// The arguments after `safe-point`. None when they are not a call.
pub fn parse(args: &[String]) -> Option<Call> {
    let (kind, rest) = args.split_first()?;
    if kind != "stop" && kind != "prompt" {
        return None;
    }
    let mut rest = rest.to_vec();
    // Codex adds the JSON last, after the user's program and its arguments.
    let payload = rest
        .last()
        .filter(|a| a.trim_start().starts_with('{'))
        .cloned();
    if payload.is_some() {
        rest.pop();
    }
    let then = match rest.split_first() {
        None => Vec::new(),
        Some((flag, prog)) if flag == "--then" && !prog.is_empty() => prog.to_vec(),
        Some(_) => return None,
    };
    let v: Option<Value> = payload
        .as_deref()
        .and_then(|p| serde_json::from_str(p).ok());
    let tell = match &v {
        Some(v) => v.get("type").and_then(Value::as_str) == Some(TURN_COMPLETE),
        // No JSON (a Claude Code hook), or JSON that does not parse: a
        // turn ended, and no thread is named.
        None => true,
    };
    let thread = v
        .as_ref()
        .and_then(|v| v.get("thread-id"))
        .and_then(Value::as_str)
        .filter(|t| crate::horse::live::tui_state::check_thread(t).is_ok())
        .map(String::from);
    Some(Call {
        kind: kind.clone(),
        tell,
        thread,
        then,
        payload,
    })
}

/// The request to the host for `call`.
pub fn request(call: &Call) -> Request {
    let mut args = std::collections::BTreeMap::new();
    args.insert(
        "op".into(),
        json!(crate::horse::live::tui_state::SAFE_POINT),
    );
    args.insert("kind".into(), json!(call.kind));
    if let Some(t) = &call.thread {
        args.insert("thread".into(), json!(t));
    }
    Request {
        v: PROTOCOL,
        cap: "horse".into(),
        args,
        reason: String::new(),
    }
}

pub fn main(args: &[String]) -> i32 {
    let Some(call) = parse(args) else {
        eprintln!(
            "usage: paddock-host horse safe-point stop|prompt [--then PROGRAM ARG...] [JSON]"
        );
        return 2;
    };
    let mut code = 0;
    if call.tell {
        code = super::report(
            super::super::send(&request(&call)),
            &mut std::io::sink(),
            &mut std::io::stderr(),
        );
    }
    if let Some((prog, a)) = call.then.split_first() {
        let status = std::process::Command::new(prog)
            .args(a)
            .args(call.payload.iter())
            .status();
        match status {
            Ok(s) if code == 0 => code = s.code().unwrap_or(1),
            Ok(_) => {}
            Err(e) => {
                eprintln!("paddock-host: cannot run the notify program {prog}: {e}");
                code = 1;
            }
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_claude_hook_has_no_json_and_names_no_thread() {
        let c = parse(&a(&["stop"])).unwrap();
        assert!(c.tell);
        assert_eq!((c.thread, c.then, c.payload), (None, vec![], None));
        assert_eq!(parse(&a(&["prompt"])).unwrap().kind, "prompt");
        assert!(parse(&a(&["busy"])).is_none());
        assert!(parse(&a(&[])).is_none());
        assert!(parse(&a(&["stop", "extra"])).is_none());
        assert!(parse(&a(&["stop", "--then"])).is_none());
    }

    #[test]
    fn codex_notify_json_gives_the_thread_and_is_passed_on() {
        let j = r#"{"type":"agent-turn-complete","thread-id":"019a5f00-aaaa-7000-9000-000000000001","turn-id":"t1","input-messages":["hi"],"last-assistant-message":"ok"}"#;
        let c = parse(&a(&["stop", j])).unwrap();
        assert!(c.tell);
        assert_eq!(
            c.thread.as_deref(),
            Some("019a5f00-aaaa-7000-9000-000000000001")
        );
        assert!(c.then.is_empty());
        let r = request(&c);
        assert_eq!(r.args["op"], "safe_point");
        assert_eq!(r.args["kind"], "stop");
        assert_eq!(r.args["thread"], "019a5f00-aaaa-7000-9000-000000000001");
        // The user's notify program comes after --then and gets the JSON.
        let c = parse(&a(&["stop", "--then", "notify-send", "-u", "low", j])).unwrap();
        assert_eq!(c.then, a(&["notify-send", "-u", "low"]));
        assert_eq!(c.payload.as_deref(), Some(j));
        assert!(!request(&parse(&a(&["stop"])).unwrap())
            .args
            .contains_key("thread"));
    }

    #[test]
    fn another_notify_type_is_no_safe_point_and_a_bad_thread_is_dropped() {
        let c = parse(&a(&[
            "stop",
            r#"{"type":"approval-requested","thread-id":"x"}"#,
        ]))
        .unwrap();
        assert!(!c.tell);
        let c = parse(&a(&[
            "stop",
            r#"{"type":"agent-turn-complete","thread-id":"../x"}"#,
        ]))
        .unwrap();
        assert!(c.tell);
        assert_eq!(c.thread, None);
        // A JSON argument that does not parse still ends a turn.
        let c = parse(&a(&["stop", "{not json"])).unwrap();
        assert!(c.tell);
        assert_eq!(c.thread, None);
    }
}
