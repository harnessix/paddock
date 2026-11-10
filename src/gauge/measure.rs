//! Measure a feature from its snapshots: per task when it appeared, when it
//! became done, how often it went back, how long it sat almost done; and
//! per day how many tasks finished and how many arrived.

use super::history::Snapshot;
use serde::Serialize;
use std::collections::BTreeMap;

pub const DAY: u64 = 86_400;
pub const HOUR: u64 = 3600;
/// The fewest steps of a window shorter than its days.
pub const MIN_STEPS: u64 = 6;

/// The life of one task in the history.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Timeline {
    pub id: String,
    pub first_seen: u64,
    /// It was not in the first snapshot: discovered during the work.
    pub discovered: bool,
    /// It appeared already done (a list rewritten after the work).
    pub added_done: bool,
    /// The last time it became done, while it is done now.
    pub done_at: Option<u64>,
    /// Every time it became done.
    pub done_events: Vec<u64>,
    /// Every time it went from done back to open.
    pub reopen_events: Vec<u64>,
    /// It left the list, and when.
    pub removed_at: Option<u64>,
    pub done: bool,
    /// The state of its newest snapshot.
    pub state: Option<String>,
    pub branch: Option<String>,
    /// Seconds in review: done by its worker, not accepted yet.
    pub review_secs: u64,
    /// In review now, since this time.
    pub review_since: Option<u64>,
}

/// One change of the open work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A task became done.
    Finished,
    /// A task appeared after the start.
    Discovered,
    /// A done task went back to open.
    Reopened,
    /// An open task left the list.
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub at: u64,
    pub task: usize,
    pub change: Change,
}

/// Everything the history says.
#[derive(Debug, Clone, Default)]
pub struct Measured {
    pub start: u64,
    pub now: u64,
    pub tasks: Vec<Timeline>,
    pub events: Vec<Event>,
}

fn in_review(state: Option<&str>) -> bool {
    state == Some("review")
}

/// Walk the snapshots (sorted by time; they may be empty) up to `now`.
pub fn measure(snaps: &[Snapshot], now: u64) -> Measured {
    let mut m = Measured {
        start: snaps.first().map_or(now, |s| s.at.min(now)),
        now,
        ..Default::default()
    };
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut prev_at = m.start;
    let mut first = true;
    for s in snaps.iter().filter(|s| s.at <= now) {
        // The review time up to this snapshot.
        for t in m.tasks.iter_mut() {
            if t.removed_at.is_none() && in_review(t.state.as_deref()) {
                t.review_secs += s.at.saturating_sub(prev_at);
            }
        }
        for st in &s.tasks {
            // A task that appears already checked was done outside the
            // list: it is neither new open work nor a finish.
            let added_done = !first && st.checked;
            let i = *index.entry(st.id.clone()).or_insert_with(|| {
                m.tasks.push(Timeline {
                    id: st.id.clone(),
                    first_seen: s.at,
                    discovered: !first && !st.checked,
                    added_done,
                    done: added_done,
                    done_at: added_done.then_some(s.at),
                    ..Default::default()
                });
                if !first && !st.checked {
                    m.events.push(Event {
                        at: s.at,
                        task: m.tasks.len() - 1,
                        change: Change::Discovered,
                    });
                }
                m.tasks.len() - 1
            });
            let t = &mut m.tasks[i];
            if t.removed_at.take().is_some() {
                // Back in the list: it arrives again.
                m.events.push(Event {
                    at: s.at,
                    task: i,
                    change: Change::Discovered,
                });
            }
            if st.checked && !t.done {
                t.done_at = Some(s.at);
                // Done in the first snapshot: finished before the history
                // starts, at a time nobody knows.
                if !first {
                    t.done_events.push(s.at);
                    m.events.push(Event {
                        at: s.at,
                        task: i,
                        change: Change::Finished,
                    });
                }
            } else if !st.checked && t.done {
                t.reopen_events.push(s.at);
                t.done_at = None;
                m.events.push(Event {
                    at: s.at,
                    task: i,
                    change: Change::Reopened,
                });
            }
            t.done = st.checked;
            let review = in_review(st.state.as_deref());
            if review && !in_review(t.state.as_deref()) {
                t.review_since = Some(s.at);
            } else if !review {
                t.review_since = None;
            }
            if st.state.is_some() || !first {
                t.state = st.state.clone();
            }
            if st.branch.is_some() {
                t.branch = st.branch.clone();
            }
        }
        for (i, t) in m.tasks.iter_mut().enumerate() {
            if t.removed_at.is_none() && s.task(&t.id).is_none() {
                t.removed_at = Some(s.at);
                t.review_since = None;
                if !t.done {
                    m.events.push(Event {
                        at: s.at,
                        task: i,
                        change: Change::Removed,
                    });
                }
            }
        }
        prev_at = s.at;
        first = false;
    }
    for t in m.tasks.iter_mut() {
        if t.removed_at.is_none() && in_review(t.state.as_deref()) {
            t.review_secs += now.saturating_sub(prev_at);
        }
    }
    m
}

/// Counts per step over the window before `now`. Bucket 0 is the oldest.
/// A step is one day. A history shorter than the window is cut into equal
/// steps instead (at least six of at least an hour, near one day each for
/// a long history), so that no step covers time before the start.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Window {
    /// The number of steps.
    pub days: usize,
    /// Seconds per bucket.
    pub width: u64,
    pub from: u64,
    pub finished: Vec<i64>,
    pub discovered: Vec<i64>,
    pub reopened: Vec<i64>,
    pub removed: Vec<i64>,
}

impl Window {
    /// The window: the last `max_days` days, or the whole history when it
    /// is shorter. `counts` says whether a finish event counts (the
    /// evidence rule).
    pub fn of(m: &Measured, max_days: usize, counts: &dyn Fn(&Event) -> bool) -> Window {
        let max_days = max_days.max(1);
        let span = m.now.saturating_sub(m.start);
        let (days, width) = if span >= max_days as u64 * DAY {
            (max_days, DAY)
        } else {
            // At least six steps, so that the forecast has a spread, of at
            // least an hour each, and near one day for a long history.
            let n = ((span + DAY / 2) / DAY).max(MIN_STEPS);
            let width = (span / n).max(HOUR);
            ((span / width).max(1) as usize, width)
        };
        let from = m.now.saturating_sub(days as u64 * width);
        let mut w = Window {
            days,
            width,
            from,
            finished: vec![0; days],
            discovered: vec![0; days],
            reopened: vec![0; days],
            removed: vec![0; days],
        };
        for e in &m.events {
            if e.at < from || e.at > m.now || (e.at == from && from > m.start) {
                continue;
            }
            let back = ((m.now - e.at) / width) as usize;
            let b = days - 1 - back.min(days - 1);
            match e.change {
                Change::Finished if counts(e) => w.finished[b] += 1,
                Change::Finished => {}
                Change::Discovered => w.discovered[b] += 1,
                Change::Reopened => w.reopened[b] += 1,
                Change::Removed => w.removed[b] += 1,
            }
        }
        w
    }

    /// The length of the window in days.
    pub fn span_days(&self) -> f64 {
        (self.days as u64 * self.width) as f64 / DAY as f64
    }

    pub fn total(v: &[i64]) -> i64 {
        v.iter().sum()
    }

    /// New open work per day: discovered and reopened, less removed.
    pub fn arrivals(&self) -> Vec<i64> {
        (0..self.days)
            .map(|i| self.discovered[i] + self.reopened[i] - self.removed[i])
            .collect()
    }

    pub fn per_day(&self, v: &[i64]) -> f64 {
        Self::total(v) as f64 / self.span_days()
    }
}
