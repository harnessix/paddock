//! The offline seed of `paddock macvm setup`, platform-neutral: the agent
//! goes into the guest disk before the first boot, with no person, no
//! Terminal and no root on the host.
//!
//! The steps, each through [`DiskTool`]:
//!
//! 1. An APFS clone of `disk.img` as `disk.img.before-seed`: any error
//!    later puts it back, so the disk is untouched for the guided window.
//!    A clone left by a crash is put back first.
//! 2. Attach the raw image and mount the Data volume without owners.
//! 3. Write the first-boot daemon plist, `firstboot.sh` and the agent
//!    tree ([`files`]), and note every folder and file that the write made.
//! 4. Detach, then set the owner of exactly those inodes to 0:0 on the
//!    image file (`apfs.rs`), and read them back.
//! 5. `fsck_apfs -n` of the container, when a plain user can run it.
//!
//! On the first boot launchd runs `firstboot.sh` as root
//! (`packaging/macos/guest/firstboot.sh`): it installs the agent as the
//! typed seed command does and removes itself.

use super::apfs;
use super::diskimage::{Attached, DiskTool, Fsck};
use super::guest_files;
use super::seed;
use super::text;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// The folder of the first-boot files on the Data volume.
pub const STAGE: &str = "Library/Application Support/paddock-firstboot";
/// The first-boot daemon on the Data volume.
pub const DAEMON: &str = "Library/LaunchDaemons/dev.paddock.firstboot.plist";
/// The label of the daemon.
pub const DAEMON_LABEL: &str = "dev.paddock.firstboot";
/// The script that the daemon runs.
pub const FIRSTBOOT: &str = include_str!("../../packaging/macos/guest/firstboot.sh");
/// The daemon plist.
pub const DAEMON_PLIST: &str =
    include_str!("../../packaging/macos/guest/dev.paddock.firstboot.plist");
/// The name of the clone of the disk while the seed runs.
pub const BACKUP_SUFFIX: &str = "before-seed";

/// One file to write into the Data volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestFile {
    /// Relative to the root of the Data volume.
    pub path: String,
    pub body: Vec<u8>,
    pub mode: u32,
}

fn stage(rel: &str) -> String {
    format!("{STAGE}/{rel}") // not user text
}

/// The files of the offline seed, from the seed folder that `Seed` wrote:
/// the agent tree, `install.sh`, the LaunchAgent plist, `firstboot.sh`
/// and the daemon plist.
pub fn files(seed_dir: &Path) -> Result<Vec<GuestFile>> {
    let read = |rel: &str| -> Result<Vec<u8>> {
        let p = seed_dir.join(rel);
        std::fs::read(&p).with_context(|| text::cannot_read(&p))
    };
    let agent = format!("{}/current/{}", seed::AGENT_DIR, guest_files::BINARY); // not user text
    let launch = format!("{}/launch", seed::AGENT_DIR); // not user text
    let mut out = vec![
        GuestFile {
            path: stage("firstboot.sh"),
            body: FIRSTBOOT.as_bytes().to_vec(),
            mode: 0o755,
        },
        GuestFile {
            path: stage(seed::INSTALL_FILE),
            body: read(seed::INSTALL_FILE)?,
            mode: 0o644,
        },
        GuestFile {
            path: stage(&seed::plist_file()),
            body: read(&seed::plist_file())?,
            mode: 0o644,
        },
        GuestFile {
            path: stage(&launch),
            body: read(&launch)?,
            mode: 0o755,
        },
    ];
    if seed_dir.join(&agent).is_file() {
        out.push(GuestFile {
            path: stage(&agent),
            body: read(&agent)?,
            mode: 0o755,
        });
    }
    // The daemon last: launchd sees it only with every file it needs.
    out.push(GuestFile {
        path: DAEMON.to_string(),
        body: DAEMON_PLIST.as_bytes().to_vec(),
        mode: 0o644,
    });
    Ok(out)
}

