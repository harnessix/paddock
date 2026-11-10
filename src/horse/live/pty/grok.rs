//! The session files of an interactive Grok Build run, for the pty
//! custody (codex.rs).
//!
//! Grok Build has no checked push path into its TUI: `x.ai/interject`
//! reaches a session only through an ACP client of the same process, and
//! whether a second client can attach to a TUI through `--leader` is not
//! known. So paddock types the entries into the pty of the run. Its hooks
//! (`UserPromptSubmit`, `Stop`, `StopCancelled`, `StopFailure`, set in the
//! sandbox copy of paddock's hooks file, run/grok_setup.rs) tell the host
//! when a turn starts and ends, with the session id and the turn
//! (`promptId`). The first `stop` that names a session with a session
//! folder binds the store session of the run to it.
//!
//! The loop tails `updates.jsonl`: a user chunk is a turn start, a
//! `turn_completed` row its end (`stopReason: cancelled` after the
//! interrupt key). The ingest reads the whole session folder.

use super::files::{Files, Mark};
use crate::horse::grok as codec;
use crate::horse::store::Lock;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub use super::codex::{CustodyCtx, Report, Timing};

/// Grok Build: `updates.jsonl` of a session folder.
pub struct Grok;

impl Files for Grok {
    const HARNESS: &'static str = codec::HARNESS;
    const SIGNAL: &'static str = "Grok's Stop hook";
    const HINT: &'static str =
        "look at .grok/hooks/paddock-compaction.json and .grok/trusted_folders.toml of the sandbox";
    // The chat history is a second file: an unchanged log does not mean
    // an unchanged session.
    const ONE_FILE: bool = false;

    fn check_version(v: &str) -> Result<()> {
        codec::check_version(v)
    }

    /// `root` is the Grok home: `sessions/<encoded cwd>/<id>/updates.jsonl`.
    fn find(root: &Path, id: &str) -> Option<PathBuf> {
        if crate::horse::claude::check_session_id(id).is_err() {
            return None;
        }
        let mut groups: Vec<PathBuf> = std::fs::read_dir(root.join("sessions"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        groups.sort();
        groups
            .into_iter()
            .map(|g| g.join(id).join(codec::UPDATES_FILE))
            .find(|f| f.is_file())
    }

    fn sub_agent(path: &Path) -> bool {
        let summary = path.with_file_name(codec::SUMMARY_FILE);
        let v: Option<Value> = std::fs::read(summary)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        v.as_ref()
            .and_then(|v| v["session_kind"].as_str())
            .is_some_and(|k| k.starts_with("subagent"))
    }

    fn mark(raw: &[u8]) -> Option<Mark> {
        let v: Value = serde_json::from_slice(raw).ok()?;
        let u = &v["params"]["update"];
        match u["sessionUpdate"].as_str()? {
            "user_message_chunk" if u["_meta"]["hostTurn"] != true => Some(Mark::User),
            "turn_completed" => Some(Mark::End {
                aborted: u["stopReason"] == "cancelled",
            }),
            _ => None,
        }
    }

    fn ingest(l: &Lock<'_>, path: &Path, version: &str, horse: &str) -> Result<String> {
        let dir = path.parent().unwrap_or(path);
        Ok(codec::ingest_locked(l, dir, version, Some(horse))?.session)
    }

    fn user_row_holds(raw: &[u8], text: &str) -> bool {
        crate::horse::live::grok::user_row_holds(raw, text)
    }
}

/// Hold the TUI run until the run ends (two `Close`s on the control
/// channel), then ingest a last time.
pub fn run(ctx: CustodyCtx, signals: crate::horse::live::tui_state::SignalRx) -> Result<Report> {
    super::codex::run_files::<Grok>(ctx, signals)
}

#[cfg(test)]
#[path = "grok_tests.rs"]
mod tests;
