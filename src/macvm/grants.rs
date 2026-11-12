//! The grants of an app profile of the macOS guest, kept in the profile:
//! `grants.json` holds the folders that the user shared with
//! `paddock app share`, the network mode of `paddock app network`, the two
//! clipboard directions of `paddock app clipboard`, and the last grant
//! changes with their time and source. Historical rule effects are kept
//! separately so that older readers can still read the active policy.
//!
//! Cooperating changes hold `grants.lock` (`flock`) and replace the policy
//! by rename. A file that does not parse is never written over. A start
//! with such a file disables saved shares, clipboard and network and warns.
//! Checkpoint writers must be stopped during migration and remain stopped
//! while a legacy closed-enum reader uses the migrated policy.
//!
//! Every change also goes to the event log of the profile
//! (`logs/events.jsonl`) as a `grant` line with a `grant` object, which
//! [`Change`] describes. Pure apart from the files: tests on Linux.

use super::events::Events;
use super::session::Profile;
use super::shares::{self, normalize};
use super::text::{self, ShareWhy};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

#[path = "grants_history.rs"]
mod history;
pub use history::HistoryState;

#[path = "grants_start.rs"]
mod start;
#[cfg(test)]
use start::confirm_start_within;
pub use start::{confirm_start, start_network, StartPolicy, START_LOCK_WAIT};

/// The file of the grants in the profile folder.
pub const FILE: &str = "grants.json";
/// The lock of a change.
pub const LOCK: &str = "grants.lock";
/// Left by the first policy write: a later missing policy is not a new profile.
pub const SAVED: &str = "grants.saved";
/// The version of the file this paddock writes.
pub const VERSION: u32 = 1;
/// The changes that the file keeps.
pub const CHANGES_KEPT: usize = 50;
/// The changes that `paddock app status --json` shows.
pub const CHANGES_SHOWN: usize = 20;
/// The kind of the event line of a change.
pub const EVENT_KIND: &str = "grant";

/// The grants of one profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    #[serde(default = "version_one")]
    pub v: u32,
    /// The folders that the app sees, besides the folders of `app run`.
    #[serde(default)]
    pub shares: Vec<FolderGrant>,
    /// The network mode word of the next start, when the user set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default)]
    pub clipboard: Clipboard,
    /// The newest last.
    #[serde(default)]
    pub changes: Vec<Change>,
    /// The separate history may be missing or damaged, and is never a
    /// certificate of completeness across older writers.
    #[serde(skip)]
    pub history_state: HistoryState,
    /// Presentation only: never serialized into, or used as, policy.
    #[serde(skip)]
    pub history_changes: Vec<Change>,
    #[serde(skip)]
    pending_history: Vec<Change>,
    /// The previous format put historical entries in the policy itself.
    #[serde(skip)]
    legacy_history_in_policy: bool,
    /// Original checkpoint bytes, kept until their recovery copy exists.
    #[serde(skip)]
    legacy_policy: Option<Vec<u8>>,
}

impl Default for Grants {
    fn default() -> Self {
        Grants {
            v: VERSION,
            shares: Vec::new(),
            network: None,
            clipboard: Clipboard::default(),
            changes: Vec::new(),
            history_state: HistoryState::Missing,
            history_changes: Vec::new(),
            pending_history: Vec::new(),
            legacy_history_in_policy: false,
            legacy_policy: None,
        }
    }
}

fn version_one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

/// One shared folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderGrant {
    /// The real path on the host.
    pub path: PathBuf,
    pub read_only: bool,
    /// Shared although the secrets check found files (`--unprotected`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unprotected: bool,
    /// Seconds since the epoch.
    #[serde(default)]
    pub added: u64,
}

/// The two directions of the clipboard. Both on by default: the
/// clipboard still moves only while a window of the app is key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clipboard {
    /// Paste from this Mac into the app.
    #[serde(default = "yes")]
    pub to_app: bool,
    /// Copy from the app to this Mac.
    #[serde(default = "yes")]
    pub from_app: bool,
}

impl Default for Clipboard {
    fn default() -> Self {
        Clipboard {
            to_app: true,
            from_app: true,
        }
    }
}

/// What a change is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum What {
    Folder,
    Network,
    Clipboard,
    Rule,
}

/// When a change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applies {
    /// Applied in the running session.
    Live,
    /// The app holds files of the folder open: applied, but the app must
    /// open them again.
    ReopenFiles,
    /// No session runs, or the VM cannot change it while it runs.
    NextStart,
    /// A historical effect whose order relative to other writers is unknown.
    Unknown,
}

