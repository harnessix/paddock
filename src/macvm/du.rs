//! The data of the macOS guest in `paddock du` and `paddock gc`, with the
//! Linux homes as the standard (`src/storage`): every profile instance is
//! listed with its parts, a profile counts as a user asset like a home
//! (never removed automatically, only with `--homes
//! --include-app-logins`), and the parts that come back by themselves
//! are reclaimable: the app archives, which the next copy makes again,
//! and the saved states of profiles that were not used for
//! `--older-than` days, whose next start is then a cold boot.
//!
//! Plain code over a [`Layout`] and an `alive` closure; the tests use a
//! folder.

use super::golden::{used_bytes, used_bytes_in};
use super::host::{self, Session};
use super::layout::Layout;
use super::session::{Profile, Stamp};
use super::text;
use crate::storage::{Action, GcOptions, Item, Tier};
use serde::Serialize;
use std::path::{Path, PathBuf};

const DAY: u64 = 86_400;

/// The state of a profile instance in the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Use {
    Running,
    Suspended,
    Stopped,
}

/// One profile instance in `paddock du`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileDu {
    pub app: String,
    pub instance: u32,
    pub path: PathBuf,
    pub state: Use,
    /// Every byte of the folder.
    pub bytes: u64,
    /// The disk pair: the system clone, the data disk and the NVRAM.
    pub disks: u64,
    pub saved_state: u64,
    /// The app clones and their archives in `apps/`.
    pub apps: u64,
    /// The archives alone: reclaimable.
    pub archives: u64,
    pub backups: u64,
    /// Seconds since the epoch of the last start or save.
    pub last_used: u64,
}

/// The macvm folder in `paddock du`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MacvmDu {
    pub root: PathBuf,
    pub bytes: u64,
    pub golden: u64,
    pub golden_prev: u64,
    pub golden_new: u64,
    pub ipsw: u64,
    pub agent: u64,
    pub trash: u64,
    pub shims: u64,
    pub profiles: Vec<ProfileDu>,
}

/// The live record of a profile instance, or None.
fn record(layout: &Layout, app: &str, home: &str, alive: &dyn Fn(u32) -> bool) -> Option<Session> {
    let text = std::fs::read_to_string(layout.record(app, home)).ok()?;
    let s: Session = serde_json::from_str(&text).ok()?;
    alive(s.pid).then_some(s)
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The archives of the app clones in `apps/`.
pub fn archives(profile: &Profile) -> Vec<PathBuf> {
    let ext = format!(".app.{}", super::appcopy::ARCHIVE_EXT); // not user text
    let mut v: Vec<PathBuf> = std::fs::read_dir(profile.apps())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(&ext))
        })
        .collect();
    v.sort();
    v
}

/// One profile instance.
fn profile_du(layout: &Layout, app: &str, home: &str, alive: &dyn Fn(u32) -> bool) -> ProfileDu {
    let p = layout.profile(app, home);
    let state = match record(layout, app, home, alive) {
        Some(s) if s.suspended => Use::Suspended,
        Some(_) => Use::Running,
        None => Use::Stopped,
    };
    let last_used = mtime(&p.stamp()).max(mtime(&p.state()));
    ProfileDu {
        app: app.to_string(),
        instance: super::instance::number(home).unwrap_or(super::instance::FIRST),
        state,
        bytes: used_bytes_in(&p.dir),
        disks: [p.system(), p.data(), p.aux()]
            .iter()
            .map(|f| used_bytes(f))
            .sum(),
        saved_state: used_bytes(&p.state()),
        apps: used_bytes_in(&p.apps()),
        archives: archives(&p).iter().map(|a| used_bytes(a)).sum(),
        backups: used_bytes_in(&p.backups()),
        last_used,
        path: p.dir,
    }
}

/// The listing of the macvm folder. Empty when the folder is missing.
pub fn scan(layout: &Layout, alive: &dyn Fn(u32) -> bool) -> MacvmDu {
    if !layout.root.is_dir() {
        return MacvmDu::default();
    }
    let profiles = layout
        .profile_dirs()
        .into_iter()
        .filter(|(_, home, _)| super::instance::number(home).is_some())
        .map(|(app, home, _)| profile_du(layout, &app, &home, alive))
        .collect();
    MacvmDu {
        root: layout.root.clone(),
        bytes: used_bytes_in(&layout.root),
        golden: used_bytes_in(&layout.golden.dir),
        golden_prev: used_bytes_in(&layout.golden_prev.dir),
        golden_new: used_bytes_in(&layout.golden_new.dir),
        ipsw: used_bytes_in(&layout.ipsw),
        agent: used_bytes_in(&layout.agent),
        trash: used_bytes_in(&layout.trash),
        shims: used_bytes_in(&layout.shims),
        profiles,
    }
}

