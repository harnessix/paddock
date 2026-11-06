//! `paddock macvm status` and `paddock macvm remove`: what is on disk, the
//! sessions that run, and the removal of the golden and of the profiles.
//! Plain code that runs on every system; the tests use a folder.

use super::golden::{self, Manifest, Prev, State};
use super::host;
use super::layout::{self, Layout};
use super::text;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// What `status` and the doctor see of the golden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoldenView {
    /// No folder, or an empty one.
    Missing,
    /// A manifest that this paddock cannot read, with the error.
    Unreadable(String),
    /// A setup in progress, stopped in this state.
    Unsealed(Manifest),
    /// Sealed, with the used bytes of the images and whether both carry
    /// the mode and the flag of the seal.
    Sealed {
        manifest: Manifest,
        size: u64,
        flags_ok: bool,
    },
}

impl GoldenView {
    /// Read the golden of a layout.
    pub fn read(layout: &Layout) -> Self {
        let g = &layout.golden;
        match g.read_manifest() {
            Ok(None) => GoldenView::Missing,
            Err(e) => GoldenView::Unreadable(format!("{e:#}")),
            Ok(Some(m)) if m.state == State::Sealed => {
                let size = golden::used_bytes(&g.disk) + golden::used_bytes(&g.aux);
                let flags_ok = g.sealed_files().iter().all(|f| sealed_ok(f));
                GoldenView::Sealed {
                    manifest: m,
                    size,
                    flags_ok,
                }
            }
            Ok(Some(m)) => GoldenView::Unsealed(m),
        }
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        match self {
            GoldenView::Sealed { manifest, .. } | GoldenView::Unsealed(manifest) => Some(manifest),
            _ => None,
        }
    }
}

/// Mode 0444, and the `uchg` flag on macOS.
pub fn sealed_ok(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let mode_ok = std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o777 == 0o444);
    mode_ok && (layout::is_immutable(path) || !cfg!(target_os = "macos"))
}

/// The sessions whose shim still runs: the id of each record (its file
/// stem) and the record that `host` wrote.
pub fn running_sessions(layout: &Layout) -> Vec<(String, host::Session)> {
    host::sessions(&layout.sessions)
        .into_iter()
        .filter(|(_, s)| host::alive(s.pid))
        .map(|(p, s)| {
            let id = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (id, s)
        })
        .collect()
}

