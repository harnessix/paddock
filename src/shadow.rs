//! `--profile default`: a harness runs on a private snapshot of its own host
//! data (shadow mode).
//!
//! - The data roots of the harness (`[state] data` of the profile, plus
//!   `[profiles.NAME] data` of the user configuration) are copied into
//!   `<paddock data>/shadow/runs/ID/lower`, at the same paths relative to
//!   the home. A copy is a reflink where the file system has them.
//! - The sandbox home is one overlay: that snapshot below, the run's
//!   `upper` and `work` folders above. paddock puts the credential files
//!   into `upper` before the start, as for every sandbox.
//! - At the end paddock prints what the run changed in `upper` and removes
//!   the run folder. Nothing goes back to the host.
//!
//! On macOS Seatbelt cannot mount anything, so there is no overlay: the
//! data roots are cloned (APFS `clonefileat`) into `runs/ID/home`, and the
//! harness writes that clone. The summary compares it with a baseline that
//! paddock takes after its own writes (shadow/summary.rs).
//!
//! The real home is never visible: only the data roots come in. paddock
//! knows no vendor format here: folders and files come in as bytes.

use crate::env::HostEnv;
use crate::profile::Profile;
use crate::safefs::{Dir, Kind, Root};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

mod clone;
mod summary;

pub use summary::{Baseline, Change, Summary};

use std::path::{Component, Path, PathBuf};

/// The value of `--profile` that turns shadow mode on.
pub const DEFAULT: &str = "default";

/// Folders that hold the data of many programs. A data root is never one of
/// them, only a folder inside.
pub const SHARED_PARENTS: &[&str] = &[
    ".config",
    ".local",
    ".local/share",
    ".local/state",
    ".cache",
    ".agents",
];

/// Folders with keys and passwords. A data root is never one of them or
/// inside one.
pub const SECRET_DIRS: &[&str] = &[".ssh", ".gnupg", ".pki", ".password-store"];

/// The size cap of a snapshot without reflinks, in MiB.
pub const DEFAULT_LIMIT_MIB: u64 = 2048;

/// Folders deeper than this are not copied.
const MAX_DEPTH: usize = 256;

/// The folder of all shadow runs.
pub fn runs_root(env: &HostEnv) -> PathBuf {
    env.paddock_data().join("shadow").join("runs")
}

/// The run folder of the sandbox `id`.
pub fn run_dir(env: &HostEnv, id: &str) -> PathBuf {
    runs_root(env).join(id)
}

/// Whether the sandbox `id` runs in shadow mode, for `paddock ps`.
pub fn is_shadow(env: &HostEnv, id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && run_dir(env, id).is_dir()
}

/// Check one data root, relative to the home.
pub fn validate_root(env: &HostEnv, rel: &str) -> Result<()> {
    let p = Path::new(rel);
    let bad = |why: &str| -> Result<()> {
        bail!("the data root '{rel}' is refused: {why}");
    };
    if rel.trim().is_empty() || p.is_absolute() {
        return bad("it must be a path relative to the home");
    }
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return bad("it must be a plain relative path, without '..' or '.'");
    }
    let norm: PathBuf = p.components().collect();
    if norm.as_os_str().is_empty() {
        return bad("it is the home itself");
    }
    if SHARED_PARENTS.iter().any(|s| norm == Path::new(s)) {
        return bad("it holds the data of many programs; name a folder inside it");
    }
    if SECRET_DIRS.iter().any(|s| norm.starts_with(s)) {
        return bad("it holds keys or passwords");
    }
    if crate::policy::DENIED_HOME
        .iter()
        .any(|d| norm.starts_with(d) || Path::new(d).starts_with(&norm))
    {
        return bad("it overlaps a path that paddock never mounts");
    }
    let full = env.home.join(&norm);
    for own in [
        env.paddock_data(),
        env.paddock_cache(),
        env.paddock_state(),
        env.paddock_config(),
        env.paddock_run(),
    ] {
        if full.starts_with(&own) || own.starts_with(&full) {
            return bad("it holds paddock's own folders");
        }
    }
    Ok(())
}