/// The `paddock du` section, after the Linux sections.
pub fn render(d: &MacvmDu, now: u64) -> String {
    use crate::storage::human;
    if d.root.as_os_str().is_empty() {
        return String::new();
    }
    let mut t = text::du_head(&human(d.bytes), &d.root);
    for (what, bytes) in [
        ("golden image", d.golden),
        ("previous golden image", d.golden_prev),
        ("golden image in the making", d.golden_new),
        ("restore images (IPSW)", d.ipsw),
        ("guest agents", d.agent),
        ("trash of reset profiles", d.trash),
        ("Dock faces", d.shims),
    ] {
        if bytes > 0 {
            t.push_str(&text::du_part(&human(bytes), what));
        }
    }
    for p in &d.profiles {
        let label = super::instance::label(&p.app, p.instance);
        let used = if p.last_used == 0 {
            "-".to_string()
        } else {
            text::du_days_ago(now.saturating_sub(p.last_used) / DAY)
        };
        t.push_str(&text::du_profile(
            &human(p.bytes),
            &used,
            &label,
            match p.state {
                Use::Running => "running",
                Use::Suspended => "suspended",
                Use::Stopped => "stopped",
            },
        ));
        t.push_str(&text::du_profile_parts(
            &human(p.disks),
            &human(p.saved_state),
            &human(p.apps),
            &human(p.backups),
        ));
    }
    t
}

/// The gc items of the profiles: the app archives and the old saved
/// states (automatic), and the profiles themselves (only with `--homes
/// --include-app-logins`, as a home of a desktop app). A running or
/// suspended instance gives nothing.
pub fn gc_items(
    layout: &Layout,
    o: &GcOptions,
    now: u64,
    alive: &dyn Fn(u32) -> bool,
) -> Vec<Item> {
    let mut out = Vec::new();
    let old = now.saturating_sub(o.older_than_days * DAY);
    for p in scan(layout, alive).profiles {
        if p.state != Use::Stopped {
            continue;
        }
        let label = super::instance::label(&p.app, p.instance);
        let profile = Profile::new(&p.path);
        for a in archives(&profile) {
            out.push(Item {
                what: text::gc_archive_what(&label, &a),
                tier: Tier::Auto,
                bytes: used_bytes(&a),
                reason: text::gc_archive_reason(),
                warning: None,
                needs: vec![],
                action: Action::Macvm { dir: a },
            });
        }
        let unused = p.last_used < old;
        let days = now.saturating_sub(p.last_used) / DAY;
        if unused && p.saved_state > 0 {
            out.push(Item {
                what: text::gc_state_what(&label),
                tier: Tier::Auto,
                bytes: p.saved_state,
                reason: text::gc_state_reason(days),
                warning: None,
                needs: vec![],
                action: Action::Macvm {
                    dir: profile.state(),
                },
            });
        }
        if unused {
            let mut needs = Vec::new();
            if !o.homes {
                needs.push("--homes".to_string());
            }
            if !o.include_app_logins {
                needs.push("--include-app-logins".to_string());
            }
            out.push(Item {
                what: text::gc_profile_what(&label),
                tier: Tier::Consent,
                bytes: p.bytes,
                reason: text::gc_profile_reason(days),
                warning: Some(text::gc_profile_warning()),
                needs,
                action: Action::Macvm { dir: p.path },
            });
        }
    }
    out
}

/// The kind of a path that [`gc_items`] lists, for the removal: an
/// archive, a saved state or a profile folder under `profiles.noindex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Archive,
    State,
    Profile,
}

/// The part of a path, when it is one of a profile instance:
/// `profiles.noindex/APP/INSTANCE[/apps/X.app.cpio | /state.vzs]`.
pub fn part(layout: &Layout, path: &Path) -> Option<(Part, String, String)> {
    let rel = path.strip_prefix(&layout.profiles).ok()?;
    let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
    let plain = |s: &str| s == super::layout::safe_name(s);
    let (app, home) = match parts.as_slice() {
        [app, home, ..] if plain(app) && super::instance::number(home).is_some() => (*app, *home),
        _ => return None,
    };
    let kind = match parts.as_slice() {
        [_, _] => Part::Profile,
        [_, _, "state.vzs"] => Part::State,
        [_, _, "apps", file] if !file.starts_with('.') => {
            let ext = format!(".app.{}", super::appcopy::ARCHIVE_EXT); // not user text
            if !file.ends_with(&ext) {
                return None;
            }
            Part::Archive
        }
        _ => return None,
    };
    Some((kind, app.to_string(), home.to_string()))
}

/// Remove one part that [`gc_items`] listed, unless its instance runs or
/// is suspended now. `Ok(false)`: the path is not a part of a profile.
pub fn remove(layout: &Layout, path: &Path, alive: &dyn Fn(u32) -> bool) -> anyhow::Result<bool> {
    let Some((kind, app, home)) = part(layout, path) else {
        return Ok(false);
    };
    if record(layout, &app, &home, alive).is_some() {
        anyhow::bail!(text::gc_profile_in_use(&app));
    }
    // The profile lock: no start between the check and the removal.
    let profile = layout.profile(&app, &home);
    let _lock = super::lock::ProfileLock::take(&profile)
        .map_err(|_| anyhow::anyhow!(text::gc_profile_in_use(&app)))?;
    match kind {
        Part::Profile => {
            super::manage::unseal_profile(path);
            super::layout::remove_any(path)?;
        }
        Part::State | Part::Archive => super::layout::remove_any(path)?,
    }
    if kind == Part::State {
        // The stamp keeps its data id; the next start is a cold boot.
        if let Ok(mut s) = Stamp::read(&profile.stamp()) {
            s.state_id = None;
            s.state_len = None;
            let _ = s.write(&profile.stamp());
        }
    }
    Ok(true)
}

/// [`scan`] of this host, for `paddock du`.
pub fn scan_env(env: &crate::env::HostEnv) -> MacvmDu {
    scan(&Layout::new(env), &host::alive)
}

#[cfg(test)]
#[path = "du_tests.rs"]
mod tests;
