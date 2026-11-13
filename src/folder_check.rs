//! The folder guard: paddock refuses to start an agent in a folder that
//! exposes too much. The agent gets the project folder with auto-approval,
//! so a start in the home folder or in `/` by mistake would give it
//! everything below.
//!
//! Two levels, for the project folder after symlinks:
//!
//! - Refuse, with no flag: `/`, the home folder, a folder above it, the
//!   system folders, and paddock's own data and state folders.
//! - Confirm: the personal and cloud sync folders, a folder that holds
//!   keys or logins, a folder of projects (more than 5 git repositories in
//!   its direct folders), and a folder with more than 200 000 files or
//!   50 GB. `--yes-wide-folder` or `[run] allow_wide = ["PATH"]` of the
//!   user configuration confirms it. A project `paddock.toml` cannot set
//!   `allow_wide`: the agent can edit the project.
//!
//! The path rules are pure (`Ctx` holds the paths), so the tests check the
//! Linux and the macOS paths on any system.

use crate::env::HostEnv;
use anyhow::{bail, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The fix that every message names.
pub const FIX: &str = "Start paddock in the project folder, for example ~/code/myapp";

/// The flag that confirms a wide folder.
pub const FLAG: &str = "--yes-wide-folder";

/// The root folders of the system, refused as they are.
const SYSTEM: &[&str] = &[
    "/etc",
    "/usr",
    "/usr/local",
    "/var",
    "/opt",
    "/srv",
    "/mnt",
    "/media",
    "/root",
    "/run",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/nix",
    "/snap",
    "/System",
    "/Library",
    "/Applications",
    "/Volumes",
    "/cores",
    "/private",
    "/private/etc",
    "/private/var",
];

/// System folders whose subfolders are refused too: no project lives there.
/// `/var` and `/opt` are not here: `/var/folders` is the temporary folder of
/// macOS, and `/var/www` or `/opt/app` can be a project.
const SYSTEM_TREES: &[&str] = &[
    "/etc",
    "/private/etc",
    "/System",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib32",
    "/usr/lib64",
    "/usr/libexec",
    "/usr/share",
    "/usr/include",
];

/// The folders that hold the home folders.
const HOMES_PARENTS: &[&str] = &["/home", "/Users"];

/// Personal folders in the home folder (macOS and the XDG defaults).
const PERSONAL: &[&str] = &[
    "Documents",
    "Desktop",
    "Downloads",
    "Pictures",
    "Movies",
    "Videos",
    "Music",
    "Public",
];

/// Cloud sync folders in the home folder.
const CLOUD: &[&str] = &[
    "Library/Mobile Documents",
    "Library/Mobile Documents/com~apple~CloudDocs",
    "Library/CloudStorage",
    "iCloud Drive",
    "iCloudDrive",
    "Dropbox",
    "OneDrive",
    "Google Drive",
    "GoogleDrive",
    "My Drive",
    "Box",
    "Box Sync",
    "pCloud Drive",
    "Nextcloud",
    "ownCloud",
    "MEGA",
    "MEGAsync",
    "Sync",
    "Yandex.Disk",
    "Proton Drive",
    "Creative Cloud Files",
];

/// Name prefixes of cloud sync folders in the home folder: `OneDrive -
/// Company`, `Dropbox (Team)`.
const CLOUD_PREFIXES: &[&str] = &["OneDrive - ", "Dropbox (", "Google Drive ("];

/// Folders whose presence makes a folder hold keys, logins or passwords.
const SECRETS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gcloud",
    ".password-store",
    ".local/share/keyrings",
    "Library/Keychains",
    "Keychains",
];

/// File name endings of a keychain or a password database.
const SECRET_FILES: &[&str] = &[".keychain", ".keychain-db", ".kdbx"];

/// Build and dependency folders that the count skips: a normal project can
/// have a large `target` or `node_modules`.
const SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
    ".next",
    ".nuxt",
    ".cache",
    ".direnv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".terraform",
    ".build",
    "DerivedData",
    "Pods",
    "dist",
    "build",
    "vendor",
];

