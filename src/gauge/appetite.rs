//! The appetite of a feature: the time that the person wants to spend on
//! it, counted from the start of the feature. paddock only warns, it never
//! refuses work (the user's decision). The warnings come at 50 %, 80 % and
//! 100 % of the appetite and at every 25 % after that, each one once, as a
//! paddock event `appetite` in the mailbox of the run that works on the
//! feature: in its `up` box for the starter, and in its `down` box for the
//! coordinator while the run lives.
//!
//! Stored in `appetite.json` next to the task records.

use crate::env::HostEnv;
use crate::mail::{self, Dir, Message};
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appetite {
    /// Seconds from the start of the feature.
    pub secs: u64,
    /// As the person wrote it, for example `3d`.
    pub text: String,
    pub set_at: u64,
    /// The steps (in percent) whose warning reached a mailbox.
    #[serde(default)]
    pub warned: Vec<u32>,
}

pub fn load(path: &Path) -> Option<Appetite> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn save(path: &Path, a: &Appetite) -> Result<()> {
    if let Some(d) = path.parent() {
        crate::privfs::dir(d)?;
    }
    crate::privfs::write_atomic(path, serde_json::to_vec_pretty(a)?)?;
    Ok(())
}

fn lock(path: &Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd as _;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    // SAFETY: flock on an open file; the lock goes with the file.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(f)
}

/// Parse `3d`, `36h`, `1d12h`, or `off`/`none` (None).
pub fn parse(value: &str) -> Result<Option<u64>> {
    let v = value.trim();
    if matches!(v, "off" | "none" | "0") {
        return Ok(None);
    }
    let secs = crate::timer::parse_duration(v).with_context(|| {
        format!("'{v}' is no appetite: use a time such as 3d, 36h or 1d12h, or off")
    })?;
    anyhow::ensure!(secs >= 60, "an appetite under one minute is no appetite");
    Ok(Some(secs))
}

/// The share used, in percent.
pub fn share(a: &Appetite, start: u64, now: u64) -> f64 {
    now.saturating_sub(start) as f64 * 100.0 / a.secs.max(1) as f64
}

/// The warning steps up to `pct`: 50, 80, 100, 125, 150, ...
pub fn steps(pct: f64) -> Vec<u32> {
    let mut out = Vec::new();
    for s in [50u32, 80] {
        if pct >= s as f64 {
            out.push(s);
        }
    }
    let mut s = 100u32;
    while pct >= s as f64 && s <= 100_000 {
        out.push(s);
        s += 25;
    }
    out
}

/// Short duration text: `2d 4h`, `5h 10m`, `12m`.
pub fn human(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

/// The text of one warning.
pub fn warning_text(feature: &str, a: &Appetite, step: u32, pct: f64, forecast: &str) -> String {
    format!(
        "feature {feature} used {pct:.0} % of its appetite of {} (the {step} % step). {forecast} paddock does not stop the work. Choose one: cut scope (take tasks out of tasks.md), extend the appetite once with a reason (`paddock spec appetite {feature} VALUE`), or stop the feature.",
        a.text
    )
}

/// Send the warnings of the steps that `pct` reached and that no mailbox
/// got yet. `run` is the run that works on the feature. A step counts as
/// warned only when its event is in a mailbox. Returns the steps sent.
pub fn warn(
    env: &HostEnv,
    path: &Path,
    run: Option<&str>,
    feature: &str,
    pct: f64,
    forecast: &str,
) -> Vec<u32> {
    // Two gauges at once must not both warn.
    let Ok(_lock) = lock(path) else {
        return Vec::new();
    };
    let Some(mut a) = load(path) else {
        return Vec::new();
    };
    let due: Vec<u32> = steps(pct)
        .into_iter()
        .filter(|s| !a.warned.contains(s))
        .collect();
    // One event for the newest step: older steps that nobody saw are
    // covered by it.
    let (Some(&step), Some(run)) = (due.last(), run) else {
        return Vec::new();
    };
    let text = warning_text(feature, &a, step, pct, forecast);
    if mail::append(env, run, Dir::Up, Message::event("appetite", text.clone())).is_err() {
        return Vec::new();
    }
    if mail::end(env, run).is_none() {
        let _ = mail::send_down(env, run, Message::event("appetite", text));
    }
    a.warned.extend(&due);
    a.warned.sort_unstable();
    a.warned.dedup();
    if let Err(e) = save(path, &a) {
        log::warn!("cannot record the appetite warning: {e:#}");
    }
    due
}
