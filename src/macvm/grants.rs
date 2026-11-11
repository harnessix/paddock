//! The grants of an app profile of the macOS guest, kept in the profile:
//! `grants.json` holds the folders that the user shared with
//! `paddock app share`, the network mode of `paddock app network`, the two
//! clipboard directions of `paddock app clipboard`, and the last grant
//! changes with their time and source. The next start reads it, so a
//! change made while the app runs stays.
//!
//! Every change runs under `grants.lock` (`flock`), reads the file, and
//! writes the new one with a rename: a crash in the middle leaves the old
//! file, and two commands at once never lose a change. A file that does
//! not parse is never written over: the change stops with a line that
//! names the file. A start with such a file starts without its grants
//! (the least the app can see) and says so.
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

/// The file of the grants in the profile folder.
pub const FILE: &str = "grants.json";
/// The lock of a change.
pub const LOCK: &str = "grants.lock";
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
}

impl Default for Grants {
    fn default() -> Self {
        Grants {
            v: VERSION,
            shares: Vec::new(),
            network: None,
            clipboard: Clipboard::default(),
            changes: Vec::new(),
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
}

impl Applies {
    pub fn word(self) -> &'static str {
        match self {
            Applies::Live => "live",
            Applies::ReopenFiles => "reopen-files",
            Applies::NextStart => "next-start",
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
        let (action, value, applies) = (&self.action, &self.value, self.applies.word());
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
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Grants::default()),
        Err(e) => return Err(e).with_context(|| text::grants_unreadable(&p)),
    };
    let g: Grants = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!(text::grants_damaged(&p, &e)))?;
    if g.v > VERSION {
        bail!(text::grants_newer(&p, g.v));
    }
    Ok(g)
}

/// The grants of a start: a damaged file gives the defaults and the
/// line that says so.
pub fn load_for_start(profile: &Profile) -> (Grants, Option<String>) {
    match load(profile) {
        Ok(g) => (g, None),
        Err(e) => (
            Grants {
                clipboard: Clipboard {
                    to_app: false,
                    from_app: false,
                },
                ..Grants::default()
            },
            Some(text::grants_skipped(&format!("{e:#}"))),
        ),
    }
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

/// Write the grants atomically.
pub fn save(profile: &Profile, g: &Grants) -> Result<()> {
    save_with_sync(profile, g, |dir| std::fs::File::open(dir)?.sync_all())
}

fn save_with_sync(
    profile: &Profile,
    g: &Grants,
    sync: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    let p = path(profile);
    let mut g = g.clone();
    g.v = VERSION;
    let start = g.changes.len().saturating_sub(CHANGES_KEPT);
    g.changes.drain(..start);
    let bytes = serde_json::to_vec_pretty(&g)?;
    crate::privfs::write_atomic(&p, bytes).with_context(|| text::grants_unwritable(&p))?;
    sync(&profile.dir).with_context(|| text::grants_unwritable(&p))
}

/// One change under the lock: `f` gets the grants of the file and
/// changes them; when it returns `Ok` the new grants are written. An
/// error leaves the file as it was.
pub fn update<T>(profile: &Profile, f: impl FnOnce(&mut Grants) -> Result<T>) -> Result<T> {
    let _lock = Lock::take(profile)?;
    let mut g = load(profile)?;
    let out = f(&mut g)?;
    save(profile, &g)?;
    Ok(out)
}

/// The event line of a change, into the event log of the profile.
pub fn event(profile: &Profile, c: &Change, home: &Path) {
    let grant = serde_json::to_value(c).unwrap_or_default();
    Events::open(profile).record_with(EVENT_KIND, &c.line(home), EVENT_KIND, grant);
}

/// A change made outside a grants command (a rule from the firewall
/// panel, `paddock app rules`): into `changes[]` and the event log.
pub fn note(profile: &Profile, c: Change, home: &Path) {
    let r = update(profile, |g| {
        g.changes.push(c.clone());
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
                    g.changes.push(c.clone());
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
/// into the grant changes of the profile when the profile exists. The
/// proxy reads the rules file per connection, so it is live.
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
        value,
        Applies::Live,
        Source::You,
    );
    note(&profile, c, &env.home);
}

/// The newest changes first, at most [`CHANGES_SHOWN`].
pub fn recent(g: &Grants) -> Vec<Change> {
    g.changes
        .iter()
        .rev()
        .take(CHANGES_SHOWN)
        .cloned()
        .collect()
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
/// `paddock app network` saved. A damaged grants file prints its line
/// and gives neither.
pub fn for_start(profile: &Profile, roots: &[shares::Root]) -> (Vec<shares::Root>, Option<String>) {
    let (g, skipped) = load_for_start(profile);
    if let Some(line) = skipped {
        eprintln!("{line}");
    }
    (merge_roots(roots, &g), g.network.clone())
}

/// The network mode words that `paddock app network` sets.
pub const NETWORK_MODES: [&str; 3] = ["ask", "allowlist", "none"];

#[cfg(test)]
#[path = "grants_tests.rs"]
mod tests;