/// `[profiles.NAME] data` of the user configuration text.
pub fn user_roots_in(text: &str, harness: &str) -> Result<Vec<String>> {
    let v: toml::Value = toml::from_str(text).context("the user configuration is invalid")?;
    let Some(t) = v.get("profiles").and_then(|p| p.get(harness)) else {
        return Ok(Vec::new());
    };
    let Some(list) = t.get("data") else {
        return Ok(Vec::new());
    };
    let arr = list
        .as_array()
        .with_context(|| format!("[profiles.{harness}] data must be a list of paths"))?;
    arr.iter()
        .map(|x| {
            x.as_str()
                .map(str::to_string)
                .with_context(|| format!("[profiles.{harness}] data must be a list of paths"))
        })
        .collect()
}

fn user_config(env: &HostEnv) -> String {
    std::fs::read_to_string(env.paddock_config().join("config.toml")).unwrap_or_default()
}

/// The checked data roots of a harness: the profile's, then the user's.
pub fn roots(env: &HostEnv, profile: &Profile) -> Result<Vec<String>> {
    let mut all = profile.state.data.clone();
    all.extend(user_roots_in(&user_config(env), &profile.name)?);
    let mut out: Vec<String> = Vec::new();
    for r in all {
        validate_root(env, &r)?;
        let norm: PathBuf = Path::new(&r).components().collect();
        let s = norm.to_string_lossy().into_owned();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    if out.is_empty() {
        bail!(
            "the {} profile has no data roots ([state] data), so --profile default does not know which host data to copy. Add them with [profiles.{}] data = [...] in the user configuration",
            profile.name,
            profile.name
        );
    }
    Ok(out)
}

/// `[storage] shadow_copy_limit_mib` of the user configuration, in bytes.
pub fn copy_limit(env: &HostEnv) -> u64 {
    copy_limit_in(&user_config(env))
}

pub fn copy_limit_in(text: &str) -> u64 {
    let mib = toml::from_str::<toml::Value>(text)
        .ok()
        .and_then(|v| v.get("storage")?.get("shadow_copy_limit_mib")?.as_integer())
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
        .unwrap_or(DEFAULT_LIMIT_MIB);
    mib.saturating_mul(1 << 20)
}

/// How the sandbox home holds the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapKind {
    /// Linux: an overlay of `upper` on the copy in `lower`.
    Overlay,
    /// macOS: one writable clone in `home`, and no mount.
    Clone,
}

impl SnapKind {
    /// The kind of this platform.
    pub fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::Clone
        } else {
            Self::Overlay
        }
    }
}

/// What the spec records of a shadow run.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub profile: String,
    pub kind: SnapKind,
    pub dir: PathBuf,
    /// The sandbox home (`home_src`): the upper folder of an overlay, or the
    /// clone.
    pub home: PathBuf,
    /// The overlay folders. None for a clone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<PathBuf>,
    /// The data roots, relative to the home.
    pub roots: Vec<String>,
    /// The credential files (`[state] auth`), never in the snapshot.
    pub auth: Vec<String>,
}

impl Plan {
    /// Make the empty run folder of the sandbox `id`, of the kind of this
    /// platform. It must be on the file system of the home, so a copy can
    /// be a reflink or a clone.
    pub fn create(env: &HostEnv, id: &str, roots: Vec<String>, auth: Vec<String>) -> Result<Self> {
        Self::create_kind(env, id, roots, auth, SnapKind::native())
    }

    pub fn create_kind(
        env: &HostEnv,
        id: &str,
        roots: Vec<String>,
        auth: Vec<String>,
        kind: SnapKind,
    ) -> Result<Self> {
        if id.is_empty() || id.contains('/') || id.starts_with('.') {
            bail!("invalid sandbox id {id}");
        }
        let dir = run_dir(env, id);
        crate::privfs::dir(&env.paddock_data())?;
        crate::privfs::dir(&env.paddock_data().join("shadow"))?;
        crate::privfs::dir(&runs_root(env))?;
        if dir.exists() {
            remove_dir(&dir)?;
        }
        crate::privfs::dir(&dir)?;
        let overlay = |n: &str| (kind == SnapKind::Overlay).then(|| dir.join(n));
        let plan = Self {
            profile: DEFAULT.into(),
            kind,
            home: match kind {
                SnapKind::Overlay => dir.join("upper"),
                SnapKind::Clone => dir.join("home"),
            },
            lower: overlay("lower"),
            upper: overlay("upper"),
            work: overlay("work"),
            dir,
            roots,
            auth,
        };
        for d in [&plan.lower, &plan.upper, &plan.work].into_iter().flatten() {
            crate::privfs::dir(d)?;
        }
        crate::privfs::dir(&plan.home)?;
        let home_dev = std::fs::metadata(&env.home)
            .with_context(|| format!("cannot read {}", env.home.display()))?
            .dev();
        let run_dev = std::fs::metadata(&plan.dir)?.dev();
        if home_dev != run_dev {
            let d = plan.dir.clone();
            let _ = remove_dir(&d);
            bail!(
                "--profile default needs its run folder on the file system of the home, and {} is on another one. Set XDG_DATA_HOME to a folder on the file system of the home",
                runs_root(env).display()
            );
        }
        Ok(plan)
    }