impl Applies {
    pub fn word(self) -> &'static str {
        match self {
            Applies::Live => "live",
            Applies::ReopenFiles => "reopen-files",
            Applies::NextStart => "next-start",
            Applies::Unknown => "unknown",
        }
    }
}

/// Who made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The user, with the CLI or the launcher.
    You,
    /// An answer on the firewall panel.
    QuestionPanel,
    /// The approver hook of the system configuration.
    Approver,
    /// The seeds of the app profile.
    Profile,
}

/// One grant change: a line of `changes[]` and the `grant` object of its
/// event line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Seconds since the epoch.
    pub t: u64,
    pub what: What,
    /// `add`, `remove`, `mode` or `set`.
    pub action: String,
    /// The folder, the host of a rule, or the network or clipboard word.
    pub target: String,
    /// `ro` or `rw`, the mode word, `to-app on, from-app off`, `allow`.
    pub value: String,
    pub applies: Applies,
    /// `unknown` means this historical effect may have been superseded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,
    pub source: Source,
}

impl Change {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        t: u64,
        what: What,
        action: &str,
        target: &str,
        value: &str,
        applies: Applies,
        source: Source,
    ) -> Self {
        Change {
            t,
            what,
            action: action.to_string(),
            target: target.to_string(),
            value: value.to_string(),
            applies,
            current_state: (applies == Applies::Unknown).then(|| "unknown".to_string()),
            source,
        }
    }

    /// The text of the event line: `grant folder add ~/Projects rw live`.
    pub fn line(&self, home: &Path) -> String {
        let what = match self.what {
            What::Folder => "folder",
            What::Network => "network",
            What::Clipboard => "clipboard",
            What::Rule => "rule",
        };
        let target = if self.what == What::Folder {
            tilde(Path::new(&self.target), home)
        } else {
            self.target.clone()
        };
        let applies = if self.current_state.as_deref() == Some("unknown") {
            "unknown"
        } else {
            self.applies.word()
        };
        let (action, value) = (&self.action, &self.value);
        format!("grant {what} {action} {target} {value} {applies}") // not user text
    }
}

/// `path` with `~` for the home folder.
pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()), // not user text
        Err(_) => path.display().to_string(),
    }
}

/// The file of the grants of a profile.
pub fn path(profile: &Profile) -> PathBuf {
    profile.dir.join(FILE)
}

/// The grants of a profile. A missing file is the defaults. A file that
/// does not parse, or of a newer paddock, is an error that names it.
pub fn load(profile: &Profile) -> Result<Grants> {
    let p = path(profile);
    let bytes = match std::fs::read(&p) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| text::grants_unreadable(&p)),
    };
    let g: Grants = match &bytes {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|e| anyhow::anyhow!(text::grants_damaged(&p, &e)))?,
        None => Grants::default(),
    };
    if g.v > VERSION {
        bail!(text::grants_newer(&p, g.v));
    }
    if let Some(word) = g.network.as_deref().filter(|w| !NETWORK_MODES.contains(w)) {
        bail!(text::grants_bad_network(&p, word));
    }
    Ok(assemble(
        g,
        &history::read(&profile.dir),
        bytes.as_deref(),
        history::has_recovery(&profile.dir),
    ))
}

fn assemble(
    mut g: Grants,
    sidecar: &history::Read,
    policy: Option<&[u8]>,
    recovery: bool,
) -> Grants {
    history::split(&mut g, sidecar, policy, recovery);
    g
}

/// The grants of a start: an unreadable policy, an unknown network word,
/// or a missing policy of a profile that saved one disables network and shares.
pub fn load_for_start(profile: &Profile) -> (Grants, Option<String>) {
    if let Some(line) = missing_after_save(profile) {
        return (unavailable_policy(), Some(line));
    }
    match load(profile) {
        Ok(g) if g.legacy_history_in_policy => match migrate_previous_policy(profile) {
            Ok(g) => (g, Some(text::grant_history_found().into())),
            Err(e) => (g, Some(text::grant_history_not_moved(&path(profile), &e))),
        },
        Ok(g) => (g, None),
        Err(e) => (
            unavailable_policy(),
            Some(text::grants_skipped(&format!("{e:#}"))),
        ),
    }
}