/// The limits of the confirm level.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// More git repositories than this in the direct folders.
    pub repos: usize,
    /// More files than this.
    pub files: u64,
    /// More bytes than this.
    pub bytes: u64,
    /// The time limit of the count.
    pub time: Duration,
}

pub const LIMITS: Limits = Limits {
    repos: 5,
    files: 200_000,
    bytes: 50_000_000_000,
    time: Duration::from_secs(2),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// A project folder.
    Ok,
    /// A wide folder that the flag or `allow_wide` confirms.
    Confirmed,
    /// A wide folder without a confirmation: the start is refused.
    Confirm,
    /// Refused, with no override.
    Refuse,
}

/// What the count found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Found {
    /// Git repositories in the direct folders.
    pub repos: usize,
    /// Files outside the build and dependency folders.
    pub files: u64,
    pub bytes: u64,
    /// The count ended before the time limit (or at a limit it crossed).
    pub complete: bool,
    pub millis: u64,
    /// The first repository, for the example of the fix.
    #[serde(skip)]
    pub first_repo: Option<PathBuf>,
}

/// The result of the guard for one folder.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub folder: PathBuf,
    pub level: Level,
    /// Why, in plain words. Empty for a project folder.
    pub reason: String,
    /// What to do. Empty for a project folder.
    pub fix: String,
    /// What confirms a wide folder: the flag or `[run] allow_wide`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found: Option<Found>,
}

impl Check {
    /// Whether the start may go on.
    pub fn passes(&self) -> bool {
        matches!(self.level, Level::Ok | Level::Confirmed)
    }

    /// The one line of a refused start: the folder, the reason, the fix.
    pub fn error_line(&self, ctx: &Ctx) -> Option<String> {
        let f = ctx.show(&self.folder);
        match self.level {
            Level::Refuse => Some(format!(
                "the agent does not start in {f}: {}. {}",
                self.reason, self.fix
            )),
            Level::Confirm => Some(format!(
                "the agent does not start in {f} without a confirmation: {}. {}",
                self.reason, self.fix
            )),
            Level::Ok | Level::Confirmed => None,
        }
    }

    /// The line of `paddock explain` for a folder that is not plainly a
    /// project, or None.
    pub fn explain_line(&self, ctx: &Ctx) -> Option<String> {
        match self.level {
            Level::Ok => None,
            Level::Confirmed => Some(format!(
                "confirmed by {}: {}",
                self.confirmed_by.as_deref().unwrap_or("?"),
                self.reason
            )),
            _ => self.error_line(ctx),
        }
    }
}

/// The paths that the rules compare with.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    /// The home folder as given, and its real path.
    pub homes: Vec<PathBuf>,
    /// paddock's own folders: data, state, configuration, cache, runtime.
    pub paddock: Vec<PathBuf>,
    /// Folders inside paddock's data that hold projects, with the depth a
    /// project has below them: worktrees (`worktrees/REPO/ID`), scratch
    /// projects (`scratch/AGENT`) and the projects of `paddock doctor
    /// --escape` (`doctor-runs/RUN/project` in the cache).
    pub project_roots: Vec<(PathBuf, usize)>,
    /// Personal folders with another name (`~/.config/user-dirs.dirs`).
    pub user_dirs: Vec<PathBuf>,
    /// `[run] allow_wide` of the user configuration, resolved.
    pub allow_wide: Vec<PathBuf>,
    /// The user configuration file, for the messages.
    pub config_file: PathBuf,
}

/// Both spellings of a path: as given and real.
fn spellings(p: &Path) -> Vec<PathBuf> {
    let mut v = vec![p.to_path_buf()];
    if let Ok(real) = std::fs::canonicalize(p) {
        if real != p {
            v.push(real);
        }
    }
    v
}

impl Ctx {
    /// The paths of this host, with `allow_wide` of the user configuration.
    pub fn of(env: &HostEnv) -> Result<Self> {
        let user = crate::manifest::Manifest::load_user(env)?;
        let mut ctx = Self::paths(env);
        ctx.allow_wide = user
            .run
            .allow_wide
            .iter()
            .flat_map(|raw| spellings(&env.expand(raw, &env.home)))
            .collect();
        Ok(ctx)
    }