    /// The overlay at the home `at`. None for a clone: it is the home.
    pub fn overlay_mount(&self, at: &Path) -> Option<crate::spec::Mount> {
        let (Some(lower), Some(upper), Some(work)) = (&self.lower, &self.upper, &self.work) else {
            return None;
        };
        let why = "profile default: a private snapshot of the data roots";
        Some(crate::spec::Mount::overlay(lower, upper, work, at, why))
    }

    /// The root that holds the home-relative path `rel`, if any.
    pub fn root_of(&self, rel: &Path) -> Option<&str> {
        self.roots
            .iter()
            .find(|r| rel.starts_with(Path::new(r)))
            .map(String::as_str)
    }

    fn is_auth(&self, rel: &Path) -> bool {
        self.auth.iter().any(|a| {
            let a = a.trim_end_matches('/');
            rel.starts_with(Path::new(a))
        })
    }

    /// The folder that the snapshot is copied into: `lower`, or the clone.
    pub fn snapshot_dir(&self) -> &Path {
        self.lower.as_deref().unwrap_or(&self.home)
    }

    /// Copy the data roots of `home` into the snapshot folder. `reflink`
    /// None: probe the file system; tests force it.
    pub fn fill(&self, home: &Path, limit: u64, reflink: Option<bool>) -> Result<Copied> {
        let root = Root::open(home)?;
        let lower = Root::open(self.snapshot_dir())?;
        let clones = self.kind == SnapKind::Clone;
        // Pass one: the size and the first file, without copying.
        let mut size = Sizer::default();
        self.walk(home, &root, &mut size)?;
        let reflinks = match reflink {
            Some(r) => r,
            None => match &size.first {
                Some(rel) => clone::probe_reflink(&root, &self.dir, rel),
                None => true,
            },
        };
        if !reflinks && size.bytes > limit {
            bail!(
                "--profile default copies {} of host data, and this file system has no {}, so each byte is a real copy. The cap is {}. Raise [storage] shadow_copy_limit_mib in the user configuration, or remove old data from the data roots ({})",
                human(size.bytes),
                if clones { "clones" } else { "reflinks" },
                human(limit),
                self.roots.join(", ")
            );
        }
        // The sandbox home of a clone is not at the host home: an absolute
        // link into a root points into the clone instead.
        let relink = clones.then(|| (home.to_path_buf(), self.home.clone()));
        let mut copier = Copier {
            lower,
            reflinks,
            keep: clones,
            relink,
            out: Copied {
                reflinks,
                clones,
                ..Default::default()
            },
        };
        self.walk(home, &root, &mut copier)?;
        Ok(copier.out)
    }

