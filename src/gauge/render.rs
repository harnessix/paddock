//! The text of `paddock gauge`: the numbers, the forecast and a burn-up
//! chart with the scope line.

use super::appetite::human;
use super::evidence::{Status, Tests};
use super::forecast::{Point, Verdict};
use super::{BurnPoint, Report, TaskRow};
use std::fmt::Write as _;

/// The chart size in characters.
const WIDTH: usize = 56;
const HEIGHT: usize = 10;

/// `2026-11-08 14:05` in the local time zone.
pub fn when(t: u64) -> String {
    crate::storage::local_time(t)
}

fn day(t: u64) -> String {
    when(t)[5..10].to_string()
}

/// A trend point: the day, or the time for points within one day.
fn mark(t: u64, short: bool) -> String {
    if short {
        when(t)[11..16].to_string()
    } else {
        day(t)
    }
}

/// The length of a window: in hours under two days.
pub fn span(w: &super::measure::Window) -> String {
    let d = w.span_days();
    if d < 2.0 {
        format!("{:.0} hours", d * 24.0)
    } else {
        format!("{d:.1} days")
    }
}

fn ids<'a>(rows: impl Iterator<Item = &'a TaskRow>) -> String {
    let v: Vec<&str> = rows.map(|r| r.id.as_str()).collect();
    if v.is_empty() {
        "none".into()
    } else {
        v.join(", ")
    }
}

fn point(p: &Point, now: u64) -> String {
    match (p.at, p.days) {
        (Some(at), Some(d)) => format!(
            "{} (in {}, {} tasks)",
            when(at),
            if d < 1.0 {
                human(at - now)
            } else {
                format!("{d:.1} days")
            },
            p.total
        ),
        _ => format!("not within {} days", super::forecast::MAX_STEPS),
    }
}

/// One line about the forecast, for the text and for mailbox warnings.
pub fn forecast_line(r: &Report) -> String {
    let f = &r.forecast;
    match f.verdict {
        Verdict::Done => "Every task is done.".into(),
        Verdict::NoActivity => format!(
            "No forecast: no task finished and none arrived in the last {}.",
            span(&r.window)
        ),
        Verdict::NotConverging => format!(
            "Not converging: {:.1} tasks arrive a day and {:.1} finish a day, so the open work does not shrink and no finish date exists.",
            f.arrival_per_day, f.finish_per_day
        ),
        Verdict::Converging if f.early => match (f.low, f.high) {
            (Some(lo), Some(hi)) => format!(
                "Early forecast ({}, no reference data yet), a wide range: between {} and {}.",
                if r.window.days < 3 {
                    "less than three steps of history".to_string()
                } else {
                    format!("fewer than {} finished tasks", super::forecast::EARLY)
                },
                point(&lo, r.now),
                point(&hi, r.now)
            ),
            _ => "Early forecast: not enough data.".into(),
        },
        Verdict::Converging => match (f.p50, f.p85) {
            (Some(a), Some(b)) => format!(
                "P50 {}, P85 {}.",
                point(&a, r.now),
                point(&b, r.now)
            ),
            _ => "No forecast.".into(),
        },
    }
}

