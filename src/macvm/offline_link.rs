//! The `Downloads` link of the guest home, made offline, platform-neutral.
//!
//! The configure step `downloads-link` once replaced `~/Downloads` with a
//! link from a command in the running guest. macOS guards the Downloads
//! folder with a Files and Folders prompt (TCC), which nobody answers, so
//! the command hung until the step timeout killed it. With the guest
//! stopped, the folder is a plain folder of the Data volume:
//!
//! 1. Attach the disk and mount the Data volume without owners.
//! 2. [`make`]: the empty `Users/paddock/Downloads` goes (only a
//!    `.localized` or `.DS_Store` in it; any other file is an error, so no
//!    file is lost), and a link to [`DOWNLOADS_TARGET`] takes its place.
//! 3. Detach, then [`own`]: the link gets the owner of the home folder,
//!    501:20 for the first account, through the APFS owner patch
//!    (`apfs.rs`) that the seed uses for its root files, and the owner is
//!    read back.
//!
//! The same attach writes the default browser of the guest account, the
//! link helper of the agent (`launch_services.rs`), and gives that file
//! the owner of the home too.
//!
//! A link of an earlier try is kept. The `downloads-link` step in the
//! guest proves the link with `readlink`, which macOS does not guard. The
//! agent checks the link again at each boot (`guest/downloads.rs`).

use super::apfs;
use super::configure::DOWNLOADS_TARGET;
use super::diskimage::{Attached, DiskTool};
use super::golden::GUEST_USER;
use super::text;
use anyhow::{anyhow, Context, Result};
use std::io::ErrorKind;
use std::path::Path;

/// The files of a fresh `Downloads` folder that may go with it.
pub const DISPOSABLE: [&str; 2] = [".localized", ".DS_Store"];

/// What [`make`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linked {
    /// The link is new: the folder went, or nothing stood there.
    Made,
    /// The right link was already there.
    Already,
}

impl Linked {
    /// The word of the event log.
    pub fn word(self) -> &'static str {
        match self {
            Linked::Made => "made",
            Linked::Already => "already",
        }
    }
}

/// The home of the guest account, relative to the Data volume root.
pub fn home() -> String {
    format!("Users/{GUEST_USER}") // not user text
}

/// The link, relative to the Data volume root.
pub fn downloads() -> String {
    format!("{}/{}", home(), crate::guest::downloads::DOWNLOADS) // not user text
}

/// Make `Downloads` of the guest home a link to [`DOWNLOADS_TARGET`],
/// under the mount `data` of the Data volume.
pub fn make(data: &Path) -> Result<Linked> {
    let home = data.join(home());
    if !home.is_dir() {
        return Err(anyhow!(text::downloads_no_home(&home)));
    }
    let link = data.join(downloads());
    match std::fs::symlink_metadata(&link) {
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::Error::new(e).context(text::cannot_open(&link))),
        Ok(md) if md.file_type().is_symlink() => {
            let now = std::fs::read_link(&link).with_context(|| text::cannot_open(&link))?;
            if now == Path::new(DOWNLOADS_TARGET) {
                return Ok(Linked::Already);
            }
            std::fs::remove_file(&link).with_context(|| text::cannot_remove(&link))?;
        }
        Ok(md) if md.is_dir() => empty_folder(&link)?,
        Ok(_) => return Err(anyhow!(text::downloads_not_a_folder(&link))),
    }
    std::os::unix::fs::symlink(DOWNLOADS_TARGET, &link)
        .with_context(|| text::cannot_write(&link))?;
    Ok(Linked::Made)
}

/// Remove the folder at `dir` when it holds only [`DISPOSABLE`] files.
fn empty_folder(dir: &Path) -> Result<()> {
    let mut extra = Vec::new();
    let mut gone = Vec::new();
    for e in std::fs::read_dir(dir).with_context(|| text::cannot_open(dir))? {
        let e = e.with_context(|| text::cannot_open(dir))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let plain_file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        if plain_file && DISPOSABLE.contains(&name.as_str()) {
            gone.push(e.path());
        } else {
            extra.push(name);
        }
    }
    if !extra.is_empty() {
        extra.sort();
        return Err(anyhow!(text::downloads_not_empty(dir, &extra)));
    }
    for p in gone {
        std::fs::remove_file(&p).with_context(|| text::cannot_remove(&p))?;
    }
    // macOS gives ~/Downloads the ACL `everyone deny delete`, and a mount
    // without owners still obeys ACLs: drop them first (`chmod -N`; the
    // mount makes this user the owner). Elsewhere there is no ACL to drop.
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    std::fs::remove_dir(dir).with_context(|| text::cannot_remove(dir))
}

/// The owner for the link: the owner of the home folder on the detached
/// image. A home of root or of the mount without owners is an error.
pub fn home_owner(disk: &Path) -> Result<(u32, u32)> {
    let home = home();
    let o = apfs::owners(disk, apfs::ROLE_DATA, &[home.as_str()])?[0];
    if o.0 == 0 || o.0 == apfs::NOOWNERS_ID {
        return Err(anyhow!(text::apfs_unexpected_owner(&home, o.0, o.1)));
    }
    Ok(o)
}

/// Give the link the owner of the home on the detached image `disk`, and
/// read it back. Returns the owner.
pub fn own(disk: &Path) -> Result<(u32, u32)> {
    let (uid, gid) = home_owner(disk)?;
    let link = downloads();
    apfs::chown(disk, apfs::ROLE_DATA, uid, gid, &[link.as_str()])?;
    let now = apfs::owners(disk, apfs::ROLE_DATA, &[link.as_str()])?[0];
    if now != (uid, gid) {
        return Err(anyhow!(text::apfs_unexpected_owner(&link, now.0, now.1)));
    }
    Ok((uid, gid))
}

/// The whole offline link of the stopped guest `disk`, with the mounts
/// under `mnt`: attach, [`make`], detach, [`own`]. The detach runs
/// whatever `make` did.
pub fn link_disk(tool: &mut dyn DiskTool, disk: &Path, mnt: &Path) -> Result<Linked> {
    tool.detach_stale(mnt);
    let a: Attached = tool.attach(disk, mnt)?;
    let made = make(&a.data);
    // The default browser of the guest, in the same attach: the link
    // helper for http and https (`launch_services.rs`). Its failure is a
    // warning, never a failure of the link.
    let browser = made.as_ref().ok().map(|_| {
        super::launch_services::write(
            &a.data,
            crate::guest::link::BUNDLE_ID,
            &super::launch_services::plutil_xml,
        )
    });
    let detached = tool.detach(&a);
    super::diskimage::remove_mount_root(mnt);
    let linked = made?;
    detached?;
    own(disk)?;
    if let Some(w) = browser {
        match w.and_then(|w| super::launch_services::own(disk, &w).map(|_| w)) {
            Ok(w) => log::info!("macvm: {}", text::launch_services_written(w.merged)),
            Err(e) => log::warn!("macvm: {}", text::launch_services_failed(&format!("{e:#}"))),
        }
    }
    Ok(linked)
}

#[cfg(test)]
#[path = "offline_link_tests.rs"]
mod tests;
