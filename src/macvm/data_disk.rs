//! The data disk of a new profile, made on the host before its first
//! boot. A raw disk of zeros makes the guest show the dialog "The disk you
//! attached was not readable by this computer" at boot, before the agent
//! can format it. So the host gives the new `data.img` a GPT and an APFS
//! container with the volume `Data`, as a plain user: `hdiutil attach
//! -nomount` of the raw image, `diskutil partitionDisk` of the attached
//! disk, `hdiutil detach`. The guest then sees a readable disk, and the
//! agent's `data.prepare` finds it formatted and empty and moves the home
//! onto it (`guest::data`).
//!
//! A format that fails leaves the disk blank again, with one warning: the
//! agent then partitions it as before, and a dialog that shows is pressed
//! away by the agent (`guest::dialogs`). The disk is made as `.partial`
//! and renamed when complete, so a crash in the format leaves a partial
//! that the next start removes (`Repair`).
//!
//! A host-made volume is not encrypted, so it is not bound to the system
//! disk by keys: a guest-made volume on Apple silicon is. The pair rule
//! (`pair.rs`) stays for both: the account record and the mount daemon of
//! the home live on the system disk.

use super::diskimage::{self, parse_plist, Plist};
use super::pair::data_formatted;
use super::session::{commit, partial};
use super::text;
use anyhow::{anyhow, Context, Result};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

/// The name of the volume, as the agent mounts it.
pub const VOLUME: &str = "Data";

/// What [`make`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Made {
    /// GPT and APFS with the volume `Data`.
    Formatted,
    /// Zeros: the format failed, for this reason. The agent formats it.
    Blank(String),
}

/// Make the data disk `path` of `bytes`, sparse, mode 0600, and give it to
/// `format` as the partial file before the rename.
pub fn make(path: &Path, bytes: u64, format: &dyn Fn(&Path) -> Result<()>) -> Result<Made> {
    let p = partial(path);
    let _ = std::fs::remove_file(&p);
    {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p)
            .with_context(|| text::cannot_make(&p))?;
        f.set_len(bytes).with_context(|| text::cannot_size(&p))?;
        f.sync_all()?;
    }
    let made = match format(&p) {
        Ok(()) if !data_formatted(&p) => Made::Blank(text::data_format_no_table()),
        Ok(()) if std::fs::metadata(&p).map(|m| m.len()).ok() != Some(bytes) => {
            Made::Blank(text::data_format_size_changed())
        }
        Ok(()) => Made::Formatted,
        Err(e) => Made::Blank(format!("{e:#}")),
    };
    if let Made::Blank(why) = &made {
        log::warn!("macvm: {}", text::data_format_failed(why));
        blank(&p, bytes)?;
    }
    commit(&p)?;
    Ok(made)
}

/// Zeros again: whatever a failed format wrote is gone.
fn blank(p: &Path, bytes: u64) -> Result<()> {
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(p)
        .with_context(|| text::cannot_make(p))?;
    f.set_len(0).with_context(|| text::cannot_size(p))?;
    f.set_len(bytes).with_context(|| text::cannot_size(p))?;
    f.sync_all()?;
    Ok(())
}

/// The argv of the format of an attached disk.
pub fn partition_argv(disk: &str) -> Vec<String> {
    [
        "/usr/sbin/diskutil",
        "partitionDisk",
        disk,
        "1",
        "GPT",
        "APFS",
        VOLUME,
        "100%",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The whole disk of `hdiutil attach -plist` of a blank image: the entry
/// `/dev/diskN` without a partition suffix. A blank image has no
/// `GUID_partition_scheme` hint, so `diskimage::entities` finds no disk.
pub fn whole_disk(xml: &str) -> Result<Option<String>> {
    let p = parse_plist(xml)?;
    let list = p
        .get("system-entities")
        .ok_or_else(|| anyhow!(text::plist_unreadable("no system-entities")))?;
    Ok(list.array().iter().find_map(|e| {
        let dev = e.get("dev-entry").and_then(Plist::str)?;
        let n = dev.strip_prefix("/dev/disk")?;
        (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| dev.to_string())
    }))
}

/// A command runner: the argv gives the success and both streams.
pub type Run<'a> = &'a mut dyn FnMut(&[String]) -> Result<(bool, String, String)>;

/// Attach the blank `image` without a mount, partition it, detach it. The
/// detach runs after a failed partition too; it retries a busy disk
/// (Spotlight looks at the new volume) and forces the last try.
pub fn format_image(image: &Path, run: Run, pause: &dyn Fn(Duration)) -> Result<()> {
    let attach = diskimage::attach_nomount_argv(image);
    let (ok, out, err) = run(&attach)?;
    if !ok {
        return Err(anyhow!(text::disk_command_failed(
            &attach.join(" "),
            out.trim(),
            err.trim()
        )));
    }
    let disk = whole_disk(&out)?.ok_or_else(|| anyhow!(text::attach_no_disk(image)))?;
    let argv = partition_argv(&disk);
    let formatted = match run(&argv) {
        Ok((true, _, _)) => Ok(()),
        Ok((false, out, err)) => Err(anyhow!(text::disk_command_failed(
            &argv.join(" "),
            out.trim(),
            err.trim()
        ))),
        Err(e) => Err(e),
    };
    let detached = detach(&disk, run, pause);
    formatted?;
    detached
}

fn detach(disk: &str, run: Run, pause: &dyn Fn(Duration)) -> Result<()> {
    let mut last = None;
    for attempt in 0..4 {
        if attempt > 0 {
            pause(Duration::from_secs(2));
        }
        let argv = diskimage::detach_argv(disk, attempt == 3);
        match run(&argv) {
            Ok((true, _, _)) => return Ok(()),
            Ok((false, out, err)) => {
                last = Some(anyhow!(text::disk_command_failed(
                    &argv.join(" "),
                    out.trim(),
                    err.trim()
                )))
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("one try ran"))
}

/// The format of a Mac: the real commands.
pub fn format_on_mac(image: &Path) -> Result<()> {
    format_image(image, &mut |argv| diskimage::run(argv), &std::thread::sleep)
}

#[cfg(test)]
#[path = "data_disk_tests.rs"]
mod tests;
