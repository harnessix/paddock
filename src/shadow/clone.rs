//! The copies of a snapshot: a reflink (`FICLONE`) on Linux, an APFS clone
//! (`clonefileat`) on macOS, else a plain copy.

use crate::safefs::{Dir, Root};
use anyhow::Result;
use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

/// The name of the probe file in the run folder.
const PROBE: &str = ".paddock-reflink-probe";

#[cfg(not(target_os = "macos"))]
const FICLONE: libc::c_ulong = 0x4004_9409;

/// A reflink of `src` into the empty file `dst`.
#[cfg(not(target_os = "macos"))]
fn reflink(src: &std::fs::File, dst: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: FICLONE takes the source fd as its argument; both fds are open.
    unsafe { libc::ioctl(dst.as_raw_fd(), FICLONE as _, src.as_raw_fd()) == 0 }
}

/// macOS clones by name before the file exists (`clone_at`), never into an
/// open file.
#[cfg(target_os = "macos")]
fn reflink(_src: &std::fs::File, _dst: &std::fs::File) -> bool {
    false
}

/// `CLONE_NOFOLLOW` and `CLONE_NOOWNERCOPY` of <sys/clonefile.h>. The libc
/// crate has the function but not the flags.
#[cfg(target_os = "macos")]
const CLONE_NOFOLLOW: u32 = 0x0001;
#[cfg(target_os = "macos")]
const CLONE_NOOWNERCOPY: u32 = 0x0002;

/// An APFS clone of the file `src` in `from` as the new file `dst` in `to`,
/// with `mode`. The clone is owned by the user (`CLONE_NOOWNERCOPY`), and a
/// symlink at `src` is not followed. `dst` must not exist.
#[cfg(target_os = "macos")]
pub(crate) fn clone_at(
    from: &Dir,
    src: &OsStr,
    to: &Dir,
    dst: &OsStr,
    mode: u32,
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let s = CString::new(src.as_bytes())?;
    let d = CString::new(dst.as_bytes())?;
    // SAFETY: two open folder fds and two NUL-terminated names.
    let r = unsafe {
        libc::clonefileat(
            from.as_raw_fd(),
            s.as_ptr(),
            to.as_raw_fd(),
            d.as_ptr(),
            CLONE_NOFOLLOW | CLONE_NOOWNERCOPY,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // A clone keeps the mode and the times already; the mode is set again
    // so that the result never depends on the flags above.
    // SAFETY: an open folder fd and a NUL-terminated name; the new file is
    // not a symlink, and AT_SYMLINK_NOFOLLOW keeps it so.
    let r = unsafe {
        libc::fchmodat(
            to.as_raw_fd(),
            d.as_ptr(),
            (mode & 0o7777) as libc::mode_t,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if r != 0 {
        let e = std::io::Error::last_os_error();
        let _ = to.remove(dst);
        return Err(e);
    }
    Ok(())
}

/// Copy the file `src_name` of `from` into `to` as `name`: a reflink or a
/// clone when `reflinks`, else a plain copy, with its mode and change time.
/// Returns the bytes, or None when the source went away or cannot be
/// opened.
pub(crate) fn copy_file(
    from: &Dir,
    src_name: &OsStr,
    to: &Dir,
    name: &OsStr,
    reflinks: bool,
) -> Result<Option<u64>> {
    let Ok(Some(src)) = from.open_read(src_name) else {
        return Ok(None);
    };
    let meta = src.metadata()?;
    let len = meta.len();
    let _ = to.remove(name);
    #[cfg(target_os = "macos")]
    if reflinks && clone_at(from, src_name, to, name, meta.mode()).is_ok() {
        return Ok(Some(len));
    }
    let mut dst = to.create_new(name, 0o600)?;
    if !(reflinks && reflink(&src, &dst)) {
        let mut s = &src;
        std::io::copy(&mut s, &mut dst)?;
    }
    dst.set_permissions(std::fs::Permissions::from_mode(meta.mode() & 0o7777))?;
    if let Ok(t) = meta.modified() {
        let _ = dst.set_modified(t);
    }
    Ok(Some(len))
}

/// Whether a reflink (Linux) or a clone (macOS) of the home file `rel`
/// into the run folder `into` works. Not APFS (`ENOTSUP`), another volume
/// (`EXDEV`) or any other error: no.
pub(crate) fn probe_reflink(home: &Root, into: &Path, rel: &Path) -> bool {
    let parent = rel.parent().unwrap_or(Path::new(""));
    let Some(name) = rel.file_name() else {
        return false;
    };
    let Ok(from) = home.dir(parent, false) else {
        return false;
    };
    let Ok(to) = Root::open(into).and_then(|r| r.dir(Path::new(""), false)) else {
        return false;
    };
    let probe = OsStr::new(PROBE);
    let _ = to.remove(probe);
    let ok = probe_one(&from, name, &to, probe);
    let _ = to.remove(probe);
    ok
}

#[cfg(target_os = "macos")]
fn probe_one(from: &Dir, name: &OsStr, to: &Dir, probe: &OsStr) -> bool {
    clone_at(from, name, to, probe, 0o600).is_ok()
}

#[cfg(not(target_os = "macos"))]
fn probe_one(from: &Dir, name: &OsStr, to: &Dir, probe: &OsStr) -> bool {
    let Ok(Some(src)) = from.open_read(name) else {
        return false;
    };
    let Ok(dst) = to.create_new(probe, 0o600) else {
        return false;
    };
    reflink(&src, &dst)
}

/// APFS clones. They run only on macOS.
#[cfg(all(test, target_os = "macos"))]
#[path = "clone_macos_tests.rs"]
mod macos_tests;
