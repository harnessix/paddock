//! The one schema of the launcher event stream. Every line on the socket
//! and on the stdout of `paddock events --follow --json` is one JSON
//! object: `v` (the schema version), `t` (RFC 3339, UTC, whole seconds),
//! `kind`, and the fields of the kind. `paddock macvm setup --json` prints
//! lines of kind `setup` in the same schema.
//!
//! The rules for a change:
//! - A new optional field keeps [`V`]. A reader ignores the fields it does
//!   not know.
//! - A new kind keeps [`V`]. A reader skips the kinds it does not know.
//! - A field that changes meaning or goes away raises [`V`].
//! - Every kind has a fixture in `tests/fixtures/launcher/<kind>.jsonl`,
//!   which the fake paddock of the Swift launcher replays.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The version of the schema, the `v` of every line.
pub const V: u32 = 1;

/// The longest line that a follower takes from a publisher.
pub const MAX_LINE: usize = 64 << 10;

/// One line: the version, the time and the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub v: u32,
    /// RFC 3339, UTC, whole seconds.
    pub t: String,
    #[serde(flatten)]
    pub event: Event,
}

impl Line {
    /// A line of this version at the time `secs`.
    pub fn at(secs: u64, event: Event) -> Self {
        Self {
            v: V,
            t: crate::waiting::format_rfc3339(secs),
            event,
        }
    }

    /// A line of this version now.
    pub fn now(event: Event) -> Self {
        Self::at(crate::state::now(), event)
    }

    /// The JSON text with the newline.
    pub fn to_text(&self) -> String {
        let mut s = serde_json::to_string(self).expect("an event line serializes");
        s.push('\n');
        s
    }
}

/// The kinds of the stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Session(Session),
    Question(Question),
    Grant(Grant),
    Setup(Setup),
    Golden(Golden),
    Disk(Disk),
    Stream(Stream),
    Notice(Notice),
    DeviceCall(DeviceCall),
}

impl Event {
    /// The `kind` word.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Session(_) => "session",
            Event::Question(_) => "question",
            Event::Grant(_) => "grant",
            Event::Setup(_) => "setup",
            Event::Golden(_) => "golden",
            Event::Disk(_) => "disk",
            Event::Stream(_) => "stream",
            Event::Notice(_) => "notice",
            Event::DeviceCall(_) => "device_call",
        }
    }
}

/// Every `kind` word, in the order of [`Event`].
pub const KINDS: [&str; 9] = [
    "session",
    "question",
    "grant",
    "setup",
    "golden",
    "disk",
    "stream",
    "notice",
    "device_call",
];

/// The state of an app session in the macOS guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Starting,
    Running,
    Stopping,
    Stopped,
    /// The session ended without its save: a crash or a kill.
    Error,
}

/// A state change of a session, from its event log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The app profile.
    pub app: String,
    /// The home id of the profile, `default` for the usual one.
    pub home: String,
    pub state: SessionState,
    /// While starting: `cloning`, `booting`, `copying app` or `opening`.
    /// While stopping: `quitting` or `saving`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The line of the event log, as paddock wrote it.
    #[serde(default)]
    pub text: String,
}

/// What happened to a firewall question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    Open,
    /// A selection was recorded, or a rule resolved the question when
    /// `by` is `rule`. This does not acknowledge persistence or a
    /// completed connection.
    Answered,
    /// This presentation was hidden without deciding the question.
    Withdrawn,
    /// Too many questions: the session stopped asking one by one.
    Paused,
    /// The pause notice was answered and the queue can move again.
    Resumed,
}

/// A firewall question of a session in network mode `ask`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub state: QuestionState,
    /// The id that `paddock approvals answer ID` takes. For `paused`, the
    /// id of the notice. `withdrawn` retires one presentation without an
    /// answer; a later `open` uses a new id.
    pub id: String,
    /// The app profile.
    pub app: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    /// The executable in the guest that connects, when the guest told it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// For `answered`: the selected answer, or the synthetic decision when
    /// `by` is `rule`, not an applied-rule or persistence acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// For `answered`: `user`, `cli`, `approver` or `rule`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// For `paused`: the open questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<u64>,
}

/// What a grant is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantWhat {
    Folder,
    Rule,
    Clipboard,
    Device,
    Network,
}

