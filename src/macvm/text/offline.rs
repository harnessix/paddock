//! The texts of the offline seed and the offline trust of `paddock macvm
//! setup`: the APFS owner patch, the disk image tool, the first-boot files
//! and the TCC database of the guest.

use super::shown;
use crate::macvm::diskimage::Fsck;
use crate::macvm::provision::Why;
use std::path::Path;
use std::time::Duration;

// ---- The APFS owner patch ------------------------------------------------

/// The image is not the APFS disk that the patch expects.
pub fn apfs_damaged(what: &str) -> String {
    format!("the guest disk is not readable as APFS: {what}")
}

/// As [`apfs_damaged`], with the number that is wrong.
pub fn apfs_damaged_at(what: &str, n: u64) -> String {
    format!("the guest disk is not readable as APFS: {what} {n}")
}

/// An inode that paddock did not write.
pub fn apfs_unexpected_owner(path: &str, uid: u32, gid: u32) -> String {
    format!(
        "{path} on the guest disk has the owner {uid}:{gid}, not the 99:99 of a file that paddock wrote; nothing was changed"
    )
}

/// A path of the patch is not on the volume.
pub fn apfs_path_missing(path: &str, name: &str) -> String {
    format!("{path} is not on the guest disk: no entry {name}")
}

/// The write of one block failed.
pub fn apfs_write_failed(block: u64, error: &str) -> String {
    format!("cannot write block {block} of the guest disk: {error}")
}

// ---- The disk image tool --------------------------------------------------

/// The output of a tool is not the property list that paddock reads.
pub fn plist_unreadable(what: &str) -> String {
    format!("the output of the disk tool is not a readable property list: {what}")
}

/// A disk command ran past its time.
pub fn disk_command_timeout(command: &str, after: Duration) -> String {
    format!(
        "{command} did not end within {} s and was stopped",
        after.as_secs()
    )
}

/// A disk command failed: its output.
pub fn disk_command_failed(command: &str, out: &str, err: &str) -> String {
    let said = [out, err]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" / ");
    if said.is_empty() {
        format!("{command} failed with no output")
    } else {
        format!("{command} failed: {said}")
    }
}

/// The attach gave no whole disk.
pub fn attach_no_disk(image: &Path) -> String {
    format!("hdiutil attached {} but named no whole disk", shown(image))
}

/// The attach mounted no Data volume.
pub fn attach_no_data_volume(image: &Path) -> String {
    format!(
        "hdiutil attached {} but no Data volume was mounted; the disk was detached",
        shown(image)
    )
}

/// The attach gave no APFS physical store for the check.
pub fn attach_no_store(image: &Path) -> String {
    format!(
        "hdiutil attached {} but named no APFS partition",
        shown(image)
    )
}

/// The clone of the guest disk failed.
pub fn cannot_clone_disk(path: &Path, error: &str) -> String {
    format!(
        "cannot make an APFS clone of {}: {error}. The golden folder must be on an APFS volume",
        shown(path)
    )
}

// ---- The offline seed -----------------------------------------------------

/// The clone of the disk could not be put back.
pub fn seed_restore_failed(backup: &Path, disk: &Path, error: &str) -> String {
    format!(
        "cannot put the untouched guest disk {} back as {}: {error}. Move it there by hand before the next paddock macvm setup",
        shown(backup),
        shown(disk)
    )
}

// ---- The TCC database of the guest ----------------------------------------

/// The database is not on the Data volume.
pub fn tcc_db_missing(db: &Path) -> String {
    format!(
        "the guest has no TCC database at {}: the guest did not finish its first boot",
        shown(db)
    )
}

/// The table lacks a column that paddock needs, or has a required one
/// that paddock does not know.
pub fn tcc_schema_unknown(db: &Path, column: &str) -> String {
    format!(
        "the TCC database {} has a form that paddock does not know (column {column}); nothing was changed",
        shown(db)
    )
}

/// The database has no `access` table.
pub fn tcc_no_access_table() -> String {
    "the TCC database of the guest has no access table; nothing was changed".to_string()
}

/// The integrity check after the write failed.
pub fn tcc_integrity(db: &Path, said: &str) -> String {
    format!(
        "the TCC database {} failed its integrity check after the write: {said}",
        shown(db)
    )
}

/// A write-ahead log stayed after the close.
pub fn tcc_wal_left(wal: &Path) -> String {
    format!(
        "the write left the log {} that the guest would not own",
        shown(wal)
    )
}

// ---- The setup driver -----------------------------------------------------

/// `PADDOCK_MACVM_GUIDED=1` turns the offline patches off.
pub fn offline_off_by_flag() -> String {
    "PADDOCK_MACVM_GUIDED=1 leaves every step to the person".into()
}

/// The backend cannot mount the guest disk.
pub fn offline_no_tool() -> String {
    "this paddock cannot mount the guest disk on this system".into()
}

/// The offline seed did not run.
pub fn seed_offline_skipped(why: &str) -> String {
    format!("the agent is not written into the guest disk ({why}): the window shows the command to type in the guest")
}

