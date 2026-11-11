//! The drop staging of a session, pure with Linux tests. A drop on a
//! session window never gives the guest the original: the files are cloned
//! into `profile/drop/<time>-<n>/` (an APFS clone on macOS, a copy
//! elsewhere), a text or an image becomes `dropped.txt` or `dropped.png`
//! there, two items of one name get `name (2).ext`, and the guest gets the
//! paths under the `drop` share. One drop is at most 2 GiB. The staging
//! folders go at quit, unless the app still has a file of one open.

use super::proto::SHARE_ROOT;
use super::session::Profile;
use super::text;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// The most one drop may hold.
pub const DROP_MAX: u64 = 2 << 30;
/// The file of a dropped text.
pub const TEXT_NAME: &str = "dropped.txt";
/// The file of a dropped image.
pub const IMAGE_NAME: &str = "dropped.png";
/// The folder under `drop/` where the clipboard transfers of the host to
/// the guest spill when they are too big for a line.
pub const CLIPBOARD_DIR: &str = ".clipboard";

/// One item of a drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A file or a folder of the host.
    File(PathBuf),
    Text(String),
    /// PNG bytes.
    Image(Vec<u8>),
}

/// A staged drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// The staging folder on the host.
    pub dir: PathBuf,
    /// Each item, as the guest sees it.
    pub guest_paths: Vec<PathBuf>,
    pub bytes: u64,
}

/// Why a drop was not staged.
#[derive(Debug)]
pub enum StageError {
    TooLarge { size: u64 },
    Empty,
    Io(io::Error),
}

impl StageError {
    /// The badge for the window.
    pub fn badge(&self) -> String {
        match self {
            StageError::TooLarge { size } => text::drop_too_large(*size),
            StageError::Empty | StageError::Io(_) => text::drop_failed(),
        }
    }
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StageError::Io(e) => write!(f, "{}: {e}", self.badge()),
            _ => f.write_str(&self.badge()),
        }
    }
}

impl From<io::Error> for StageError {
    fn from(e: io::Error) -> Self {
        StageError::Io(e)
    }
}

/// The size of an item: a folder counts every file in it.
pub fn size_of(item: &Item) -> u64 {
    match item {
        Item::Text(t) => t.len() as u64,
        Item::Image(b) => b.len() as u64,
        Item::File(p) => tree_size(p),
    }
}

fn tree_size(p: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    if md.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum())
            .unwrap_or(0)
    } else {
        md.len()
    }
}

/// `name`, or `name (2)`, `name (3)` ... before the extension, the first
/// that is free in `dir`. A name that starts with a dot has no extension.
pub fn unique(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}")) // not user text
        .find(|c| !dir.join(c).exists())
        .expect("a free name")
}

/// The staging folder `<now>-<n>` with the first free `n` from 1.
fn free_dir(root: &Path, now_secs: u64) -> PathBuf {
    (1..)
        .map(|n| root.join(format!("{now_secs}-{n}"))) // not user text
        .find(|d| !d.exists())
        .expect("a free folder")
}

/// Clone or copy a file or a folder. On macOS `clonefile` shares the
/// blocks and never changes the original; across volumes, and on other
/// systems, a copy.
pub fn clone_into(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let s = CString::new(src.as_os_str().as_bytes())?;
        let d = CString::new(dst.as_os_str().as_bytes())?;
        // CLONE_NOFOLLOW | CLONE_NOOWNERCOPY of <sys/clonefile.h>.
        // SAFETY: two NUL-terminated paths.
        let r = unsafe {
            libc::clonefileat(
                libc::AT_FDCWD,
                s.as_ptr(),
                libc::AT_FDCWD,
                d.as_ptr(),
                0x0001 | 0x0002,
            )
        };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        let cross = matches!(e.raw_os_error(), Some(libc::EXDEV | libc::ENOTSUP));
        if !cross {
            return Err(e);
        }
    }
    copy_tree(src, dst)
}

fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    let md = std::fs::symlink_metadata(src)?;
    if md.is_dir() {
        std::fs::DirBuilder::new().mode(0o700).create(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else if md.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dst)
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// Stage one drop into `profile/drop/<now>-<n>/`. The guest paths come
/// back in the order of the items.
pub fn stage(profile: &Profile, items: &[Item], now_secs: u64) -> Result<Staged, StageError> {
    if items.is_empty() {
        return Err(StageError::Empty);
    }
    let bytes: u64 = items.iter().map(size_of).sum();
    if bytes > DROP_MAX {
        return Err(StageError::TooLarge { size: bytes });
    }
    let root = profile.drop_dir();
    if !root.is_dir() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
    }
    let dir = free_dir(&root, now_secs);
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    let folder = dir.file_name().expect("a folder name").to_os_string();
    let mut guest_paths = Vec::new();
    let result = (|| -> io::Result<()> {
        for item in items {
            let want = match item {
                Item::File(p) => p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "dropped".into()),
                Item::Text(_) => TEXT_NAME.into(),
                Item::Image(_) => IMAGE_NAME.into(),
            };
            let name = unique(&dir, &want);
            let dst = dir.join(&name);
            match item {
                Item::File(p) => clone_into(p, &dst)?,
                Item::Text(t) => std::fs::write(&dst, t)?,
                Item::Image(b) => std::fs::write(&dst, b)?,
            }
            guest_paths.push(Path::new(SHARE_ROOT).join("drop").join(&folder).join(name));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(StageError::Io(e));
    }
    Ok(Staged {
        dir,
        guest_paths,
        bytes,
    })
}

/// The paths of `lsof -Fn` output: the `n` lines.
pub fn open_paths(lsof: &str) -> Vec<PathBuf> {
    lsof.lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|p| p.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}

/// Remove the staging folders of `profile/drop/`, except one that holds
/// a path of `open`. Returns what was removed.
pub fn sweep(profile: &Profile, open: &[PathBuf]) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(rd) = std::fs::read_dir(profile.drop_dir()) else {
        return removed;
    };
    for e in rd.flatten() {
        let dir = e.path();
        if open.iter().any(|o| o.starts_with(&dir)) {
            continue;
        }
        let ok = if dir.is_dir() {
            std::fs::remove_dir_all(&dir)
        } else {
            std::fs::remove_file(&dir)
        };
        if ok.is_ok() {
            removed.push(dir);
        }
    }
    removed.sort();
    removed
}

#[cfg(test)]
#[path = "drop_tests.rs"]
mod tests;