    fn walk(&self, home_path: &Path, home: &Root, v: &mut dyn Visit) -> Result<()> {
        let home_dir = home_path;
        for r in &self.roots {
            let rel = PathBuf::from(r);
            let parent = rel.parent().unwrap_or(Path::new(""));
            let Some(name) = rel.file_name() else {
                continue;
            };
            let dir = match home.dir(parent, false) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let stats = dir.stats()?;
            let Some((_, st)) = stats.iter().find(|(n, _)| n == name) else {
                continue;
            };
            if self.is_auth(&rel) {
                continue;
            }
            match st.kind {
                Kind::Dir => {
                    v.dir(&rel)?;
                    let sub = dir.subdir(name, false)?;
                    self.walk_dir(home_dir, &sub, &rel, &rel, v, 1)?;
                }
                Kind::File => v.file(&dir, name, &rel, st.size)?,
                // A root that is a symlink leaves the root.
                Kind::Symlink => v.skip_link(&rel),
                Kind::Socket | Kind::Other => v.skip_special(&rel),
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_dir(
        &self,
        home: &Path,
        dir: &Dir,
        rel: &Path,
        root: &Path,
        v: &mut dyn Visit,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            bail!("{} is deeper than {MAX_DEPTH} folders", rel.display());
        }
        let mut stats = dir.stats()?;
        stats.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, st) in stats {
            let r = rel.join(&name);
            if self.is_auth(&r) {
                continue;
            }
            match st.kind {
                Kind::Dir => {
                    let Ok(sub) = dir.subdir(&name, false) else {
                        continue;
                    };
                    v.dir(&r)?;
                    self.walk_dir(home, &sub, &r, root, v, depth + 1)?;
                }
                Kind::File => v.file(dir, &name, &r, st.size)?,
                Kind::Symlink => match dir.read_link(&name) {
                    Ok(t) if link_stays_inside(home, &r, &t, root) => v.link(&r, &t)?,
                    _ => v.skip_link(&r),
                },
                Kind::Socket | Kind::Other => v.skip_special(&r),
            }
        }
        Ok(())
    }

    /// Before paddock writes a settings file into the upper folder, copy
    /// the snapshot's version up, so the write changes it instead of
    /// hiding it. A clone has the file already.
    pub fn copy_up(&self, rel: &Path) -> Result<()> {
        let (Some(upper_dir), Some(lower_dir)) = (&self.upper, &self.lower) else {
            return Ok(());
        };
        if upper_dir.join(rel).symlink_metadata().is_ok() {
            return Ok(());
        }
        let Ok(m) = lower_dir.join(rel).symlink_metadata() else {
            return Ok(());
        };
        if !m.is_file() {
            return Ok(());
        }
        let parent = rel.parent().unwrap_or(Path::new(""));
        let upper = Root::open(upper_dir)?;
        let low = Root::open(lower_dir)?;
        let to = upper.dir(parent, true)?;
        let from = low.dir(parent, false)?;
        let name = rel.file_name().context("no file name")?;
        clone::copy_file(&from, name, &to, name, true)?;
        Ok(())
    }

    /// Remove the run folder.
    pub fn remove(&self) -> Result<()> {
        remove_dir(&self.dir)
    }
}

/// Remove a run folder whatever the modes inside (the overlay leaves a
/// work folder with mode 000, and a run can make read-only folders).
pub fn remove_dir(dir: &Path) -> Result<()> {
    if std::fs::symlink_metadata(dir).is_err() {
        return Ok(());
    }
    if std::fs::remove_dir_all(dir).is_ok() {
        return Ok(());
    }
    open_up(dir, 0);
    std::fs::remove_dir_all(dir).with_context(|| format!("cannot remove {}", dir.display()))
}

fn open_up(dir: &Path, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            open_up(&e.path(), depth + 1);
        }
    }
}

/// The sandbox ended or never started: remove its run folder.
pub fn discard(spec: &crate::spec::SandboxSpec) {
    if let Some(p) = &spec.shadow {
        if let Err(e) = p.remove() {
            log::warn!("{e:#}");
        }
    }
}

/// What a snapshot copied.
#[derive(Debug, Clone, Default)]
pub struct Copied {
    pub files: usize,
    pub bytes: u64,
    pub reflinks: bool,
    /// A clone run (macOS): the copies are APFS clones.
    pub clones: bool,
    /// Symlinks that leave their root.
    pub skipped_links: Vec<PathBuf>,
    /// Sockets, FIFOs and devices.
    pub skipped_special: Vec<PathBuf>,
}

impl Copied {
    pub fn line(&self) -> String {
        let mut s = format!(
            "profile default: a private snapshot of {} file(s), {}{}",
            self.files,
            human(self.bytes),
            match (self.clones, self.reflinks) {
                (true, true) => " (clones)",
                (true, false) => " (plain copies: no clones here)",
                (false, true) => "",
                (false, false) => " (plain copies: no reflinks here)",
            }
        );
        if !self.skipped_links.is_empty() {
            s.push_str(&format!(
                ", {} symlink(s) that leave their root skipped",
                self.skipped_links.len()
            ));
        }
        if !self.skipped_special.is_empty() {
            s.push_str(&format!(
                ", {} socket(s), FIFO(s) or device(s) skipped",
                self.skipped_special.len()
            ));
        }
        s.push_str(". The run changes only this snapshot");
        s
    }
}

trait Visit {
    fn dir(&mut self, rel: &Path) -> Result<()>;
    fn file(&mut self, dir: &Dir, name: &OsStr, rel: &Path, size: u64) -> Result<()>;
    fn link(&mut self, _rel: &Path, _target: &Path) -> Result<()> {
        Ok(())
    }
    fn skip_link(&mut self, _rel: &Path) {}
    fn skip_special(&mut self, _rel: &Path) {}
}

