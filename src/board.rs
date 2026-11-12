//! `paddock board`: every project, ticket and agent run at a glance.
//!
//! - A project is the repository of a run (a worktree counts as its main
//!   work tree). Its tickets are the files `.paddock/tickets/*.md` of its
//!   loft (tickets.rs).
//! - A run belongs to a ticket when its prompt file is that ticket. paddock
//!   records the prompt file when the run starts (origin.rs).
//! - The state rules are pure functions of the facts of a run (rules.rs).
//!   collect.rs reads the facts from the run records, the mailboxes, the
//!   approval cards and the kv leases.
//! - `--json` prints `Board` (version 1). The Omarchy panel reads it, so
//!   the field names and the state words do not change within a version.
//!   A new optional field is additive and keeps the version: a reader of
//!   version 1 must ignore fields that it does not know. `Project::loft`
//!   is such a field.

pub mod collect;
pub mod origin;
pub mod rules;
pub mod text;
pub mod tickets;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub use collect::collect;

/// The `version` of the JSON contract.
pub const VERSION: u32 = 1;

/// The state of a run or a ticket. The order is the urgency: the first
/// state needs the user most. A run is never `Open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// A card or a question waits for the user.
    Waiting,
    /// Its last status starts with `blocked:`.
    Blocked,
    /// Idle with unread mail, or the same screen for longer than its limit.
    Stuck,
    Working,
    /// At its prompt, nothing new.
    Idle,
    /// A ticket with a `Status:` line and no run.
    Open,
    /// Its last status starts with `done:`.
    Done,
    Ended,
}

impl State {
    pub const ALL: [State; 8] = [
        State::Waiting,
        State::Blocked,
        State::Stuck,
        State::Working,
        State::Idle,
        State::Open,
        State::Done,
        State::Ended,
    ];

    pub fn name(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Blocked => "blocked",
            State::Stuck => "stuck",
            State::Working => "working",
            State::Idle => "idle",
            State::Open => "open",
            State::Done => "done",
            State::Ended => "ended",
        }
    }

    /// A shape for each state, so the text does not depend on color.
    pub fn glyph(self) -> &'static str {
        match self {
            State::Waiting => "◆",
            State::Blocked => "■",
            State::Stuck => "▲",
            State::Working => "●",
            State::Idle => "○",
            State::Open => "◇",
            State::Done => "✓",
            State::Ended => "·",
        }
    }
}

/// `paddock board --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub version: u32,
    pub approvals_waiting: usize,
    pub compile_slots: Vec<Slot>,
    pub projects: Vec<Project>,
}

/// A held compile slot: the kv lease `compile-slot-N` of the user scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub slot: u32,
    pub holder: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    /// The absolute notes folder of the project's loft
    /// (`<data>/loft/<key>/tree/.paddock`), with or without a `.paddock`
    /// link in the project. Absent when the loft has no such folder, or
    /// when `plain_path` rejects its path. A loft-only
    /// ticket file is at `<loft>/tickets/NAME.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loft: Option<String>,
    pub tickets: Vec<Ticket>,
    pub runs_without_ticket: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ticket {
    pub id: String,
    pub title: String,
    /// Relative to the project: `.paddock/tickets/NAME.md`.
    pub file: String,
    pub state: State,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// The short id, for `paddock view ID`.
    pub id: String,
    pub harness: String,
    /// Empty when paddock did not choose the model.
    pub model: String,
    pub state: State,
    /// The last thing the run said: a card, a question, a status, a message.
    pub last: String,
    /// When the current state began, RFC 3339 in UTC.
    pub since: String,
    /// A card of this run waits in the panel.
    pub card: bool,
    /// Who approves the run's steps: `auto`, `manual` or `command`. Absent
    /// for an ended run.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approvals: String,
}

/// A message of a run: its time and its text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Said {
    pub ts: u64,
    pub text: String,
}

