//! `paddock macvm bench`: the cases, the percentiles, the targets and the
//! committed baseline. This file is the pure part with Linux tests;
//! `bench_run` drives a session on a Mac. The targets live in
//! `packaging/macos/bench/targets.json`, one source for the script and the
//! test, and the committed reference run in
//! `packaging/macos/bench/baseline.json`.

use super::events::Event;
use super::proto::{ErrorCode, Reply, Request};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The targets of the bench, as the repository holds them.
pub const TARGETS_JSON: &str = include_str!("../../packaging/macos/bench/targets.json");
/// The committed reference run.
pub const BASELINE_JSON: &str = include_str!("../../packaging/macos/bench/baseline.json");

/// The environment variable that lets a release build run the bench.
pub const ALLOW_VAR: &str = "PADDOCK_MACVM_BENCH";
/// `PADDOCK_MACVM_BENCH_TARGET_<MEASURE>=N` lowers (or raises) every
/// bound of one measure, for the test of the script's exit code.
pub const TARGET_VAR_PREFIX: &str = "PADDOCK_MACVM_BENCH_TARGET_";

/// The cases of the bench. One command each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    Warm,
    Cold,
    First,
    Quit,
    Update,
    Input,
    Move,
    Frames,
    Idle,
    Video,
    Disk,
}

impl Case {
    pub const ALL: [Case; 11] = [
        Case::Warm,
        Case::Cold,
        Case::First,
        Case::Quit,
        Case::Update,
        Case::Input,
        Case::Move,
        Case::Frames,
        Case::Idle,
        Case::Video,
        Case::Disk,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Case::Warm => "warm",
            Case::Cold => "cold",
            Case::First => "first",
            Case::Quit => "quit",
            Case::Update => "update",
            Case::Input => "input",
            Case::Move => "move",
            Case::Frames => "frames",
            Case::Idle => "idle",
            Case::Video => "video",
            Case::Disk => "disk",
        }
    }

    pub fn parse(word: &str) -> Option<Case> {
        Case::ALL.into_iter().find(|c| c.name() == word)
    }

    /// The measures the case produces: one, or three for `idle`.
    pub fn measures(self) -> &'static [&'static str] {
        match self {
            Case::Warm => &["warm"],
            Case::Cold => &["cold"],
            Case::First => &["first"],
            Case::Quit => &["quit"],
            Case::Update => &["update"],
            Case::Input => &["input"],
            Case::Move => &["move"],
            Case::Frames => &["frames"],
            Case::Idle => &["idle.cpu", "idle.ram", "idle.load"],
            Case::Video => &["video"],
            Case::Disk => &["disk"],
        }
    }

    /// The unit of the first measure of the case.
    pub fn unit(self) -> &'static str {
        match self {
            Case::Warm | Case::Cold | Case::First | Case::Quit | Case::Update => "s",
            Case::Input | Case::Move | Case::Frames => "ms",
            Case::Idle | Case::Video => "pct",
            Case::Disk => "gib",
        }
    }

    /// The runs of the case without `--runs`: five for the event cases.
    pub fn default_runs(self) -> u32 {
        match self {
            Case::Warm | Case::Cold | Case::First | Case::Quit | Case::Update => 5,
            _ => 1,
        }
    }

    /// The names, for a usage line.
    pub fn names() -> String {
        Case::ALL
            .iter()
            .map(|c| c.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One measured number set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    /// The measure name; in a run's map it is the key.
    #[serde(default)]
    pub name: String,
    pub unit: String,
    pub p50: f64,
    pub p95: f64,
    pub mean: f64,
    pub max: f64,
    pub samples: usize,
}

/// The value at percentile `p` (0 to 100) of `sorted`, nearest rank.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

impl Measure {
    /// The statistics of `values`. Empty values give zeros with no samples.
    pub fn from_samples(name: &str, unit: &str, values: &[f64]) -> Measure {
        let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        let n = sorted.len();
        let mean = if n == 0 {
            0.0
        } else {
            sorted.iter().sum::<f64>() / n as f64
        };
        Measure {
            name: name.into(),
            unit: unit.into(),
            p50: round3(percentile(&sorted, 50.0)),
            p95: round3(percentile(&sorted, 95.0)),
            mean: round3(mean),
            max: round3(sorted.last().copied().unwrap_or(0.0)),
            samples: n,
        }
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// One target of `targets.json`: each bound that is set must hold.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Target {
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p50: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    /// The least number of samples.
    #[serde(default)]
    pub samples: usize,
}

/// The targets by measure name, in name order.
pub type Targets = BTreeMap<String, Target>;

/// The targets of a JSON text. Keys that start with `_` are notes.
pub fn parse_targets(json: &str) -> anyhow::Result<Targets> {
    let raw: BTreeMap<String, serde_json::Value> = serde_json::from_str(json)?;
    let mut out = Targets::new();
    for (k, v) in raw {
        if k.starts_with('_') {
            continue;
        }
        let t: Target = serde_json::from_value(v)
            .map_err(|e| anyhow::anyhow!(super::text::bench_target_invalid(&k, &e)))?;
        out.insert(k, t);
    }
    Ok(out)
}

/// The targets of the repository.
pub fn targets() -> Targets {
    parse_targets(TARGETS_JSON).expect("targets.json parses: bench::targets_cover_every_case")
}

/// `PADDOCK_MACVM_BENCH_TARGET_WARM=1` sets every bound of `warm` to 1;
/// a dot of the measure name is an underscore (`IDLE_CPU`). A value that
/// is not a number is ignored. The names of the measures changed come
/// back, for the script's line.
pub fn apply_env_targets(
    targets: &mut Targets,
    vars: impl Iterator<Item = (String, String)>,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (k, v) in vars {
        let Some(rest) = k.strip_prefix(TARGET_VAR_PREFIX) else {
            continue;
        };
        let Ok(n) = v.trim().parse::<f64>() else {
            continue;
        };
        let name = rest.to_ascii_lowercase().replace('_', ".");
        if let Some(t) = targets.get_mut(&name) {
            if t.p50.is_some() {
                t.p50 = Some(n);
            }
            if t.p95.is_some() {
                t.p95 = Some(n);
            }
            if t.mean.is_some() {
                t.mean = Some(n);
            }
            changed.push(name);
        }
    }
    changed
}

/// One run of the script: the Mac and the measures.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Run {
    /// The run comes from a Mac. False in the placeholder baseline.
    #[serde(default)]
    pub measured: bool,
    pub model: String,
    pub macos: String,
    pub paddock: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub measures: BTreeMap<String, Measure>,
}

impl Run {
    pub fn parse(json: &str) -> anyhow::Result<Run> {
        let mut run: Run = serde_json::from_str(json)?;
        for (k, m) in run.measures.iter_mut() {
            m.name = k.clone();
        }
        Ok(run)
    }

    pub fn add(&mut self, m: Measure) {
        self.measures.insert(m.name.clone(), m);
    }
}

/// A missed target or a missing measure.
#[derive(Debug, Clone, PartialEq)]
pub struct Miss {
    pub measure: String,
    /// `p50`, `p95`, `mean`, `samples` or `missing`.
    pub stat: &'static str,
    pub got: f64,
    pub want: f64,
    pub unit: String,
}

impl std::fmt::Display for Miss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.stat {
            "missing" => write!(f, "{}: no measure", self.measure),
            "samples" => write!(
                f,
                "{}: {} samples, the target needs {}",
                self.measure, self.got, self.want
            ),
            stat => write!(
                f,
                "{} {stat} {} {} is over the target {} {}",
                self.measure, self.got, self.unit, self.want, self.unit
            ),
        }
    }
}

