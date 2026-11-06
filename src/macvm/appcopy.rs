//! The app of a session: the host bundle it comes from, whether the guest
//! needs a new copy of it, the clone into the profile, and the `app.copy`
//! request.
//!
//! Everything here is plain Rust, so the tests run on Linux. The one
//! macOS-only part is [`quarantined_unapproved`], which reads an extended
//! attribute; on other systems it answers false. The clone of the bundle
//! is a closure, so a Linux test passes a plain copy.
//!
//! The order of a copy, which no half state can break:
//!
//! 1. The host bundle is read. An App Store bundle is refused here.
//! 2. [`needs_copy`] compares its version and build with the stamp.
//! 3. The bundle is cloned into `profile/apps/<Name>.app`, and the clone
//!    is archived with `ditto -c` as `<Name>.app.cpio` next to it; the
//!    session shares the folder read-only as `apps`.
//! 4. `app.copy` tells the agent to `ditto` it to the guest home. The
//!    agent writes `<Name>.app.partial` and renames it, so a killed
//!    session never leaves a half app that Launch Services can start.
//! 5. The stamp gets `app.version` and `app.build` only after the reply.

use super::proto::Request;
use super::session::{AppStamp, Profile};
use super::text;
use crate::backend::seatbelt::{plist_string, plist_xml};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// The free space the guest needs on the data disk on top of the bundle
/// size before a copy starts.
pub const COPY_HEADROOM: u64 = 500 << 20;
/// The Applications folder of the guest home, where the copy lives.
pub const GUEST_APPS: &str = "Applications";
/// The share name of the profile's `apps` folder.
pub const APPS_SHARE: &str = "apps";
/// The entries of a bundle that [`size`] walks at most, so a bundle with
/// a link loop or a million files never holds up a start.
const SIZE_ENTRIES: usize = 200_000;
/// How fast the copy over virtiofs runs, for the estimate of the start
/// line. A 500 MB bundle takes about 20 s.
const COPY_BYTES_PER_SEC: u64 = 25 << 20;

/// The app bundle on the host that a session copies into the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBundle {
    pub path: PathBuf,
    /// `ChatGPT` of `/Applications/ChatGPT.app`: the name of the copy in
    /// the guest and of the shim bundle.
    pub name: String,
    /// `CFBundleName`, else the file name without the extension.
    pub display_name: String,
    /// `CFBundleShortVersionString`.
    pub version: String,
    /// `CFBundleVersion`, empty when the bundle has none.
    pub build: String,
    /// `CFBundleIdentifier`, empty when the bundle has none.
    pub bundle_id: String,
    /// The icon file inside the bundle, when it has one.
    pub icon: Option<PathBuf>,
    /// The bundle carries `Contents/_MASReceipt`: an App Store copy,
    /// which needs an Apple ID the guest does not have.
    pub mas: bool,
    /// The size of the bundle on disk.
    pub bytes: u64,
}

impl HostBundle {
    /// The stamp fields of this bundle.
    pub fn stamp(&self) -> AppStamp {
        AppStamp {
            version: self.version.clone(),
            build: self.build.clone(),
            copied: None,
        }
    }

    /// The clone of this bundle in the profile, `apps/<Name>.app`.
    pub fn clone_path(&self, profile: &Profile) -> PathBuf {
        profile.apps().join(format!("{}.app", self.name))
    }

    /// The archive of the clone, `apps/<Name>.app.cpio`: what the guest
    /// copies. The shared folder of a macOS guest follows symbolic links,
    /// so a bundle read through it loses the links of its frameworks and
    /// its signature: macOS then calls the app damaged. An archive of
    /// `ditto -c` keeps the links, the modes and the extended attributes.
    pub fn archive_path(&self, profile: &Profile) -> PathBuf {
        profile
            .apps()
            .join(format!("{}.app.{ARCHIVE_EXT}", self.name))
    }

    /// Where the guest sees the archive, under the `apps` share.
    pub fn guest_source(&self) -> PathBuf {
        Path::new(super::proto::SHARE_ROOT)
            .join(APPS_SHARE)
            .join(format!("{}.app.{ARCHIVE_EXT}", self.name))
    }

