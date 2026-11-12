//! A host file or folder that opens once, for a copy.

#[cfg(target_os = "linux")]
use super::fd_path;
use super::{cstr, fd_real_path, fstat, list, Kind, Node, Root, MAX_DEPTH};
use anyhow::{bail, Context, Result};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// A host file or folder that the user asked to copy, opened once. Every
/// later step works on the open fd, so a path that changes after the check,
/// for example while the user decides, cannot redirect the copy.
pub struct Source {
    pub(super) node: Node,
    pub(super) kind: Kind,
}

impl Source {
    /// Open `path`. Symlinks in it are followed, as the user named it;
    /// `real_path` tells where it led. Only a regular file or a folder opens.
    pub fn open(path: &Path) -> Result<Self> {
        let node = Node::at(libc::AT_FDCWD, &cstr(path.as_os_str())?, true)
            .with_context(|| format!("{} does not exist on the host", path.display()))?;
        let kind = node.kind();
        if !matches!(kind, Kind::File | Kind::Dir) {
            bail!(
                "{} is not a regular file or a folder; paddock copies only those",
                path.display()
            );
        }
        Ok(Self { node, kind })
    }

    /// Where the open file or folder is now.
    pub fn real_path(&self) -> Result<PathBuf> {
        fd_real_path(self.node.fd()?)
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// A path that names the open file or folder itself, valid in this
    /// process while the Source lives.
    #[cfg(target_os = "linux")]
    pub fn proc_path(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(fd_path(self.node.fd()?)))
    }

    /// macOS has no path that names an open fd and can be walked below.
    #[cfg(target_os = "macos")]
    pub fn proc_path(&self) -> Result<PathBuf> {
        bail!("a path to an open host folder (/proc/self/fd) is not available on macOS yet")
    }

    /// The open folder as a `Root`, to walk it without following a symlink.
    pub fn root(&self) -> Result<Root> {
        if self.kind != Kind::Dir {
            bail!("not a folder");
        }
        Ok(Root {
            fd: self.node.dir()?,
            path: self.real_path()?,
        })
    }

    /// The device and inode of the open file or folder.
    #[allow(clippy::unnecessary_cast)]
    pub fn id(&self) -> Result<(u64, u64)> {
        let st = fstat(self.node.fd()?)?;
        // st_dev is i32 on macOS.
        Ok((st.st_dev as u64, st.st_ino))
    }

    /// The bytes a copy takes; it stops counting above `limit`. Symlinks,
    /// FIFOs and devices inside a folder do not count: a copy skips them.
    pub fn size(&self, limit: u64) -> Result<u64> {
        let mut total = 0;
        size_of(&self.node, limit, &mut total, 0)?;
        Ok(total)
    }
}

fn size_of(node: &Node, limit: u64, total: &mut u64, depth: usize) -> Result<()> {
    match node.kind() {
        Kind::File => *total += fstat(node.fd()?)?.st_size as u64,
        Kind::Dir => {
            if depth > MAX_DEPTH {
                bail!("the folder is too deep");
            }
            let d = node.dir()?;
            for name in list(&d)? {
                if *total > limit {
                    break;
                }
                let e = Node::at(d.as_raw_fd(), &cstr(&name)?, false)?;
                size_of(&e, limit, total, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
