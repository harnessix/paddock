//! Host sessions: the sessions that a harness keeps on the host, outside
//! a sandbox.
//!
//! paddock never reads a host session file in place for an ingest, and
//! it never writes one. It copies the file (or the session folder, or the
//! database with its WAL) into a private staging folder, reads the copy,
//! and removes the copy. The copy keeps the path of the file below the
//! transcript root of the harness, because a codec checks it (Codex wants
//! the date folders of a rollout).
//!
//! A host session in the store is a [`Session`] with [`HostOrigin`]. Run
//! code never finds it by its native id (target.rs), so the rows of a run
//! never go into it.

use super::run::snapshot;
use super::store::Lock;
use super::{agy, claude, codex, dsh, grok, HostOrigin, Session, Store};
use anyhow::{bail, Context as _, Result};
use serde_json::Value;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

pub mod adopt;
pub mod hookcmd;
mod hookfile;
pub mod hooks;
pub mod import;
pub(crate) mod jsontext;
pub mod shadow;

/// The harnesses whose host sessions paddock can read.
pub const HARNESSES: [&str; 5] = [
    claude::HARNESS,
    codex::HARNESS,
    grok::HARNESS,
    agy::HARNESS,
    dsh::HARNESS,
];

/// The transcript root of a harness, relative to the home.
pub fn root(harness: &str) -> Result<&'static str> {
    snapshot::layout(harness)
}

/// The codec version that reads a host session of `harness`. For Codex:
/// the producer version of the rollout when the codec knows it, else the
/// newest reader.
pub fn reader_version(harness: &str, path: &Path) -> Result<String> {
    Ok(match harness {
        claude::HARNESS => claude::VERSION.into(),
        dsh::HARNESS => dsh::VERSION.into(),
        grok::HARNESS => grok::VERSION.into(),
        agy::HARNESS => agy::VERSION.into(),
        codex::HARNESS => {
            let first = first_codex_row(path)?;
            match first["payload"]["cli_version"].as_str() {
                Some(v) if codex::check_version(v).is_ok() => v.into(),
                _ => codex::VERSION.into(),
            }
        }
        other => bail!("paddock cannot read host sessions of {other}"),
    })
}

fn first_codex_row(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)?;
    let first: Value =
        serde_json::from_str(text.lines().next().context("empty Codex transcript")?)?;
    anyhow::ensure!(
        first["type"] == "session_meta",
        "missing Codex session metadata"
    );
    Ok(first)
}

/// The native id of the session at `path`: a file, or a Grok session folder.
pub fn native_id(harness: &str, path: &Path) -> Result<String> {
    Ok(match harness {
        claude::HARNESS => claude::read_file(path)?.session_id,
        codex::HARNESS => first_codex_row(path)?["payload"]["id"]
            .as_str()
            .context("missing Codex session id")?
            .to_owned(),
        dsh::HARNESS => dsh::read_file(path)?.header.id,
        grok::HARNESS => grok::read_summary_id(path)?,
        agy::HARNESS => agy::read_id(path)?,
        other => bail!("no codec for {other}"),
    })
}

/// Read the copy at `path` into the host session of its native id, or
/// into a new host session in `horse`. Returns the session id.
pub(crate) fn ingest_locked(
    locked: &Lock<'_>,
    harness: &str,
    path: &Path,
    version: &str,
    horse: &str,
    origin: &HostOrigin,
) -> Result<String> {
    Ok(match harness {
        claude::HARNESS => {
            claude::ingest_host_locked(locked, path, version, horse, origin)?.session
        }
        codex::HARNESS => codex::ingest_host_locked(locked, path, version, horse, origin)?.session,
        dsh::HARNESS => dsh::ingest_host_locked(locked, path, version, horse, origin)?.session,
        grok::HARNESS => grok::ingest_host_locked(locked, path, version, horse, origin)?.session,
        agy::HARNESS => agy::ingest_host_locked(locked, path, version, horse, origin)?.session,
        other => bail!("no codec for {other}"),
    })
}

/// One session that a harness keeps under its transcript root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The session file or Grok session folder, relative to the home.
    pub rel: PathBuf,
    /// The files that make the session, relative to the home.
    pub files: Vec<PathBuf>,
    /// The native id that the user asked for (`paddock horse import
    /// HARNESS ID`): the read checks that the copy holds that session.
    pub want: Option<String>,
}

/// Every session under `under` (relative to the home, inside the
/// transcript root of the harness), sorted. Symlinks are not followed.
pub fn list(home: &Path, harness: &str, under: Option<&Path>) -> Result<Vec<Found>> {
    let root = Path::new(root(harness)?);
    let start = match under {
        Some(u) => {
            check_rel(u)?;
            if !u.starts_with(root) {
                bail!(
                    "{} is not inside the {harness} transcript folder ~/{}",
                    u.display(),
                    root.display()
                );
            }
            u.to_path_buf()
        }
        None => root.to_path_buf(),
    };
    no_links(home, &start)?;
    let mut files = Vec::new();
    visit(home, &home.join(&start), harness, &mut files)?;
    files.sort();
    let mut out: Vec<Found> = Vec::new();
    for f in files {
        let rel = session_of(harness, &f);
        match out.iter_mut().find(|x| x.rel == rel) {
            Some(x) => x.files.push(f),
            None => out.push(Found {
                rel,
                files: vec![f],
                want: None,
            }),
        }
    }
    // A Grok folder needs its summary, an agy WAL needs its database.
    out.retain(|f| match harness {
        grok::HARNESS => f
            .files
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == grok::SUMMARY_FILE)),
        agy::HARNESS => f.files.contains(&f.rel),
        _ => true,
    });
    Ok(out)
}