    /// Where the copy lives in the guest home.
    pub fn guest_target(&self) -> PathBuf {
        Path::new(crate::guest::data::HOME)
            .join(GUEST_APPS)
            .join(format!("{}.app", self.name))
    }

    /// The seconds that the start line promises for the copy.
    pub fn copy_seconds(&self) -> u32 {
        (self.bytes / COPY_BYTES_PER_SEC).max(2) as u32
    }
}

/// Read an app bundle. It is never opened, signed or changed.
pub fn read(path: &Path) -> Result<HostBundle> {
    let contents = path.join("Contents");
    if !contents.is_dir() {
        bail!(text::bundle_not_a_bundle(path));
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && !n.contains('/'))
        .unwrap_or_else(|| "App".into());
    let xml = std::fs::read(contents.join("Info.plist"))
        .ok()
        .and_then(|b| plist_xml(&b))
        .unwrap_or_default();
    let get = |key: &str| plist_string(&xml, key).filter(|v| !v.is_empty());
    let Some(version) = get("CFBundleShortVersionString") else {
        bail!(text::bundle_no_version(path));
    };
    let icon = get("CFBundleIconFile").and_then(|f| icon_file(&contents, &f));
    Ok(HostBundle {
        display_name: get("CFBundleName").unwrap_or_else(|| name.clone()),
        bundle_id: get("CFBundleIdentifier").unwrap_or_default(),
        build: get("CFBundleVersion").unwrap_or_default(),
        mas: contents.join("_MASReceipt").exists(),
        bytes: size(path),
        path: path.to_path_buf(),
        name,
        version,
        icon,
    })
}