/// What paddock knows about one run. collect.rs fills it; the rules read it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facts {
    /// The full sandbox id.
    pub id: String,
    pub harness: String,
    pub model: String,
    /// The repository of the run.
    pub repo: PathBuf,
    pub started: u64,
    /// When the run ended. None while it runs.
    pub ended: Option<u64>,
    /// The last `status` message.
    pub status: Option<Said>,
    /// The last `say` message of the agent.
    pub say: Option<Said>,
    /// Approval cards and questions of the run that wait for the user.
    pub cards: Vec<Said>,
    /// The oldest `ask` mail of the agent without an answer.
    pub ask: Option<Said>,
    pub idle_since: Option<u64>,
    /// The screen stayed the same since then (`[detach] stuck_after`).
    pub stuck_since: Option<u64>,
    /// Down mail that the agent has not read.
    pub unread_mail: bool,
    /// The done text of the run end.
    pub end_text: Option<String>,
    pub origin: Option<origin::Origin>,
    /// The approvals mode of a running run (`Running::approvals`). Empty
    /// for an ended one: its record is gone.
    pub approvals: String,
}

/// Put the facts together. Pure: the same input gives the same board.
pub fn build(
    facts: &[Facts],
    tickets: &BTreeMap<PathBuf, Vec<tickets::TicketFile>>,
    approvals_waiting: usize,
    mut compile_slots: Vec<Slot>,
) -> Board {
    compile_slots.sort_by_key(|s| s.slot);
    // A lease holder is text that an agent chose.
    for s in &mut compile_slots {
        s.holder = rules::one_line(&s.holder);
    }
    let mut repos: Vec<PathBuf> = facts.iter().map(|f| f.repo.clone()).collect();
    repos.sort();
    repos.dedup();
    let none = Vec::new();
    let projects = repos
        .into_iter()
        .map(|repo| {
            let files = tickets.get(&repo).unwrap_or(&none);
            let mut by_ticket: Vec<Vec<(State, u64, Run)>> = vec![Vec::new(); files.len()];
            let mut loose = Vec::new();
            for f in facts.iter().filter(|f| f.repo == repo) {
                let (state, since) = rules::run_state(f);
                let run = Run {
                    // Text that a run or its starter chose: on one line,
                    // without escapes or invisible characters.
                    id: rules::one_line(crate::ids::short(&f.id)),
                    harness: rules::one_line(&f.harness),
                    model: rules::one_line(&f.model),
                    state,
                    last: rules::last(f, state),
                    since: crate::waiting::format_rfc3339(since),
                    card: !f.cards.is_empty(),
                    approvals: rules::one_line(&f.approvals),
                };
                match f.origin.as_ref().and_then(|o| tickets::owner(o, files)) {
                    Some(i) => by_ticket[i].push((state, since, run)),
                    None => loose.push((state, since, run)),
                }
            }
            let mut rows: Vec<Ticket> = files
                .iter()
                .zip(by_ticket)
                .filter_map(|(t, mut runs)| {
                    if runs.is_empty() && t.status.is_none() {
                        return None;
                    }
                    rules::order_runs(&mut runs);
                    let runs: Vec<Run> = runs.into_iter().map(|(_, _, r)| r).collect();
                    Some(Ticket {
                        id: t.id.clone(),
                        title: t.title.clone(),
                        file: t.file.clone(),
                        state: rules::ticket_state(runs.iter().map(|r| r.state)),
                        runs,
                    })
                })
                .collect();
            rows.sort_by(|a, b| (a.state, &a.id, &a.file).cmp(&(b.state, &b.id, &b.file)));
            rules::order_runs(&mut loose);
            Project {
                // The name is only shown, so it is cleaned. The path stays
                // exact: a reader opens it, and to_json escapes it.
                name: rules::one_line(
                    &repo
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| repo.display().to_string()),
                ),
                path: repo.display().to_string(),
                loft: None,
                tickets: rows,
                runs_without_ticket: loose.into_iter().map(|(_, _, r)| r).collect(),
            }
        })
        .collect();
    Board {
        version: VERSION,
        approvals_waiting,
        compile_slots,
        projects,
    }
}

