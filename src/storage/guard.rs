//! The space guard: free space on the volumes of paddock's folders and the
//! project, before a run and while sandboxes run.
//!
//! - Below the hard floor, a run does not start. The automatic pass runs
//!   first, in the foreground with a time limit, and the check repeats.
//! - Below the soft floor, the run starts with one warning line, and a
//!   desktop notification at most once an hour.
//! - While a sandbox runs, a thread checks every few minutes and sends a
//!   notification at the hard floor. It never stops a sandbox.
//!
//! A floor is the smaller of `*_floor_gib` and `*_floor_percent` of the
//! volume (`[storage]`, settings.rs).

use super::settings::{Floor, Settings};
use super::*;
use crate::env::HostEnv;
use crate::state;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a start waits for the foreground cleanup at the hard floor.
pub const FOREGROUND_SECS: u64 = 120;
/// How often a running sandbox checks the space.
pub const WATCH_SECS: u64 = 300;
/// One notification per level in this time.
pub const NOTIFY_EVERY: u64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    /// Below the soft floor.
    Low,
    /// Below the hard floor.
    Full,
}

/// One volume and what of paddock's lives on it.
#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    /// The first folder checked on this volume.
    pub path: PathBuf,
    pub holds: Vec<String>,
    pub free: u64,
    pub total: u64,
    #[serde(skip)]
    dev: u64,
}

impl Volume {
    pub fn level(&self, s: &Settings) -> Level {
        if self.free < s.hard.bytes(self.total) {
            Level::Full
        } else if self.free < s.soft.bytes(self.total) {
            Level::Low
        } else {
            Level::Ok
        }
    }

    fn line(&self, f: &Floor) -> String {
        format!(
            "{} ({}) has {} free of {}, below {}",
            self.path.display(),
            self.holds.join(", "),
            human(self.free),
            human(self.total),
            human(f.bytes(self.total))
        )
    }
}

/// statvfs of `path`, or of its nearest folder that exists.
fn statvfs(path: &Path) -> Option<(u64, u64, u64)> {
    let at = path.ancestors().find(|p| p.exists())?;
    let dev = std::fs::metadata(at).ok()?.dev();
    let c = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: a valid C string and a zeroed struct that statvfs fills.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    Some((
        (st.f_bavail as u64).saturating_mul(frsize),
        (st.f_blocks as u64).saturating_mul(frsize),
        dev,
    ))
}

/// The volumes of `folders` (name, path), one entry per file system.
pub fn volumes(folders: &[(&str, PathBuf)]) -> Vec<Volume> {
    let mut out: Vec<Volume> = Vec::new();
    for (what, p) in folders {
        let Some((free, total, dev)) = statvfs(p) else {
            continue;
        };
        match out.iter_mut().find(|v| v.dev == dev) {
            Some(v) => v.holds.push(what.to_string()),
            None => out.push(Volume {
                path: p.clone(),
                holds: vec![what.to_string()],
                free,
                total,
                dev,
            }),
        }
    }
    out
}

/// paddock's folders that grow, and the project when there is one.
pub fn folders(env: &HostEnv, project: Option<&Path>) -> Vec<(&'static str, PathBuf)> {
    let mut v = vec![
        ("worktrees", worktrees_root(env)),
        ("sandbox homes", homes_root(env)),
        ("build cache", super::buildcache::root(env)),
    ];
    if let Some(p) = project {
        v.push(("the project", p.to_path_buf()));
    }
    v
}

/// Free bytes over the volumes of paddock's folders, for the pass report.
pub fn free_bytes(env: &HostEnv) -> u64 {
    volumes(&folders(env, None)).iter().map(|v| v.free).sum()
}

fn worst(vols: &[Volume], s: &Settings) -> Level {
    vols.iter().map(|v| v.level(s)).max().unwrap_or(Level::Ok)
}

fn notify_file(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("space-notified.json")
}

