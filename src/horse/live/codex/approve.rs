//! The answers to the requests of `codex app-server`.
//!
//! paddock answers approvals as the plain Codex run of the profile would
//! decide them, and never more permissively. The plain run is the
//! `[oneshot]` command of `profiles/codex.toml`: `codex exec
//! --dangerously-bypass-approvals-and-sandbox`, which is approval policy
//! `never` with sandbox `danger-full-access`. Only that pair accepts a
//! command or a file change, and only once (never `acceptForSession`).
//! Every other policy declines: under `never` Codex does not ask, and a
//! request that comes anyway gets the answer that the plain run gives to
//! the model, a refusal. A permission request gets no extra permission,
//! a question to the user and an MCP elicitation get no user, and a
//! request that paddock does not know gets "method not found".

use super::wire;
use crate::profile::Profile;
use anyhow::{bail, Context as _, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

/// The approval and sandbox settings of the plain run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// `untrusted`, `on-request` or `never`.
    pub approval: String,
    /// `read-only`, `workspace-write` or `danger-full-access`.
    pub sandbox: String,
}

const APPROVALS: [&str; 3] = ["untrusted", "on-request", "never"];
const SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

impl Policy {
    /// The settings that the arguments of a `codex exec` run give. A value
    /// that paddock does not know is an error: paddock does not guess a
    /// permission.
    pub fn of(args: &[String]) -> Result<Policy> {
        let mut p = Policy {
            approval: "never".into(),
            sandbox: "read-only".into(),
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = |name: &str| {
                it.next()
                    .cloned()
                    .with_context(|| format!("{name} needs a value"))
            };
            match a.as_str() {
                "--dangerously-bypass-approvals-and-sandbox" | "--yolo" => {
                    p.approval = "never".into();
                    p.sandbox = "danger-full-access".into();
                }
                "--full-auto" => {
                    p.approval = "on-request".into();
                    p.sandbox = "workspace-write".into();
                }
                "-a" | "--ask-for-approval" => p.approval = value(a)?,
                "-s" | "--sandbox" => p.sandbox = value(a)?,
                x if x.starts_with("--ask-for-approval=") || x.starts_with("--sandbox=") => {
                    let (k, v) = x.split_once('=').unwrap_or_default();
                    if k == "--sandbox" {
                        p.sandbox = v.into();
                    } else {
                        p.approval = v.into();
                    }
                }
                _ => {}
            }
        }
        if !APPROVALS.contains(&p.approval.as_str()) {
            bail!("the Codex approval policy {} is not known", p.approval);
        }
        if !SANDBOXES.contains(&p.sandbox.as_str()) {
            bail!("the Codex sandbox mode {} is not known", p.sandbox);
        }
        Ok(p)
    }

    /// The plain run lets every command run without a question.
    pub fn bypass(&self) -> bool {
        self.approval == "never" && self.sandbox == "danger-full-access"
    }

    /// The answer to a request of the server.
    pub fn answer(&self, method: &str) -> Answer {
        let yes = self.bypass();
        let ok = |v: Value, d: &str| Answer::Result(v, d.to_string());
        match method {
            wire::APPROVE_COMMAND | wire::APPROVE_FILE_CHANGE => {
                let d = if yes { "accept" } else { "decline" };
                ok(json!({ "decision": d }), d)
            }
            wire::LEGACY_EXEC | wire::LEGACY_PATCH => {
                let d = if yes { "approved" } else { "denied" };
                ok(json!({ "decision": d }), d)
            }
            wire::APPROVE_PERMISSIONS => ok(
                json!({ "permissions": {}, "scope": "turn" }),
                "no extra permission",
            ),
            wire::ELICITATION => ok(json!({ "action": "decline" }), "decline"),
            wire::USER_INPUT => Answer::Error(
                wire::NO_METHOD,
                "no user answers questions in a paddock custody".into(),
            ),
            m => Answer::Error(wire::NO_METHOD, format!("paddock does not answer {m}")),
        }
    }
}

/// The answer to one request.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// The result, and the decision in a word for the ledger.
    Result(Value, String),
    Error(i64, String),
}

impl Answer {
    pub fn decision(&self) -> &str {
        match self {
            Answer::Result(_, d) => d,
            Answer::Error(_, m) => m,
        }
    }
}

/// One line of the approval ledger of a session.
pub fn ledger_line(method: &str, params: &Value, answer: &Answer, policy: &Policy) -> Value {
    let p = |k: &str| params.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "at_ms": crate::horse::now_ms(),
        "method": method,
        "thread": p("threadId"),
        "turn": p("turnId"),
        "item": p("itemId"),
        "command": p("command"),
        "reason": p("reason"),
        "decision": answer.decision(),
        "approval_policy": policy.approval,
        "sandbox": policy.sandbox,
    })
}

/// Append a line to `approvals/<session>.jsonl` of the store, and sync it.
pub fn append_ledger(store: &crate::horse::Store, session: &str, line: &Value) -> Result<()> {
    let dir = store.root().join("approvals");
    crate::privfs::dir(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(format!("{session}.jsonl"));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(crate::privfs::FILE_MODE)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let mut b = serde_json::to_vec(line)?;
    b.push(b'\n');
    f.write_all(&b)
        .and_then(|_| f.sync_data())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// The ledger of a session, oldest first.
pub fn ledger(store: &crate::horse::Store, session: &str) -> Result<Vec<Value>> {
    let path = store.root().join(format!("approvals/{session}.jsonl"));
    let b = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    b.split(|c| *c == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).context("a line of the approval ledger is damaged"))
        .collect()
}

/// The approval and sandbox settings of the plain Codex run.
pub fn policy(profile: &Profile) -> Result<Policy> {
    let one = profile.oneshot.as_ref().with_context(|| {
        format!(
            "the {} profile has no [oneshot] command, so paddock cannot tell how its plain run answers approvals",
            profile.name
        )
    })?;
    Policy::of(&one.args)
}

/// The arguments after the codex executable: the app server on stdio,
/// with the approval and sandbox settings of the plain run.
pub fn args(profile: &Profile) -> Result<Vec<String>> {
    let p = policy(profile)?;
    Ok(vec![
        "app-server".into(),
        "--listen".into(),
        "stdio://".into(),
        "-c".into(),
        format!("approval_policy=\"{}\"", p.approval),
        "-c".into(),
        format!("sandbox_mode=\"{}\"", p.sandbox),
    ])
}
