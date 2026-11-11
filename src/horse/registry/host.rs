//! The files of the registry on the host, under `<state>/registry`:
//! `local.key` (the signing key), `trust.toml` (the trust policy) and
//! `registry.toml` (the indexes). The folder is 0700 and every file 0600.
//! No sandbox mounts or reads any of them: a sandbox asks the broker.
//!
//! A change of `trust.toml` or `registry.toml` reads, changes and writes
//! the file under one lock, so two edits at once do not lose one.

use super::index::{Config, IndexEntry};
use super::reference::valid_ident;
use super::trust::{Trust, TrustedKey};
use crate::privfs;
use anyhow::{bail, Context as _, Result};
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

/// The registry folder of a state folder.
pub fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join("registry")
}

/// The signing key of this host.
pub fn key_path(state_dir: &Path) -> PathBuf {
    dir(state_dir).join("local.key")
}

/// The trust policy.
pub fn trust_path(state_dir: &Path) -> PathBuf {
    dir(state_dir).join("trust.toml")
}

/// The list of indexes.
pub fn config_path(state_dir: &Path) -> PathBuf {
    dir(state_dir).join("registry.toml")
}

/// The lock of the registry folder, held until the file is dropped.
fn lock(state_dir: &Path) -> Result<File> {
    let d = dir(state_dir);
    privfs::dir(&d).with_context(|| format!("cannot create {}", d.display()))?;
    let path = d.join(".lock");
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(privfs::FILE_MODE)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    // SAFETY: flock on a file descriptor that `f` owns.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
        bail!(
            "cannot lock {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(f)
}

/// Add a trusted key for `org`. Returns false when the same key is
/// already trusted for `org`; its name then changes to `name` when given.
pub fn trust_add(state_dir: &Path, org: &str, public: &str, name: Option<&str>) -> Result<bool> {
    let _lock = lock(state_dir)?;
    let path = trust_path(state_dir);
    let mut t = Trust::load(&path)?;
    let public = public.trim().to_ascii_lowercase();
    let added = match t
        .keys
        .iter_mut()
        .find(|k| k.org == org && k.public == public)
    {
        Some(k) => {
            if let Some(n) = name {
                k.name = Some(n.to_string());
            }
            false
        }
        None => {
            t.keys.push(TrustedKey {
                org: org.to_string(),
                public,
                name: name.map(str::to_string),
            });
            true
        }
    };
    t.save(&path)?;
    Ok(added)
}

/// Add an index `name` at `path`. Without `registry.toml`, the file is
/// made with the default index `local` first, so the local index stays.
/// The path must be absolute; it need not exist yet.
pub fn add_index(state_dir: &Path, name: &str, path: &Path) -> Result<()> {
    if !valid_ident(name) {
        bail!("the index name {name:?} must be lower-case letters, digits and dashes");
    }
    if !path.is_absolute() {
        bail!("the index path {} must be absolute", path.display());
    }
    let _lock = lock(state_dir)?;
    let file = config_path(state_dir);
    let mut c: Config = match std::fs::read_to_string(&file) {
        Ok(t) => toml::from_str(&t).with_context(|| format!("{} is not valid", file.display()))?,
        Err(e) if e.kind() == ErrorKind::NotFound => Config {
            index: vec![IndexEntry {
                name: "local".into(),
                path: dir(state_dir).join("local"),
            }],
        },
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    if let Some(e) = c.index.iter().find(|e| e.name == name) {
        if e.path == path {
            return Ok(());
        }
        bail!(
            "an index {name} is already set up at {}; choose another name",
            e.path.display()
        );
    }
    c.index.push(IndexEntry {
        name: name.to_string(),
        path: path.to_path_buf(),
    });
    let text = toml::to_string(&c).context("cannot encode the index list")?;
    privfs::write_atomic(&file, text).with_context(|| format!("cannot write {}", file.display()))
}

/// How long `sync` waits for git.
pub const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// What `sync` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Synced {
    /// The index is a git checkout, and `git pull --ff-only` worked. Its
    /// output, for the host user only.
    Pulled(String),
    /// The index is a plain folder: there is nothing to pull.
    Plain,
}

/// Bring the index `name` up to date: `git -C <path> pull --ff-only` for
/// a git checkout, nothing for a plain folder. git never asks for a
/// password here (there is no terminal), and it stops after
/// `SYNC_TIMEOUT`. This is the only network path of the registry.
pub fn sync(state_dir: &Path, name: &str) -> Result<Synced> {
    let Some(index) = super::configured(state_dir)?
        .into_iter()
        .find(|i| i.name == name)
    else {
        bail!("there is no index {name}: `paddock horse registry list` shows the indexes");
    };
    if !index.root.is_dir() {
        bail!(
            "the folder {} of index {name} does not exist",
            index.root.display()
        );
    }
    if !index.root.join(".git").exists() {
        return Ok(Synced::Plain);
    }
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(&index.root)
        .args(["pull", "--ff-only"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("cannot start git")?;
    // Read both pipes on threads, so a full pipe does not stop git.
    let read = |p: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = p {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as _));
    let err = read(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
    let status = loop {
        if let Some(s) = child.try_wait().context("cannot wait for git")? {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "git pull in index {name} did not finish in {} seconds; it was stopped",
                SYNC_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let text = format!(
        "{}{}",
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default()
    );
    if !status.success() {
        bail!(
            "git pull --ff-only in index {name} ({}) failed with {status}:\n{}",
            index.root.display(),
            text.trim_end()
        );
    }
    Ok(Synced::Pulled(text))
}
