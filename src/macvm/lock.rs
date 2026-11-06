//! The two locks of a session: the profile lock (one start per profile)
//! and the VM slots (two macOS VMs per Mac, for every user). Both are
//! `flock` on a file whose text names the holder. The text is read for
//! the refusal line and never trusted beyond display.

use super::session::Profile;
use super::text::{self, Holder};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The slot folder of this Mac, for every user.
pub const SLOT_DIR: &str = "/private/tmp/dev.paddock.macvm";
/// The macOS VMs that the framework runs at once.
pub const SLOTS: usize = 2;
/// The longest holder text that is read.
const HOLDER_MAX: u64 = 10 * 1024;
/// The holder line is cut to this many characters.
const LINE_MAX: usize = 80;

/// `flock(LOCK_EX | LOCK_NB)`. `Ok(false)` when another holder has it.
fn try_lock(f: &File) -> io::Result<bool> {
    // SAFETY: flock on an open file of this process.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(e)
}

/// The first `HOLDER_MAX` bytes of the file, as text.
fn read_holder(f: &mut File) -> String {
    let mut text = String::new();
    let _ = f.rewind();
    let _ = f.take(HOLDER_MAX).read_to_string(&mut text);
    text
}

/// Replace the file content with `text`.
fn write_holder(f: &mut File, text: &str) -> io::Result<()> {
    f.set_len(0)?;
    f.rewind()?;
    f.write_all(text.as_bytes())?;
    f.sync_data()
}

/// One line of at most 80 characters, with no control characters, for
/// the refusal line. Empty text gives `unknown`.
pub fn holder_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    let cut: String = first
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(LINE_MAX)
        .collect();
    let cut = cut.trim().to_string();
    if cut.is_empty() {
        "unknown".into()
    } else {
        cut
    }
}

/// The lock of one profile folder: one start at a time.
#[derive(Debug)]
pub struct ProfileLock {
    _file: File,
    pub path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process holds it. The pid is from the file text.
    Held {
        pid: Option<u32>,
    },
    Io(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Held { pid: Some(pid) } => write!(f, "the profile already runs (pid {pid})"),
            LockError::Held { pid: None } => write!(f, "the profile already runs"),
            LockError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LockError {}

impl From<io::Error> for LockError {
    fn from(e: io::Error) -> Self {
        LockError::Io(e)
    }
}

impl ProfileLock {
    /// Take the lock of `profile` and write this pid into it. The folder is
    /// made when missing, with mode 0700.
    pub fn take(profile: &Profile) -> Result<Self, LockError> {
        Self::take_as(profile, std::process::id())
    }

    pub fn take_as(profile: &Profile, pid: u32) -> Result<Self, LockError> {
        if std::fs::symlink_metadata(&profile.dir).is_err() {
            std::fs::create_dir_all(&profile.dir)?;
            std::fs::set_permissions(&profile.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = profile.lock();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        if !try_lock(&file)? {
            let text = read_holder(&mut file);
            let pid = text.trim().parse::<u32>().ok().filter(|p| *p > 0);
            return Err(LockError::Held { pid });
        }
        write_holder(&mut file, &format!("{pid}\n"))?;
        Ok(Self { _file: file, path })
    }
}

/// One of the two VM slots of this Mac, held for the life of the session.
#[derive(Debug)]
pub struct Slot {
    _file: File,
    pub index: usize,
    pub path: PathBuf,
}

/// Unlock before the close. The lock belongs to the open file, which a
/// child that another thread forks holds until it execs; the close alone
/// would keep the lock until then, the unlock ends it at once.
fn unlock(f: &File) {
    // SAFETY: flock on an open file of this process.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        unlock(&self._file);
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        unlock(&self._file);
    }
}

/// Both slots are held. The holders are display texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotsFull {
    pub holders: Vec<Holder>,
}

impl fmt::Display for SlotsFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&text::third_session(&self.holders))
    }
}

impl std::error::Error for SlotsFull {}

#[derive(Debug)]
pub enum SlotError {
    Full(SlotsFull),
    Io(io::Error),
}

impl fmt::Display for SlotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotError::Full(full) => full.fmt(f),
            SlotError::Io(e) => f.write_str(&text::slot_io(&e.to_string())),
        }
    }
}

