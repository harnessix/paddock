//! The history of a feature's task list: snapshots of `tasks.md` and of
//! the task states over time. Sources, best first:
//!
//! - the loft's own git history (external store), one version per run end;
//! - the repository's git history, when git tracks `tasks.md`;
//! - the state log `history.jsonl` next to the task records. paddock
//!   appends a line each time a synchronization changes a task's state, so
//!   a project without git gets a history from its first paddock run on;
//! - the current `tasks.md`, always the last snapshot.
//!
//! The snapshots of all sources merge into one list sorted by time. The
//! model is shared with the flow features: a snapshot says which tasks
//! existed at a time, which were checked, and which state paddock showed.

use crate::speckit::{board, tasks};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;
use std::time::Duration;

/// The largest state log that paddock still appends to.
pub const MAX_LOG: u64 = 4 << 20;
/// The most git versions of `tasks.md` that gauge reads.
pub const MAX_VERSIONS: usize = 2000;

/// One task in one snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapTask {
    pub id: String,
    pub checked: bool,
    /// The state that paddock showed: `review`, `running`, `failed`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// Where a snapshot came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    LoftGit,
    RepoGit,
    StateLog,
    Current,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::LoftGit => "loft git history",
            Source::RepoGit => "repository git history",
            Source::StateLog => "paddock's task state log",
            Source::Current => "the current tasks.md",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub at: u64,
    pub source: Source,
    pub tasks: Vec<SnapTask>,
}

impl Snapshot {
    pub fn task(&self, id: &str) -> Option<&SnapTask> {
        self.tasks.iter().find(|t| t.id == id)
    }
}

/// The state word and the branch of a paddock note (`review, BRANCH`,
/// `changes asked, BRANCH`, `BRANCH`).
pub fn parse_note(note: &str) -> (Option<String>, Option<String>) {
    let note = note.trim();
    if note.is_empty() {
        return (None, None);
    }
    let (state, branch) = match note.rsplit_once(", ") {
        Some((s, b)) => (Some(s.trim()), b.trim()),
        None if note.contains(' ') => (Some(note), ""),
        None if is_state(note) => (Some(note), ""),
        None => (None, note),
    };
    let state = state.map(|s| match s {
        "changes asked" => "todo".to_string(),
        s => s.to_string(),
    });
    let branch = (!branch.is_empty()).then(|| branch.to_string());
    (state, branch)
}

fn is_state(word: &str) -> bool {
    matches!(
        word,
        "todo" | "claimed" | "running" | "done" | "failed" | "review"
    )
}

/// The tasks of one version of `tasks.md`.
pub fn from_text(text: &str) -> Vec<SnapTask> {
    let doc = tasks::parse(text);
    let lines: Vec<&str> = text.lines().collect();
    doc.tasks
        .iter()
        .map(|t| {
            let note = lines.get(t.line).and_then(|l| tasks::strip_note(l).1);
            let (state, branch) = note.map(parse_note).unwrap_or_default();
            SnapTask {
                id: t.id.clone(),
                checked: t.checked,
                state,
                branch,
            }
        })
        .collect()
}

/// The tasks of the records, as a snapshot of the state log.
pub fn from_records(records: &BTreeMap<String, board::Record>) -> Vec<SnapTask> {
    records
        .values()
        .filter(|r| !r.removed)
        .map(|r| SnapTask {
            id: r.id.clone(),
            checked: r.state == board::State::Done,
            state: Some(r.state.name().to_string()),
            branch: r.branch.clone(),
        })
        .collect()
}

/// One line of the state log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub at: u64,
    pub tasks: Vec<SnapTask>,
}

/// The last line of a file, read from its end.
fn last_line(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(256 * 1024);
    f.seek(std::io::SeekFrom::Start(from)).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    s.lines()
        .rfind(|l| !l.trim().is_empty())
        .map(str::to_string)
}

/// Append the task states to the log when they differ from its last line.
/// A failure costs only the history, never the synchronization.
pub fn log_states(path: &Path, tasks: Vec<SnapTask>, now: u64) {
    if tasks.is_empty() {
        return;
    }
    let last: Option<LogLine> = last_line(path).and_then(|l| serde_json::from_str(&l).ok());
    if last.is_some_and(|l| l.tasks == tasks) {
        return;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG) {
        log::debug!("{} is full; no more task history", path.display());
        return;
    }
    let line = match serde_json::to_string(&LogLine { at: now, tasks }) {
        Ok(l) => l,
        Err(_) => return,
    };
    let res = path
        .parent()
        .map_or(Ok(()), crate::privfs::dir)
        .and_then(|()| {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            writeln!(f, "{line}")?;
            Ok(())
        });
    if let Err(e) = res {
        log::debug!("cannot write the task history {}: {e:#}", path.display());
    }
}

/// The snapshots of the state log. Bad lines are skipped.
pub fn read_log(path: &Path) -> Vec<Snapshot> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<LogLine>(l).ok())
        .map(|l| Snapshot {
            at: l.at,
            source: Source::StateLog,
            tasks: l.tasks,
        })
        .collect()
}

/// The versions of `rel` in a git history: `(commit time, text)`, oldest
/// first. `git` makes a hardened command for that repository.
pub fn git_versions(git: &dyn Fn() -> std::process::Command, rel: &str) -> Vec<(u64, String)> {
    let run = |args: &[&str]| -> Option<String> {
        let mut c = git();
        c.args(args);
        let (code, out, _) = crate::broker::run_capture(&mut c, Duration::from_secs(60)).ok()?;
        (code == 0).then_some(out)
    };
    let Some(log) = run(&[
        "log",
        "--format=%H %ct",
        &format!("-n{MAX_VERSIONS}"),
        "--",
        rel,
    ]) else {
        return Vec::new();
    };
    let mut out: Vec<(u64, String)> = log
        .lines()
        .filter_map(|l| {
            let (h, t) = l.split_once(' ')?;
            let text = run(&["show", &format!("{h}:{rel}")])?;
            Some((t.trim().parse().ok()?, text))
        })
        .collect();
    out.reverse();
    out
}

/// Merge snapshots of several sources: sorted by time, and a snapshot that
/// repeats the one before it is dropped. At one time the later source wins.
pub fn merge(mut all: Vec<Snapshot>) -> Vec<Snapshot> {
    all.sort_by_key(|s| (s.at, s.source));
    let mut out: Vec<Snapshot> = Vec::new();
    for s in all {
        if out.last().is_some_and(|l| same(l, &s)) {
            continue;
        }
        out.push(s);
    }
    out
}

/// `b` tells nothing new after `a`: the same tasks and boxes, and no state
/// that `a` does not show already.
fn same(a: &Snapshot, b: &Snapshot) -> bool {
    a.tasks.len() == b.tasks.len()
        && b.tasks.iter().all(|u| {
            a.task(&u.id).is_some_and(|t| {
                u.checked == t.checked && (u.state.is_none() || u.state == t.state)
            })
        })
}