pub fn text(r: &Report) -> String {
    let mut o = String::new();
    let c = &r.counts;
    let history = if r.sources.is_empty() {
        "no history".to_string()
    } else {
        r.sources.join(", ")
    };
    let _ = writeln!(
        o,
        "feature {}: {} snapshots since {}{} ({history})",
        r.feature,
        r.snapshots,
        when(r.start),
        if r.start_known { "" } else { " (a guess)" }
    );
    let _ = writeln!(o);
    let not_done = c.skipped_only + c.failed_after_done;
    let _ = writeln!(
        o,
        "done         {} of {} tasks: {} with evidence, {} checked without evidence, {} not done",
        c.done, c.scope, c.proven, c.claimed, not_done
    );
    let by = |s: Status| ids(r.tasks.iter().filter(move |t| t.evidence == Some(s)));
    if c.claimed > 0 {
        let _ = writeln!(o, "  no evidence: {}", by(Status::Claimed));
    }
    if c.skipped_only > 0 {
        let _ = writeln!(
            o,
            "  not done, tests only skipped: {}",
            by(Status::SkippedOnly)
        );
    }
    if c.failed_after_done > 0 {
        let _ = writeln!(
            o,
            "  not done, a gate failed after done: {}",
            by(Status::FailedAfterDone)
        );
    }
    if c.partly_skipped > 0 {
        let rows = r
            .tasks
            .iter()
            .filter(|t| t.tests == Tests::PartlySkipped && t.evidence == Some(Status::Proven));
        let _ = writeln!(
            o,
            "  partly skipped (a part waits for a real run): {}",
            ids(rows)
        );
    }
    let w = &r.window;
    let disc = super::measure::Window::total(&w.discovered);
    let _ = writeln!(
        o,
        "discovered   {disc} in the last {} ({:.1} a day), {} since the start",
        span(w),
        w.per_day(&w.discovered),
        c.discovered
    );
    let _ = writeln!(
        o,
        "reopened     {} ({}), fails after done: {}",
        c.reopened,
        ids(r.tasks.iter().filter(|t| t.reopened > 0)),
        c.fails_after_done
    );
    let mut open: Vec<&TaskRow> = r.tasks.iter().filter(|t| t.age_secs.is_some()).collect();
    open.sort_by_key(|t| std::cmp::Reverse(t.age_secs));
    let ages: Vec<String> = open
        .iter()
        .take(8)
        .map(|t| format!("{} {} {}", t.id, t.state, human(t.age_secs.unwrap_or(0))))
        .collect();
    let more = open.len().saturating_sub(8);
    let _ = writeln!(
        o,
        "open         {}{}",
        if ages.is_empty() {
            "none".into()
        } else {
            ages.join(", ")
        },
        if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        }
    );
    let mut almost: Vec<&TaskRow> = r.tasks.iter().filter(|t| t.almost_secs >= 60).collect();
    almost.sort_by_key(|t| std::cmp::Reverse(t.almost_secs));
    let almost: Vec<String> = almost
        .iter()
        .take(8)
        .map(|t| {
            let now = if t.in_review_since.is_some() {
                ", in review now"
            } else {
                ""
            };
            format!("{} {}{now}", t.id, human(t.almost_secs))
        })
        .collect();
    let _ = writeln!(
        o,
        "almost done  {}",
        if almost.is_empty() {
            "none".into()
        } else {
            almost.join(", ")
        }
    );
    let agent: u64 = r.tasks.iter().filter_map(|t| t.agent_secs).sum();
    if agent > 0 {
        let _ = writeln!(
            o,
            "agent time   {} over {} tasks",
            human(agent),
            r.tasks.iter().filter(|t| t.agent_secs.is_some()).count()
        );
    }
    let _ = writeln!(o);
    let f = &r.forecast;
    let _ = writeln!(
        o,
        "rates        {:.1} finished and {:.1} new a day over the last {}{}",
        f.finish_per_day,
        f.arrival_per_day,
        span(w),
        if r.strict {
            " (strict: only tasks with evidence count)"
        } else {
            ""
        }
    );
    let _ = writeln!(o, "forecast     {}", forecast_line(r));
    if f.p50.is_some() {
        let _ = writeln!(o, "             seed {}, {} trials", f.seed, f.trials);
    }
    if !r.trend.is_empty() {
        let short = r.trend.last().map(|p| p.at).unwrap_or(r.now)
            - r.trend.first().map(|p| p.at).unwrap_or(r.now)
            < 2 * super::measure::DAY;
        let day = |t: u64| mark(t, short);
        let pts: Vec<String> = r
            .trend
            .iter()
            .map(|p| match (p.verdict, p.p85_at) {
                (_, Some(at)) if short => format!("{} {}", day(p.at), &when(at)[5..16]),
                (_, Some(at)) => format!("{} {}", day(p.at), day(at)),
                (Verdict::NotConverging, _) => format!("{} not converging", day(p.at)),
                (Verdict::Done, _) => format!("{} done", day(p.at)),
                _ => format!("{} none", day(p.at)),
            })
            .collect();
        let _ = writeln!(o, "P85 trend    {}: {}", pts.join(", "), r.trend_verdict);
    }
    if let Some(a) = &r.appetite {
        let _ = writeln!(
            o,
            "appetite     {}: {} used ({:.0} %){}",
            a.text,
            human(a.used_secs),
            a.pct,
            a.p85_pct
                .map(|p| format!(", P85 at {p:.0} %"))
                .unwrap_or_default()
        );
    }
    if let Some(s) = &r.near_end_stall {
        let _ = writeln!(o, "stuck        {s}");
    }
    let _ = writeln!(o);
    o.push_str(&burnup(r));
    if !r.cannot_know.is_empty() {
        let _ = writeln!(o, "\npaddock cannot know:");
        for l in &r.cannot_know {
            let _ = writeln!(o, "  - {l}");
        }
    }
    o
}

