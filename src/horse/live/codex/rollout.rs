//! The rollout files of the threads in custody.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The rollout file of a thread under the sessions folder
/// (`YYYY/MM/DD/rollout-<time>-<id>.jsonl`).
pub fn find_rollout(root: &Path, id: &str) -> Option<PathBuf> {
    let tail = format!("-{id}.jsonl");
    let dirs = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p)
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    let mut found = None;
    for y in dirs(root).into_iter().filter(|p| p.is_dir()) {
        for m in dirs(&y).into_iter().filter(|p| p.is_dir()) {
            for d in dirs(&m).into_iter().filter(|p| p.is_dir()) {
                for f in dirs(&d) {
                    let n = f.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    if n.starts_with("rollout-") && n.ends_with(&tail) && f.is_file() {
                        found = Some(f);
                    }
                }
            }
        }
    }
    found
}

/// A rollout row is a message item with this text.
pub(super) fn holds(raw: &[u8], text: &str) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(raw) else {
        return false;
    };
    v["type"] == "response_item"
        && v["payload"]["type"] == "message"
        && v["payload"]["content"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x["text"].as_str() == Some(text)))
}

/// Check the parent rollout and stored history before delivery.
pub(super) fn check_delivery(
    store: &crate::horse::Store,
    root: &Path,
    thread: Option<&str>,
    session: &str,
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let thread = thread.context("no parent thread is open")?;
    let path = find_rollout(root, thread).context("no Codex rollout available for delivery")?;
    crate::horse::codex::check_delivery_file(&path)?;
    crate::horse::codex::check_mutation_session(store, session)
}
