//! The answers to the permission requests of `grok agent stdio`.
//!
//! paddock answers as the plain Grok run of the profile would decide, and
//! never more permissively. The plain run is the `[oneshot]` command of
//! `profiles/grok.toml`: `grok --always-approve --prompt-file ...`, which
//! runs every tool with no question. Only a plain run with
//! `--always-approve` (or its alias `--yolo`) gets an `allow_once`, and
//! never an `allow_always`: the next request asks again. Every other plain
//! run gets `reject_once`, the answer of a run with no user. A request
//! with no option of that kind is cancelled.

use super::wire;
use crate::profile::Profile;
use anyhow::{Context as _, Result};
use serde_json::{json, Value};

/// How the plain run answers tool permissions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub always_approve: bool,
}

impl Policy {
    /// The policy that the arguments of a plain run give.
    pub fn of(args: &[String]) -> Policy {
        Policy {
            always_approve: args
                .iter()
                .any(|a| a == "--always-approve" || a == "--yolo"),
        }
    }

    /// The answer to `session/request_permission`, and the decision in a
    /// word for the ledger.
    pub fn answer(&self, params: &Value) -> (Value, String) {
        let want = if self.always_approve {
            "allow_once"
        } else {
            "reject_once"
        };
        let option = params
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(want))
            .and_then(|o| o.get("optionId").and_then(Value::as_str));
        match option {
            Some(id) => (
                json!({ "outcome": { "outcome": "selected", "optionId": id } }),
                want.to_string(),
            ),
            None => (
                json!({ "outcome": { "outcome": "cancelled" } }),
                "cancelled".to_string(),
            ),
        }
    }
}

/// One line of the approval ledger of a session.
pub fn ledger_line(params: &Value, decision: &str, policy: &Policy) -> Value {
    json!({
        "at_ms": crate::horse::now_ms(),
        "method": wire::REQUEST_PERMISSION,
        "session": params.get("sessionId").cloned().unwrap_or(Value::Null),
        "tool_call": params.pointer("/toolCall/toolCallId").cloned().unwrap_or(Value::Null),
        "title": params.pointer("/toolCall/title").cloned().unwrap_or(Value::Null),
        "decision": decision,
        "always_approve": policy.always_approve,
    })
}

/// The policy of the plain Grok run of a profile.
pub fn policy(profile: &Profile) -> Result<Policy> {
    let one = profile.oneshot.as_ref().with_context(|| {
        format!(
            "the {} profile has no [oneshot] command, so paddock cannot tell how its plain run answers permissions",
            profile.name
        )
    })?;
    Ok(Policy::of(&one.args))
}

/// The arguments after the grok executable: the ACP agent on stdio, with
/// the permission setting of the plain run, and never a shared leader
/// process (paddock holds this one).
pub fn args(policy: &Policy) -> Vec<String> {
    let mut a = vec!["agent".to_string()];
    if policy.always_approve {
        a.push("--always-approve".into());
    }
    a.push("--no-leader".into());
    a.push("stdio".into());
    a
}
