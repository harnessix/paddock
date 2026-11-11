//! The setup side of the wire protocol (`proto.rs`): the configure
//! steps of the golden build and the host's check of a `hello`. `proto`
//! re-exports them.

use super::proto::{ErrorCode, Hello, Version};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// One step of the golden configure. The host sends the list; the password
/// goes on stdin of `sudo -S`, never in an argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureStep {
    pub name: String,
    pub as_root: bool,
    pub argv: Vec<String>,
    /// The command whose output proves the step. Empty: no verify.
    #[serde(default)]
    pub verify: Vec<String>,
}

/// The result of one configure step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepResult {
    pub name: String,
    pub ok: bool,
    /// The output of the command, both streams, the last 4 KiB.
    pub output: String,
    /// The output of the verify command, when there is one.
    pub verify: String,
    /// The exit code of the command; `None` when it did not start, was
    /// killed at its timeout, or an agent before this field ran it.
    #[serde(default)]
    pub code: Option<i32>,
    /// The time of the command and its verify, in milliseconds.
    #[serde(default)]
    pub ms: u64,
    /// The command or its verify ran past the step timeout and was killed.
    #[serde(default)]
    pub timed_out: bool,
}

/// What the host does after a `hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloAction {
    /// The versions match: send `token`.
    Talk,
    /// Send `agent.update` first, then wait for the new `hello`.
    Update,
}

/// The host's check of a `hello`. `host_agent` is the paddock version of
/// the host. The table is `version::agent_action`: an agent of another
/// version updates itself when it can, equal majors talk even when the
/// update is not possible, and a different major that cannot update is
/// the one refusal, with the text that names `paddock macvm update`.
pub fn check_hello(hello: &Hello, host: Version, host_agent: &str) -> Result<HelloAction> {
    check_hello_sha(hello, host, host_agent, None)
}

/// [`check_hello`] with the sha256 of the agent that this host ships.
/// Equal versions with another agent binary (a rebuild keeps the version)
/// update too, when the agent can and both hashes are known.
pub fn check_hello_sha(
    hello: &Hello,
    host: Version,
    host_agent: &str,
    host_sha: Option<&str>,
) -> Result<HelloAction> {
    let action = check_hello_version(hello, host, host_agent)?;
    let rebuilt = match (host_sha, hello.sha256.as_deref()) {
        (Some(h), Some(a)) => !h.eq_ignore_ascii_case(a),
        _ => false,
    };
    if action == HelloAction::Talk && rebuilt && hello.can_update {
        return Ok(HelloAction::Update);
    }
    Ok(action)
}

fn check_hello_version(hello: &Hello, host: Version, host_agent: &str) -> Result<HelloAction> {
    use super::version::{agent_action, Action, Peer};
    let h = Peer {
        paddock: host_agent,
        protocol: host,
    };
    let a = Peer {
        paddock: &hello.agent,
        protocol: hello.v,
    };
    match agent_action(h, a, hello.can_update) {
        Action::Talk => Ok(HelloAction::Talk),
        Action::Update => Ok(HelloAction::Update),
        Action::Refuse(line) => bail!(line),
    }
}

impl ErrorCode {
    /// A short text of the code, for a log line.
    pub fn describe(self) -> &'static str {
        match self {
            ErrorCode::Timeout => "the guest app did not answer in time",
            ErrorCode::NotInSession => "refused while an app runs in the guest",
            ErrorCode::NoSuchWindow => "no such window in the guest",
            ErrorCode::NoSuchApp => "no such app in the guest",
            ErrorCode::Busy => "the guest agent is busy with the same request",
            ErrorCode::Accessibility => "the guest agent has no Accessibility permission",
            ErrorCode::BadHash => "the update file does not match its hash",
            ErrorCode::Locked => "the data disk belongs to another system disk and does not mount",
            ErrorCode::Io => "a file or a command failed in the guest",
            ErrorCode::Unsupported => "the guest agent does not implement this request",
            ErrorCode::BadRequest => "the request is not valid",
            ErrorCode::Internal => "an error inside the guest agent",
        }
    }
}
