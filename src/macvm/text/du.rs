//! The texts of the macOS guest in `paddock du` and `paddock gc`. The
//! words follow the Linux homes of `src/storage`.

use std::path::Path;

/// The head of the section.
pub fn du_head(total: &str, root: &Path) -> String {
    format!("\nmacOS guest (macvm): {total} in {}\n", root.display())
}

/// One shared part of the folder.
pub fn du_part(size: &str, what: &str) -> String {
    format!("  {size:>9}  {what}\n")
}

/// The last use of a profile.
pub fn du_days_ago(days: u64) -> String {
    match days {
        0 => "today".into(),
        1 => "1 day ago".into(),
        n => format!("{n} days ago"),
    }
}

/// One profile instance.
pub fn du_profile(size: &str, used: &str, label: &str, state: &str) -> String {
    format!("  {size:>9}  {used:<12}  profile {label} [{state}]\n")
}

/// The parts of a profile instance, and what a removal loses.
pub fn du_profile_parts(disks: &str, state: &str, apps: &str, backups: &str) -> String {
    format!(
        "            disks {disks}, saved state {state}, app copies {apps}, backups {backups}\n            deleting loses: the app's login, its files in the guest and its saved state\n"
    )
}

pub fn gc_archive_what(label: &str, path: &Path) -> String {
    format!("app archive of profile {label}: {}", path.display())
}

pub fn gc_archive_reason() -> String {
    "the guest has its copy; the next copy of a new version makes the archive again".into()
}

pub fn gc_state_what(label: &str) -> String {
    format!("saved state of profile {label}")
}

pub fn gc_state_reason(days: u64) -> String {
    format!("not used for {days} days; the next start is a cold boot")
}

pub fn gc_profile_what(label: &str) -> String {
    format!("macOS guest profile {label}")
}

pub fn gc_profile_reason(days: u64) -> String {
    format!("not used for {days} days")
}

pub fn gc_profile_warning() -> String {
    "the app's login in this profile will be lost, with its files in the guest".into()
}

/// A gc removal of a profile part that started meanwhile.
pub fn gc_profile_in_use(app: &str) -> String {
    format!("a session of {app} started in it; skipped")
}

/// The automatic cleanup removed an item.
pub fn gc_auto_removed(what: &str, size: &str) -> String {
    format!("removed {what} ({size})")
}

/// The automatic cleanup could not remove an item.
pub fn gc_auto_failed(what: &str, e: &str) -> String {
    format!("cannot remove {what}: {e}")
}

/// A start found the volume short of space and ran the cleanup.
pub fn low_space_cleanup(free: &str) -> String {
    format!("only {free} free on the disk: paddock removes what it can make again (old app archives, old saved states, the trash)")
}

#[cfg(test)]
#[path = "du_tests.rs"]
mod tests;
