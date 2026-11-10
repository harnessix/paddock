//! What counts as done. A checkbox is a claim, not evidence: anyone can
//! check it, and the worker that did the task says it finished. A task is
//! done with evidence only when a party other than its worker saw the
//! result:
//!
//! - its work is merged: its branch, or a commit that its result names, is
//!   in the history of the project's HEAD;
//! - or its reviewer passed it (`paddock-host spec review ID pass`);
//! - and no recorded gate result speaks against it: the newest result of
//!   each gate of the task is `pass`. A gate that only skipped is no
//!   evidence (a test that did not run proves nothing), and a gate that
//!   failed after the task was done is a fail after done.
//!
//! A worker's report that says its tests skipped and names no passing run
//! is the same as a skipped gate. A report that names both is "partly
//! skipped": the task keeps its evidence, and gauge lists it, because the
//! skipped part still waits for a real run.
//!
//! Gate results live in `gates.jsonl` next to the task records. Only the
//! host writes them (`paddock spec gate`), never a worker.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::Path;

/// The result of one gate run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

impl Outcome {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pass" | "passed" => Outcome::Pass,
            "fail" | "failed" => Outcome::Fail,
            "skip" | "skipped" => Outcome::Skip,
            _ => bail!("'{s}' is no gate result: use pass, fail or skip"),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Skip => "skip",
        }
    }
}

/// One line of `gates.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub task: String,
    pub gate: String,
    pub outcome: Outcome,
    pub at: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<String>,
}

impl Gate {
    fn validate_metadata(&self) -> Result<()> {
        match (&self.peer, &self.snapshot_hash) {
            (None, None) => Ok(()),
            (Some(peer), Some(hash)) => {
                if peer.is_empty()
                    || peer.len() > 64
                    || !peer
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    bail!("invalid exclave peer name");
                }
                if !crate::exclave::snapshot::valid_hash(hash) {
                    bail!("snapshot hash must be 64 lowercase hexadecimal digits");
                }
                Ok(())
            }
            _ => bail!("peer and snapshot hash must be supplied together"),
        }
    }
}

/// A gate name: letters, digits, `-`, `_` and `.`, at most 40.
pub fn valid_gate(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn record_gate(path: &Path, g: &Gate) -> Result<()> {
    g.validate_metadata()?;
    if !valid_gate(&g.gate) {
        bail!(
            "'{}' is no gate name: use letters, digits, - _ and .",
            g.gate
        );
    }
    if let Some(dir) = path.parent() {
        crate::privfs::dir(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", serde_json::to_string(g)?)?;
    Ok(())
}

pub fn read_gates(path: &Path) -> Vec<Gate> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Gate>(l).ok())
        .map(|mut gate| {
            // Invalid metadata must never hide a failing gate or create a pass.
            if gate.validate_metadata().is_err() {
                gate.outcome = Outcome::Fail;
                gate.peer = None;
                gate.snapshot_hash = None;
            }
            gate
        })
        .collect()
}

/// What a worker's report says about its tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tests {
    /// The report says nothing about skipped tests.
    Unknown,
    /// Tests skipped, and no passing run is named.
    SkippedOnly,
    /// Some tests skipped, and some ran and passed.
    PartlySkipped,
}

/// Read a report. A word that starts with "skip" (skip, skipped, skips)
/// marks skipped tests; "pass", "passed", "passes", "green", "0 failed"
/// and "0 failures" mark a passing run.
pub fn tests_of(text: &str) -> Tests {
    let lower = text.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let skipped = words.iter().any(|w| w.starts_with("skip"))
        || lower.contains("did not run")
        || lower.contains("not run here");
    if !skipped {
        return Tests::Unknown;
    }
    let passed = words
        .iter()
        .any(|w| matches!(*w, "pass" | "passed" | "passes" | "green"))
        || lower.contains("0 failed")
        || lower.contains("0 failures");
    if passed {
        Tests::PartlySkipped
    } else {
        Tests::SkippedOnly
    }
}

/// The verdict of the rule for one task that is done now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Merged or reviewed, and no gate speaks against it.
    Proven,
    /// Checked, but nobody but the worker saw the result.
    Claimed,
    /// Checked, but its tests only skipped: not done.
    SkippedOnly,
    /// Checked, but a gate failed after it was done: not done.
    FailedAfterDone,
}

impl Status {
    /// The task counts as finished in the forecast. With `strict`, only
    /// proven tasks count.
    pub fn finished(self, strict: bool) -> bool {
        match self {
            Status::Proven => true,
            Status::Claimed => !strict,
            Status::SkippedOnly | Status::FailedAfterDone => false,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Proven => "done with evidence",
            Status::Claimed => "checked without evidence",
            Status::SkippedOnly => "not done: its tests only skipped",
            Status::FailedAfterDone => "not done: a gate failed after it was done",
        }
    }
}

