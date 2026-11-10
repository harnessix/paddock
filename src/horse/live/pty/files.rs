//! What the pty custody (codex.rs) needs to know of the session files of
//! one harness: where the file of a session is, which of its rows start
//! and end a turn, how it goes into the store, and which row holds a
//! typed entry. Codex reads its rollout file; Grok Build its
//! `updates.jsonl` (grok.rs); agy its conversation database (agy.rs).

use crate::horse::store::Lock;
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A row of the session file that the loop acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A user message: a turn starts.
    User,
    /// The end of a turn, `aborted` after an interrupt.
    End { aborted: bool },
}

/// The session files of one harness.
pub trait Files {
    /// The harness name of the store sessions.
    const HARNESS: &'static str;
    /// What tells the host that a turn ended, for the lines of the loop.
    const SIGNAL: &'static str;
    /// Where to look when that signal never comes.
    const HINT: &'static str;
    /// The tailed file changes with every row of the session, so an
    /// unchanged file needs no ingest.
    const ONE_FILE: bool;
    fn check_version(v: &str) -> Result<()>;
    /// The tailed file of session `id` under the folder `root`.
    fn find(root: &Path, id: &str) -> Option<PathBuf>;
    /// The file that goes into the store for the tailed file `tailed`:
    /// the tailed file itself, or (agy) the database that it is a log of.
    fn source(tailed: &Path) -> PathBuf {
        tailed.to_path_buf()
    }
    /// The file is the one of a sub-agent, not of the TUI.
    fn sub_agent(path: &Path) -> bool;
    fn mark(raw: &[u8]) -> Option<Mark>;
    /// Ingest the session of the tailed file; the store session it went into.
    fn ingest(l: &Lock<'_>, path: &Path, version: &str, horse: &str) -> Result<String>;
    fn check_delivery(_path: Option<&Path>) -> Result<()> {
        Ok(())
    }
    /// A user row of the store with `text`.
    fn user_row_holds(raw: &[u8], text: &str) -> bool;
}

/// Codex: the rollout file of a thread.
pub struct Codex;

impl Files for Codex {
    const HARNESS: &'static str = crate::horse::codex::HARNESS;
    const SIGNAL: &'static str = "Codex's notify program";
    const HINT: &'static str = "look at notify in .codex/config.toml of the sandbox";
    const ONE_FILE: bool = true;

    fn check_version(v: &str) -> Result<()> {
        crate::horse::codex::check_version(v)
    }

    fn find(root: &Path, id: &str) -> Option<PathBuf> {
        crate::horse::live::codex::rollout::find_rollout(root, id)
    }

    fn sub_agent(path: &Path) -> bool {
        use std::io::BufRead as _;
        let mut line = String::new();
        let read = std::fs::File::open(path)
            .map(std::io::BufReader::new)
            .and_then(|mut r| r.read_line(&mut line));
        let first = read
            .ok()
            .and_then(|_| serde_json::from_str::<Value>(&line).ok());
        let src = first.as_ref().map(|v| &v["payload"]["source"]);
        src.is_some_and(|s| s.get("subagent").is_some() || s == "subagent")
    }

    fn mark(raw: &[u8]) -> Option<Mark> {
        let v: Value = serde_json::from_slice(raw).ok()?;
        if v["type"] != "event_msg" {
            return None;
        }
        match v["payload"]["type"].as_str()? {
            "user_message" => Some(Mark::User),
            "task_complete" => Some(Mark::End { aborted: false }),
            "turn_aborted" => Some(Mark::End { aborted: true }),
            _ => None,
        }
    }

    fn ingest(l: &Lock<'_>, path: &Path, version: &str, horse: &str) -> Result<String> {
        Ok(crate::horse::codex::ingest_run_locked(l, path, version, horse)?.session)
    }

    fn check_delivery(path: Option<&Path>) -> Result<()> {
        crate::horse::codex::check_delivery_file(
            path.context("no Codex rollout available for delivery")?,
        )
    }

    fn user_row_holds(raw: &[u8], text: &str) -> bool {
        let Ok(v) = serde_json::from_slice::<Value>(raw) else {
            return false;
        };
        let want = text.trim();
        v["type"] == "response_item"
            && v["payload"]["type"] == "message"
            && v["payload"]["role"] == "user"
            && v["payload"]["content"].as_array().is_some_and(|c| {
                c.iter().any(|x| {
                    x["text"]
                        .as_str()
                        .is_some_and(|t| t.trim() == want || (!want.is_empty() && t.contains(want)))
                })
            })
    }
}