/// A policy that is gone although the profile saved one starts closed.
/// A policy that is there marks the profile, also for older profiles.
fn missing_after_save(profile: &Profile) -> Option<String> {
    let p = path(profile);
    match std::fs::metadata(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let saved = profile.dir.join(SAVED).exists()
                || profile.dir.join(history::FILE).exists()
                || history::has_recovery(&profile.dir);
            saved.then(|| text::grants_skipped(&text::grants_missing(&p)))
        }
        _ => {
            if let Err(e) = mark_saved(&profile.dir) {
                log::warn!("macvm: {e:#}");
            }
            None
        }
    }
}

fn mark_saved(dir: &Path) -> Result<()> {
    let marker = dir.join(SAVED);
    if dir.is_dir() && !marker.exists() {
        crate::privfs::write(&marker, b"").with_context(|| text::grants_unwritable(&marker))?;
    }
    Ok(())
}

fn unavailable_policy() -> Grants {
    Grants {
        network: Some("none".into()),
        clipboard: Clipboard {
            to_app: false,
            from_app: false,
        },
        ..Grants::default()
    }
}

/// Repair a file written by the preceding incompatible format when this
/// version starts it. Re-read under the same lock used by old writers.
fn migrate_previous_policy(profile: &Profile) -> Result<Grants> {
    let _lock = Lock::take_with(profile, true)?;
    let mut g = load(profile)?;
    if g.legacy_history_in_policy {
        save(profile, &g)?;
        g = load(profile)?;
    }
    Ok(g)
}

/// The lock of a change, held while it lives.
pub struct Lock(#[allow(dead_code)] std::fs::File);

impl Lock {
    pub fn take(profile: &Profile) -> Result<Self> {
        Self::take_with(profile, false)
    }

    fn take_with(profile: &Profile, nonblocking: bool) -> Result<Self> {
        crate::privfs::dir(&profile.dir).with_context(|| text::grants_unreadable(&profile.dir))?;
        let p = profile.dir.join(LOCK);
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&p)
            .with_context(|| text::grants_unreadable(&p))?;
        // SAFETY: flock on an open file descriptor.
        if unsafe {
            libc::flock(
                f.as_raw_fd(),
                libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 },
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .with_context(|| text::grants_unreadable(&p));
        }
        Ok(Lock(f))
    }
}

/// Replace the policy by rename and write history separately. A policy
/// that went missing after a save is replaced only with a network word.
pub fn save(profile: &Profile, g: &Grants) -> Result<()> {
    save_with_sync(profile, g, |dir| std::fs::File::open(dir)?.sync_all())
}

/// Refuse now what [`save`] would refuse for these grants, before a
/// change goes live that the save could then not keep. Only a failure of
/// the disk itself is left for the save.
pub fn check_save(profile: &Profile, g: &Grants) -> Result<()> {
    save_plan(profile, g).map(drop)
}

fn save_plan(profile: &Profile, g: &Grants) -> Result<history::Plan> {
    // Without a network word this save would reopen a closed start to ask.
    if g.network.is_none() && missing_after_save(profile).is_some() {
        bail!(text::grants_missing_unchanged(&path(profile)));
    }
    history::plan(g, &history::read(&profile.dir))
        .with_context(|| text::grant_history_at(&profile.dir.join(history::FILE)))
}

fn save_with_sync(
    profile: &Profile,
    g: &Grants,
    mut sync: impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<()> {
    let p = path(profile);
    let plan = save_plan(profile, g)?;
    let history_warning = history::execute(&plan, |step, bytes| match step {
        history::Step::Recovery => history::write_recovery(&profile.dir, bytes, &mut sync)
            .context(text::grant_checkpoint_not_kept()),
        history::Step::Sidecar => {
            let history_path = profile.dir.join(history::FILE);
            crate::privfs::write_atomic(&history_path, bytes)
                .with_context(|| text::grants_unwritable(&history_path))
        }
        history::Step::Policy => {
            mark_saved(&profile.dir)?;
            crate::privfs::write_atomic(&p, bytes).with_context(|| text::grants_unwritable(&p))
        }
    })?;
    if let Some(warning) = history_warning {
        log::warn!("macvm: {warning}");
    }
    #[cfg(test)]
    if FAIL_FINAL_SYNC.with(|f| f.get()) {
        return Err(
            anyhow::Error::new(std::io::Error::other("injected sync failure"))
                .context(Unconfirmed(text::grants_unconfirmed(&p))),
        );
    }
    sync(&profile.dir)
        .map_err(|e| anyhow::Error::new(e).context(Unconfirmed(text::grants_unconfirmed(&p))))
}

/// The error of a save whose policy rename happened but whose folder sync
/// failed: the visible policy is the new one, its durability is unknown.
#[derive(Debug)]
pub struct Unconfirmed(String);

impl std::fmt::Display for Unconfirmed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The visible policy already has the change of this failed save.
pub fn unconfirmed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Unconfirmed>().is_some()
}

