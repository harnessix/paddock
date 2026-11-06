//! The texts of failure behavior, recovery and data safety: the locked
//! data disk, the golden note, the reset tiers, the backups of the disk
//! pair, the restore, `paddock gc`, the disk space during a session, the
//! timeouts, the wake and the crash counter.

use super::{gib, shown, ResetTier};
use std::path::Path;

// ---- The disk pair ------------------------------------------------------

/// The data disk is formatted and its system disk is gone.
pub fn data_locked(profile: &Path, backup: Option<&Path>) -> String {
    let name = profile
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "the profile".into());
    let fix = match backup {
        Some(b) => format!(
            "Put the newest backup of the pair back: paddock app restore {name} {}",
            shown(b)
        ),
        None => format!(
            "There is no backup. A new pair, which removes the login and the files of the guest: paddock app reset {name} --data"
        ),
    };
    format!(
        "the data disk of {name} ({}) is formatted and its system disk is gone, so nothing can read it: on Apple silicon the keys of a data disk live in the system disk that made it. Nothing was changed. {fix}",
        shown(profile)
    )
}

/// The golden moved on; this profile keeps its own system disk.
pub fn golden_newer(name: &str, profile_golden: &str, golden: &str) -> String {
    format!(
        "{name} runs macOS {profile_golden} from its own system disk; the golden is macOS {golden}. A profile keeps its system disk for the life of its data. To move {name} to macOS {golden}: paddock app reset {name} --data (a backup of the pair is made first; the login and the files of the guest go)"
    )
}

/// The `next start` of a profile that cannot start.
pub fn next_start_refused(reason: &str) -> String {
    format!("refused: {reason}")
}

// ---- Reset ---------------------------------------------------------------

/// The line after a reset.
pub fn reset_done(name: &str, tier: ResetTier, removed: usize, aside: Option<&Path>) -> String {
    let what = match tier {
        ResetTier::System => "the saved state, the app clone, the drops and the logs",
        ResetTier::Data => {
            "the disk pair, the saved state, the stamp, the app clone, the drops and the logs"
        }
        ResetTier::All => "the whole profile",
    };
    let aside = match (tier, aside) {
        (ResetTier::Data, Some(p)) => format!(" The pair before the reset is in {}.", shown(p)),
        (ResetTier::All, Some(p)) => format!(
            " The folder is in {} for 7 days; paddock gc removes it after that.",
            shown(p)
        ),
        _ => String::new(),
    };
    format!(
        "reset {name}: removed {what} ({removed} entries).{aside} The next start is a cold boot"
    )
}

/// `--data` or `--all` without room for the backup: the reset stops.
pub fn reset_needs_backup(name: &str, needed: u64, free: u64) -> String {
    format!(
        "paddock app reset {name} stops: the backup of the pair needs {} and the disk has {} free. Free space (paddock gc) or remove old backups (paddock app backups {name}), then try again",
        gib(needed),
        gib(free)
    )
}

// ---- Backups ------------------------------------------------------------

/// A backup of the disk pair landed.
pub fn backup_pair_made(dir: &Path, bytes: u64) -> String {
    format!(
        "backup of the disk pair: {} ({} of data, the system disk shares its blocks)",
        shown(dir),
        gib(bytes)
    )
}

/// One line of `paddock app backups NAME`.
pub fn backup_list_line(name: &str, when: &str, bytes: u64, manual: bool) -> String {
    let kind = if manual { "manual" } else { "automatic" };
    format!("{name:<40} {when:<20} {:>9}  {kind}", gib(bytes))
}

/// A backup file of the data disk alone, from before the backups kept
/// the disk pair.
pub fn backup_list_data_only(name: &str) -> String {
    format!("{name:<40} data only: not restorable (it needs the system disk of its time)")
}

pub fn no_backups(name: &str, dir: &Path) -> String {
    format!(
        "{name} has no backups in {}. paddock app backup {name} makes one",
        shown(dir)
    )
}

pub fn backup_not_found(name: &str, which: &str, dir: &Path) -> String {
    format!(
        "no backup {which:?} of {name} in {}. paddock app backups {name} lists them",
        shown(dir)
    )
}

/// A restore of a data-only file is refused.
pub fn restore_refused_data_only(path: &Path) -> String {
    format!(
        "{} holds the data disk alone, which no system disk can read now. It cannot be restored",
        shown(path)
    )
}

