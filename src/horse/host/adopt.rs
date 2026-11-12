//! `paddock horse adopt`: host sessions join horse, now and from then on.
//!
//! Adoption is opt-in and per harness. `paddock horse adopt HARNESS`
//! records the chosen locations (folders below the transcript folder of
//! the harness) and the horse of the adopted sessions, then converts every
//! session there (import.rs). From then on a sweep reads each session
//! file that is new or changed since the last sweep: `paddock horse adopt
//! --sync`, `paddock horse adopt --watch`, and the hooks that adoption
//! puts into the host configuration of the harness (hooks.rs). `paddock
//! horse adopt --off` ends it. The sessions stay in the store.
//!
//! The state is one file, `<paddock state>/horse/adoption.json` (0600). A
//! lock file beside it serializes the sweeps, so two hooks that fire at
//! once do not both read the same file.

use super::import::{self, Imported};
use super::Found;
use crate::horse::Store;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The size, the mtime and the inode of each file of a session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen(pub BTreeMap<PathBuf, (u64, i64, i64, u64)>);

impl Seen {
    fn of(home: &Path, f: &Found) -> Self {
        let mut m = BTreeMap::new();
        for rel in &f.files {
            if let Ok(md) = std::fs::symlink_metadata(home.join(rel)) {
                m.insert(
                    rel.clone(),
                    (md.len(), md.mtime(), md.mtime_nsec(), md.ino()),
                );
            }
        }
        Self(m)
    }
}

/// The adoption of one harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adoption {
    pub harness: String,
    /// The host home of the sessions.
    pub home: PathBuf,
    /// The watched folders, relative to the home.
    pub locations: Vec<PathBuf>,
    /// The horse of the sessions that adoption brings in.
    pub horse: String,
    pub since_ms: u64,
    /// What the last sweep read of each session, by its path.
    #[serde(default)]
    pub seen: BTreeMap<PathBuf, Seen>,
    /// The sessions that the last sweep could not read, and why.
    #[serde(default)]
    pub errors: BTreeMap<PathBuf, String>,
    /// The hook entries that adoption added to the host configuration of
    /// the harness (hooks.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<super::hooks::Installed>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub harnesses: BTreeMap<String, Adoption>,
}

/// The folder of the adoption state: `<paddock state>/horse`.
pub fn dir(state: &Path) -> PathBuf {
    state.join("horse")
}

fn file(state: &Path) -> PathBuf {
    dir(state).join("adoption.json")
}

/// The adoption state under its lock. The lock goes when it drops.
pub struct Locked {
    _f: std::fs::File,
    state_dir: PathBuf,
    pub state: State,
}

impl Locked {
    /// The state under its lock; waits as long as another holds it.
    pub fn open(state: &Path) -> Result<Self> {
        Self::open_within(state, None)?.context("cannot lock the adoption state")
    }