/// The icon inside `Contents/Resources`. `CFBundleIconFile` may leave the
/// `.icns` out, and a name with a separator is refused.
fn icon_file(contents: &Path, named: &str) -> Option<PathBuf> {
    if named.contains('/') || named == ".." {
        return None;
    }
    let dir = contents.join("Resources");
    [dir.join(named), dir.join(format!("{named}.icns"))]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// The bytes of a folder tree, links not followed, [`SIZE_ENTRIES`]
/// entries at most.
pub fn size(path: &Path) -> u64 {
    fn walk(path: &Path, left: &mut usize, total: &mut u64) {
        if *left == 0 {
            return;
        }
        *left -= 1;
        let Ok(md) = std::fs::symlink_metadata(path) else {
            return;
        };
        if md.is_dir() {
            let Ok(rd) = std::fs::read_dir(path) else {
                return;
            };
            for e in rd.flatten() {
                walk(&e.path(), left, total);
            }
        } else if md.is_file() {
            *total += md.len();
        }
    }
    let mut left = SIZE_ENTRIES;
    let mut total = 0;
    walk(path, &mut left, &mut total);
    total
}

/// Whether the guest needs a copy of the bundle: a profile without an app
/// stamp, or one whose version or build differs from the host bundle.
pub fn needs_copy(stamp: Option<&AppStamp>, host: &AppStamp) -> bool {
    !stamp.is_some_and(|s| s.same_version(host))
}

/// An App Store bundle has no route into the guest: the guest has no
/// Apple ID. Every other bundle passes.
pub fn check(bundle: &HostBundle) -> Result<()> {
    if bundle.mas {
        bail!(text::mas_bundle(&bundle.path));
    }
    Ok(())
}

/// True when the host bundle carries a quarantine attribute that the user
/// has not approved yet: the flags field of `com.apple.quarantine`, the
/// first hex number before the first `;`, without bit `0x40`. The guest
/// removes the attribute from the copy only when this is false, so a
/// bundle the user never opened stays quarantined in the guest too.
pub fn quarantined_unapproved(path: &Path) -> bool {
    match quarantine_value(path) {
        Some(value) => !approved_flags(&value),
        None => false,
    }
}

/// The flags field says the user approved the download: bit `0x40`.
fn approved_flags(value: &str) -> bool {
    let head = value.split(';').next().unwrap_or("");
    match u64::from_str_radix(head.trim(), 16) {
        Ok(flags) => flags & 0x40 != 0,
        // An attribute paddock cannot read is not an approval.
        Err(_) => false,
    }
}

/// The `com.apple.quarantine` attribute of a path, as text. None when the
/// path has none, and always None outside macOS.
#[cfg(target_os = "macos")]
fn quarantine_value(path: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let p = CString::new(path.as_os_str().as_bytes()).ok()?;
    let name = CString::new("com.apple.quarantine").ok()?;
    let mut buf = [0u8; 256];
    // SAFETY: two NUL-terminated strings and a buffer with its own length.
    // XATTR_NOFOLLOW is 0x0001 of <sys/xattr.h>: the bundle folder itself.
    let n = unsafe {
        libc::getxattr(
            p.as_ptr(),
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0x0001,
        )
    };
    if n <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn quarantine_value(_path: &Path) -> Option<String> {
    None
}

/// Clone the host bundle into `profile/apps/<Name>.app`, as `.partial`
/// first. An old clone and an old partial go first, so the folder holds
/// one complete copy of one version and nothing else. `clone` is
/// `clonefile` on macOS and a plain copy in the tests.
pub fn clone_into_profile(
    profile: &Profile,
    bundle: &HostBundle,
    clone: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<PathBuf> {
    use super::layout::{commit, partial};
    let dst = bundle.clone_path(profile);
    crate::privfs::dir(&profile.apps()).map_err(|e| anyhow::anyhow!(e))?;
    for stale in [partial(&dst), dst.clone()] {
        if std::fs::symlink_metadata(&stale).is_ok() {
            remove_any(&stale)?;
        }
    }
    let tmp = partial(&dst);
    clone(&bundle.path, &tmp)
        .map_err(|e| anyhow::anyhow!(text::cannot_clone_app(&bundle.path, &e.to_string())))?;
    commit(&tmp).map_err(|e| anyhow::anyhow!(text::cannot_commit_app(&dst, &e.to_string())))?;
    Ok(dst)
}

/// The extension of the archive of a bundle.
pub const ARCHIVE_EXT: &str = "cpio";

/// Write the archive of the clone, as `.partial` first, from `archive`:
/// [`ditto_archive`] on macOS, a fake in the tests.
pub fn archive_into_profile(
    profile: &Profile,
    bundle: &HostBundle,
    archive: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<PathBuf> {
    use super::layout::{commit, partial};
    let dst = bundle.archive_path(profile);
    for stale in [partial(&dst), dst.clone()] {
        if std::fs::symlink_metadata(&stale).is_ok() {
            remove_any(&stale)?;
        }
    }
    let tmp = partial(&dst);
    if let Err(e) = archive(&bundle.clone_path(profile), &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!(text::cannot_clone_app(
            &bundle.path,
            &e.to_string()
        )));
    }
    commit(&tmp).map_err(|e| anyhow::anyhow!(text::cannot_commit_app(&dst, &e.to_string())))?;
    Ok(dst)
}

/// `ditto -c SRC DST`: a CPIO archive of the bundle, uncompressed.
pub fn ditto_archive(src: &Path, dst: &Path) -> std::io::Result<()> {
    let out = std::process::Command::new("/usr/bin/ditto")
        .arg("-c")
        .arg(src)
        .arg(dst)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

fn remove_any(path: &Path) -> Result<()> {
    let md = std::fs::symlink_metadata(path)?;
    if md.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// The `app.copy` request for a bundle whose clone is in the profile.
/// `unquarantine` says the guest may remove the quarantine attribute
/// from the copy, which it may only when the host copy was approved.
pub fn request(bundle: &HostBundle) -> Request {
    Request::AppCopy {
        from: bundle.guest_source(),
        to: bundle.guest_target(),
        version: bundle.version.clone(),
        unquarantine: !quarantined_unapproved(&bundle.path),
    }
}

/// A profile slug for a bundle that the user dropped on the drag target:
/// `CFBundleName` in lower case, spaces as dashes, letters, digits and dashes
/// only, and a number when the name is taken.
pub fn slug(display_name: &str, taken: &[String]) -> String {
    let mut base = String::new();
    for c in display_name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.ends_with('-') {
            base.push('-');
        }
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() {
        "app".to_string()
    } else {
        base
    };
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}")) // not user text
        .find(|c| !taken.contains(c))
        .unwrap_or(base)
}

#[cfg(test)]
#[path = "appcopy_tests.rs"]
mod tests;
