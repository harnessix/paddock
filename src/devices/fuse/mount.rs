//! The mount of the device file system, and the reader check of a
//! request that waits.

use super::DeviceFs;
use anyhow::{Context, Result};
use fuser::MountOption;
use std::path::{Path, PathBuf};

/// Whether the thread `tid` has a fatal signal pending. The kernel marks
/// every fatal signal as SIGKILL in the pending set of each thread.
pub(super) fn fatal_pending(tid: u32) -> bool {
    let Ok(text) = std::fs::read_to_string(format!("/proc/{tid}/status")) else {
        // Gone already.
        return true;
    };
    text.lines()
        .filter_map(|l| {
            l.strip_prefix("SigPnd:")
                .or_else(|| l.strip_prefix("ShdPnd:"))
        })
        .filter_map(|v| u64::from_str_radix(v.trim(), 16).ok())
        .any(|bits| bits & (1 << (libc::SIGKILL - 1)) != 0)
}

/// A mounted device file system. Dropping it unmounts it.
pub struct Mounted {
    pub dir: PathBuf,
    session: Option<fuser::BackgroundSession>,
}

impl Mounted {
    pub fn unmount(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Some(s) = self.session.take() else {
            return;
        };
        if let Err(e) = s.umount_and_join() {
            log::debug!("cannot unmount {}: {e}", self.dir.display());
            lazy_unmount(&self.dir);
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(in crate::devices) fn lazy_unmount(dir: &Path) {
    for helper in ["fusermount3", "fusermount"] {
        let ok = std::process::Command::new(helper)
            .args(["-u", "-q", "-z", "--"])
            .arg(dir)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return;
        }
    }
}

/// Mount `fs` at `dir`, an empty folder that paddock owns.
pub fn mount(dir: &Path, fs: DeviceFs) -> Result<Mounted> {
    crate::privfs::dir(dir)?;
    let mut cfg = fuser::Config::default();
    cfg.mount_options = vec![
        MountOption::FSName("paddock".into()),
        MountOption::Subtype("paddock".into()),
        MountOption::DefaultPermissions,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoExec,
        MountOption::NoAtime,
    ];
    let session = fuser::spawn_mount(fs, dir, &cfg)
        .with_context(|| format!("cannot mount the devices at {}", dir.display()))?;
    Ok(Mounted {
        dir: dir.to_path_buf(),
        session: Some(session),
    })
}