/// Write `files` under the mount `root` and give every path that the
/// write made or replaced, folders before their files, relative to the
/// volume root. A first-boot folder of an earlier try goes first.
pub fn write(root: &Path, files: &[GuestFile]) -> Result<Vec<String>> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    let old = root.join(STAGE);
    if old.exists() {
        std::fs::remove_dir_all(&old).with_context(|| text::cannot_remove(&old))?;
    }
    let mut made: Vec<String> = Vec::new();
    for f in files {
        let mut rel = PathBuf::new();
        let parts: Vec<&str> = f.path.split('/').filter(|p| !p.is_empty()).collect();
        let Some((name, dirs)) = parts.split_last() else {
            continue;
        };
        for d in dirs {
            rel.push(d);
            let p = root.join(&rel);
            if !p.is_dir() {
                std::fs::create_dir(&p).with_context(|| text::cannot_make(&p))?;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
                    .with_context(|| text::cannot_write(&p))?;
                made.push(rel.to_string_lossy().into_owned());
            }
        }
        rel.push(name);
        let p = root.join(&rel);
        let mut file = std::fs::File::create(&p).with_context(|| text::cannot_write(&p))?;
        file.write_all(&f.body)
            .and_then(|()| file.sync_all())
            .with_context(|| text::cannot_write(&p))?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(f.mode))
            .with_context(|| text::cannot_write(&p))?;
        made.push(rel.to_string_lossy().into_owned());
    }
    Ok(made)
}

/// What the offline seed did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The paths that now belong to root.
    pub owned: Vec<String>,
    pub fsck: Fsck,
}

/// The clone of `disk` while a seed runs.
pub fn backup_of(disk: &Path) -> PathBuf {
    let mut s = disk.as_os_str().to_os_string();
    s.push(".");
    s.push(BACKUP_SUFFIX);
    PathBuf::from(s)
}

/// Put back the clone of a seed that crashed. True when one was there.
pub fn restore_leftover(disk: &Path) -> Result<bool> {
    let b = backup_of(disk);
    if !b.exists() {
        return Ok(false);
    }
    std::fs::rename(&b, disk)
        .map_err(|e| anyhow!(text::seed_restore_failed(&b, disk, &e.to_string())))?;
    Ok(true)
}

/// The offline seed of `disk` with the mounts under `mnt`. On an error the
/// disk is the untouched clone again, and the error says what failed.
pub fn seed_disk(
    tool: &mut dyn DiskTool,
    disk: &Path,
    mnt: &Path,
    files: &[GuestFile],
) -> Result<Report> {
    tool.detach_stale(mnt);
    restore_leftover(disk)?;
    let backup = backup_of(disk);
    tool.clone_file(disk, &backup)?;
    let r = patch(tool, disk, mnt, files);
    super::diskimage::remove_mount_root(mnt);
    match r {
        Ok(report) => {
            if let Err(e) = std::fs::remove_file(&backup) {
                log::warn!("macvm: {}: {e}", backup.display()); // not user text
            }
            Ok(report)
        }
        Err(e) => {
            std::fs::rename(&backup, disk).map_err(|re| {
                anyhow!(text::seed_restore_failed(&backup, disk, &re.to_string()))
                    .context(format!("{e:#}")) // not user text
            })?;
            Err(e)
        }
    }
}

fn patch(tool: &mut dyn DiskTool, disk: &Path, mnt: &Path, files: &[GuestFile]) -> Result<Report> {
    let owned = {
        let a: Attached = tool.attach(disk, mnt)?;
        let written = write(&a.data, files);
        let detached = tool.detach(&a);
        let owned = written?;
        detached?;
        owned
    };
    let paths: Vec<&str> = owned.iter().map(String::as_str).collect();
    apfs::chown(disk, apfs::ROLE_DATA, 0, 0, &paths)?;
    let now = apfs::owners(disk, apfs::ROLE_DATA, &paths)?;
    if let Some((p, o)) = paths.iter().zip(&now).find(|(_, o)| **o != (0, 0)) {
        return Err(anyhow!(text::apfs_unexpected_owner(p, o.0, o.1)));
    }
    let fsck = tool.fsck(disk)?;
    Ok(Report { owned, fsck })
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "offline_macos_tests.rs"]
mod macos_tests;