/// The lines of `paddock macvm status`. A golden swap that a crash left
/// half done is finished first, and its line comes before the rest.
pub fn status_lines(layout: &Layout, host_version: Option<&str>, me: &str) -> Vec<String> {
    let mut out = vec![text::status_head(&layout.root)];
    match golden::recover(layout) {
        Ok(Some(r)) => out.push(r.line()),
        Ok(None) => {}
        Err(e) => out.push(text::status_golden_unreadable(&format!("{e:#}"))),
    }
    match GoldenView::read(layout) {
        GoldenView::Missing => out.push(text::status_golden_none()),
        GoldenView::Unreadable(e) => out.push(text::status_golden_unreadable(&e)),
        GoldenView::Unsealed(m) => {
            out.push(text::status_golden_unsealed(&m.version_text(), m.state));
            if let Some((kind, why)) = offline_failure(&layout.golden) {
                out.push(text::status_offline_failed(&kind, &why));
            }
        }
        GoldenView::Sealed {
            manifest,
            size,
            flags_ok,
        } => {
            out.push(text::status_golden_sealed(
                &manifest.version_text(),
                manifest.sealed.as_deref().unwrap_or("?"),
                size,
                flags_ok,
            ));
            if let Some(h) = host_version {
                if golden::version_older(&manifest.version, h) {
                    out.push(text::status_golden_older(&manifest.version, h));
                }
            }
        }
    }
    match golden::prev_state(layout) {
        Prev::None => {}
        Prev::Kept { version } => out.push(text::status_prev(&version, false)),
        Prev::Removable { version } => out.push(text::status_prev(&version, true)),
    }
    if let Some(h) = host_version {
        out.push(text::status_host(h));
    }
    let profiles = layout.profile_dirs();
    if profiles.is_empty() {
        out.push(text::status_profiles_none());
    }
    let golden_text = golden::version_text_of(&layout.golden);
    for (app, home, dir) in &profiles {
        out.push(text::status_profile_state(
            app,
            home,
            golden::used_bytes_in(dir),
            &profile_state(layout, app, home, &golden_text),
        ));
    }
    let sessions = running_sessions(layout);
    if sessions.is_empty() {
        out.push(text::status_sessions_none());
    }
    for (id, s) in &sessions {
        out.push(text::status_session(
            id,
            s.pid,
            &s.profile.display().to_string(),
        ));
    }
    out.push(text::status_slots(&slots(me)));
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The holders of the two VM slots of this Mac. `me` is this user.
pub fn slots(me: &str) -> Vec<Option<text::Holder>> {
    super::lock::holders(Path::new(super::lock::SLOT_DIR), me)
}

/// The state words of one profile for `paddock macvm status`: running
/// with its pid and age, or stopped with the last end and the next start.
pub fn profile_state(layout: &Layout, app: &str, home: &str, golden_text: &str) -> String {
    let profile = layout.profile(app, home);
    match host::running(layout, app, home) {
        Some(s) => text::list_running(
            s.pid,
            &crate::vm::host::duration(now_secs().saturating_sub(s.started)),
        ),
        None => {
            let display = host::DEFAULT_DISPLAY
                .parse()
                .unwrap_or(super::session::Display {
                    w: 2560,
                    h: 1600,
                    scale: 2,
                });
            let next = super::start::predict(&profile, golden_text, display);
            text::stopped_head(
                text::session_end(host::last_end(&profile)),
                &super::status::next_start_of(&next),
            )
        }
    }
}

/// The last offline step of the setup (the seed, the trust or the
/// Downloads link) when it failed: its event kind and the reason. A later
/// success clears it.
pub fn offline_failure(golden: &super::layout::Golden) -> Option<(String, String)> {
    use super::setup_events::{read, DOWNLOADS_OFFLINE, SEED_OFFLINE, TRUST_OFFLINE};
    let lines = read(&golden.events);
    [SEED_OFFLINE, TRUST_OFFLINE, DOWNLOADS_OFFLINE]
        .iter()
        .find_map(|kind| {
            let last = lines.iter().rev().find(|l| l.kind == *kind)?;
            let why = last.text.strip_prefix("failed: ")?;
            Some((kind.to_string(), why.to_string()))
        })
}

/// `paddock macvm status --json`.
pub fn status_json(layout: &Layout, host_version: Option<&str>, me: &str) -> serde_json::Value {
    let view = GoldenView::read(layout);
    let golden = match &view {
        GoldenView::Missing => serde_json::json!({ "state": "missing" }),
        GoldenView::Unreadable(e) => serde_json::json!({ "state": "unreadable", "error": e }),
        GoldenView::Unsealed(m) => serde_json::json!({
            "state": m.state.name(), "version": m.version, "build": m.build,
            "offline_failed": offline_failure(&layout.golden).map(|(k, w)| format!("{k}: {w}")), // not user text
        }),
        GoldenView::Sealed {
            manifest,
            size,
            flags_ok,
        } => serde_json::json!({
            "state": "sealed", "version": manifest.version, "build": manifest.build,
            "sealed": manifest.sealed, "bytes": size, "flags_ok": flags_ok
        }),
    };
    let golden_text = golden::version_text_of(&layout.golden);
    let profiles: Vec<serde_json::Value> = layout
        .profile_dirs()
        .iter()
        .map(|(a, h, d)| {
            serde_json::json!({
                "app": a, "home": h, "dir": d, "bytes": golden::used_bytes_in(d),
                "state": profile_state(layout, a, h, &golden_text),
                "last_end": text::session_end(host::last_end(&layout.profile(a, h))),
            })
        })
        .collect();
    let slots: Vec<serde_json::Value> = slots(me)
        .iter()
        .map(|h| match h {
            None => serde_json::Value::Null,
            Some(h) => serde_json::json!(text::holder_short(h)),
        })
        .collect();
    let sessions: Vec<serde_json::Value> = running_sessions(layout)
        .iter()
        .map(|(id, s)| serde_json::json!({ "id": id, "pid": s.pid, "profile": s.profile }))
        .collect();
    let prev = match golden::prev_state(layout) {
        Prev::None => serde_json::Value::Null,
        Prev::Kept { version } => serde_json::json!({ "version": version, "removable": false }),
        Prev::Removable { version } => {
            serde_json::json!({ "version": version, "removable": true })
        }
    };
    serde_json::json!({
        "dir": layout.root, "golden": golden, "prev": prev, "host_version": host_version,
        "profiles": profiles, "sessions": sessions, "slots": slots
    })
}

/// The days a profile stays in `trash/` after `paddock app reset --all`.
pub const TRASH_DAYS: u64 = 7;

/// The items of `paddock gc`: `golden.prev` once a profile completed a
/// session on the new golden, the agent shares of other paddock versions,
/// the trash entries older than [`TRASH_DAYS`], and the automatic backups
/// above the retention.
pub fn gc_items(
    env: &crate::env::HostEnv,
    o: &crate::storage::GcOptions,
) -> Vec<crate::storage::Item> {
    let layout = Layout::new(env);
    let now = now_secs();
    let mut items = gc_items_at(&layout, now);
    items.extend(super::du::gc_items(&layout, o, now, &host::alive));
    items
}

/// The automatic cleanup of the macvm folder: every item of the automatic
/// tier, as `paddock gc --yes` removes it. A start that finds the volume
/// short of space runs it, and so does the cleanup after an image build.
/// The lines say what went and what could not.
pub fn auto_cleanup(env: &crate::env::HostEnv) -> Vec<String> {
    let o = crate::storage::GcOptions {
        yes: true,
        older_than_days: crate::storage::DEFAULT_OLDER_THAN,
        ..Default::default()
    };
    let mut out = Vec::new();
    for i in gc_items(env, &o).into_iter().filter(|i| i.selected()) {
        let crate::storage::Action::Macvm { dir } = &i.action else {
            continue;
        };
        out.push(match gc_remove(env, dir) {
            Ok(()) => text::gc_auto_removed(&i.what, &crate::storage::human(i.bytes)),
            Err(e) => text::gc_auto_failed(&i.what, &format!("{e:#}")),
        });
    }
    out
}

/// [`gc_items`] of a layout, at a time.
pub fn gc_items_at(layout: &Layout, now: u64) -> Vec<crate::storage::Item> {
    use crate::storage::{Action, Item, Tier};
    let layout = layout.clone();
    let mut out = Vec::new();
    for (name, dir, age) in trash_entries(&layout) {
        if age >= TRASH_DAYS * 24 * 3600 {
            out.push(Item {
                what: text::gc_trash_what(&name),
                tier: Tier::Auto,
                bytes: golden::used_bytes_in(&dir),
                reason: text::gc_trash_reason(TRASH_DAYS),
                warning: None,
                needs: vec![],
                action: Action::Macvm { dir },
            });
        }
    }
    for (app, home, _) in layout.profile_dirs() {
        let profile = layout.profile(&app, &home);
        let name = if home == host::DEFAULT_HOME {
            app.clone()
        } else {
            format!("{app}/{home}") // not user text
        };
        for b in super::backup::retire_plan(&super::backup::list(&profile), now) {
            out.push(Item {
                what: text::gc_backup_what(&name, &b.name),
                tier: Tier::Auto,
                bytes: b.meta.bytes,
                reason: text::gc_backup_reason(),
                warning: None,
                needs: vec![],
                action: Action::Macvm { dir: b.dir },
            });
        }
    }
    if let Prev::Removable { version } = golden::prev_state(&layout) {
        out.push(Item {
            what: text::gc_prev_what(&version),
            tier: Tier::Auto,
            bytes: golden::used_bytes_in(&layout.golden_prev.dir),
            reason: text::gc_prev_reason(),
            warning: None,
            needs: vec![],
            action: Action::Macvm {
                dir: layout.golden_prev.dir.clone(),
            },
        });
    }
    let current = super::version::paddock_version();
    for (version, dir) in layout.other_agent_dirs(current) {
        out.push(Item {
            what: text::gc_agent_what(&version),
            tier: Tier::Auto,
            bytes: golden::used_bytes_in(&dir),
            reason: text::gc_agent_reason(current),
            warning: None,
            needs: vec![],
            action: Action::Macvm { dir },
        });
    }
    out
}

/// The entries of `trash/`: `(name, path, age in seconds)`, from the
/// modification time of each folder.
pub fn trash_entries(layout: &Layout) -> Vec<(String, PathBuf, u64)> {
    let now = now_secs();
    let mut out: Vec<(String, PathBuf, u64)> = std::fs::read_dir(&layout.trash)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| {
                    let age = e
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| now.saturating_sub(d.as_secs()))
                        .unwrap_or(0);
                    (e.file_name().to_string_lossy().into_owned(), e.path(), age)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Remove one folder that `gc_items` listed: `golden.prev` (with its
/// seal flags), an agent share folder, a trash entry or a backup of a
/// profile. Any other path is refused.
pub fn gc_remove(env: &crate::env::HostEnv, dir: &Path) -> Result<()> {
    gc_remove_in(&Layout::new(env), dir)
}

/// [`gc_remove`] in a layout.
pub fn gc_remove_in(layout: &Layout, dir: &Path) -> Result<()> {
    if super::du::remove(layout, dir, &host::alive)? {
        return Ok(());
    }
    if dir == layout.golden_prev.dir {
        return golden::remove_prev(layout);
    }
    let plain = |n: &std::ffi::OsStr| n.to_str().is_some_and(|n| n == layout::safe_name(n));
    let in_agent =
        dir.parent() == Some(layout.agent.as_path()) && dir.file_name().is_some_and(plain);
    let in_trash =
        dir.parent() == Some(layout.trash.as_path()) && dir.file_name().is_some_and(plain);
    let in_backups = dir.parent().is_some_and(|b| {
        b.file_name().is_some_and(|n| n == "backups")
            && b.parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .is_some_and(|p| p == layout.profiles)
    }) && dir.file_name().is_some_and(plain);
    if !(in_agent || in_trash || in_backups) {
        bail!(text::gc_not_macvm(dir));
    }
    if in_trash {
        unseal_tree(dir);
    }
    layout::remove_any(dir).with_context(|| text::cannot_remove(dir))
}

/// What `remove` would do: the golden and its size, the profiles and
/// their size. Nothing is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovePlan {
    pub golden: Option<(String, u64)>,
    pub profiles: Vec<(String, String, PathBuf, u64)>,
}

impl RemovePlan {
    pub fn read(layout: &Layout) -> Self {
        let golden = match GoldenView::read(layout) {
            GoldenView::Missing if !layout.golden.dir.exists() => None,
            v => Some((
                v.manifest()
                    .map(Manifest::version_text)
                    .unwrap_or_else(|| "unknown".into()),
                golden::used_bytes_in(&layout.golden.dir),
            )),
        };
        let profiles = layout
            .profile_dirs()
            .into_iter()
            .map(|(a, h, d)| {
                let size = golden::used_bytes_in(&d);
                (a, h, d, size)
            })
            .collect();
        Self { golden, profiles }
    }

    pub fn profiles_bytes(&self) -> u64 {
        self.profiles.iter().map(|p| p.3).sum()
    }
}

/// Refuse while a session runs: a removal under a running guest would
/// pull its disk away.
pub fn refuse_while_running(layout: &Layout) -> Result<()> {
    if let Some((_, s)) = running_sessions(layout).first() {
        bail!(text::remove_while_running(s.pid));
    }
    Ok(())
}

/// Remove the golden, the golden builds, the restore images and the
/// guest password. The profiles stay.
pub fn remove_golden(layout: &Layout, keyring: Option<&golden::Keyring>) -> Result<()> {
    refuse_while_running(layout)?;
    for g in [&layout.golden, &layout.golden_new, &layout.golden_prev] {
        if g.dir.exists() {
            layout::remove_golden_dir(g)?;
        }
    }
    layout::remove_any(&layout.ipsw).with_context(|| text::cannot_remove(&layout.ipsw))?;
    if let Some(k) = keyring {
        k.forget()
            .with_context(|| text::cannot_remove(&k.guest_password))?;
    }
    Ok(())
}

/// Remove every profile: each folder moves whole into
/// `trash/<app>-<home>-<time>/`, as `paddock app reset --all` does (the
/// disk pair stays together; the folder holds the login keychain, so it
/// keeps its mode 0700). Returns the trash folders made.
pub fn remove_profiles(layout: &Layout) -> Result<Vec<PathBuf>> {
    refuse_while_running(layout)?;
    let mut made = Vec::new();
    let now = now_secs();
    for (app, home, dir) in layout.profile_dirs() {
        let trash = super::reset::trash_dir(layout, &app, &home, now);
        crate::privfs::dir(&layout.trash)?;
        std::fs::rename(&dir, &trash).with_context(|| text::cannot_move(&dir, &trash))?;
        made.push(trash);
        let _ = std::fs::remove_dir(dir.parent().unwrap_or(&dir));
    }
    let _ = std::fs::remove_dir(&layout.profiles);
    Ok(made)
}

/// Take the seal flags off every file of a profile before its removal.
pub fn unseal_profile(dir: &Path) {
    unseal_tree(dir)
}

fn unseal_tree(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            unseal_tree(&p);
        } else if sealed_ok(&p) || layout::is_immutable(&p) {
            let _ = layout::unseal_file(&p);
        }
    }
}

/// A copy that shares blocks where the file system can (`clonefile` on
/// APFS), else a plain copy. The copy is 0600.
pub fn clone_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let s = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let d = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: two NUL-terminated paths; the flags of <sys/clonefile.h>
        // (CLONE_NOFOLLOW = 1, CLONE_NOOWNERCOPY = 2).
        if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 1 | 2) } == 0 {
            return std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600));
        }
    }
    let _ = std::fs::remove_file(to);
    std::fs::copy(from, to)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600))
}

#[cfg(test)]
#[path = "manage_tests.rs"]
mod tests;
