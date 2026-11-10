//! `paddock gauge`: how far a Spec Kit feature is, and when it ends.
//!
//! It measures without asking any agent: done with evidence (evidence.rs),
//! tasks discovered per day, reopened tasks, fails after done, the age of
//! open tasks and the time tasks sit almost done (in review, or a worker
//! blocked). From the finish rate and the arrival rate of the last days it
//! forecasts the finish date and the final task count (forecast.rs), says
//! "not converging" when new tasks come as fast as tasks finish, and shows
//! the trend of P85.
//!
//! The parts:
//!
//! - history.rs: snapshots of the task list over time (loft git, repository
//!   git, paddock's state log, the current file).
//! - measure.rs: timelines and per-day counts from the snapshots.
//! - evidence.rs: the rule for done with evidence, and the gate results.
//! - forecast.rs: the seeded Monte Carlo.
//! - appetite.rs: the time budget of a feature, and its warnings.
//! - collect.rs: reads all of it for one project.
//! - render.rs: the text and the burn-up chart.
//!
//! `analyze` is pure: the same input and seed give the same report.

pub mod appetite;
pub mod collect;
pub mod evidence;
pub mod forecast;
pub mod history;
pub mod measure;
pub mod render;

use crate::speckit::board;
use evidence::{Gate, Status, Tests};
use forecast::{Forecast, Verdict};
use history::{Snapshot, Source};
use measure::{Change, Measured, Window, DAY};
use serde::Serialize;
use std::collections::BTreeMap;

pub const API_VERSION: &str = "paddock.gauge/v1";
/// The default window of the rates, in days.
pub const WINDOW_DAYS: usize = 7;

/// What a worker cost, from the mailbox and the registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effort {
    /// Seconds the last worker of the task ran.
    pub agent_secs: u64,
    /// Seconds that worker reported `blocked`.
    pub blocked_secs: u64,
}

