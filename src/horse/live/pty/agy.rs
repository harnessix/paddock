//! The session files of an interactive agy run, for the pty custody
//! (codex.rs).
//!
//! agy has a hook that could push into its TUI: a `Stop` hook may answer
//! `decision: "continue"` with a `reason`, and agy then starts a new loop
//! with that reason as a system message. paddock does not use it. Every
//! entry would land as a system message, also a `user` entry, and the
//! entry would have to leave the inbox from a hook process inside the
//! sandbox. `PreInvocation` `injectSteps` lands inside a running turn,
//! which is not a safe point. So paddock types the entries into the pty
//! of the run, and each one becomes a user step, as a typed message does.
//!
//! The hooks of the sandbox copy of `.gemini/config/hooks.json`
//! (run/agy_setup.rs) tell the host when a turn runs and ends:
//! `PreInvocation` (before each model call) is `prompt`, and `Stop` is
//! `stop`. A `Stop` with `fullyIdle: false` (a background task still
//! runs) and the hooks of a subagent conversation are no safe point
//! (hostcli/safe_point.rs). The first `stop` that names a conversation
//! with a database binds the store session of the run to it.
//!
//! The loop tails `brain/<id>/.system_generated/logs/transcript.jsonl`,
//! the log that agy derives from the database: a `USER_INPUT` step from
//! the user is a turn start. The log has no turn end, so only the `Stop`
//! hook ends a turn. The ingest reads the database, not the log.

use super::files::{Files, Mark};
use crate::horse::agy as codec;
use crate::horse::store::Lock;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub use super::codex::{CustodyCtx, Report, Timing};

/// The derived log of a conversation under the agy home.
pub fn transcript(home: &Path, id: &str) -> PathBuf {
    home.join("brain")
        .join(id)
        .join(".system_generated/logs/transcript.jsonl")
}

/// agy: the transcript log of a conversation, and its database.
pub struct Agy;

impl Files for Agy {
    const HARNESS: &'static str = codec::HARNESS;
    const SIGNAL: &'static str = "agy's Stop hook";
    const HINT: &'static str =
        "look at the paddock-horse entry of .gemini/config/hooks.json of the sandbox";
    // The database changes with every step; the log is derived.
    const ONE_FILE: bool = false;

    fn check_version(v: &str) -> Result<()> {
        codec::check_version(v)
    }

    /// `root` is the agy home. The conversation must have its database;
    /// its log may come later.
    fn find(root: &Path, id: &str) -> Option<PathBuf> {
        crate::horse::claude::check_session_id(id).ok()?;
        crate::horse::live::agy::db_path(root, id)
            .is_file()
            .then(|| transcript(root, id))
    }

    /// `<home>/brain/<id>/.system_generated/logs/transcript.jsonl` is the
    /// log of `<home>/conversations/<id>.db`.
    fn source(tailed: &Path) -> PathBuf {
        let conv = tailed.ancestors().nth(3);
        let id = conv.and_then(Path::file_name).and_then(|n| n.to_str());
        let home = conv.and_then(Path::parent).and_then(Path::parent);
        match (home, id) {
            (Some(h), Some(id)) => crate::horse::live::agy::db_path(h, id),
            _ => tailed.to_path_buf(),
        }
    }

    /// The hooks name subagent conversations; the TUI binds only the one
    /// whose `Stop` came with no parent.
    fn sub_agent(_path: &Path) -> bool {
        false
    }

    fn mark(raw: &[u8]) -> Option<Mark> {
        let v: Value = serde_json::from_slice(raw).ok()?;
        (v["type"] == "USER_INPUT" && v["source"] == "USER_EXPLICIT").then_some(Mark::User)
    }

    fn ingest(l: &Lock<'_>, path: &Path, version: &str, horse: &str) -> Result<String> {
        Ok(codec::ingest_locked(l, path, version, Some(horse))?.session)
    }

    fn user_row_holds(raw: &[u8], text: &str) -> bool {
        crate::horse::live::agy::user_text(raw).is_some_and(|t| t.trim() == text.trim())
    }
}

/// Hold the TUI run until the run ends (two `Close`s on the control
/// channel), then ingest a last time.
pub fn run(ctx: CustodyCtx, signals: crate::horse::live::tui_state::SignalRx) -> Result<Report> {
    super::codex::run_files::<Agy>(ctx, signals)
}

#[cfg(test)]
#[path = "agy_tests.rs"]
mod tests;