impl std::error::Error for SlotError {}

/// The text in a slot file: `user app pid`.
pub fn holder_text(user: &str, app: &str, pid: u32) -> String {
    format!("{} {} {pid}\n", one_word(user), one_word(app)) // not user text
}

fn one_word(s: &str) -> String {
    let w: String = s
        .chars()
        .map(|c| {
            if c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(64)
        .collect();
    if w.is_empty() {
        "unknown".into()
    } else {
        w
    }
}

/// The holder of a slot from its file text, for the refusal line of
/// `text::third_session`: a session of `me`, or one of another user. A
/// text of another shape counts as another user named by the cut line.
pub fn describe_holder(text: &str, me: &str) -> Holder {
    let line = holder_line(text);
    let words: Vec<&str> = line.split(' ').collect();
    match words.as_slice() {
        [user, app, pid] if *user == me && pid.parse::<u32>().is_ok() => Holder::Mine {
            app: app.to_string(),
            pid: pid.parse().unwrap_or(0),
        },
        [user, _, pid] if pid.parse::<u32>().is_ok() => Holder::User {
            name: user.to_string(),
        },
        _ => Holder::User { name: line },
    }
}

impl Slot {
    /// Take a free slot under [`SLOT_DIR`].
    pub fn take(user: &str, app: &str, pid: u32) -> Result<Self, SlotError> {
        Self::take_in(Path::new(SLOT_DIR), user, app, pid)
    }

    /// Take a free slot under `dir`: the folder is made with mode 01777
    /// when missing, the files with 0666, and the holder text is written
    /// at the take. Both held: the refusal names the holders.
    pub fn take_in(dir: &Path, user: &str, app: &str, pid: u32) -> Result<Self, SlotError> {
        ensure_dir(dir).map_err(SlotError::Io)?;
        let mut holders = Vec::new();
        for index in 0..SLOTS {
            let path = dir.join(format!("slot-{index}"));
            let mut file = match open_slot(&path) {
                Ok(f) => f,
                Err(e) => {
                    log::warn!("macvm: cannot open {}: {e}", path.display());
                    holders.push(Holder::User {
                        name: text::slot_unreadable(index, &e.to_string()),
                    });
                    continue;
                }
            };
            if try_lock(&file).map_err(SlotError::Io)? {
                write_holder(&mut file, &holder_text(user, app, pid)).map_err(SlotError::Io)?;
                return Ok(Self {
                    _file: file,
                    index,
                    path,
                });
            }
            holders.push(describe_holder(&read_holder(&mut file), user));
        }
        Err(SlotError::Full(SlotsFull { holders }))
    }
}

/// The holders of the slots now, for `paddock macvm status`: `None` for a
/// free slot. `me` is the user of this process.
pub fn holders(dir: &Path, me: &str) -> Vec<Option<Holder>> {
    (0..SLOTS)
        .map(|index| {
            let path = dir.join(format!("slot-{index}"));
            let mut f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&path)
                .ok()?;
            match try_lock(&f) {
                Ok(true) => {
                    // SAFETY: release the probe lock of this process.
                    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
                    None
                }
                Ok(false) => Some(describe_holder(&read_holder(&mut f), me)),
                Err(_) => None,
            }
        })
        .collect()
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => return Err(io::Error::other(text::not_a_folder(dir))),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        Err(_) => {}
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let c = CString::new(dir.as_os_str().as_bytes())?;
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::mkdir(c.as_ptr(), 0o1777) } != 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(e);
        }
        return Ok(());
    }
    // The umask took bits away: this process made the folder, so it can
    // set the mode. Another user's folder keeps its mode.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777))?;
    Ok(())
}

fn open_slot(path: &Path) -> io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o666)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = file.metadata()?;
    if !m.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    // SAFETY: getuid has no preconditions.
    if m.uid() == unsafe { libc::getuid() } && m.mode() & 0o777 != 0o666 {
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o666));
    }
    Ok(file)
}

use std::os::unix::fs::PermissionsExt;

#[cfg(test)]
#[path = "lock_tests.rs"]
mod tests;