/// The session that a matching file belongs to.
fn session_of(harness: &str, file: &Path) -> PathBuf {
    match harness {
        grok::HARNESS => file.parent().map(Path::to_path_buf).unwrap_or_default(),
        agy::HARNESS => match file.to_str().and_then(|s| s.strip_suffix("-wal")) {
            Some(db) => PathBuf::from(db),
            None => file.to_path_buf(),
        },
        _ => file.to_path_buf(),
    }
}

fn visit(home: &Path, dir: &Path, harness: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    for e in entries {
        let path = e?.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            visit(home, &path, harness, out)?;
        } else if meta.is_file() && snapshot::matches(harness, &path) {
            out.push(path.strip_prefix(home)?.to_path_buf());
        }
    }
    Ok(())
}

/// A plain relative path: no root, no `..`, no `.`.
pub fn check_rel(rel: &Path) -> Result<()> {
    if rel.as_os_str().is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!(
            "{} must be a plain path relative to the home",
            rel.display()
        );
    }
    Ok(())
}

/// No part of `rel` below `home` is a symlink: paddock reads only what
/// is really in the transcript folder.
fn no_links(home: &Path, rel: &Path) -> Result<()> {
    let mut p = home.to_path_buf();
    for c in rel.components() {
        p.push(c);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                bail!("{} is a symlink; paddock does not follow it", p.display())
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// A private folder for the copies of one read. It goes when it drops.
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    /// A new staging folder in `parent` (made 0700).
    pub fn new(parent: &Path) -> Result<Self> {
        crate::privfs::dir(parent)?;
        let dir = parent.join(format!(
            "stage-{}-{}",
            std::process::id(),
            super::hex(&super::random_bytes::<6>()?)
        ));
        crate::privfs::dir(&dir)?;
        Ok(Self { dir })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Copy the files of a session from `home` into this folder, at the
    /// same paths. Returns the copy of the session (file or folder).
    pub fn copy(&self, home: &Path, found: &Found) -> Result<PathBuf> {
        for f in &found.files {
            check_rel(f)?;
            no_links(home, f)?;
            let to = self.dir.join(f);
            let parent = to.parent().context("a session file has a folder")?;
            std::fs::create_dir_all(parent)?;
            copy_file(&home.join(f), &to)?;
        }
        Ok(self.dir.join(&found.rel))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Copy one regular file. The source opens read-only, without following
/// a symlink; nothing else touches it. The copy is a reflink (Linux) or an
/// APFS clone (macOS) when the file system can, so a large transcript
/// costs no bytes on the disk; else a plain copy.
pub fn copy_file(from: &Path, to: &Path) -> Result<()> {
    let mut src = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(from)
        .with_context(|| format!("cannot open {}", from.display()))?;
    if !src.metadata()?.is_file() {
        bail!("{} is not a regular file", from.display());
    }
    #[cfg(target_os = "macos")]
    if clone_path(from, to) {
        return Ok(());
    }
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(crate::privfs::FILE_MODE)
        .open(to)
        .with_context(|| format!("cannot write {}", to.display()))?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        const FICLONE: libc::c_ulong = 0x4004_9409;
        // SAFETY: FICLONE takes the source fd as its argument; both are open.
        if unsafe { libc::ioctl(dst.as_raw_fd(), FICLONE as _, src.as_raw_fd()) } == 0 {
            return Ok(());
        }
    }
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
    }
    Ok(())
}

/// An APFS clone of `from` as the new file `to`, mode 0600. False: no
/// clone (another file system, or the source is no regular file now).
#[cfg(target_os = "macos")]
fn clone_path(from: &Path, to: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    const CLONE_NOFOLLOW: u32 = 0x0001;
    const CLONE_NOOWNERCOPY: u32 = 0x0002;
    let (Ok(s), Ok(d)) = (
        CString::new(from.as_os_str().as_bytes()),
        CString::new(to.as_os_str().as_bytes()),
    ) else {
        return false;
    };
    // SAFETY: two NUL-terminated paths; a symlink at `from` is not followed.
    let r = unsafe {
        libc::clonefileat(
            libc::AT_FDCWD,
            s.as_ptr(),
            libc::AT_FDCWD,
            d.as_ptr(),
            CLONE_NOFOLLOW | CLONE_NOOWNERCOPY,
        )
    };
    if r != 0 {
        return false;
    }
    // A symlink that took the place of the file is cloned as a symlink:
    // it goes again, and the plain copy refuses it.
    let ok = std::fs::symlink_metadata(to).is_ok_and(|m| m.is_file())
        && std::fs::set_permissions(to, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .is_ok();
    if !ok {
        let _ = std::fs::remove_file(to);
    }
    ok
}

/// The home as one path: symlinks and `..` resolved, so the same home
/// always gives the same key. A home that is not there stays as it is.
pub fn canon_home(home: &Path) -> PathBuf {
    std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf())
}

/// The longest common prefix of two chains, by the raw rows: the number of
/// rows that `run` keeps of `host`.
pub(crate) fn common_rows(store: &Store, host: &Session, run: &Session) -> Result<usize> {
    let a = store.session_nodes(host)?;
    let b = store.session_nodes(run)?;
    Ok(a.iter()
        .zip(&b)
        .take_while(|(x, y)| x.raw == y.raw && x.meta.key == y.meta.key)
        .count())
}

#[cfg(test)]
mod adopt_tests;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod hookcmd_tests;
#[cfg(test)]
mod hooks_tests;
#[cfg(test)]
mod import_tests;
#[cfg(test)]
mod shadow_tests;
#[cfg(test)]
mod tests;
