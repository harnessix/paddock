//! The forecast: a Monte Carlo over the finish rate and the arrival rate.
//! Each trial walks forward one step at a time. Per step it draws the
//! number of finished tasks and the number of arriving tasks (discovered,
//! reopened, less removed) from one random past step of the window, until
//! no open task is left. The trials give P50 and P85 of the finish date and
//! of the final task count.
//!
//! The random numbers come from a fixed generator (SplitMix64) with a seed,
//! so one history and one seed always give the same forecast.

use super::measure::{Window, DAY};
use serde::Serialize;

pub const TRIALS: usize = 10_000;
/// The default seed.
pub const SEED: u64 = 1;
/// A trial that does not finish within this many steps never finishes.
pub const MAX_STEPS: usize = 730;
/// Fewer finished tasks than this: an early forecast with a wide range.
pub const EARLY: i64 = 5;

/// SplitMix64: small, fast, and the same on every platform.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`, n > 0.
    pub fn below(&mut self, n: usize) -> usize {
        (((self.next_u64() >> 11) as u128 * n as u128) >> 53) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Every task is done.
    Done,
    /// Tasks finish faster than new ones arrive: a date is shown.
    Converging,
    /// New tasks arrive at least as fast as tasks finish.
    NotConverging,
    /// Nothing finished and nothing arrived in the window.
    NoActivity,
}

/// One percentile of the trials.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    /// The finish time (Unix seconds). None: not within `MAX_STEPS` steps.
    pub at: Option<u64>,
    pub days: Option<f64>,
    /// The number of tasks of the feature at its end.
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forecast {
    pub verdict: Verdict,
    pub seed: u64,
    pub trials: usize,
    pub open: i64,
    pub scope: i64,
    pub finish_per_day: f64,
    pub arrival_per_day: f64,
    pub p50: Option<Point>,
    pub p85: Option<Point>,
    /// Fewer than `EARLY` tasks finished, or fewer than three steps of
    /// history: `low` and `high` give the wide range to show instead of
    /// P50 and P85.
    pub early: bool,
    pub low: Option<Point>,
    pub high: Option<Point>,
}

/// The percentile `p` (0 to 100) of sorted values.
fn pick<T: Copy>(sorted: &[T], p: usize) -> T {
    let i = (sorted.len() * p).div_ceil(100).saturating_sub(1);
    sorted[i.min(sorted.len() - 1)]
}

/// Forecast `open` tasks of `scope` from the window, at `now`.
/// `finished_total` counts all finished tasks of the history, for the
/// early label.
pub fn run(
    w: &Window,
    open: i64,
    scope: i64,
    finished_total: i64,
    now: u64,
    seed: u64,
    trials: usize,
) -> Forecast {
    let finished = Window::total(&w.finished);
    let arrivals = w.arrivals();
    let arrived = Window::total(&arrivals);
    let mut f = Forecast {
        verdict: Verdict::Converging,
        seed,
        trials,
        open,
        scope,
        finish_per_day: w.per_day(&w.finished),
        arrival_per_day: w.per_day(&arrivals),
        p50: None,
        p85: None,
        // One or two steps of history give no spread either.
        early: finished_total < EARLY || w.days < 3,
        low: None,
        high: None,
    };
    if open <= 0 {
        f.verdict = Verdict::Done;
        return f;
    }
    if finished == 0 && arrived == 0 {
        f.verdict = Verdict::NoActivity;
        return f;
    }
    if arrived >= finished {
        f.verdict = Verdict::NotConverging;
        return f;
    }
    let mut rng = Rng::new(seed);
    let n = w.days;
    // (steps, final total); steps None: did not finish.
    let mut runs: Vec<(Option<usize>, i64)> = Vec::with_capacity(trials);
    for _ in 0..trials.max(1) {
        let (mut left, mut total) = (open, scope);
        let mut steps = None;
        for step in 1..=MAX_STEPS {
            let d = rng.below(n);
            let a = arrivals[rng.below(n)];
            left += a - w.finished[d];
            total += a;
            if left <= 0 {
                // Removed tasks never take the count below the done ones.
                total = total.max(scope - open);
                steps = Some(step);
                break;
            }
        }
        runs.push((steps, total));
    }
    let mut steps: Vec<usize> = runs.iter().map(|(s, _)| s.unwrap_or(usize::MAX)).collect();
    steps.sort_unstable();
    let mut totals: Vec<i64> = runs.iter().map(|(_, t)| *t).collect();
    totals.sort_unstable();
    let point = |p: usize| {
        let s = pick(&steps, p);
        let secs = (s != usize::MAX).then(|| s as u64 * w.width);
        Point {
            at: secs.map(|x| now + x),
            days: secs.map(|x| x as f64 / DAY as f64),
            total: pick(&totals, p),
        }
    };
    f.p50 = Some(point(50));
    f.p85 = Some(point(85));
    if f.early {
        // The cone of uncertainty: early estimates miss by a factor of
        // several. Show at least half to twice P50, and P10 to P95.
        let (p10, p95, p50) = (point(10), point(95), point(50));
        let scale = |p: &Point, k: f64| Point {
            at: p.days.map(|d| now + (d * k * DAY as f64) as u64),
            days: p.days.map(|d| d * k),
            total: p.total,
        };
        let low = if p10.days < scale(&p50, 0.5).days {
            p10
        } else {
            scale(&p50, 0.5)
        };
        let twice = scale(&p50, 2.0);
        let high = match (p95.days, twice.days) {
            (Some(a), Some(b)) if a >= b => p95,
            (None, _) => p95,
            _ => twice,
        };
        f.low = Some(low);
        f.high = Some(Point {
            total: p95.total,
            ..high
        });
    }
    f
}