/// Who changed a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantSource {
    /// A command of the person: the CLI or the launcher.
    You,
    /// An answer in the question panel or `paddock approvals answer`.
    QuestionPanel,
    /// The approver hook.
    Approver,
    /// The app profile.
    Profile,
}

/// When a grant change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    Live,
    NextStart,
    /// Live, but the app holds a file of the old grant open.
    ReopenFiles,
    /// The effect is historical and may have been superseded.
    Unknown,
}

/// A change of what an app may do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub app: String,
    pub what: GrantWhat,
    /// `add`, `remove` or `change`.
    pub action: String,
    /// What changed, for the log line: `allow api.openai.com:443 always`,
    /// `~/Projects rw`.
    pub detail: String,
    pub source: GrantSource,
    pub applies: Applies,
    /// The id of a rule, when the grant is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// The progress of a golden build.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    /// The state of the build: `fetch`, `install`, `seed`, `guided`,
    /// `trust`, `configure`, `seal`, or `sealed` at the end.
    pub step: String,
    /// The bytes of the download so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The size of the download, when the server told it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Bytes per second of the download, averaged since its start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<u64>,
    /// The download waits for `paddock macvm setup --resume`.
    #[serde(default)]
    pub paused: bool,
    /// The step failed: paddock's line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// The person stopped the build; a later setup continues.
    #[serde(default)]
    pub stopped: bool,
    /// A line of the setup for a person, as the terminal shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
}

/// A change of the golden image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoldenState {
    Ready,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Golden {
    pub state: GoldenState,
    /// `50.1 (99A101)` when ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The used bytes of the images when ready; the freed bytes when
    /// removed.
    #[serde(default)]
    pub bytes: u64,
}

/// The free space of the volume of paddock's macOS guest folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Disk {
    /// The `macvm` folder.
    pub dir: PathBuf,
    pub free: u64,
}

/// A line of the stream itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamState {
    /// The first line: the stream is live. Query the state now.
    Started,
    /// Events were lost (a full buffer): query the state again.
    Lost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    pub state: StreamState,
}

/// The longest `title` of a notice, in characters.
pub const NOTICE_TITLE_MAX: usize = 200;
/// The longest `text` of a notice, in characters. The line stays far
/// below [`MAX_LINE`].
pub const NOTICE_TEXT_MAX: usize = 4000;

/// A desktop notification of paddock. paddock publishes each one, also
/// when no launcher runs. A follower that started with `--notify` shows
/// it, and then paddock on macOS does not post its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    /// What the notice is about, in snake case: `approval`, `question`,
    /// `frozen`, `worktree`, `storage` and others. `info` when the caller
    /// did not say.
    pub what: String,
    pub title: String,
    pub text: String,
    /// paddock asks for attention now: an approval or a failure.
    #[serde(default)]
    pub critical: bool,
    /// What answers the notice, for example the request id that `paddock
    /// approvals answer ID` takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl Notice {
    /// A notice with the title and the text cut to their limits.
    pub fn new(what: &str, title: &str, text: &str, critical: bool, action: Option<&str>) -> Self {
        let what = if what.is_empty() { "info" } else { what };
        Self {
            what: what.to_string(),
            title: title.chars().take(NOTICE_TITLE_MAX).collect(),
            text: text.chars().take(NOTICE_TEXT_MAX).collect(),
            critical,
            action: action.filter(|a| !a.is_empty()).map(str::to_string),
        }
    }
}

/// A sandbox called a device or a tool of `/dev/paddock`. paddock
/// publishes one line when the call starts. The line names the call and
/// never holds its arguments, its request or its response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceCall {
    /// The full sandbox id of the caller. `remote:PEER` for a call that
    /// another paddock forwarded.
    pub run: String,
    /// The device name, for example `ask`, `mail/out` or a user device.
    pub device: String,
    /// The call waits for an approval: a card or a question of the user.
    #[serde(default)]
    pub ask: bool,
}

/// A line that a follower passes on: a JSON object with a numeric `v`
/// and a string `kind`, at most [`MAX_LINE`] bytes. A kind or a version
/// that this paddock does not know passes too: the reader decides.
pub fn valid_line(text: &str) -> bool {
    if text.len() > MAX_LINE {
        return false;
    }
    let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    o.get("v").is_some_and(|v| v.is_u64()) && o.get("kind").is_some_and(|k| k.is_string())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