/// The facts about one done task that the rule reads.
#[derive(Debug, Clone, Default)]
pub struct Seen<'a> {
    /// None: paddock cannot tell (no git, no branch, no commit named).
    pub merged: Option<bool>,
    pub reviewed: bool,
    /// The worker's report and result text.
    pub report: &'a str,
    /// The task's gate results, oldest first.
    pub gates: Vec<&'a Gate>,
    /// When the task became done, if the history knows.
    pub done_at: Option<u64>,
}

/// The rule. Returns the status and the reasons in plain words.
pub fn judge(s: &Seen) -> (Status, Vec<String>) {
    let mut why = Vec::new();
    let mut newest: Vec<&Gate> = Vec::new();
    for g in &s.gates {
        match newest.iter_mut().find(|n| n.gate == g.gate) {
            Some(n) if n.at <= g.at => *n = g,
            Some(_) => {}
            None => newest.push(g),
        }
    }
    if let Some(g) = newest.iter().find(|g| g.outcome == Outcome::Fail) {
        if s.done_at.is_none_or(|d| g.at >= d) {
            why.push(format!("gate {} failed", g.gate));
            return (Status::FailedAfterDone, why);
        }
    }
    let passed_gate = newest.iter().any(|g| g.outcome == Outcome::Pass);
    if let Some(g) = newest.iter().find(|g| g.outcome == Outcome::Skip) {
        why.push(format!("gate {} only skipped", g.gate));
        return (Status::SkippedOnly, why);
    }
    match tests_of(s.report) {
        Tests::SkippedOnly if !passed_gate => {
            why.push("the report says its tests skipped and names no passing run".into());
            return (Status::SkippedOnly, why);
        }
        Tests::PartlySkipped if !passed_gate => {
            why.push("the report says some tests skipped".into())
        }
        _ => {}
    }
    if s.merged == Some(true) {
        why.insert(0, "merged".into());
    }
    if s.reviewed {
        why.insert(0, "review passed".into());
    }
    if passed_gate {
        why.insert(0, "gate passed".into());
    }
    if s.merged == Some(true) || s.reviewed || passed_gate {
        (Status::Proven, why)
    } else {
        why.insert(
            0,
            match s.merged {
                Some(false) => "its branch is not merged, and no review passed".into(),
                _ => "no merged commit, review or gate known".into(),
            },
        );
        (Status::Claimed, why)
    }
}

/// Commit ids that a text names: words of 7 to 40 hex digits with at least
/// one letter and one digit.
pub fn commits_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| (7..=40).contains(&w.len()))
        .filter(|w| w.chars().all(|c| c.is_ascii_hexdigit()))
        .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
        .filter(|w| w.chars().any(|c| c.is_ascii_alphabetic()))
        .map(str::to_ascii_lowercase)
        .collect();
    out.dedup();
    out
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn gate_metadata_roundtrip_legacy_and_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gates.jsonl");
        let mut gate = Gate {
            task: "T001".into(),
            gate: "tests".into(),
            outcome: Outcome::Pass,
            at: 1,
            note: String::new(),
            peer: Some("build_1".into()),
            snapshot_hash: Some("a1".repeat(32)),
        };
        record_gate(&path, &gate).unwrap();
        assert_eq!(read_gates(&path), vec![gate.clone()]);
        for (peer, hash) in [
            (Some("bad/name".into()), Some("a".repeat(64))),
            (Some("x".repeat(65)), Some("a".repeat(64))),
            (Some("x".into()), Some("A".repeat(64))),
            (Some("x".into()), Some("a".repeat(63))),
            (Some("x".into()), None),
            (None, Some("a".repeat(64))),
        ] {
            gate.peer = peer;
            gate.snapshot_hash = hash;
            assert!(record_gate(&path, &gate).is_err());
        }
        std::fs::write(&path, concat!(
            "{\"task\":\"T001\",\"gate\":\"tests\",\"outcome\":\"pass\",\"at\":1}\n",
            "{\"task\":\"T002\",\"gate\":\"tests\",\"outcome\":\"pass\",\"at\":2,\"peer\":\"bad/name\",\"snapshot_hash\":\"bad\"}\n"
        )).unwrap();
        let legacy = read_gates(&path);
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy[1].outcome, Outcome::Fail);
        assert_eq!(
            (
                legacy[0].peer.as_deref(),
                legacy[0].snapshot_hash.as_deref()
            ),
            (None, None)
        );
        assert!(!serde_json::to_string(&legacy[0]).unwrap().contains("peer"));
    }
}
