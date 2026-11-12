//! The state rules of the board. Pure functions, so the tests give them
//! fixture facts and a fixed order.

use super::{Facts, Said, State};

/// The status text starts with this word and a colon.
fn status_is(s: &Said, word: &str) -> bool {
    s.text
        .trim_start()
        .strip_prefix(word)
        .is_some_and(|r| r.starts_with(':'))
}

/// The state of a run and the Unix time when that state began.
///
/// - A run that ended is `done` when its last status was `done:`, else
///   `ended`.
/// - A running run, in this order: `waiting` (a card or an unanswered
///   question), `blocked` (last status `blocked:`), `done` (last status
///   `done:`), `stuck` (the same screen for `[detach] stuck_after`, or idle
///   with unread mail), `idle`, `working`.
pub fn run_state(f: &Facts) -> (State, u64) {
    let status = f.status.as_ref();
    let done = status.filter(|s| status_is(s, "done"));
    if let Some(end) = f.ended {
        return match done {
            Some(s) => (State::Done, s.ts),
            None => (State::Ended, end),
        };
    }
    let waiting = f.cards.iter().chain(f.ask.iter()).map(|s| s.ts).min();
    if let Some(ts) = waiting {
        return (State::Waiting, ts);
    }
    if let Some(s) = status.filter(|s| status_is(s, "blocked")) {
        return (State::Blocked, s.ts);
    }
    if let Some(s) = done {
        return (State::Done, s.ts);
    }
    if let Some(ts) = f.stuck_since {
        return (State::Stuck, ts);
    }
    if let Some(ts) = f.idle_since {
        if f.unread_mail {
            return (State::Stuck, ts);
        }
        return (State::Idle, ts);
    }
    // Working since its last status, or since the start.
    let since = status.map_or(f.started, |s| s.ts.max(f.started));
    (State::Working, since)
}

/// The most urgent state of the runs of a ticket. A ticket without runs
/// is `open`: only a ticket with a `Status:` line has no runs on the board.
pub fn ticket_state(runs: impl IntoIterator<Item = State>) -> State {
    runs.into_iter().min().unwrap_or(State::Open)
}

/// The line that says what the run does now, on one line.
///
/// A waiting run shows what waits (the oldest card or question). Else the
/// newest of the last status and the last message. An ended run without
/// either shows its done text.
pub fn last(f: &Facts, state: State) -> String {
    let pick = if state == State::Waiting {
        f.cards
            .iter()
            .chain(f.ask.iter())
            .min_by_key(|s| s.ts)
            .map(|s| s.text.clone())
    } else {
        None
    };
    let said = || {
        [f.status.as_ref(), f.say.as_ref()]
            .into_iter()
            .flatten()
            .max_by_key(|s| s.ts)
            .map(|s| s.text.clone())
    };
    // A stuck run says why, when it said nothing itself.
    let why = || match state {
        State::Stuck if f.stuck_since.is_some() => Some("the screen does not change".to_string()),
        State::Stuck => Some("idle with unread mail".to_string()),
        _ => None,
    };
    one_line(
        &pick
            .or_else(said)
            .or_else(|| f.end_text.clone())
            .or_else(why)
            .unwrap_or_default(),
    )
}

/// The most urgent runs first; within a state the newest first; then by id.
pub fn order_runs<T: HasId>(runs: &mut [(State, u64, T)]) {
    runs.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then(a.2.key().cmp(b.2.key()))
    });
}

pub trait HasId {
    fn key(&self) -> &str;
}

impl HasId for super::Run {
    fn key(&self) -> &str {
        &self.id
    }
}

/// Agent text on one line: no control characters, no escape sequences, no
/// bidi or zero-width characters, spaces collapsed, at most `MAX_LAST`
/// characters.
pub fn one_line(text: &str) -> String {
    let plain = plain(&text.replace(['\n', '\r', '\t'], " "));
    let mut out = String::new();
    for w in plain.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.chars().count() > MAX_LAST {
        out = out.chars().take(MAX_LAST - 1).collect::<String>() + "…";
    }
    out
}

/// Text made safe to show, at any length: escape sequences and control
/// characters removed (`view::plain_limit`), then the bidi and zero-width
/// characters (`broker::clean_text`, the filter of the approval cards).
/// The escapes go first: removing ESC alone would leave the rest of its
/// sequence as text.
pub fn plain(text: &str) -> String {
    crate::broker::clean_text(&crate::view::plain_limit(text, usize::MAX))
}

/// The longest `last` line in the JSON. The text view cuts it shorter.
pub const MAX_LAST: usize = 200;