/// Every target that `measures` misses, in target order. A target
/// without a measure is a miss too, so a case that did not run fails.
pub fn check(targets: &Targets, measures: &BTreeMap<String, Measure>) -> Vec<Miss> {
    let mut out = Vec::new();
    for (name, t) in targets {
        let Some(m) = measures.get(name) else {
            out.push(Miss {
                measure: name.clone(),
                stat: "missing",
                got: 0.0,
                want: 0.0,
                unit: t.unit.clone(),
            });
            continue;
        };
        let mut miss = |stat, got: f64, want: f64| {
            out.push(Miss {
                measure: name.clone(),
                stat,
                got,
                want,
                unit: t.unit.clone(),
            })
        };
        if m.samples < t.samples {
            miss("samples", m.samples as f64, t.samples as f64);
        }
        for (stat, got, want) in [
            ("p50", m.p50, t.p50),
            ("p95", m.p95, t.p95),
            ("mean", m.mean, t.mean),
        ] {
            if let Some(w) = want {
                if got > w {
                    miss(stat, got, w);
                }
            }
        }
    }
    out
}

/// The misses of a measure set against one case's measures only.
pub fn check_case(
    case: Case,
    targets: &Targets,
    measures: &BTreeMap<String, Measure>,
) -> Vec<Miss> {
    let names = case.measures();
    let subset: Targets = targets
        .iter()
        .filter(|(k, _)| names.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    check(&subset, measures)
}

/// The table of a run: one line per target, with the bounds and `ok`
/// or `MISS`.
pub fn table(run: &Run, targets: &Targets) -> String {
    let mut s = super::text::bench_table_header();
    let misses = check(targets, &run.measures);
    for (name, t) in targets {
        let want = [("p50", t.p50), ("p95", t.p95), ("mean", t.mean)]
            .iter()
            .filter_map(|(k, v)| v.map(|v| format!("{k}<={v}"))) // not user text
            .collect::<Vec<_>>()
            .join(" ");
        let result = match misses.iter().find(|m| &m.measure == name) {
            None => "ok".to_string(),
            Some(m) => super::text::bench_miss_line(m),
        };
        s.push_str(&super::text::bench_table_row(
            name,
            run.measures.get(name),
            &t.unit,
            &want,
            &result,
        ));
    }
    s
}

/// The bench runs in a development build, or with `PADDOCK_MACVM_BENCH=1`.
pub fn allowed(dev_build: bool, var: Option<&str>) -> Result<(), String> {
    if dev_build || var == Some("1") {
        Ok(())
    } else {
        Err(super::text::bench_release_refused())
    }
}

/// The requests that only a `--bench` session honors.
pub fn is_bench_op(r: &Request) -> bool {
    matches!(
        r,
        Request::BenchAnimate { .. }
            | Request::BenchText { .. }
            | Request::BenchHang
            | Request::BenchSpawnChild { .. }
            | Request::BenchPasteboard { .. }
            | Request::BenchEnv
            | Request::BenchDockMenu
            | Request::BenchLock
            | Request::BenchKeychain
    )
}

/// The guard of the agent's dispatch: the reply that refuses a bench op
/// when the session did not start with `--bench`, None when the request
/// goes on. `ax.watch` is not gated: the Mac checks of other features use
/// it.
pub fn op_gate(req: u64, r: &Request, bench: bool) -> Option<Reply> {
    if is_bench_op(r) && !bench {
        Some(Reply::error(
            req,
            ErrorCode::Unsupported,
            super::text::bench_op_refused(),
        ))
    } else {
        None
    }
}

/// The time of an event in milliseconds since the epoch.
fn at_ms(e: &Event) -> u64 {
    e.secs() * 1000 + u64::from(e.ms)
}

/// A line of the event log that starts or ends a case.
type Pick = fn(&Event) -> bool;

/// The seconds between the last `start` and the first window opened
/// after it (`warm`, `cold`, `first`), between the last `quit` and the
/// `end` after it (`quit`), or between the last agent update line and the
/// `hello` after it (`update`), from the event log of the profile.
pub fn from_events(case: Case, events: &[Event]) -> Result<f64, String> {
    let (from, to): (Pick, Pick) = match case {
        Case::Warm | Case::Cold | Case::First => (
            |e| e.kind == "start",
            |e| e.kind == "window" && e.text == "opened",
        ),
        Case::Quit => (|e| e.kind == "quit", |e| e.kind == "end"),
        Case::Update => (
            |e| e.kind == "agent" && e.text.contains("updates itself"),
            |e| e.kind == "hello",
        ),
        other => return Err(super::text::bench_not_an_event_case(other.name())),
    };
    let start = events
        .iter()
        .rposition(from)
        .ok_or_else(|| super::text::bench_no_start_line(case.name()))?;
    let end = events[start + 1..]
        .iter()
        .find(|e| to(e))
        .ok_or_else(|| super::text::bench_no_end_line(case.name()))?;
    let ms = at_ms(end).saturating_sub(at_ms(&events[start]));
    Ok(ms as f64 / 1000.0)
}

/// The `ms` of every `bench` event `windows applied` (the shim's clock
/// from the arrival of a `windows` event to the end of its frames).
pub fn move_samples(events: &[Event]) -> Vec<f64> {
    events
        .iter()
        .filter(|e| e.kind == "bench" && e.text == "windows applied")
        .filter_map(|e| e.n.get("ms").and_then(|v| v.as_f64()))
        .collect()
}

/// The input latencies in ms: each key `(char, sent at ms)` pairs with
/// the first `ax.value` text `(seen at ms, text)` that ends with the
/// chars typed so far and came after it. A key with no value is dropped.
pub fn pair_input(sent: &[(char, u64)], seen: &[(u64, String)]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut typed = String::new();
    let mut from = 0;
    for (c, t) in sent {
        typed.push(*c);
        let Some(i) = seen[from..]
            .iter()
            .position(|(at, text)| *at >= *t && text.ends_with(typed.as_str()))
        else {
            continue;
        };
        out.push(seen[from + i].0.saturating_sub(*t) as f64);
        from += i;
    }
    out
}

/// The gaps between frame times in ms, for `frames`.
pub fn frame_gaps(times_ms: &[f64]) -> Vec<f64> {
    times_ms.windows(2).map(|w| w[1] - w[0]).collect()
}

/// A `ps -axo pid=,lstart=,comm=` table: the newest process whose
/// command holds `name`, by pid order of the lines that match.
pub fn newest_process(ps_lines: &str, name: &str) -> Option<u32> {
    ps_lines
        .lines()
        .filter(|l| l.contains(name))
        .filter_map(|l| l.split_whitespace().next()?.parse::<u32>().ok())
        .max()
}

/// One `ps -o %cpu=,rss=` line: the percent and the RSS in GiB.
pub fn parse_ps_sample(line: &str) -> Option<(f64, f64)> {
    let mut it = line.split_whitespace();
    let cpu: f64 = it.next()?.parse().ok()?;
    let rss_kib: f64 = it.next()?.parse().ok()?;
    Some((cpu, rss_kib / (1024.0 * 1024.0)))
}

/// `du -sk` output: the first number as GiB.
pub fn parse_du_gib(out: &str) -> Option<f64> {
    let kib: f64 = out.split_whitespace().next()?.parse().ok()?;
    Some(kib / (1024.0 * 1024.0))
}

#[cfg(test)]
#[path = "bench_tests.rs"]
mod tests;