/// The loft's notes folder of a repository (`Loft::notes()`,
/// `<data>/loft/<key>/tree/.paddock`), with or without a `.paddock` link in
/// the project. None when that folder does not exist. A path that
/// `plain_path` rejects is left out with a warning, never cleaned: a
/// cleaned path would name another folder. The board never fails on it.
pub fn loft_of(env: &crate::env::HostEnv, repo: &std::path::Path) -> Option<String> {
    let notes = crate::loft::Loft::of(env, repo).notes();
    if !notes.is_dir() {
        return None;
    }
    let real = std::fs::canonicalize(&notes).ok()?;
    let text = real.to_str().filter(|t| plain_path(t));
    if text.is_none() {
        log::warn!(
            "the loft folder of a project has control or invisible characters, \
             or is over {LOFT_PATH_MAX} characters; board --json leaves out its loft field"
        );
    }
    text.map(str::to_string)
}

/// The longest loft path that the board gives, in UTF-16 code units as the
/// panel counts it (JavaScript `length`): a character outside the Basic
/// Multilingual Plane counts as two.
pub const LOFT_PATH_MAX: usize = 1024;

/// Whether a loft path may go to `board --json`. The panel uses the same set
/// (`Board.loftDir`), so both sides list and open the same tickets. Rejected:
/// a path over `LOFT_PATH_MAX`, control characters (U+0000-U+001F,
/// U+007F-U+009F), U+00AD, U+061C, U+180E, U+200B-U+200F, U+2028, U+2029,
/// U+202A-U+202E, U+2060-U+206F, U+FEFF and U+FFF9-U+FFFB.
pub fn plain_path(s: &str) -> bool {
    s.chars().count() <= LOFT_PATH_MAX && !s.chars().any(|c| c.is_control() || invisible(c))
}

/// The invisible format characters that `plain_path` rejects and that
/// `to_json` escapes: U+00AD, U+061C, U+180E, U+200B-U+200F, U+2028,
/// U+2029, U+202A-U+202E, U+2060-U+206F, U+FEFF and U+FFF9-U+FFFB.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
        | '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// `paddock board --json`: one line, or pretty. The paths `path` and `file`
/// keep their exact characters, because a reader opens them. serde_json
/// writes DEL, the C1 controls (U+0080-U+009F, with the 8-bit CSI U+009B)
/// and the invisible characters as they are, so a terminal would run them
/// or reorder the line. Here each of them is a `\uXXXX` escape: the JSON
/// value stays the same, and the output holds no such character.
pub fn to_json(board: &Board, pretty: bool) -> serde_json::Result<String> {
    let json = if pretty {
        serde_json::to_string_pretty(board)?
    } else {
        serde_json::to_string(board)?
    };
    Ok(escape_unsafe(&json))
}

/// Escape DEL, C1 and the invisible characters of a JSON text. serde_json
/// writes only ASCII outside strings, so each of them is inside a string,
/// where a `\uXXXX` escape stands for the same character. Every one is in
/// the Basic Multilingual Plane: one escape, no surrogate pair. A newline
/// outside a string is the layout of pretty JSON and stays.
fn escape_unsafe(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c == '\u{7F}' || ('\u{80}'..='\u{9F}').contains(&c) || invisible(c) {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The Status line of a ticket without runs, for the text view.
pub fn open_status<'a>(
    tickets: &'a BTreeMap<PathBuf, Vec<tickets::TicketFile>>,
    project: &Project,
    file: &str,
) -> Option<&'a str> {
    tickets
        .get(&PathBuf::from(&project.path))?
        .iter()
        .find(|t| t.file == file)?
        .status
        .as_deref()
}
