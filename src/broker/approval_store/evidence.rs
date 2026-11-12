//! Bounded reads of untrusted approval evidence.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

const LIMIT: u64 = 512 * 1024;
/// The file type bits of a mode, `S_IFMT` on Linux and macOS.
const FILE_TYPE: u32 = 0o170000;

type Stored<T> = Result<T, (io::ErrorKind, String)>;
struct Entry {
    file: Stored<Option<File>>,
    directory: Stored<File>,
    /// The type, device, inode and link count before the open.
    seen: Option<(u32, u64, u64, u64)>,
}
pub(super) struct Prepared(HashMap<PathBuf, Entry>);
#[cfg(test)]
thread_local! {
    pub(crate) static PREPARED_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}
thread_local! {
    static ACTIVE: RefCell<Vec<Prepared>> = const { RefCell::new(Vec::new()) };
}

// This is called before taking the store lock. O_NONBLOCK also covers a FIFO
// substituted between lstat and open; O_NOFOLLOW rejects leaf symlinks.
fn open_unlocked(path: &Path, write: bool) -> io::Result<File> {
    let before = std::fs::symlink_metadata(path)?;
    if !before.file_type().is_file() || before.len() > LIMIT || before.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "approval evidence is not a small regular file",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_path(path, &file)?;
    let after = file.metadata()?;
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "approval evidence changed while opening",
        ));
    }
    Ok(file)
}

fn check_path(path: &Path, file: &File) -> io::Result<()> {
    let path_meta = std::fs::symlink_metadata(path)?;
    let meta = file.metadata()?;
    if !path_meta.file_type().is_file()
        || !meta.file_type().is_file()
        || meta.len() > LIMIT
        || meta.nlink() != 1
        || path_meta.dev() != meta.dev()
        || path_meta.ino() != meta.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "approval evidence identity changed",
        ));
    }
    Ok(())
}

pub(super) fn prepare(dir: &Path, id: &str, extra: Option<&Path>) -> Prepared {
    let mut entries = HashMap::new();
    let mut paths: Vec<PathBuf> = ["json", "answer", "done", "owner", "retired", "cleanup"]
        .into_iter()
        .map(|ext| dir.join(format!("{id}.{ext}")))
        .collect();
    if let Some(path) = extra {
        paths.push(path.to_path_buf());
    }
    for path in paths {
        let write = path.extension().is_some_and(|ext| ext == "owner");
        let seen = std::fs::symlink_metadata(&path)
            .ok()
            .map(|m| (m.mode() & FILE_TYPE, m.dev(), m.ino(), m.nlink()));
        let opened = match open_unlocked(&path, write) {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err((e.kind(), e.to_string())),
        };
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("missing evidence parent"))
            .and_then(|parent| {
                OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(parent)
            })
            .map_err(|e| (e.kind(), e.to_string()));
        entries.insert(
            path,
            Entry {
                file: opened,
                directory,
                seen,
            },
        );
    }
    #[cfg(test)]
    PREPARED_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    Prepared(entries)
}

pub(super) struct Scope(std::marker::PhantomData<std::rc::Rc<()>>);
pub(super) fn enter(prepared: Prepared) -> Scope {
    ACTIVE.with(|active| active.borrow_mut().push(prepared));
    Scope(std::marker::PhantomData)
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.borrow_mut().pop();
        });
    }
}

// fstatat never traverses a leaf link. The directory was opened before the
// store lock, so revalidation does not resolve any untrusted path under it.
fn at(directory: &File, name: &std::ffi::OsStr) -> io::Result<libc::stat> {
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let rc = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { stat.assume_init() })
}

fn directory(entry: &Entry) -> io::Result<&File> {
    entry
        .directory
        .as_ref()
        .map_err(|(kind, message)| io::Error::new(*kind, message.clone()))
}