/// True when no notification for `key` went out in the last `every`
/// seconds; it records `now` for `key` then. Concurrent runs may both send
/// one in a race; that is all a race costs.
pub fn notify_due(env: &HostEnv, key: &str, now: u64, every: u64) -> bool {
    let f = notify_file(env);
    let mut seen: BTreeMap<String, u64> = std::fs::read_to_string(&f)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if seen
        .get(key)
        .is_some_and(|t| now < t.saturating_add(every) && *t <= now)
    {
        return false;
    }
    seen.insert(key.to_string(), now);
    if let Ok(b) = serde_json::to_vec(&seen) {
        let _ = crate::privfs::write_atomic(&f, b);
    }
    true
}

fn notify(env: &HostEnv, key: &str, summary: &str, body: &str, critical: bool) {
    if !notify_due(env, key, state::now(), NOTIFY_EVERY) {
        return;
    }
    let n = crate::broker::notify::Note {
        summary: summary.into(),
        body: body.into(),
        actions: vec![],
        critical,
    };
    if let Err(e) = crate::broker::notify::show(env, &n) {
        log::debug!("no desktop notification: {e:#}");
    }
}

/// The refusal at the hard floor, with the numbers and what to run.
fn refusal(vols: &[Volume], s: &Settings) -> String {
    let full: Vec<String> = vols
        .iter()
        .filter(|v| v.level(s) == Level::Full)
        .map(|v| v.line(&s.hard))
        .collect();
    format!(
        "not enough free space to start a sandbox: {}. The automatic cleanup already removed what it can without a question. \
To free more, check `paddock worktree list`, then run `paddock gc --worktrees --yes` (add --force for worktrees with unmerged work). \
`paddock du` shows what else uses space. The floor is [storage] hard_floor_gib and hard_floor_percent in ~/.config/paddock/config.toml.",
        full.join("; ")
    )
}

/// Before a run: start the background cleanup when it is due, then check
/// the space. Refuses below the hard floor after a foreground cleanup.
pub fn before_run(env: &HostEnv, project: Option<&Path>, quiet: bool) -> Result<()> {
    let (s, warnings) = Settings::load(env);
    if !quiet {
        for w in &warnings {
            eprintln!("paddock: {w}");
        }
    }
    super::auto::start_background(env, &s);
    let dirs = folders(env, project);
    let mut vols = volumes(&dirs);
    if worst(&vols, &s) == Level::Full {
        if !quiet {
            eprintln!("paddock: the disk is almost full. paddock removes merged worktrees and old data first, for at most {FOREGROUND_SECS} seconds.");
        }
        let deadline = Instant::now() + Duration::from_secs(FOREGROUND_SECS);
        foreground_pass(env, &s, deadline);
        vols = volumes(&dirs);
    }
    match worst(&vols, &s) {
        Level::Full => bail!("{}", refusal(&vols, &s)),
        Level::Low => {
            let low: Vec<String> = vols
                .iter()
                .filter(|v| v.level(&s) == Level::Low)
                .map(|v| v.line(&s.soft))
                .collect();
            let line = format!(
                "free space is low: {}. `paddock du` shows what paddock stores.",
                low.join("; ")
            );
            if !quiet {
                eprintln!("paddock: warning: {line}");
            }
            notify(env, "low", "paddock: free space is low", &line, false);
        }
        Level::Ok => {}
    }
    Ok(())
}

/// The pass at the hard floor. When a background pass holds the lock, wait
/// for it until the deadline, then run once more.
fn foreground_pass(env: &HostEnv, s: &Settings, deadline: Instant) {
    loop {
        match super::auto::pass(env, s, true, Some(deadline)) {
            super::auto::Outcome::Done(st) => {
                log::info!("foreground cleanup: {} removed", st.removed.len());
                return;
            }
            super::auto::Outcome::Skipped(why) if Instant::now() < deadline => {
                log::debug!("foreground cleanup waits: {why}");
                std::thread::sleep(Duration::from_millis(500));
            }
            super::auto::Outcome::Skipped(_) => return,
        }
    }
}

