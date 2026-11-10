//! Peer-local, project-scoped build caches.
//!
//! The caller must authenticate and authorize the stable project identity before
//! opening a cache. Hold `ProjectCache` for the entire sandbox run, and bind
//! only `target_dir()` (never its parent) into that sandbox. Cache contents are
//! untrusted build output; they must not be shared with another project.

use anyhow::{bail, Context, Result};
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const SHARED_DISK_ERROR: &str = "exclave cache root shares the root filesystem; mount a dedicated bounded volume or set allow_shared_disk = true";

/// An exclusive lease on one project's Cargo target directory.
pub struct ProjectCache {
    target: PathBuf,
    cargo: PathBuf,
    python: PathBuf,
    project: PathBuf,
    _lock: File,
}

impl ProjectCache {
    /// Open a cache under the peer's private paddock data directory.
    /// `project_id` is an opaque, stable identity, not a path or display name.
    /// Calls for the same identity serialize until the previous lease is dropped.
    pub fn open(data_dir: &Path, project_id: &[u8]) -> Result<Self> {
        if project_id.is_empty() || project_id.len() > 4096 {
            bail!("invalid project identity");
        }
        // The data root is supplied by the trusted peer, not by a request.
        let root = open_root(data_dir)?;
        let exclave = directory(&root, "exclave")?;
        let caches = directory(&exclave, "build-cache")?;
        let key = blake3::hash(project_id).to_hex().to_string();
        let project = directory(&caches, &key)?;
        let lock = open_lock(&project)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error()).context("lock project cache");
        }
        let project_path = data_dir.join("exclave/build-cache").join(&key);
        crate::privfs::write_atomic(&project_path.join("access"), "")?;
        let _target = directory(&project, "target")?;
        let _cargo = directory(&project, "cargo")?;
        let _python = directory(&project, "python")?;
        let _venv = directory(&_python, "venv")?;
        Ok(Self {
            target: data_dir
                .join("exclave/build-cache")
                .join(&key)
                .join("target"),
            cargo: data_dir.join("exclave/build-cache").join(key).join("cargo"),
            python: project_path.join("python"),
            project: project_path,
            _lock: lock,
        })
    }

    pub fn target_dir(&self) -> &Path {
        &self.target
    }
    pub fn cargo_dir(&self) -> &Path {
        &self.cargo
    }
    pub fn python_dir(&self) -> &Path {
        &self.python
    }
    pub fn record_size(&self) -> Result<u64> {
        let bytes = tree_size(&self.project)?;
        crate::privfs::write_atomic(&self.project.join("size"), bytes.to_string())?;
        Ok(bytes)
    }
    pub fn check_size(&self, max: u64) -> Result<u64> {
        let bytes = tree_size(&self.project)?;
        if bytes > max {
            bail!("exclave project cache exceeds cache_bytes");
        }
        Ok(bytes)
    }
    /// Evict oldest idle caches until the total existing cache fits the cap.
    /// The active cache lease stays held; stable lock files are never removed.
    pub fn enforce_cap(&self, max: u64) -> Result<()> {
        let base = self.project.parent().context("cache parent missing")?;
        let base_fd = open_root(base)?;
        let mut candidates = Vec::new();
        let mut total = 0u64;
        for entry in std::fs::read_dir(base)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !crate::exclave::snapshot::valid_hash(&id) || !entry.file_type()?.is_dir() {
                continue;
            }
            let size = tree_size(&entry.path())?;
            total = total.checked_add(size).context("cache size overflow")?;
            if entry.path() != self.project {
                let time = std::fs::metadata(entry.path().join("access"))
                    .or_else(|_| entry.metadata())?
                    .modified()?;
                candidates.push((time, id, size));
            }
        }
        candidates.sort();
        for (_, id, size) in candidates {
            if total <= max {
                break;
            }
            let project = directory(&base_fd, &id)?;
            let lock = open_lock(&project)?;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                continue;
            }
            let safe = crate::safefs::Root::open(&base.join(&id))?;
            for name in ["target", "cargo", "python", "size", "access"] {
                safe.remove_tree(Path::new(name))?;
            }
            let remaining = tree_size(&base.join(&id))?;
            total = total
                .saturating_sub(size)
                .checked_add(remaining)
                .context("cache size overflow")?;
        }
        if total > max {
            bail!("exclave cache exceeds cache_bytes; active or in-use caches cannot be evicted");
        }
        Ok(())
    }
}

fn tree_size(path: &Path) -> Result<u64> {
    let root = crate::safefs::Root::open(path)?;
    let mut pending = vec![PathBuf::new()];
    let mut total = 0u64;
    while let Some(path) = pending.pop() {
        let dir = root.dir(&path, false)?;
        for (name, stat) in dir.stats()? {
            match stat.kind {
                crate::safefs::Kind::File => {
                    total = total
                        .checked_add(stat.size)
                        .context("cache size overflow")?
                }
                crate::safefs::Kind::Dir => pending.push(path.join(name)),
                _ => {}
            }
        }
    }
    Ok(total)
}

fn open_root(path: &Path) -> Result<File> {
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).context("invalid data path")?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open peer data directory");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    check_private(&file)?;
    Ok(file)
}

fn check_private(file: &File) -> Result<()> {
    let meta = file.metadata()?;
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        bail!("cache directory must be private and owned by the peer user");
    }
    Ok(())
}

fn directory(parent: &File, name: &str) -> Result<File> {
    let name = CString::new(name)?;
    let rc = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
    if rc != 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
        return Err(std::io::Error::last_os_error()).context("create cache directory");
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open cache directory");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    check_private(&file)?;
    Ok(file)
}

fn open_lock(parent: &File) -> Result<File> {
    let name = c"lock";
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("open cache lock");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
    {
        bail!("invalid cache lock");
    }
    Ok(file)
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;

/// List or clear project caches from the peer's local CLI.
pub fn manage(root: &Path, remove: Option<&str>) -> Result<Vec<(String, u64)>> {
    let path = root.join("exclave/build-cache");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let base = open_root(&path)?;
    if let Some(id) = remove {
        if !crate::exclave::snapshot::valid_hash(id) {
            bail!("invalid cache id");
        }
        let project = directory(&base, id)?;
        let lock = open_lock(&project)?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("project cache is in use");
        }
        let safe = crate::safefs::Root::open(&path.join(id))?;
        for name in ["target", "cargo", "python", "size"] {
            safe.remove_tree(Path::new(name))?;
        }
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in std::fs::read_dir(&path)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if crate::exclave::snapshot::valid_hash(&id) && entry.file_type()?.is_dir() {
            result.push((id, tree_size(&entry.path())?));
        }
    }
    result.sort();
    Ok(result)
}

fn full_counts(blocks: u64, files: u64, free_files: u64) -> bool {
    blocks == 0 || (files != 0 && free_files == 0)
}

/// Whether the cache filesystem has no space or inodes available to this user.
pub fn volume_full(path: &Path) -> Result<bool> {
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    Ok(full_counts(info.f_bavail, info.f_files, info.f_favail))
}