/// Everything that `analyze` reads.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub feature: String,
    pub now: u64,
    /// Sorted by time. The last one is the current state.
    pub snapshots: Vec<Snapshot>,
    /// The history sources that gave snapshots, besides the current file.
    pub sources: Vec<Source>,
    /// The task records that paddock stored (none for tasks it never ran).
    pub records: BTreeMap<String, board::Record>,
    pub gates: Vec<Gate>,
    /// Per task: its work is merged (true), or not (false). Missing: paddock
    /// cannot tell.
    pub merged: BTreeMap<String, bool>,
    /// The project is a git repository, so merges are visible.
    pub git: bool,
    pub effort: BTreeMap<String, Effort>,
    /// The start of the feature when no history shows it (file times).
    pub start_hint: Option<u64>,
    pub appetite: Option<appetite::Appetite>,
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub seed: u64,
    pub trials: usize,
    pub window_days: usize,
    /// Count only tasks with evidence as finished.
    pub strict: bool,
    /// Measure as at this past time: later history is left out, and
    /// nothing is written or sent.
    pub at: Option<u64>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            seed: forecast::SEED,
            trials: forecast::TRIALS,
            window_days: WINDOW_DAYS,
            strict: false,
            at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskRow {
    pub id: String,
    pub text: String,
    /// paddock's state: todo, running, review, done, ...
    pub state: String,
    pub done: bool,
    /// For a done task: the verdict of the evidence rule.
    pub evidence: Option<Status>,
    pub why: Vec<String>,
    pub tests: Tests,
    pub first_seen: u64,
    pub discovered: bool,
    pub done_at: Option<u64>,
    /// Seconds since it appeared, for an open task.
    pub age_secs: Option<u64>,
    /// Seconds in review, plus seconds its worker was blocked.
    pub almost_secs: u64,
    pub in_review_since: Option<u64>,
    pub reopened: usize,
    pub fails_after_done: usize,
    pub agent_secs: Option<u64>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Counts {
    pub scope: i64,
    pub done: i64,
    pub proven: i64,
    pub claimed: i64,
    pub skipped_only: i64,
    pub failed_after_done: i64,
    /// Not finished in the forecast's sense.
    pub open: i64,
    pub discovered: i64,
    pub reopened: i64,
    pub removed: i64,
    pub fails_after_done: i64,
    pub partly_skipped: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrendPoint {
    pub at: u64,
    pub verdict: Verdict,
    pub p50_at: Option<u64>,
    pub p85_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppetiteView {
    pub text: String,
    pub secs: u64,
    pub used_secs: u64,
    pub pct: f64,
    /// P85 as a share of the appetite.
    pub p85_pct: Option<f64>,
    pub warned: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BurnPoint {
    pub at: u64,
    pub scope: i64,
    pub done: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub api_version: &'static str,
    pub feature: String,
    pub now: u64,
    pub start: u64,
    /// The start comes from a history, not from file times.
    pub start_known: bool,
    pub sources: Vec<&'static str>,
    pub snapshots: usize,
    /// The list was rewritten then; the measures start there.
    pub rewritten_at: Option<u64>,
    pub strict: bool,
    pub counts: Counts,
    pub tasks: Vec<TaskRow>,
    pub window: Window,
    pub forecast: Forecast,
    pub trend: Vec<TrendPoint>,
    /// `narrowing`, `widening`, `steady`, `slipping`, `not converging now`
    /// or `unknown`.
    pub trend_verdict: &'static str,
    /// Open work did not go down while most tasks were done.
    pub near_end_stall: Option<String>,
    pub appetite: Option<AppetiteView>,
    pub burnup: Vec<BurnPoint>,
    /// What paddock cannot know for this project, in plain words.
    pub cannot_know: Vec<String>,
}

/// The evidence verdict of every task that is done now.
fn judge_all(i: &Input, m: &Measured) -> BTreeMap<String, (Status, Vec<String>, Tests)> {
    let mut out = BTreeMap::new();
    let current = i.snapshots.last();
    for t in &m.tasks {
        let Some(st) = current.and_then(|s| s.task(&t.id)) else {
            continue;
        };
        if !st.checked {
            continue;
        }
        let r = i.records.get(&t.id);
        let mut report = String::new();
        if let Some(p) = r.and_then(|r| r.report.as_ref()) {
            report.push_str(&p.text);
            report.push('\n');
        }
        if let Some(res) = r.and_then(|r| r.result.as_deref()) {
            report.push_str(res);
        }
        let seen = evidence::Seen {
            merged: i.merged.get(&t.id).copied(),
            reviewed: r.is_some_and(|r| r.verdict.as_deref() == Some("pass")),
            report: &report,
            gates: i.gates.iter().filter(|g| g.task == t.id).collect(),
            done_at: t.done_at,
        };
        let (s, why) = evidence::judge(&seen);
        out.insert(t.id.clone(), (s, why, evidence::tests_of(&report)));
    }
    out
}

/// The core of one forecast at `now` over `snaps`.
fn core(
    snaps: &[Snapshot],
    now: u64,
    statuses: &BTreeMap<String, (Status, Vec<String>, Tests)>,
    o: &Opts,
) -> (Measured, Window, Forecast) {
    let m = measure::measure(snaps, now);
    let finished_now = |id: &str| {
        statuses
            .get(id)
            .is_none_or(|(s, _, _)| s.finished(o.strict))
    };
    let counts = |e: &measure::Event| {
        let t = &m.tasks[e.task];
        !(t.done && !finished_now(&t.id))
    };
    let w = Window::of(&m, o.window_days, &counts);
    let live = m.tasks.iter().filter(|t| t.removed_at.is_none());
    let scope = live.clone().count() as i64;
    let finished = live.filter(|t| t.done && finished_now(&t.id)).count() as i64;
    let finished_total = m
        .events
        .iter()
        .filter(|e| e.change == Change::Finished && counts(e))
        .count() as i64
        + m.tasks
            .iter()
            .filter(|t| t.done && t.done_events.is_empty() && finished_now(&t.id))
            .count() as i64;
    let f = forecast::run(
        &w,
        scope - finished,
        scope,
        finished_total,
        now,
        o.seed,
        o.trials,
    );
    (m, w, f)
}

/// Measure and forecast one feature of `project`, and send the appetite
/// warnings that are due.
pub fn gauge(
    env: &crate::env::HostEnv,
    project: &std::path::Path,
    feature: Option<&str>,
    o: &Opts,
) -> anyhow::Result<Report> {
    let real = crate::state::now();
    let now = o.at.unwrap_or(real).min(real);
    let live = now == real;
    let (input, layout) = collect::collect(env, project, feature, now, live)?;
    let report = analyze(&input, o);
    if let (Some(a), true) = (&report.appetite, live) {
        let run = crate::speckit::flow::newest_tree(
            env,
            &crate::speckit::repo_of(project),
            &report.feature,
            now,
        );
        appetite::warn(
            env,
            &collect::Files::of(&layout, &report.feature).appetite(),
            run.as_deref(),
            &report.feature,
            a.pct,
            &render::forecast_line(&report),
        );
    }
    Ok(report)
}

/// After a task ended: send the appetite warnings of a feature that has an
/// appetite. Without one this costs one file check.
pub fn check_appetite(env: &crate::env::HostEnv, project: &std::path::Path, feature: &str) {
    let Some(layout) = crate::speckit::Layout::resolve(env, project) else {
        return;
    };
    if !collect::Files::of(&layout, feature).appetite().is_file() {
        return;
    }
    let o = Opts {
        trials: 2000,
        ..Opts::default()
    };
    if let Err(e) = gauge(env, project, Some(feature), &o) {
        log::debug!("appetite check of {feature}: {e:#}");
    }
}

/// The snapshot where the list was rewritten: more tasks appeared at once
/// than it had before, and at least `REWRITE_MIN`. The newest one wins.
pub fn rewrite(snaps: &[Snapshot]) -> Option<usize> {
    (1..snaps.len()).rev().find(|&k| {
        let before = &snaps[k - 1];
        let new = snaps[k]
            .tasks
            .iter()
            .filter(|t| before.task(&t.id).is_none())
            .count();
        new >= REWRITE_MIN && new > before.tasks.len()
    })
}

/// The fewest new tasks at once that count as a rewrite of the list.
pub const REWRITE_MIN: usize = 5;

pub fn analyze(full: &Input, o: &Opts) -> Report {
    // After a rewrite of the list, measure from the rewrite: tasks of the
    // old list are no work that arrived or finished.
    let rewritten = rewrite(&full.snapshots).map(|k| (k, full.snapshots[k].at));
    let rebased;
    let i = match rewritten {
        Some((k, _)) => {
            let mut r = full.clone();
            r.snapshots.drain(..k);
            rebased = r;
            &rebased
        }
        None => full,
    };
    let now = i.now;
    let m0 = measure::measure(&i.snapshots, now);
    let statuses = judge_all(i, &m0);
    let (m, w, f) = core(&i.snapshots, now, &statuses, o);
    let has_history = !i.sources.is_empty();
    let start = match (has_history, i.start_hint) {
        (false, Some(h)) => h.min(m.start),
        _ => m.start,
    };
    let mut rows = Vec::new();
    let mut c = Counts::default();
    for t in m.tasks.iter().filter(|t| t.removed_at.is_none()) {
        let r = i.records.get(&t.id);
        let st = statuses.get(&t.id);
        let effort = i.effort.get(&t.id);
        let fails = i
            .gates
            .iter()
            .filter(|g| g.task == t.id && g.outcome == evidence::Outcome::Fail)
            .filter(|g| {
                t.done_events
                    .first()
                    .or(t.done_at.as_ref())
                    .is_some_and(|d| g.at >= *d)
            })
            .count();
        let almost = t.review_secs + effort.map_or(0, |e| e.blocked_secs);
        let state = t
            .state
            .clone()
            .or_else(|| r.map(|r| r.state.name().to_string()))
            .unwrap_or_else(|| if t.done { "done" } else { "todo" }.into());
        c.scope += 1;
        if t.done {
            c.done += 1;
        }
        match st.map(|s| s.0) {
            Some(Status::Proven) => c.proven += 1,
            Some(Status::Claimed) => c.claimed += 1,
            Some(Status::SkippedOnly) => c.skipped_only += 1,
            Some(Status::FailedAfterDone) => c.failed_after_done += 1,
            None => {}
        }
        if st.is_some_and(|s| s.2 == Tests::PartlySkipped && s.0 == Status::Proven) {
            c.partly_skipped += 1;
        }
        let finished = t.done && st.is_none_or(|s| s.0.finished(o.strict));
        if !finished {
            c.open += 1;
        }
        c.discovered += t.discovered as i64;
        c.reopened += t.reopen_events.len() as i64;
        c.fails_after_done += fails as i64;
        rows.push(TaskRow {
            id: t.id.clone(),
            text: r.map(|r| r.text.clone()).unwrap_or_default(),
            state,
            done: t.done,
            evidence: st.map(|s| s.0),
            why: st.map(|s| s.1.clone()).unwrap_or_default(),
            tests: st.map_or(Tests::Unknown, |s| s.2),
            first_seen: t.first_seen,
            discovered: t.discovered,
            done_at: t.done_at,
            age_secs: (!finished).then(|| now.saturating_sub(t.first_seen.max(start))),
            almost_secs: almost,
            in_review_since: t.review_since,
            reopened: t.reopen_events.len(),
            fails_after_done: fails,
            agent_secs: effort.map(|e| e.agent_secs),
            branch: t.branch.clone(),
        });
    }
    c.removed = m.tasks.iter().filter(|t| t.removed_at.is_some()).count() as i64;
    let trend = trend(i, &statuses, o, start);
    let trend_verdict = trend_verdict(&trend);
    let appetite = i.appetite.as_ref().map(|a| {
        let used = now.saturating_sub(start);
        AppetiteView {
            text: a.text.clone(),
            secs: a.secs,
            used_secs: used,
            pct: appetite::share(a, start, now),
            p85_pct: f
                .p85
                .and_then(|p| p.at)
                .map(|at| appetite::share(a, start, at)),
            warned: a.warned.clone(),
        }
    });
    let burnup = full
        .snapshots
        .iter()
        .filter(|s| s.at <= now)
        .map(|s| BurnPoint {
            at: s.at,
            scope: s.tasks.len() as i64,
            done: s.tasks.iter().filter(|t| t.checked).count() as i64,
        })
        .collect();
    Report {
        api_version: API_VERSION,
        feature: i.feature.clone(),
        now,
        start,
        start_known: has_history,
        sources: i.sources.iter().map(|s| s.name()).collect(),
        snapshots: full.snapshots.len(),
        rewritten_at: rewritten.map(|(_, at)| at),
        strict: o.strict,
        near_end_stall: near_end_stall(i, &statuses, o, &w),
        counts: c,
        cannot_know: {
            let mut k = cannot_know(i, has_history);
            if let Some((_, at)) = rewritten {
                k.insert(0, format!("tasks.md was rewritten at {}: more tasks appeared at once than it had before, so gauge measures from there", render::when(at)));
            }
            k
        },
        tasks: rows,
        window: w,
        forecast: f,
        trend,
        trend_verdict,
        appetite,
        burnup,
    }
}

/// The forecast as it was at the end of each of the last days.
fn trend(
    i: &Input,
    statuses: &BTreeMap<String, (Status, Vec<String>, Tests)>,
    o: &Opts,
    start: u64,
) -> Vec<TrendPoint> {
    let mut out = Vec::new();
    // One point a day, or five over a history shorter than five days.
    let span = i.now.saturating_sub(start);
    let step = if span >= 5 * DAY {
        DAY
    } else {
        (span / 5).max(measure::HOUR)
    };
    for k in (0..5u64).rev() {
        let at = i.now.saturating_sub(k * step);
        if at < start + step / 2 && k > 0 {
            continue;
        }
        let snaps: Vec<Snapshot> = i.snapshots.iter().filter(|s| s.at <= at).cloned().collect();
        if snaps.is_empty() {
            continue;
        }
        let (_, _, f) = core(&snaps, at, statuses, o);
        out.push(TrendPoint {
            at,
            verdict: f.verdict,
            p50_at: f.p50.and_then(|p| p.at),
            p85_at: f.p85.and_then(|p| p.at),
        });
    }
    out
}

fn trend_verdict(t: &[TrendPoint]) -> &'static str {
    let dated: Vec<(u64, u64, u64)> = t
        .iter()
        .filter_map(|p| Some((p.at, p.p50_at?, p.p85_at?)))
        .collect();
    if t.last()
        .is_some_and(|p| p.verdict == Verdict::NotConverging)
    {
        return "not converging now";
    }
    let (Some(first), Some(last)) = (dated.first(), dated.last()) else {
        return "unknown";
    };
    if dated.len() < 2 {
        return "unknown";
    }
    let spread = |p: &(u64, u64, u64)| p.2.saturating_sub(p.1) as f64;
    if last.2 > first.2 + DAY && dated.windows(2).all(|w| w[1].2 >= w[0].2) {
        return "slipping";
    }
    if spread(last) > spread(first) * 1.25 + (DAY / 4) as f64 {
        "widening"
    } else if spread(last) < spread(first) * 0.8 {
        "narrowing"
    } else {
        "steady"
    }
}

/// The 90 % syndrome: most tasks were done at the start of the window, and
/// the open count did not go down since.
fn near_end_stall(
    i: &Input,
    statuses: &BTreeMap<String, (Status, Vec<String>, Tests)>,
    o: &Opts,
    w: &Window,
) -> Option<String> {
    if w.span_days() < 2.0 {
        return None;
    }
    let open_at = |at: u64| -> Option<(i64, i64)> {
        let snaps: Vec<Snapshot> = i.snapshots.iter().filter(|s| s.at <= at).cloned().collect();
        snaps.last()?;
        let (m, _, f) = core(&snaps, at, statuses, o);
        let _ = m;
        Some((f.open, f.scope))
    };
    let (open0, scope0) = open_at(w.from)?;
    let (open1, scope1) = open_at(i.now)?;
    if scope0 == 0 || open1 == 0 {
        return None;
    }
    let share0 = 100 * (scope0 - open0) / scope0;
    (share0 >= 80 && open1 >= open0).then(|| {
        format!(
            "{share0} % of the tasks were done {:.0} days ago, and the open count did not go down since ({open0} then, {open1} now, {} tasks more in scope)",
            w.span_days(),
            scope1 - scope0
        )
    })
}

fn cannot_know(i: &Input, has_history: bool) -> Vec<String> {
    let mut out = Vec::new();
    if !has_history {
        out.push("tasks.md has no history here (no loft history, git does not track it, and paddock's state log is empty): tasks discovered per day, reopened tasks and the scope line start now. paddock keeps a state log from now on, so the next runs know more.".into());
        if i.start_hint.is_some() {
            out.push("the start of the feature is a guess from the times of its files".into());
        }
    } else if i.sources == [Source::StateLog] {
        out.push(
            "the history starts with paddock's first state log line, not with the feature".into(),
        );
    }
    if !i.git {
        out.push("this project is no git repository: paddock cannot see merged commits, so only review passes and gate results are evidence".into());
    }
    if i.records.values().any(|r| r.attempts > 1) {
        out.push(
            "agent hours count the last worker of each task; earlier attempts are not recorded"
                .into(),
        );
    }
    out.push("quota per task is not measured yet".into());
    out
}

#[cfg(test)]
mod tests;