    /// The state under its lock, or None when another holds the lock for
    /// longer than `wait` (None: wait as long as it takes). A hook must
    /// not wait for a long sweep: its harness kills it after a time.
    pub fn open_within(state: &Path, wait: Option<Duration>) -> Result<Option<Self>> {
        crate::privfs::dir(&dir(state))?;
        let Some(f) = flock_within(&dir(state).join("adoption.lock"), wait)? else {
            return Ok(None);
        };
        let state_v = match std::fs::read(file(state)) {
            Ok(b) => serde_json::from_slice(&b).context("the adoption state is damaged")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(Some(Self {
            _f: f,
            state_dir: state.to_path_buf(),
            state: state_v,
        }))
    }

    pub fn save(&self) -> Result<()> {
        crate::privfs::write_atomic(
            &file(&self.state_dir),
            serde_json::to_vec_pretty(&self.state)?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: `adopt` stops after it saved the state and before it writes
    /// the hook file, as a crash there would.
    pub static CRASH_BEFORE_HOOK_FILE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// An exclusive flock of the file at `path`, or None when another holds it
/// for longer than `wait` (None: wait as long as it takes).
fn flock_within(path: &Path, wait: Option<Duration>) -> Result<Option<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)?;
    let until = wait.map(|w| Instant::now() + w);
    loop {
        let how = if until.is_some() {
            libc::LOCK_EX | libc::LOCK_NB
        } else {
            libc::LOCK_EX
        };
        // SAFETY: flock on an open fd.
        if unsafe { libc::flock(f.as_raw_fd(), how) } == 0 {
            return Ok(Some(f));
        }
        let e = std::io::Error::last_os_error();
        match until {
            Some(u) if e.raw_os_error() == Some(libc::EWOULDBLOCK) => {
                if Instant::now() >= u {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            _ if e.kind() == std::io::ErrorKind::Interrupted => {}
            _ => bail!("cannot lock {}: {e}", path.display()),
        }
    }
}

/// The delivery lock of one host session: two hooks of the session take
/// turns, so each queued entry goes out once. None: another hook held it
/// for longer than `wait`.
pub fn session_lock(state: &Path, session: &str, wait: Duration) -> Result<Option<std::fs::File>> {
    let d = dir(state).join("deliver");
    crate::privfs::dir(&d)?;
    flock_within(&d.join(format!("{session}.lock")), Some(wait))
}

/// The adoption state without the lock, for a look.
pub fn load(state: &Path) -> Result<State> {
    match std::fs::read(file(state)) {
        Ok(b) => Ok(serde_json::from_slice(&b).context("the adoption state is damaged")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}

/// The staging folder for the copies of a sweep.
fn staging(state: &Path) -> PathBuf {
    dir(state).join("staging")
}

/// What a sweep did for one harness.
#[derive(Debug, Default)]
pub struct Swept {
    pub harness: String,
    pub imported: Vec<Imported>,
    pub failed: Vec<(PathBuf, String)>,
    /// Sessions that did not change since the last sweep.
    pub unchanged: usize,
    /// Changed sessions that the deadline of a hook left for the next sweep.
    pub left: usize,
    /// Problems that did not stop the adopt, for example a hook file that
    /// paddock cannot read when it repairs the hook entries.
    pub warnings: Vec<String>,
}

/// Start the adoption of `harness`: record it and convert the sessions
/// of the locations. A second adopt of the same harness adds locations,
/// and puts the hook entries in again when their program changed or went
/// (an upgrade moved it).
/// `hooks`: the paddock program for the host hook entries (hooks.rs);
/// None: no hook entries, the sweeps of `--sync` and `--watch` only.
pub fn adopt(
    store: &Store,
    state: &Path,
    harness: &str,
    home: &Path,
    locations: &[PathBuf],
    hooks: Option<&Path>,
) -> Result<Swept> {
    if !super::HARNESSES.contains(&harness) {
        bail!(
            "paddock cannot adopt the host sessions of {harness}; it reads {}",
            super::HARNESSES.join(", ")
        );
    }
    let home = super::canon_home(home);
    let home = home.as_path();
    let root = PathBuf::from(super::root(harness)?);
    let mut locs = Vec::new();
    for l in locations {
        let l = if l.is_absolute() {
            super::canon_home(l)
        } else {
            l.clone()
        };
        let rel = match l.strip_prefix(home) {
            Ok(r) => r.to_path_buf(),
            Err(_) if l.is_absolute() => {
                bail!("{} is not inside the home {}", l.display(), home.display())
            }
            Err(_) => l.clone(),
        };
        super::check_rel(&rel)?;
        if !rel.starts_with(&root) {
            bail!(
                "{} is not inside the {harness} transcript folder ~/{}",
                rel.display(),
                root.display()
            );
        }
        locs.push(rel);
    }
    if locs.is_empty() {
        locs.push(root);
    }
    let backups = dir(state).join("hooks-backup");
    let mut warnings = Vec::new();
    let mut l = Locked::open(state)?;
    match l.state.harnesses.get_mut(harness) {
        Some(a) => {
            if super::canon_home(&a.home) != home {
                bail!(
                    "{harness} is adopted from the home {} already; run `paddock horse adopt --off {harness}` first",
                    a.home.display()
                );
            }
            for x in locs {
                if !a.locations.contains(&x) {
                    a.locations.push(x);
                }
            }
            l.save()?;
            if let Some(exe) = hooks {
                // The adoption is there already: a hook file that paddock
                // cannot change now (the user broke its JSON, say) does not
                // stop the sweep of the new locations.
                if let Some(w) = repair_hooks(&mut l, harness, home, &backups, exe)? {
                    log::warn!("horse: {w}");
                    warnings.push(w);
                }
            }
        }
        None => {
            // The checks of the hook file first: when the file is refused,
            // nothing else happens.
            let plan = match hooks {
                Some(exe) => super::hooks::plan(home, &backups, harness, exe)?,
                None => None,
            };
            let horse = store.new_default_horse()?;
            let a = Adoption {
                harness: harness.into(),
                home: home.to_path_buf(),
                locations: locs,
                horse: horse.id,
                since_ms: crate::horse::now_ms(),
                seen: BTreeMap::new(),
                errors: BTreeMap::new(),
                hooks: plan.as_ref().map(|p| p.installed().clone()),
            };
            l.state.harnesses.insert(harness.into(), a);
            // The record of the hook entries is saved before the hook file
            // changes: after a crash between the two, `--off` knows them.
            l.save()?;
            #[cfg(test)]
            if CRASH_BEFORE_HOOK_FILE.with(std::cell::Cell::get) {
                bail!("test: paddock dies before the hook file changes");
            }
            if let Some(p) = plan {
                if let Err(e) = p.apply() {
                    l.state.harnesses.remove(harness);
                    l.save()?;
                    return Err(e);
                }
            }
        }
    }
    let mut swept = sweep_one(store, state, &mut l, harness, None, None)?;
    l.save()?;
    swept.warnings = warnings;
    Ok(swept)
}

/// Put the hook entries of an adoption in again when their command is not
/// the one of `exe` (the program moved), or put them in when the adoption
/// had none.
///
/// Some(warning): the hook file cannot be read or changed, and the hook
/// entries stay as they are. The record of the adoption matches the file
/// then. An error: the adoption state cannot be saved.
fn repair_hooks(
    l: &mut Locked,
    harness: &str,
    home: &Path,
    backups: &Path,
    exe: &Path,
) -> Result<Option<String>> {
    let a = l
        .state
        .harnesses
        .get(harness)
        .expect("the adoption is there")
        .clone();
    if let Some(h) = &a.hooks {
        let want: Vec<String> = h
            .entries
            .iter()
            .map(|e| super::hooks::command(exe, harness, &e.event))
            .collect();
        let have: Vec<&String> = h.entries.iter().map(|e| &e.command).collect();
        // The record alone does not tell: a crash after its save and before
        // the write of the hook file leaves a record of entries that the
        // file does not have. So the file is read.
        let missing = match super::hooks::missing(home, h) {
            Ok(m) => m,
            Err(e) => {
                return Ok(Some(format!(
                    "paddock cannot read its hook entries in ~/{}, so it leaves them as they are: {e:#}",
                    h.file.display()
                )))
            }
        };
        if have.iter().zip(&want).all(|(a, b)| *a == b)
            && have.len() == want.len()
            && !h.program_missing()
            && missing.is_empty()
        {
            return Ok(None);
        }
        if let Err(e) = super::hooks::remove(home, h) {
            return Ok(Some(format!(
                "paddock cannot take the old hook entries out of ~/{}, so it leaves them as they are: {e:#}",
                h.file.display()
            )));
        }
        // The old entries are out of the file: the record says so before
        // anything else can fail.
        l.state
            .harnesses
            .get_mut(harness)
            .expect("the adoption is there")
            .hooks = None;
        l.save()?;
    }
    let plan = match super::hooks::plan(home, backups, harness, exe) {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(None),
        Err(e) => {
            return Ok(Some(format!(
                "paddock cannot put its hook entries in for {harness}: {e:#}"
            )))
        }
    };
    let entry = l
        .state
        .harnesses
        .get_mut(harness)
        .expect("the adoption is there");
    entry.hooks = Some(plan.installed().clone());
    l.save()?;
    #[cfg(test)]
    if CRASH_BEFORE_HOOK_FILE.with(std::cell::Cell::get) {
        bail!("test: paddock dies before the hook file changes");
    }
    if let Err(e) = plan.apply() {
        l.state
            .harnesses
            .get_mut(harness)
            .expect("the adoption is there")
            .hooks = None;
        l.save()?;
        return Ok(Some(format!(
            "paddock cannot write its hook entries for {harness}: {e:#}"
        )));
    }
    Ok(None)
}

/// End the adoption of `harness`: paddock's hook entries go, then the
/// adoption. The sessions stay in the store. When the hook file cannot be
/// changed, the adoption stays, so `--off` can run again.
pub fn off(
    state: &Path,
    harness: &str,
) -> Result<Option<(Adoption, Option<super::hooks::Removed>)>> {
    let mut l = Locked::open(state)?;
    let Some(a) = l.state.harnesses.get(harness).cloned() else {
        return Ok(None);
    };
    let removed = match &a.hooks {
        Some(h) => Some(super::hooks::remove(&a.home, h).with_context(|| {
            format!("the adoption of {harness} stays on: paddock cannot take its hook entries out of ~/{}", h.file.display())
        })?),
        None => None,
    };
    l.state.harnesses.remove(harness);
    l.save()?;
    Ok(Some((a, removed)))
}

/// Read every new or changed session of the adopted locations. `only`:
/// one harness; None: each adopted harness.
pub fn sweep(store: &Store, state: &Path, only: Option<&str>) -> Result<Vec<Swept>> {
    let mut l = Locked::open(state)?;
    let names: Vec<String> = l
        .state
        .harnesses
        .keys()
        .filter(|h| only.is_none_or(|o| o == h.as_str()))
        .cloned()
        .collect();
    let mut out = Vec::new();
    for h in names {
        out.push(sweep_one(store, state, &mut l, &h, None, None)?);
        l.save()?;
    }
    Ok(out)
}

/// The limits of the sweep of a hook. Its harness kills a hook after a
/// time (Claude Code: 30 s), so a hook never waits long for the lock of a
/// sweep, and reads what it can before its deadline. What it leaves is
/// read by the next sweep.
#[derive(Clone, Debug)]
pub struct Bounds {
    /// The longest wait for the lock of another sweep.
    pub wait: Duration,
    /// The time for the reads.
    pub budget: Duration,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            wait: Duration::from_secs(5),
            budget: Duration::from_secs(10),
        }
    }
}

/// The sweep of a hook: the session at `transcript` (an absolute path in
/// the home of the adoption) when it is in an adopted location, else every
/// new or changed session, within `bounds`. None: another sweep held the
/// lock, nothing was read.
pub fn sweep_hook(
    store: &Store,
    state: &Path,
    harness: &str,
    transcript: Option<&Path>,
    bounds: &Bounds,
) -> Result<Option<Swept>> {
    let Some(mut l) = Locked::open_within(state, Some(bounds.wait))? else {
        return Ok(None);
    };
    let deadline = Instant::now() + bounds.budget;
    let Some(a) = l.state.harnesses.get(harness).cloned() else {
        return Ok(None);
    };
    // The path of the hook input can name the home by another path (a
    // symlink); the adoption has the real one.
    let transcript = transcript.map(|t| match (t.parent(), t.file_name()) {
        (Some(d), Some(n)) => super::canon_home(d).join(n),
        _ => t.to_path_buf(),
    });
    let only = transcript
        .as_deref()
        .and_then(|t| t.strip_prefix(&a.home).ok())
        .map(Path::to_path_buf)
        .filter(|rel| a.locations.iter().any(|loc| rel.starts_with(loc)))
        .map(|rel| match harness {
            // A Grok session is its folder.
            super::grok::HARNESS => rel.parent().map(Path::to_path_buf).unwrap_or(rel),
            _ => rel,
        });
    let swept = sweep_one(
        store,
        state,
        &mut l,
        harness,
        only.as_deref(),
        Some(deadline),
    )?;
    l.save()?;
    Ok(Some(swept))
}

/// One sweep of `harness`. `only`: the session (file, or Grok folder) to
/// read, relative to the home; None: every session of the locations.
/// `deadline`: the sessions not read by then wait for the next sweep.
fn sweep_one(
    store: &Store,
    state: &Path,
    l: &mut Locked,
    harness: &str,
    only: Option<&Path>,
    deadline: Option<Instant>,
) -> Result<Swept> {
    let a = l
        .state
        .harnesses
        .get(harness)
        .cloned()
        .with_context(|| format!("{harness} is not adopted"))?;
    let select = match only {
        Some(rel) => import::Select::Under(vec![rel.to_path_buf()]),
        None => import::Select::Under(a.locations.clone()),
    };
    let (found, _) = import::choose(&a.home, harness, &select)?;
    let mut todo = Vec::new();
    let mut seen_now = BTreeMap::new();
    let mut unchanged = 0;
    for f in found {
        let seen = Seen::of(&a.home, &f);
        if a.seen.get(&f.rel) == Some(&seen) {
            unchanged += 1;
        } else {
            todo.push(f.clone());
        }
        seen_now.insert(f.rel.clone(), seen);
    }
    // The fingerprint comes before the copy: a file that changes during the
    // copy is read again by the next sweep.
    let mut report = import::Report::default();
    let mut left = Vec::new();
    for f in todo {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            left.push(f.rel.clone());
            continue;
        }
        let r = import::import(
            store,
            &staging(state),
            harness,
            &a.home,
            std::slice::from_ref(&f),
            Some(&a.horse),
            "adopt",
        )?;
        report.imported.extend(r.imported);
        report.failed.extend(r.failed);
    }
    let entry = l
        .state
        .harnesses
        .get_mut(harness)
        .expect("the adoption is there");
    // A session that failed is read again when it changes.
    for (rel, why) in &report.failed {
        entry.errors.insert(rel.clone(), why.clone());
    }
    for i in &report.imported {
        entry.errors.remove(&i.rel);
    }
    // A session that the deadline left keeps its old fingerprint, so the
    // next sweep reads it.
    for rel in &left {
        match entry.seen.get(rel) {
            Some(old) => {
                seen_now.insert(rel.clone(), old.clone());
            }
            None => {
                seen_now.remove(rel);
            }
        }
    }
    match only {
        // One session: the fingerprints of the others stay.
        Some(_) => entry.seen.extend(seen_now),
        None => {
            entry.errors.retain(|rel, _| seen_now.contains_key(rel));
            // A session file that went from the host stays in the store;
            // only its fingerprint goes.
            entry.seen = seen_now;
        }
    }
    Ok(Swept {
        harness: harness.into(),
        imported: report.imported,
        failed: report.failed,
        unchanged,
        left: left.len(),
        warnings: Vec::new(),
    })
}