    /// The paths of this host, without the user configuration.
    pub fn paths(env: &HostEnv) -> Self {
        let mut paddock = Vec::new();
        for p in [
            env.paddock_data(),
            env.paddock_state(),
            env.paddock_config(),
            env.paddock_cache(),
            env.paddock_run(),
        ] {
            paddock.extend(spellings(&p));
        }
        let mut project_roots = Vec::new();
        for (p, depth) in [
            (env.paddock_data().join("worktrees"), 2),
            (crate::storage::scratch_root(env), 1),
            (crate::doctor::scratch_base(env), 2),
        ] {
            project_roots.extend(spellings(&p).into_iter().map(|p| (p, depth)));
        }
        let user_dirs = PERSONAL
            .iter()
            .filter_map(|d| crate::recover::host_user_dir(env, d))
            .flat_map(|p| spellings(&p))
            .collect();
        Self {
            homes: crate::policy::homes(env),
            paddock,
            project_roots,
            user_dirs,
            allow_wide: Vec::new(),
            config_file: env.paddock_config().join("config.toml"),
        }
    }

    /// A path for messages, with `~` for the home folder.
    pub fn show(&self, p: &Path) -> String {
        for h in &self.homes {
            if p.starts_with(h) {
                return crate::devices::grant::tilde(p, h);
            }
        }
        p.display().to_string()
    }

    fn is_home(&self, p: &Path) -> bool {
        self.homes.iter().any(|h| h == p)
    }
}

/// The hard rules: a reason when the folder is refused with no override.
/// `folder` is a real path. Pure: it reads no file.
pub fn hard(ctx: &Ctx, folder: &Path) -> Option<String> {
    let root = Path::new("/");
    if folder == root || folder.as_os_str().is_empty() {
        return Some(
            "it is the root of the file system, so the agent would get every file of the computer"
                .into(),
        );
    }
    if ctx.is_home(folder) {
        return Some("it is your home directory, so the agent would get all your files".into());
    }
    if ctx.homes.iter().any(|h| h.starts_with(folder)) {
        return Some("it holds your home directory, so the agent would get all your files".into());
    }
    if HOMES_PARENTS.iter().any(|p| folder == Path::new(p)) {
        return Some("it holds the home directories of the users of this computer".into());
    }
    if SYSTEM.iter().any(|p| folder == Path::new(p)) {
        return Some("it is a system folder".into());
    }
    if let Some(t) = SYSTEM_TREES.iter().find(|t| folder.starts_with(t)) {
        return Some(format!("it is inside the system folder {t}"));
    }
    for p in &ctx.paddock {
        if folder == p {
            return Some(format!(
                "it is paddock's own folder {}, where it keeps its data and state",
                ctx.show(p)
            ));
        }
        if p.starts_with(folder) {
            return Some(format!(
                "it holds paddock's own folder {}, where it keeps its data and state",
                ctx.show(p)
            ));
        }
    }
    for p in &ctx.paddock {
        if folder.starts_with(p) {
            let project = ctx.project_roots.iter().any(|(r, depth)| {
                folder
                    .strip_prefix(r)
                    .is_ok_and(|rest| rest.components().count() >= *depth)
            });
            if !project {
                return Some(format!(
                    "it is inside paddock's own folder {}, and not a worktree or scratch project",
                    ctx.show(p)
                ));
            }
        }
    }
    None
}

/// The personal and cloud folders: a reason, or None. Pure.
fn personal(ctx: &Ctx, folder: &Path) -> Option<String> {
    let name = folder.file_name()?.to_string_lossy().into_owned();
    for h in &ctx.homes {
        if PERSONAL.iter().any(|d| folder == h.join(d)) {
            return Some(format!(
                "it is your {name} folder, which holds personal files"
            ));
        }
        if CLOUD.iter().any(|d| folder == h.join(d))
            || (folder.parent() == Some(h.as_path())
                && CLOUD_PREFIXES.iter().any(|p| name.starts_with(p)))
            || folder.parent() == Some(&h.join("Library/CloudStorage"))
        {
            return Some(format!(
                "it is the cloud sync folder {name}, so the agent could change files on all your devices"
            ));
        }
    }
    if ctx.user_dirs.iter().any(|d| d == folder) {
        return Some(format!(
            "it is your {name} folder, which holds personal files"
        ));
    }
    None
}