#[cfg(test)]
thread_local! {
    /// Fail the folder sync after the policy rename, in this thread.
    pub(crate) static FAIL_FINAL_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One change under the lock: `f` gets the grants of the file and
/// changes them; when it returns `Ok` the new grants are written. An
/// error before the policy write leaves the policy as it was. A failed
/// history write is logged and does not roll back the policy.
pub fn update<T>(profile: &Profile, f: impl FnOnce(&mut Grants) -> Result<T>) -> Result<T> {
    let _lock = Lock::take(profile)?;
    let mut g = load(profile)?;
    let out = f(&mut g)?;
    save(profile, &g)?;
    Ok(out)
}

/// The event line of a change, into the event log of the profile.
pub fn event(profile: &Profile, c: &Change, home: &Path) {
    let mut change = c.clone();
    history::normalize(&mut change);
    let grant = serde_json::to_value(&change).unwrap_or_default();
    let (kind, key) = event_fields(&change);
    Events::open(profile).record_with(kind, &change.line(home), key, grant);
}

fn event_fields(c: &Change) -> (&'static str, &'static str) {
    if c.applies == Applies::Unknown || c.current_state.as_deref() == Some("unknown") {
        ("grant_history", "grant_history")
    } else {
        (EVENT_KIND, EVENT_KIND)
    }
}

/// A change made outside a grants command (a rule from the firewall
/// panel, `paddock app rules`): into history and the event log.
pub fn note(profile: &Profile, c: Change, home: &Path) {
    let r = update(profile, |g| {
        history::record(g, c.clone());
        Ok(())
    });
    if let Err(e) = r {
        log::warn!("macvm: {e:#}");
    }
    event(profile, &c, home);
}

/// Record a firewall rule without waiting indefinitely for another writer.
pub fn note_bounded(profile: &Profile, c: Change, home: &Path) {
    let mut last = None;
    for _ in 0..3 {
        match Lock::take_with(profile, true) {
            Ok(_lock) => {
                let result = load(profile).and_then(|mut g| {
                    history::record(&mut g, c.clone());
                    save(profile, &g)
                });
                if let Err(e) = result {
                    log::warn!("macvm: {e:#}");
                }
                event(profile, &c, home);
                return;
            }
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    if let Some(e) = last {
        log::warn!("macvm: cannot record grant change: {e:#}");
    }
    event(profile, &c, home);
}

/// A rule change of `paddock app rules NAME add|remove`, by the user,
/// into the grant changes of the profile when the profile exists. This
/// delayed note does not establish current state or order against a writer.
pub fn note_user_rule(
    env: &crate::env::HostEnv,
    app: &str,
    action: &str,
    target: &str,
    value: &str,
) {
    let profile = super::layout::Layout::new(env).profile(app, super::host::DEFAULT_HOME);
    if !profile.dir.is_dir() {
        return;
    }
    let c = Change::new(
        crate::state::now(),
        What::Rule,
        action,
        target,
        &text::grant_historical(value),
        Applies::Unknown,
        Source::You,
    );
    note(&profile, c, &env.home);
}

/// Up to [`CHANGES_SHOWN`] changes for display, with newer timestamps first
/// when separate historical effects are present.
pub fn recent(g: &Grants) -> Vec<Change> {
    let mut changes = g.changes.clone();
    changes.extend(g.history_changes.iter().cloned());
    // The two files have no common ordering; timestamps are only a display
    // hint, not evidence of the order in which effects became current.
    if changes.iter().any(|c| c.applies == Applies::Unknown) {
        changes.sort_by_key(|c| c.t);
    }
    changes.into_iter().rev().take(CHANGES_SHOWN).collect()
}

/// Split the status snapshot so legacy consumers never encounter an
/// unknown `applies` value in the original `changes` field.
pub fn recent_compatible(g: &Grants) -> (Vec<Change>, Vec<Change>) {
    let changes = g
        .changes
        .iter()
        .rev()
        .take(CHANGES_SHOWN)
        .cloned()
        .collect();
    let mut history = g.history_changes.clone();
    history.sort_by_key(|c| c.t);
    (
        changes,
        history.into_iter().rev().take(CHANGES_SHOWN).collect(),
    )
}

/// The folders of paddock itself: its data, its configuration, its state
/// and its cache. A share of one of them, inside one or above one, is
/// refused.
pub fn paddock_dirs(env: &crate::env::HostEnv) -> Vec<PathBuf> {
    vec![
        env.paddock_data(),
        env.paddock_config(),
        env.paddock_state(),
        env.paddock_cache(),
    ]
}

/// Why a folder is never shared, with the folders of paddock too.
pub fn refusal(path: &Path, home: &Path, paddock: &[PathBuf]) -> Option<ShareWhy> {
    if let Some(why) = shares::refusal(path, home) {
        return Some(why);
    }
    let p = normalize(path);
    paddock
        .iter()
        .map(|d| normalize(d))
        .any(|d| d.starts_with(&p) || p.starts_with(&d))
        .then_some(ShareWhy::PaddockData)
}

/// A folder that `paddock app share NAME add` may share: the real path,
/// or the line that refuses it. `cwd` makes a relative path absolute.
/// The secrets check refuses a folder with a `.env` or key file unless
/// `unprotected`, which keeps the warning lines.
pub fn check_folder(
    app: &str,
    path: &Path,
    cwd: &Path,
    home: &Path,
    paddock: &[PathBuf],
    unprotected: bool,
) -> Result<(PathBuf, Vec<String>)> {
    let path = normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    });
    let refuse =
        |p: &Path, why| text::grant_share_refused(app, &tilde(p, home), &text::share_reason(why));
    if let Some(why) = refusal(&path, home, paddock) {
        bail!(refuse(&path, why));
    }
    if !path.is_dir() {
        bail!(text::no_folder_exists(&path));
    }
    // A link on the way can point a named folder into a refused one: the
    // real path is checked too, and it is the one that is shared.
    let real = std::fs::canonicalize(&path).with_context(|| text::no_folder_exists(&path))?;
    let real_home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let real_paddock: Vec<PathBuf> = paddock
        .iter()
        .map(|d| std::fs::canonicalize(d).unwrap_or_else(|_| d.clone()))
        .collect();
    if let Some(why) = refusal(&real, &real_home, &real_paddock) {
        bail!(refuse(&real, why));
    }
    let secrets = shares::secrets_in(&real);
    if !secrets.is_empty() && !unprotected {
        let mut lines: Vec<String> = secrets.iter().map(|f| text::env_file(app, f)).collect();
        lines.push(text::grant_share_anyway(app, &tilde(&path, home)));
        bail!(lines.join("\n")); // not user text
    }
    let warnings = secrets
        .iter()
        .map(|f| text::env_file_visible(app, f))
        .collect();
    Ok((real, warnings))
}

/// The roots of a start with the shared folders of the grants: a grant
/// for a root sets its mode, the other grants come after the roots.
pub fn merge_roots(roots: &[shares::Root], g: &Grants) -> Vec<shares::Root> {
    let mut out: Vec<shares::Root> = roots.to_vec();
    for f in &g.shares {
        match out.iter_mut().find(|r| normalize(&r.path) == f.path) {
            Some(r) => {
                r.read_only |= f.read_only;
                r.unprotected |= f.unprotected;
            }
            None => out.push(shares::Root {
                path: f.path.clone(),
                read_only: f.read_only,
                unprotected: f.unprotected,
            }),
        }
    }
    out
}

/// The roots and the network mode of a start: the folders of the grants
/// after the roots of `app run` ([`merge_roots`]), and the mode word that
/// `paddock app network` saved. An unreadable file disables network and
/// saved shares and prints a warning.
pub fn for_start(profile: &Profile, roots: &[shares::Root]) -> (Vec<shares::Root>, Grants) {
    let (g, skipped) = load_for_start(profile);
    if let Some(line) = skipped {
        eprintln!("{line}");
    }
    (merge_roots(roots, &g), g)
}

/// The network mode words that `paddock app network` sets.
pub const NETWORK_MODES: [&str; 3] = ["ask", "allowlist", "none"];

#[cfg(test)]
#[path = "grants_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "grants_compat_tests.rs"]
mod compat_tests;
