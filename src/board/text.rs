//! The text view of `paddock board`: the waiting approvals and the compile
//! slots at the top, then one block per project with one line per ticket,
//! the most urgent first. Each line is cut to the terminal width.

use super::collect::Snapshot;
use super::{Project, Run, State};

/// A ticket line shows at most this many run ids.
const RUN_IDS: usize = 3;
/// The widest title column.
const TITLE_MAX: usize = 48;
/// A narrow terminal cuts titles to this, at the least.
const TITLE_MIN: usize = 12;
/// The room that a narrow terminal keeps for the last status.
const LAST_MIN: usize = 24;
/// The widest ID column.
const ID_MAX: usize = 12;

/// The ANSI color of a state: the roles of the panel (accent, warning,
/// error, success, muted) in the 16 terminal colors.
fn color(s: State) -> &'static str {
    match s {
        State::Waiting => "33",
        State::Blocked | State::Stuck => "31",
        State::Working => "34",
        State::Done => "32",
        State::Idle | State::Open | State::Ended => "2",
    }
}

/// One row before it is laid out.
struct Row {
    state: State,
    id: String,
    title: String,
    runs: String,
    last: String,
}

pub fn render(snap: &Snapshot, width: Option<usize>, ansi: bool) -> String {
    let b = &snap.board;
    let mut out = Vec::new();
    if b.approvals_waiting > 0 {
        let n = b.approvals_waiting;
        out.push(paint(
            cut(
                &format!(
                    "{} {n} approval{} wait{} for you: paddock approvals",
                    State::Waiting.glyph(),
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "s" } else { "" },
                ),
                width,
            ),
            1,
            color(State::Waiting),
            ansi,
        ));
    }
    if !b.compile_slots.is_empty() {
        let held: Vec<String> = b
            .compile_slots
            .iter()
            .map(|s| format!("{} {}", s.slot, s.holder))
            .collect();
        out.push(cut(&format!("compile slots: {}", held.join(", ")), width));
    }
    if b.projects.is_empty() {
        out.push("No runs on the board.".into());
        return out.join("\n") + "\n";
    }
    for p in &b.projects {
        if !out.is_empty() {
            out.push(String::new());
        }
        let rows = rows(snap, p);
        out.push(cut(
            // The JSON keeps the exact path. Here it is only shown.
            &format!(
                "{} ({})  {}",
                p.name,
                rows.len(),
                super::rules::plain(&p.path)
            ),
            width,
        ));
        let idw = rows
            .iter()
            .map(|r| r.id.chars().count())
            .max()
            .unwrap_or(0)
            .min(ID_MAX);
        let tw = rows
            .iter()
            .map(|r| r.title.chars().count())
            .max()
            .unwrap_or(0)
            .min(TITLE_MAX);
        let rw = rows
            .iter()
            .map(|r| r.runs.chars().count())
            .max()
            .unwrap_or(0);
        // A narrow terminal shortens the titles first, so the last status
        // keeps some room.
        let tw = match width {
            Some(w) => {
                let fixed = 13 + idw + 2 + if rw > 0 { rw + 2 } else { 0 } + 2;
                tw.min(w.saturating_sub(fixed + LAST_MIN).max(TITLE_MIN))
            }
            None => tw,
        };
        for r in &rows {
            let mut line = format!(
                "  {} {:<7}  {}  {}",
                r.state.glyph(),
                r.state.name(),
                pad(&r.id, idw),
                pad(&r.title, tw)
            );
            if rw > 0 {
                line.push_str("  ");
                line.push_str(&pad(&r.runs, rw));
            }
            if !r.last.is_empty() {
                line.push_str("  ");
                line.push_str(&r.last);
            }
            out.push(paint(cut(line.trim_end(), width), 11, color(r.state), ansi));
        }
    }
    out.join("\n") + "\n"
}

fn rows(snap: &Snapshot, p: &Project) -> Vec<Row> {
    let mut rows: Vec<Row> = p
        .tickets
        .iter()
        .map(|t| Row {
            state: t.state,
            id: t.id.clone(),
            title: t.title.clone(),
            runs: run_ids(&t.runs),
            last: match t.runs.first() {
                Some(r) => r.last.clone(),
                None => super::open_status(&snap.tickets, p, &t.file)
                    .unwrap_or_default()
                    .to_string(),
            },
        })
        .collect();
    rows.extend(p.runs_without_ticket.iter().map(|r| Row {
        state: r.state,
        id: r.id.clone(),
        title: if r.model.is_empty() {
            format!("{} (no ticket)", r.harness)
        } else {
            format!("{} {} (no ticket)", r.harness, r.model)
        },
        runs: String::new(),
        last: r.last.clone(),
    }));
    // The most urgent first, also across tickets and runs without one.
    rows.sort_by_key(|r| r.state);
    rows
}

fn run_ids(runs: &[Run]) -> String {
    let mut ids: Vec<&str> = runs.iter().take(RUN_IDS).map(|r| r.id.as_str()).collect();
    let more = runs.len().saturating_sub(RUN_IDS);
    let extra = format!("+{more}");
    if more > 0 {
        ids.push(&extra);
    }
    ids.join(",")
}

/// Pad or cut to exactly `w` characters.
fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        format!("{s}{}", " ".repeat(w - n))
    } else if w == 0 {
        String::new()
    } else {
        s.chars().take(w - 1).collect::<String>() + "…"
    }
}

/// Cut a line to the width. `None`: no terminal, the line stays whole.
pub fn cut(line: &str, width: Option<usize>) -> String {
    match width {
        Some(w) if w > 0 && line.chars().count() > w => {
            line.chars().take(w - 1).collect::<String>() + "…"
        }
        _ => line.to_string(),
    }
}

/// Color the first `n` characters of a line.
fn paint(line: String, n: usize, code: &str, ansi: bool) -> String {
    if !ansi {
        return line;
    }
    let split = line.char_indices().nth(n).map_or(line.len(), |(i, _)| i);
    let (head, tail) = line.split_at(split);
    format!("\x1b[{code}m{head}\x1b[0m{tail}")
}