/// Keys, logins or passwords right in the folder: a reason, or None.
fn secrets(folder: &Path) -> Option<String> {
    let mut held: Vec<String> = SECRETS
        .iter()
        .filter(|s| std::fs::symlink_metadata(folder.join(s)).is_ok())
        .map(|s| s.to_string())
        .collect();
    if let Ok(rd) = std::fs::read_dir(folder) {
        for e in rd.flatten().take(10_000) {
            let n = e.file_name().to_string_lossy().into_owned();
            if SECRET_FILES.iter().any(|s| n.ends_with(s)) {
                held.push(n);
            }
        }
    }
    if held.is_empty() {
        return None;
    }
    held.truncate(3);
    Some(format!(
        "it holds {}, so the agent would see keys, logins or passwords",
        held.join(", ")
    ))
}

/// Count the git repositories in the direct folders, then the files and
/// bytes outside build and dependency folders. Stops at the time limit or
/// when it crosses a limit.
pub fn count(folder: &Path, limits: &Limits) -> Found {
    let start = Instant::now();
    let mut f = Found::default();
    let late = |start: &Instant| start.elapsed() >= limits.time;
    // A git repository is one project, even with submodules or nested
    // clones below it.
    if std::fs::symlink_metadata(folder.join(".git")).is_err() {
        if let Ok(rd) = std::fs::read_dir(folder) {
            let mut names: Vec<PathBuf> = rd
                .flatten()
                .take(100_000)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
            names.sort();
            for d in names {
                if late(&start) {
                    break;
                }
                if std::fs::symlink_metadata(d.join(".git")).is_ok() {
                    f.repos += 1;
                    f.first_repo.get_or_insert(d);
                }
            }
        }
    }
    let mut stack = vec![folder.to_path_buf()];
    let mut seen = 0u64;
    let mut complete = true;
    'walk: while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen.is_multiple_of(512) && late(&start) {
                complete = false;
                break 'walk;
            }
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                let n = e.file_name();
                if !SKIP.iter().any(|s| n == *s) {
                    stack.push(e.path());
                }
                continue;
            }
            f.files += 1;
            if t.is_file() {
                f.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
            if f.files > limits.files || f.bytes > limits.bytes {
                break 'walk;
            }
        }
    }
    f.complete = complete;
    f.millis = start.elapsed().as_millis() as u64;
    f
}

/// `1234567` as `1 234 567`.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn gigabytes(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

/// What the count found, in words.
pub fn found_words(f: &Found) -> String {
    let secs = f.millis as f64 / 1000.0;
    let how = if f.complete {
        format!("counted in {secs:.1} s")
    } else {
        format!("stopped at the time limit after {secs:.1} s")
    };
    format!(
        "{} files, {}, {} git repositories in its folders ({how}, without build and dependency folders)",
        grouped(f.files),
        gigabytes(f.bytes),
        f.repos
    )
}

/// The wide rules: a reason and what the count found.
pub fn wide(ctx: &Ctx, folder: &Path, limits: &Limits) -> (Option<String>, Option<Found>) {
    if let Some(r) = personal(ctx, folder) {
        return (Some(r), None);
    }
    if let Some(r) = secrets(folder) {
        return (Some(r), None);
    }
    let f = count(folder, limits);
    let reason = if f.repos > limits.repos {
        Some(format!(
            "its folders hold {} git repositories, so it is a folder of projects, not one project",
            f.repos
        ))
    } else if f.files > limits.files {
        Some(format!(
            "it holds more than {} files ({})",
            grouped(limits.files),
            found_words(&f)
        ))
    } else if f.bytes > limits.bytes {
        Some(format!(
            "it holds more than {} ({})",
            gigabytes(limits.bytes),
            found_words(&f)
        ))
    } else {
        None
    };
    (reason, Some(f))
}

