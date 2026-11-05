//! The automatic cleanup pass.
//!
//! `paddock run` starts it in the background at most once an hour
//! (`paddock gc-auto`, a process of its own at the lowest priority), so the
//! start never waits for it. When the disk is below the hard floor, the
//! start runs it in the foreground with a time limit first (guard.rs).
//!
//! The pass removes, without a question:
//!
//! - everything that `paddock gc --yes` removes, except images;
//! - worktrees whose work is merged (storage/merged.rs) and that have no
//!   uncommitted changes, with their `paddock/` branch;
//! - the build output of the other worktrees that were not used for
//!   `[storage] worktree_cache_days` (storage/buildout.rs);
//! - snapshots and undo trash folders older than `snapshot_days`;
//! - homes whose project is gone and that were not used for
//!   `orphan_home_days`, but never a desktop app home (its login);
//! - the trash, leftovers and oldest bases of the build caches.
//!
//! It never touches the home, worktree or snapshot of a running sandbox:
//! the plan skips them, and `apply` reads the running sandboxes again just
//! before each removal. An flock keeps two passes apart. The status file is
//! written when the pass starts, so a pass that crashes still counts for the
//! rate limit, and again when it ends, for `paddock doctor`.

use super::settings::Settings;
use super::*;
use crate::env::HostEnv;
use crate::state::{self, Running};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The pass runs at most this often from `paddock run`.
pub const EVERY: u64 = 3600;
/// A worktree used less than this ago is never removed: its run may still
/// be starting. The lease (apply.rs) covers the rest of the start.
pub const GRACE: u64 = 600;
/// The folder sizes for `paddock doctor` are measured at most this often.
const SIZES_EVERY: u64 = DAY;

/// The last pass, for the rate limit and `paddock doctor`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Status {
    pub started: u64,
    /// None while it runs, or when it crashed.
    pub ended: Option<u64>,
    pub pid: u32,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
    /// Free space that came back, over the volumes of paddock's folders.
    pub freed: u64,
    /// Folder name and size, measured at `sizes_at`.
    pub sizes: Vec<(String, u64)>,
    pub sizes_at: u64,
}

fn status_file(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("storage-auto.json")
}