/// While a sandbox runs: a check every `every`, and a notification at the
/// hard floor. Dropping it stops the thread.
pub struct Watch {
    stop: Arc<(Mutex<bool>, Condvar)>,
}

impl Watch {
    pub fn start(env: &HostEnv, project: &Path) -> Watch {
        Self::start_every(env, project, Duration::from_secs(WATCH_SECS))
    }

    pub fn start_every(env: &HostEnv, project: &Path, every: Duration) -> Watch {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let (s2, env, project) = (stop.clone(), env.clone(), project.to_path_buf());
        std::thread::spawn(move || loop {
            let (m, c) = &*s2;
            let Ok(g) = m.lock() else { return };
            let Ok((g, _)) = c.wait_timeout_while(g, every, |stopped| !*stopped) else {
                return;
            };
            if *g {
                return;
            }
            drop(g);
            check_running(&env, &project);
        });
        Watch { stop }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let (m, c) = &*self.stop;
        if let Ok(mut g) = m.lock() {
            *g = true;
        }
        c.notify_all();
    }
}

/// One check while sandboxes run. True when a volume is below the hard
/// floor.
pub fn check_running(env: &HostEnv, project: &Path) -> bool {
    let s = Settings::load(env).0;
    let vols = volumes(&folders(env, Some(project)));
    if worst(&vols, &s) != Level::Full {
        return false;
    }
    let full: Vec<String> = vols
        .iter()
        .filter(|v| v.level(&s) == Level::Full)
        .map(|v| v.line(&s.hard))
        .collect();
    let body = format!(
        "{}. Running sandboxes go on, but new ones do not start. Free space with `paddock gc --worktrees --yes`.",
        full.join("; ")
    );
    log::warn!("free space below the hard floor: {body}");
    notify(env, "full", "paddock: the disk is almost full", &body, true);
    true
}

/// `paddock doctor`: the volumes, the floors, the last automatic cleanup
/// and the size of paddock's folders.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    let (s, warnings) = Settings::load(env);
    let mut out = vec!["\nDisk space:".to_string()];
    for v in volumes(&folders(env, None)) {
        let mark = match v.level(&s) {
            Level::Ok => "ok",
            Level::Low => "low",
            Level::Full => "FULL",
        };
        out.push(format!(
            "  {mark:<8} {}: {} free of {} ({}); runs stop below {}, warn below {}",
            v.path.display(),
            human(v.free),
            human(v.total),
            v.holds.join(", "),
            human(s.hard.bytes(v.total)),
            human(s.soft.bytes(v.total)),
        ));
    }
    for w in warnings {
        out.push(format!("  note     {w}"));
    }
    let off = !s.auto_gc || env.var("PADDOCK_AUTO_GC") == Some("0");
    match super::auto::load(env) {
        _ if off => out.push(
            "  cleanup  the automatic cleanup is off ([storage] auto_gc = false or PADDOCK_AUTO_GC=0)"
                .into(),
        ),
        None => out.push("  cleanup  the automatic cleanup has not run yet; it starts with the next run".into()),
        Some(st) => {
            let when = local_time(st.started);
            match st.ended {
                Some(_) => out.push(format!(
                    "  cleanup  last automatic cleanup {when}: {} removed, {} came back, {} errors",
                    st.removed.len(),
                    human(st.freed),
                    st.errors.len()
                )),
                None => out.push(format!(
                    "  cleanup  the automatic cleanup that started {when} did not finish (it runs, or it crashed)"
                )),
            }
            for e in st.errors.iter().take(3) {
                out.push(format!("           {e}"));
            }
            if !st.sizes.is_empty() {
                let list: Vec<String> = st
                    .sizes
                    .iter()
                    .map(|(n, b)| format!("{n} {}", human(*b)))
                    .collect();
                out.push(format!(
                    "  sizes    {} (measured {}; paddock du measures now)",
                    list.join(", "),
                    local_time(st.sizes_at)
                ));
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