/// How a refused wide folder can start anyway.
fn confirm_fix(ctx: &Ctx, folder: &Path, flag: bool, example: Option<&Path>) -> String {
    let start = match example {
        Some(e) => format!(
            "Start paddock in one project folder, for example {}",
            ctx.show(e)
        ),
        None => FIX.to_string(),
    };
    let entry = format!(
        "[run] allow_wide = [\"{}\"] in {}",
        ctx.show(folder),
        ctx.show(&ctx.config_file)
    );
    if flag {
        format!("{start}, or confirm this folder with {FLAG} or {entry}")
    } else {
        format!("{start}, or confirm this folder with {entry}")
    }
}

/// Check `folder` (a real path). `confirmed`: the command line confirms a
/// wide folder. `flag`: the command has `--yes-wide-folder`, for the fix.
pub fn check_with(ctx: &Ctx, folder: &Path, confirmed: bool, flag: bool, limits: &Limits) -> Check {
    let mut c = Check {
        folder: folder.to_path_buf(),
        level: Level::Ok,
        reason: String::new(),
        fix: String::new(),
        confirmed_by: None,
        found: None,
    };
    if let Some(r) = hard(ctx, folder) {
        c.level = Level::Refuse;
        c.reason = r;
        c.fix = FIX.into();
        return c;
    }
    let (reason, found) = wide(ctx, folder, limits);
    c.found = found;
    let Some(reason) = reason else {
        return c;
    };
    c.reason = reason;
    let allowed = ctx
        .allow_wide
        .iter()
        .any(|a| folder == a || folder.starts_with(a));
    if confirmed {
        c.level = Level::Confirmed;
        c.confirmed_by = Some(FLAG.into());
    } else if allowed {
        c.level = Level::Confirmed;
        c.confirmed_by = Some("[run] allow_wide".into());
    } else {
        c.level = Level::Confirm;
        let repos = c.found.as_ref().filter(|f| f.repos > limits.repos);
        let example = repos.and_then(|f| f.first_repo.as_deref());
        c.fix = confirm_fix(ctx, folder, flag, example);
    }
    c
}

/// Check the project folder of a run with this host's paths and limits.
pub fn check(env: &HostEnv, folder: &Path, confirmed: bool, flag: bool) -> Result<(Ctx, Check)> {
    let ctx = Ctx::of(env)?;
    let c = check_with(&ctx, folder, confirmed, flag, &LIMITS);
    Ok((ctx, c))
}

/// Refuse a start in `folder` that the guard does not pass.
pub fn enforce(env: &HostEnv, folder: &Path, confirmed: bool, flag: bool) -> Result<Check> {
    let (ctx, c) = check(env, folder, confirmed, flag)?;
    if let Some(line) = c.error_line(&ctx) {
        bail!("{line}");
    }
    Ok(c)
}

/// Only the hard rules: for a spawned sandbox, whose parent's start passed
/// the whole check for the same project.
pub fn enforce_hard(env: &HostEnv, folder: &Path) -> Result<()> {
    let ctx = Ctx::paths(env);
    if let Some(r) = hard(&ctx, folder) {
        bail!(
            "the agent does not start in {}: {r}. {FIX}",
            ctx.show(folder)
        );
    }
    Ok(())
}

/// The lines of `paddock doctor`: the `allow_wide` entries of the user.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    let user = match crate::manifest::Manifest::load_user(env) {
        Ok(u) => u,
        Err(e) => {
            return vec![format!(
                "  error    cannot read the user configuration: {e:#}"
            )]
        }
    };
    let ctx = Ctx::paths(env);
    let entries = &user.run.allow_wide;
    if entries.is_empty() {
        return vec![format!(
            "  none     no [run] allow_wide entries: a wide folder (a folder of projects, Documents, more than {} files) needs {FLAG}",
            grouped(LIMITS.files)
        )];
    }
    entries
        .iter()
        .map(|raw| {
            let p = env.expand(raw, &env.home);
            let real = std::fs::canonicalize(&p).ok();
            let note = match &real {
                None => "missing: the entry confirms nothing".to_string(),
                Some(r) => match hard(&ctx, r) {
                    Some(why) => format!("never allowed: {why}"),
                    None => "a wide folder that starts without the flag".to_string(),
                },
            };
            format!("  allowed  {}  ({note})", ctx.show(&p))
        })
        .collect()
}

#[cfg(test)]
#[path = "folder_check_tests.rs"]
mod tests;