/// The value of the newest point at or before `t`.
fn at(points: &[BurnPoint], t: u64) -> Option<&BurnPoint> {
    points.iter().rev().find(|p| p.at <= t)
}

/// The burn-up: `#` is done work, `-` is the scope line on top of it. The
/// columns after "now" mark P50 and P85.
pub fn burnup(r: &Report) -> String {
    let pts = &r.burnup;
    if pts.len() < 2 {
        return "burn-up: needs a history; there is one snapshot only\n".into();
    }
    let p50 = r.forecast.p50.and_then(|p| p.at);
    let p85 = r.forecast.p85.and_then(|p| p.at);
    let span = r.now.saturating_sub(r.start).max(3600);
    let end = p85
        .or(p50)
        .unwrap_or(r.now)
        .max(r.now)
        .min(r.now + 2 * span);
    let len = end.saturating_sub(r.start).max(1);
    let col_at =
        |c: usize| r.start + (len as u128 * (2 * c as u128 + 1) / (2 * WIDTH as u128)) as u64;
    let col_of = |t: u64| {
        ((t.saturating_sub(r.start) as u128 * WIDTH as u128 / len as u128) as usize).min(WIDTH - 1)
    };
    let top = pts
        .iter()
        .map(|p| p.scope)
        .chain(r.forecast.p85.map(|p| p.total))
        .max()
        .unwrap_or(1)
        .max(1);
    let level = |v: i64| ((v as f64 * HEIGHT as f64 / top as f64).round() as usize).min(HEIGHT);
    let now_col = col_of(r.now);
    let marks: Vec<usize> = [p50, p85].iter().flatten().map(|t| col_of(*t)).collect();
    let mut o = String::from("burn-up (# done, - scope)\n");
    for row in (0..HEIGHT).rev() {
        let label = match row {
            r0 if r0 == HEIGHT - 1 => format!("{top:>4} |"),
            0 => "   0 |".to_string(),
            _ => "     |".to_string(),
        };
        o.push_str(&label);
        for c in 0..WIDTH {
            let t = col_at(c);
            let ch = if c <= now_col {
                let t = if c == now_col { r.now } else { t.min(r.now) };
                match at(pts, t) {
                    Some(p) => {
                        let (hd, hs) = (level(p.done), level(p.scope));
                        if row < hd {
                            '#'
                        } else if hs > 0 && row == hs - 1 {
                            '-'
                        } else {
                            ' '
                        }
                    }
                    None => ' ',
                }
            } else if marks.contains(&c) {
                ':'
            } else {
                ' '
            };
            o.push(ch);
        }
        o.push('\n');
    }
    o.push_str("     +");
    o.push_str(&"-".repeat(WIDTH));
    o.push('\n');
    let mut axis = vec![' '; WIDTH + 12];
    let mut put = |col: usize, s: &str| {
        for (k, ch) in s.chars().enumerate() {
            if let Some(x) = axis.get_mut(col + k) {
                *x = ch;
            }
        }
    };
    put(0, &day(r.start));
    put(now_col.max(6), "now");
    if let Some(t) = p50 {
        put(col_of(t).max(now_col + 4), "P50");
    }
    if let Some(t) = p85 {
        let c = col_of(t).max(now_col + 8);
        put(c, "P85");
    }
    o.push_str("      ");
    o.push_str(axis.iter().collect::<String>().trim_end());
    o.push('\n');
    o
}