/// The offline seed is done.
pub fn seed_offline_done(files: usize, fsck: &Fsck) -> String {
    let check = match fsck {
        Fsck::Clean => "fsck_apfs found no error".to_string(),
        Fsck::Skipped(why) => format!("fsck_apfs did not run as your user ({why})"),
    };
    format!(
        "the guest agent and its first-boot daemon are on the guest disk ({files} files and folders owned by root; {check}): nothing to type in the guest"
    )
}

/// The offline seed failed; the disk is untouched.
pub fn seed_offline_failed(why: &str) -> String {
    format!(
        "paddock could not write the agent into the guest disk, and the disk is as it was: {why}. The window shows the command to type in the guest instead"
    )
}

/// The TCC database has the row.
pub fn trust_offline_done() -> String {
    "the Accessibility permission of paddock-guest is set in the stopped guest; the guest boots again to check it".into()
}

/// The agent reported the permission after the patch.
pub fn trust_reported() -> String {
    "the guest agent reports the Accessibility permission".into()
}

/// The agent reported no permission after the patch.
pub fn trust_not_reported() -> String {
    "the guest agent still reports no Accessibility permission after the patch".into()
}

/// The offline trust failed: the person turns the switch on.
pub fn trust_offline_failed(why: &str) -> String {
    format!(
        "paddock could not set the Accessibility permission offline ({why}). Turn on paddock-guest in the Accessibility settings of the guest, as the window shows"
    )
}

/// Why a step is left to the person.
pub fn guided_why(why: &Why) -> String {
    match why {
        Why::Forced => "PADDOCK_MACVM_GUIDED=1 asks for it".into(),
        Why::HostOld(v) => format!("this Mac runs macOS {v}, and the guest account without Setup Assistant needs macOS 27"),
        Why::GuestOld(v) => format!("the guest is macOS {v}, and the guest account without Setup Assistant needs macOS 27"),
        Why::Missing(n) => format!("the Virtualization framework of this Mac has no {n}"),
        Why::FirstBootUsed => {
            "the guest booted once without the provisioning options, and it reads them only on its first boot".into()
        }
        Why::Refused(e) => format!("macOS refused the provisioning options ({e})"),
    }
}

/// The terminal line when the person makes the guest account.
pub fn route_guided(why: &Why) -> String {
    format!(
        "make the guest account in Setup Assistant in the window, as it shows: {}.",
        guided_why(why)
    )
}

/// The banner of the window while no step waits on the person.
pub fn unattended_banner() -> String {
    "paddock sets up the guest, there is nothing to do in this window. Click Stop to do the steps yourself."
        .into()
}

/// The agent did not dial in after the boot of a seeded disk.
pub fn seeded_agent_late(waited_secs: u64, console: &Path, command: &str) -> String {
    format!(
        "the guest agent did not start within {waited_secs} s of the boot. The first-boot log is in {} and in /Library/Logs/paddock-firstboot.log of the guest. Type this in Terminal of the guest instead: {command}",
        shown(console)
    )
}

/// The line of `paddock macvm status` after a failed offline step.
pub fn status_offline_failed(kind: &str, why: &str) -> String {
    format!("  setup    the {kind} failed: {why}. paddock macvm setup leaves that step to you in the window.")
}

/// The Data volume of the stopped guest has no home of the account.
pub fn downloads_no_home(home: &Path) -> String {
    format!(
        "the guest disk has no home folder {}: the guest account is not made yet",
        shown(home)
    )
}

/// A plain file stands where the Downloads link goes.
pub fn downloads_not_a_folder(path: &Path) -> String {
    format!(
        "{} on the guest disk is not a folder and not a link; paddock leaves it alone",
        shown(path)
    )
}

/// The Downloads folder of the guest holds files, so it cannot go.
pub fn downloads_not_empty(path: &Path, names: &[String]) -> String {
    format!(
        "{} on the guest disk holds files ({}); paddock removes only an empty folder. Move the files out in the guest, then run paddock macvm setup again",
        shown(path),
        names.join(", ")
    )
}

/// The offline Downloads link is in place.
pub fn downloads_offline_done(made: bool) -> String {
    if made {
        "~/Downloads of the guest is now a link to the downloads share, made in the stopped guest"
            .into()
    } else {
        "~/Downloads of the guest already is a link to the downloads share".into()
    }
}

/// The offline Downloads link failed or did not run.
pub fn downloads_offline_failed(why: &str) -> String {
    format!(
        "paddock could not link ~/Downloads of the guest to the downloads share in the stopped guest ({why}). The configure step downloads-link fails until the link is there"
    )
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;

// ---- The data disk of a new profile (`data_disk.rs`) -------------------

/// The host could not format a new data disk: it stays blank, and the
/// guest agent formats it.
pub fn data_format_failed(why: &str) -> String {
    format!(
        "the new data disk was not formatted on the host, so the guest agent formats it at the first start: {why}"
    )
}

pub fn data_format_no_table() -> String {
    "the format gave no partition table".into()
}

pub fn data_format_size_changed() -> String {
    "the format changed the size of the disk file".into()
}