#[derive(Default)]
struct Sizer {
    bytes: u64,
    first: Option<PathBuf>,
}

impl Visit for Sizer {
    fn dir(&mut self, _rel: &Path) -> Result<()> {
        Ok(())
    }
    fn file(&mut self, _dir: &Dir, _name: &OsStr, rel: &Path, size: u64) -> Result<()> {
        self.bytes = self.bytes.saturating_add(size);
        if self.first.is_none() && size > 0 {
            self.first = Some(rel.to_path_buf());
        }
        Ok(())
    }
}

struct Copier {
    lower: Root,
    reflinks: bool,
    /// A clone: a file that paddock wrote into the home before the copy
    /// stays, as the upper folder of an overlay hides the lower one.
    keep: bool,
    /// A clone: an absolute link target in the host home (first) moves to
    /// the clone (second).
    relink: Option<(PathBuf, PathBuf)>,
    out: Copied,
}

impl Visit for Copier {
    fn dir(&mut self, rel: &Path) -> Result<()> {
        self.lower.dir(rel, true)?;
        Ok(())
    }
    fn file(&mut self, dir: &Dir, name: &OsStr, rel: &Path, _size: u64) -> Result<()> {
        let parent = rel.parent().unwrap_or(Path::new(""));
        let to = self.lower.dir(parent, true)?;
        if self.keep && to.kind(name)?.is_some() {
            return Ok(());
        }
        // A file that went away since the walk is no error.
        let n = clone::copy_file(dir, name, &to, name, self.reflinks)
            .with_context(|| format!("cannot copy ~/{}", rel.display()))?;
        if let Some(n) = n {
            self.out.files += 1;
            self.out.bytes += n;
        }
        Ok(())
    }
    fn link(&mut self, rel: &Path, target: &Path) -> Result<()> {
        let parent = rel.parent().unwrap_or(Path::new(""));
        let to = self.lower.dir(parent, true)?;
        let name = rel.file_name().context("no file name")?;
        if self.keep && to.kind(name)?.is_some() {
            return Ok(());
        }
        let moved = match &self.relink {
            Some((host, clone)) => target.strip_prefix(host).ok().map(|r| clone.join(r)),
            None => None,
        };
        to.symlink(moved.as_deref().unwrap_or(target), name)
    }
    fn skip_link(&mut self, rel: &Path) {
        self.out.skipped_links.push(rel.to_path_buf());
    }
    fn skip_special(&mut self, rel: &Path) {
        self.out.skipped_special.push(rel.to_path_buf());
    }
}

/// Whether the symlink at `rel` (relative to the home) with `target` stays
/// inside the data root `root`. Lexical: each symlink on the way is checked
/// on its own, so a chain of links inside the root stays inside.
pub fn link_stays_inside(home: &Path, rel: &Path, target: &Path, root: &Path) -> bool {
    // The home is at its host path in the sandbox too, so an absolute
    // target inside the root works there.
    let (start, target) = match target.strip_prefix(home) {
        Ok(t) if target.is_absolute() => (Path::new(""), t),
        _ if target.is_absolute() => return false,
        _ => (rel.parent().unwrap_or(Path::new("")), target),
    };
    let mut cur: Vec<&OsStr> = start
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n),
            _ => None,
        })
        .collect();
    for c in target.components() {
        match c {
            Component::Normal(n) => cur.push(n),
            Component::CurDir => {}
            Component::ParentDir => {
                if cur.pop().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    let p: PathBuf = cur.iter().collect();
    p.starts_with(root)
}

fn human(bytes: u64) -> String {
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

/// Host processes of this harness that run outside any sandbox now: same
/// user namespace as paddock, and the harness executable or one of its
/// command names.
pub fn host_copies(profile: &Profile, exe: &Path) -> Vec<u32> {
    let own_ns = std::fs::read_link("/proc/self/ns/user").ok();
    let me = std::process::id();
    let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid == me {
            continue;
        }
        let base = e.path();
        if std::fs::read_link(base.join("ns/user")).ok() != own_ns {
            continue;
        }
        let by_exe = std::fs::read_link(base.join("exe")).is_ok_and(|p| p == exe);
        let by_name = std::fs::read(base.join("cmdline")).is_ok_and(|c| {
            let argv0 = c.split(|b| *b == 0).next().unwrap_or_default();
            let name = Path::new(OsStr::from_bytes(argv0))
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            profile.locate.commands.contains(&name)
        });
        if by_exe || by_name {
            out.push(pid);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests;