/// A backup folder without its data disk or its system disk.
pub fn backup_incomplete(dir: &Path, missing: &str) -> String {
    format!(
        "the backup {} has no {missing}: it cannot be restored",
        shown(dir)
    )
}

pub fn restore_pair_done(name: &str, from: &Path, aside: &Path) -> String {
    format!(
        "{name} runs from the pair of {} at its next start (a cold boot). The pair from before the restore is {}",
        shown(from),
        shown(aside)
    )
}

/// `paddock app backup NAME --out DIR` wrote the pair there.
pub fn backup_written(dir: &Path) -> String {
    format!("backup of the disk pair written to {}", shown(dir))
}

/// No pair to back up.
pub fn nothing_to_backup(name: &str) -> String {
    format!("{name} has no data disk yet: nothing to back up")
}

/// `retain` or `paddock gc` removed an old automatic backup.
pub fn backup_retired(dir: &Path) -> String {
    format!("removed the old automatic backup {}", shown(dir))
}

// ---- paddock gc ---------------------------------------------------------

pub fn gc_trash_what(name: &str) -> String {
    format!("the profile {name} of the macOS guest, removed with paddock app reset --all")
}

pub fn gc_trash_reason(days: u64) -> String {
    format!("the trash of the macOS guest keeps a removed profile for {days} days")
}

pub fn gc_backup_what(profile: &str, name: &str) -> String {
    format!("the automatic backup {name} of the macOS guest profile {profile}")
}

pub fn gc_backup_reason() -> String {
    "paddock keeps the newest 3 automatic backups of a profile for 30 days at most; manual backups stay"
        .into()
}

// ---- Disk space during the session --------------------------------------

/// 1 GiB free: the guest shows an alert and the next quit skips the save.
pub fn disk_low_line(free: u64) -> String {
    format!(
        "the disk has {} free: the guest shows an alert, and the next quit skips the saved state (the next start is a cold boot)",
        gib(free)
    )
}

// ---- Timeouts, wake and crashes -----------------------------------------

/// Three timeouts in a row on the control line.
pub fn agent_timeouts(n: u32) -> String {
    format!(
        "the guest agent did not answer {n} requests in a row: paddock treats it as crashed, closes the control connection and waits for launchd to restart it"
    )
}

/// The host woke up.
pub fn wake_line(slept_secs: u64) -> String {
    format!("the Mac slept for {slept_secs} s: paddock pings the guest, closes the idle tunnels and asks the open firewall questions again")
}

pub fn sleep_line() -> String {
    "the Mac sleeps: the VM pauses with the process".into()
}

/// Three crashes of the app in a minute stop the relaunch.
pub fn relaunch_stopped(app: &str, report: Option<&Path>) -> String {
    match report {
        Some(p) => format!(
            "{app} exited three times in a minute, so paddock does not open it again. The crash report of the guest: {}",
            shown(p)
        ),
        None => format!(
            "{app} exited three times in a minute, so paddock does not open it again. The guest wrote no crash report under ~/Library/Logs/DiagnosticReports"
        ),
    }
}

/// The guest side of a formatted data disk that does not mount.
pub fn data_disk_locked_guest(detail: &str) -> String {
    format!("the data disk has a partition table but its volume does not mount ({detail}): it belongs to another system disk, so it was not touched")
}

/// The guest side of a mounted data disk with another profile id.
pub fn data_id_differs(have: &str, want: &str) -> String {
    format!("the data disk carries the id {have} and the host named {want}: the disk and its id stay as they are")
}

// ---- Shared refusals ----------------------------------------------------

pub fn no_profile(app: &str, dir: &Path) -> String {
    format!("{app} has no profile in the macOS guest ({})", shown(dir))
}

pub fn exists_already(path: &Path) -> String {
    format!(
        "{} exists already: name a folder that does not",
        shown(path)
    )
}

// ---- Event texts of the matrix ------------------------------------------

pub fn guest_restart_event() -> String {
    "guest restart: no health for 120 s, stop with force, then a cold boot with the same arguments"
        .into()
}

pub fn agent_connection_closed() -> String {
    "the control connection closed: launchd restarts the agent, the window map is rebuilt from its hello".into()
}

pub fn session_ended_without_save_event(pid: u32) -> String {
    format!("session ended without a save (the shim pid {pid} is gone)")
}

pub fn host_restarted() -> String {
    "paddock restarted on the host: the session continues on its own".into()
}

pub fn health_back() -> String {
    "health back".into()
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