pub fn load(env: &HostEnv) -> Option<Status> {
    let text = std::fs::read_to_string(status_file(env)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(env: &HostEnv, s: &Status) {
    let r = serde_json::to_vec_pretty(s)
        .map_err(std::io::Error::other)
        .and_then(|b| crate::privfs::write_atomic(&status_file(env), b));
    if let Err(e) = r {
        log::warn!("cannot write {}: {e}", status_file(env).display());
    }
}

/// A new pass is due: none ran yet, the last one started at least `EVERY`
/// seconds ago, or the clock went back.
pub fn due(last_started: Option<u64>, now: u64) -> bool {
    match last_started {
        None => true,
        Some(t) => now >= t.saturating_add(EVERY) || t > now.saturating_add(EVERY),
    }
}

/// `PADDOCK_AUTO_GC=0` turns the automatic pass off, like `[storage]
/// auto_gc = false`.
fn off(env: &HostEnv, s: &Settings) -> bool {
    !s.auto_gc || env.var("PADDOCK_AUTO_GC") == Some("0")
}

/// Start `paddock gc-auto ARGS` in a session of its own. Only the paddock
/// binary does this, never a test binary.
pub fn spawn(env: &HostEnv, args: &[&str]) {
    if env.var("PADDOCK_AUTO_GC") == Some("0") {
        return;
    }
    let Ok(me) = std::env::current_exe() else {
        return;
    };
    if me.file_name().and_then(|n| n.to_str()) != Some("paddock") {
        return;
    }
    let mut cmd = std::process::Command::new(me);
    cmd.arg("gc-auto")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe. The pass outlives the run.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => log::warn!("cannot start the automatic cleanup: {e}"),
    }
}

/// At the start of a run: the background pass, when one is due.
pub fn start_background(env: &HostEnv, s: &Settings) {
    if off(env, s) || !due(load(env).map(|s| s.started), state::now()) {
        return;
    }
    spawn(env, &[]);
}

#[derive(Debug)]
pub enum Outcome {
    /// Why nothing ran.
    Skipped(String),
    Done(Status),
}

/// No container CLI: the pass leaves images to `paddock gc` and to the
/// cleanup after an image build.
struct NoContainers;

impl Containers for NoContainers {
    fn run(&self, cli: &str, _: &[&str]) -> Result<String> {
        anyhow::bail!("{cli} is not used by the automatic cleanup")
    }
    fn clis(&self) -> Vec<String> {
        vec![]
    }
}

/// An flock that is free, or None while another process holds it.
fn try_lock(path: &Path) -> Option<std::fs::File> {
    crate::privfs::dir(path.parent()?).ok()?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .ok()?;
    // SAFETY: flock on a file descriptor that this function owns.
    (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(f)
}

/// Run the pass. `force` ignores the rate limit (the hard floor, and
/// `paddock gc-auto --now`). `deadline` stops before the next removal when it
/// is over; the rest waits for the next pass.
pub fn pass(env: &HostEnv, s: &Settings, force: bool, deadline: Option<Instant>) -> Outcome {
    let Some(_lock) = try_lock(&env.paddock_state().join("storage-auto.lock")) else {
        return Outcome::Skipped("another cleanup runs".into());
    };
    let prev = load(env).unwrap_or_default();
    let now = state::now();
    if !force && !due(Some(prev.started).filter(|t| *t > 0), now) {
        return Outcome::Skipped("the last cleanup started less than an hour ago".into());
    }
    let mut st = Status {
        started: now,
        pid: std::process::id(),
        sizes: prev.sizes,
        sizes_at: prev.sizes_at,
        ..Default::default()
    };
    save(env, &st);
    let before = super::guard::free_bytes(env);
    let host = Host::real(env, &NoContainers);
    let inv = scan(env, &host, false);
    st.errors.extend(inv.notes.iter().cloned());
    let items = plan_auto(&inv, s, now, &|w| super::buildout::droppable(&w.path));
    for (n, i) in items.iter().enumerate() {
        if deadline.is_some_and(|d| Instant::now() > d) {
            st.errors.push(format!(
                "the time limit ended the pass: {} items wait for the next one",
                items.len() - n
            ));
            break;
        }
        record(&mut st, apply(env, &NoContainers, std::slice::from_ref(i)));
    }
    build_cache(env, s, &mut st);
    if deadline.is_none() && now >= st.sizes_at.saturating_add(SIZES_EVERY) {
        st.sizes = folder_sizes(env);
        st.sizes_at = state::now();
    }
    st.freed = super::guard::free_bytes(env).saturating_sub(before);
    st.ended = Some(state::now());
    save(env, &st);
    log::info!(
        "automatic cleanup: removed {} items, {} errors, {} came back",
        st.removed.len(),
        st.errors.len(),
        human(st.freed)
    );
    Outcome::Done(st)
}

/// The build caches alone, after a sandbox ended: the old base it replaced
/// goes at once, not at the next hourly pass.
pub fn build_cache_pass(env: &HostEnv, s: &Settings) -> Status {
    let mut st = Status::default();
    build_cache(env, s, &mut st);
    st
}

fn record(st: &mut Status, removed: Vec<Removed>) {
    for r in removed {
        match r.error {
            None => st.removed.push(r.what),
            Some(e) => st.errors.push(format!("{}: {e}", r.what)),
        }
    }
}

fn build_cache(env: &HostEnv, s: &Settings, st: &mut Status) {
    let Some(_lock) = try_lock(&super::buildcache::root(env).join(".gc.lock")) else {
        return;
    };
    let running: BTreeSet<String> = Running::list(env).into_iter().map(|r| r.id).collect();
    let plan = super::buildcache::plan(env, s, &running, state::now(), &allocated);
    for (path, why) in plan {
        let what = format!("build cache {} ({why})", path.display());
        match remove_path(env, &path) {
            Ok(()) => st.removed.push(what),
            Err(e) => st.errors.push(format!("{what}: {e:#}")),
        }
    }
}

/// The build cache items for `paddock gc`: the same as the pass removes.
pub fn build_cache_items(env: &HostEnv) -> Vec<Item> {
    let s = Settings::load(env).0;
    let running: BTreeSet<String> = Running::list(env).into_iter().map(|r| r.id).collect();
    super::buildcache::plan(env, &s, &running, state::now(), &allocated)
        .into_iter()
        .map(|(path, reason)| Item {
            what: format!("build cache {}", path.display()),
            tier: Tier::Auto,
            bytes: allocated(&path),
            reason,
            warning: None,
            needs: vec![],
            action: Action::Path { path },
        })
        .collect()
}

/// The size of each of paddock's large folders.
pub fn folder_sizes(env: &HostEnv) -> Vec<(String, u64)> {
    let folders = [
        ("worktrees", worktrees_root(env)),
        ("homes", homes_root(env)),
        ("snapshots", crate::snapshot::root(env)),
        ("undo trash", crate::snapshot::trash_root(env)),
        ("build cache", super::buildcache::root(env)),
        ("cache layers", layers::layers_root(env)),
        ("project cache layers", layers::projects_root(env)),
    ];
    let present: Vec<(&str, PathBuf)> = folders.into_iter().filter(|(_, p)| p.is_dir()).collect();
    let paths: Vec<PathBuf> = present.iter().map(|(_, p)| p.clone()).collect();
    let (v, _) = sizes(&paths);
    present
        .iter()
        .zip(v)
        .map(|((n, _), b)| (n.to_string(), b))
        .collect()
}

fn days_ago(now: u64, t: u64) -> u64 {
    now.saturating_sub(t) / DAY
}

/// What the pass removes. Pure: the inventory holds the facts, and
/// `build_dirs` names the droppable build output of a worktree.
pub fn plan_auto(
    inv: &Inventory,
    s: &Settings,
    now: u64,
    build_dirs: &dyn Fn(&Worktree) -> Vec<PathBuf>,
) -> Vec<Item> {
    let o = GcOptions {
        yes: true,
        older_than_days: DEFAULT_OLDER_THAN,
        ..Default::default()
    };
    let mut items: Vec<Item> = plan(inv, &o, now)
        .into_iter()
        .filter(|i| i.selected() && i.tier == Tier::Auto)
        .filter(|i| !matches!(i.action, Action::Image { .. } | Action::BuildCache { .. }))
        .collect();
    let mut seen: BTreeSet<PathBuf> = items
        .iter()
        .filter_map(|i| match &i.action {
            Action::Path { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    let auto = |what: String, reason: String, warning: Option<String>, action| Item {
        what,
        tier: Tier::Auto,
        bytes: 0,
        reason,
        warning,
        needs: vec![],
        action,
    };
    for w in inv.worktrees.iter().filter(|w| !w.running) {
        let label = match &w.branch {
            Some(b) => format!("worktree {} ({b})", w.path.display()),
            None => format!("worktree {}", w.path.display()),
        };
        // A worktree made minutes ago may belong to a run that is starting.
        let settled = w.last_used.saturating_add(GRACE) <= now;
        if settled && w.repo.is_some() && w.merged == Some(true) && w.dirty == Some(false) {
            items.push(auto(
                label,
                "its work is merged: another branch has every commit, and nothing is uncommitted"
                    .into(),
                None,
                Action::Worktree {
                    path: w.path.clone(),
                    repo: w.repo.clone(),
                    branch: w.branch.clone(),
                    delete_branch: w.branch.as_ref().is_some_and(|b| b.starts_with("paddock/")),
                    force: false,
                },
            ));
            continue;
        }
        let idle = s.worktree_cache_days > 0
            && w.last_used.saturating_add(s.worktree_cache_days * DAY) < now;
        if !idle {
            continue;
        }
        let folders = build_dirs(w);
        if folders.is_empty() {
            continue;
        }
        let names: Vec<String> = folders.iter().map(|f| f.display().to_string()).collect();
        items.push(auto(
            format!("build output in {label}"),
            format!(
                "the worktree is kept, but it was not used for {} days; build caches that git ignores: {}",
                days_ago(now, w.last_used),
                names.join(", ")
            ),
            None,
            Action::BuildOutput {
                worktree: w.path.clone(),
                folders,
            },
        ));
    }
    let running: BTreeSet<&str> = inv
        .sessions
        .iter()
        .filter(|s| s.running)
        .map(|s| s.id.as_str())
        .collect();
    if s.snapshot_days > 0 {
        let old = now.saturating_sub(s.snapshot_days * DAY);
        for c in &inv.snapshots {
            if c.started >= old || running.contains(name(&c.path).as_str()) {
                continue;
            }
            if !seen.insert(c.path.clone()) {
                continue;
            }
            let what = match &c.project {
                Some(p) if !c.trash => format!("snapshot {} of {}", name(&c.path), p.display()),
                _ => format!("undo trash {}", c.path.display()),
            };
            items.push(auto(
                what,
                format!(
                    "{} days old, over [storage] snapshot_days = {}",
                    days_ago(now, c.started),
                    s.snapshot_days
                ),
                c.trash
                    .then(|| "the files that an undo moved away are lost".to_string()),
                Action::Path {
                    path: c.path.clone(),
                },
            ));
        }
    }
    if s.orphan_home_days > 0 {
        let old = now.saturating_sub(s.orphan_home_days * DAY);
        for h in &inv.homes {
            if h.running || h.app || h.harness == DOCTOR_HARNESS {
                continue;
            }
            if h.project_exists != Some(false) || h.last_active() >= old {
                continue;
            }
            if !seen.insert(h.path.clone()) {
                continue;
            }
            let p = h.project.as_ref().map(|p| p.display().to_string());
            items.push(auto(
                format!("home {}", h.path.display()),
                format!(
                    "its project {} is gone, and it was not used for {} days",
                    p.unwrap_or_default(),
                    days_ago(now, h.last_active())
                ),
                Some(h.loses.clone()),
                Action::Path {
                    path: h.path.clone(),
                },
            ));
        }
    }
    items
}

#[cfg(test)]
#[path = "auto_tests.rs"]
mod tests;