fn check_cached(path: &Path, entry: &Entry, file: &File) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing evidence name"))?;
    let path_meta = at(directory(entry)?, name)?;
    let meta = file.metadata()?;
    if path_meta.st_mode & libc::S_IFMT != libc::S_IFREG
        || !meta.file_type().is_file()
        || meta.len() > LIMIT
        || meta.nlink() != 1
        || path_meta.st_dev as u64 != meta.dev()
        || path_meta.st_ino as u64 != meta.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "approval evidence identity changed",
        ));
    }
    Ok(())
}

pub(super) fn verify_lock(prepared: &Prepared, dir: &Path, lock: &File) -> io::Result<()> {
    let entry = prepared
        .0
        .iter()
        .find(|(path, _)| path.parent() == Some(dir))
        .map(|(_, entry)| entry)
        .ok_or_else(|| io::Error::other("missing evidence directory"))?;
    let stat = at(directory(entry)?, std::ffi::OsStr::new(".lock"))?;
    let meta = lock.metadata()?;
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_dev as u64 != meta.dev()
        || stat.st_ino as u64 != meta.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("approval directory changed: {}", dir.display()),
        ));
    }
    Ok(())
}

/// Whether every prepared leaf is still the one in the directory. A lock
/// holder can rename, add or link evidence while this process waits.
// The stat field types differ between Linux and macOS.
#[allow(clippy::unnecessary_cast)]
pub(super) fn still_current(prepared: &Prepared) -> bool {
    prepared.0.iter().all(|(path, entry)| {
        let (Ok(directory), Some(name)) = (directory(entry), path.file_name()) else {
            return true;
        };
        let now = at(directory, name).ok().map(|st| {
            (
                st.st_mode as u32 & FILE_TYPE,
                st.st_dev as u64,
                st.st_ino as u64,
                st.st_nlink as u64,
            )
        });
        match &entry.file {
            Ok(Some(file)) => check_cached(path, entry, file).is_ok(),
            _ => now == entry.seen,
        }
    })
}

fn with_entry<T>(path: &Path, f: impl FnOnce(&Entry) -> io::Result<T>) -> Option<io::Result<T>> {
    ACTIVE.with(|active| {
        let active = active.borrow();
        active.last().map(|prepared| {
            let entry = prepared.0.get(path).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "approval evidence was not prepared before locking",
                )
            })?;
            f(entry)
        })
    })
}

// A store critical section may only duplicate a descriptor opened before the
// lock. A newly appeared, replaced, unprepared or unreadable file is unknown.
pub(super) fn open_regular(path: &Path, write: bool) -> io::Result<File> {
    if let Some(result) = with_entry(path, |entry| match &entry.file {
        Ok(Some(file)) => {
            check_cached(path, entry, file)?;
            file.try_clone()
        }
        Ok(None) => {
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("missing evidence name"))?;
            match at(directory(entry)?, name) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Err(e),
                Err(e) => Err(e),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "approval evidence appeared after preparation",
                )),
            }
        }
        Err((kind, message)) => Err(io::Error::new(*kind, message.clone())),
    }) {
        return result;
    }
    open_unlocked(path, write)
}

// None means that no store lock is held; the caller may inspect ancestors.
pub(super) fn missing_locked(path: &Path) -> Option<io::Result<()>> {
    with_entry(path, |entry| {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("missing evidence name"))?;
        match at(directory(entry)?, name) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "approval evidence exists but cannot be read",
            )),
        }
    })
}

pub(super) fn read_open(file: &File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = (LIMIT + 1 - bytes.len() as u64) as usize;
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "approval evidence exceeds the size limit",
            ));
        }
        let length = remaining.min(chunk.len());
        let n = match file.read_at(&mut chunk[..length], bytes.len() as u64) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}

pub(super) fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    let file = open_regular(path, false)?;
    let bytes = read_open(&file)?;
    match with_entry(path, |entry| check_cached(path, entry, &file)) {
        Some(result) => result?,
        None => check_path(path, &file)?,
    }
    Ok(bytes)
}

pub(in crate::broker) fn read_text(path: impl AsRef<Path>) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
