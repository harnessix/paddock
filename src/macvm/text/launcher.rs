//! The texts of the launcher commands: the pause and the resume of the
//! golden download, `paddock app add` and `paddock app remove`.

use super::gib;
use std::path::Path;

// ---- `paddock macvm setup --pause-download | --resume` -----------------

/// The pause file is written.
pub fn pause_requested() -> String {
    "The download pauses within 2 s. paddock macvm setup --resume continues it.".into()
}

/// No build holds the setup lock.
pub fn pause_no_build() -> String {
    "No golden build runs, so there is no download to pause. paddock macvm setup starts one.".into()
}

/// The build is in a later state than the download.
pub fn pause_past_download(step: &str) -> String {
    format!(
        "The golden build is in the {step} state, past the download. Only the download can pause."
    )
}

/// `--resume` while a build runs: it continues by itself.
pub fn resume_running() -> String {
    "The download continues.".into()
}

/// The helper of the build ended without a last line.
pub fn setup_helper_exit(code: i32) -> String {
    format!("paddock-vm macvm-setup ended with exit code {code}. paddock macvm setup continues where it stopped.")
}

// ---- `paddock app add` and `paddock app remove` ------------------------

/// The name of `--name` belongs to another profile.
pub fn add_name_taken(name: &str) -> String {
    format!("The name {name} belongs to another app profile. Choose another one with --name.")
}

/// The golden is not sealed.
pub fn add_no_golden() -> String {
    "Build the base image first: paddock macvm setup.".into()
}

/// The first start would not fit.
pub fn add_low_space(need: u64, free: u64) -> String {
    format!(
        "Needs {}, {} free. Free up space, then add the app again.",
        gib(need),
        gib(free)
    )
}

/// The bundle is a profile already.
pub fn add_already(name: &str, bundle: &Path) -> String {
    format!(
        "{} is the app {name} already. paddock app run {name} starts it.",
        bundle.display()
    )
}

/// `--dry-run` passed every check.
pub fn add_would(display: &str, name: &str) -> String {
    format!("{display} can be added as {name}. Run the command again without --dry-run.")
}

/// The profile is in the user configuration.
pub fn add_done(display: &str, name: &str) -> String {
    format!("Added {display} as {name}. paddock app install {name} adds its launcher, paddock app run {name} starts it.")
}

/// No profile folder and no `[apps.NAME] bundle`.
pub fn app_not_added(name: &str) -> String {
    format!("No app {name} in paddock's macOS guest. paddock app list --json shows the apps.")
}

/// The question before a removal.
pub fn app_remove_question(name: &str, bytes: u64, delete_data: bool) -> String {
    if delete_data {
        format!(
            "Remove {name} and delete its data now ({})? The original app stays.",
            gib(bytes)
        )
    } else {
        format!(
            "Remove {name} ({})? Its data disk goes to the trash folder for 7 days. The original app stays.",
            gib(bytes)
        )
    }
}

/// The removal ended.
pub fn app_removed(name: &str, freed: u64, trash: Option<&Path>) -> String {
    match trash {
        Some(t) => format!(
            "Removed {name}: {} freed. Its data disk is in {} for 7 days.",
            gib(freed),
            t.display()
        ),
        None => format!("Removed {name}: {} freed.", gib(freed)),
    }
}

// ---- `paddock doctor --json` -------------------------------------------

/// The chip check passed.
pub fn doctor_chip_ok(chip: Option<&str>) -> String {
    match chip {
        Some(c) => format!("This Mac has Apple silicon ({c})."),
        None => "This Mac has Apple silicon.".into(),
    }
}

/// The macOS check passed.
pub fn doctor_macos_ok(version: &str) -> String {
    format!("macOS {version}.")
}

/// `sw_vers` said nothing.
pub fn doctor_macos_unknown() -> String {
    "paddock cannot read the macOS version: sw_vers gave no answer.".into()
}

/// The free space against the need of the next action.
pub fn doctor_space(free: u64, need: u64) -> String {
    if free >= need {
        format!("{} free, {} needed.", gib(free), gib(need))
    } else {
        format!(
            "Needs {}, {} free. Free up space, then check again.",
            gib(need),
            gib(free)
        )
    }
}

/// The memory of the Mac and what each app gets.
pub fn doctor_memory(bytes: u64, low: bool) -> String {
    let per_app = (bytes / 2).max(4 << 30);
    if low {
        format!(
            "{}: apps get {} each, which is the minimum.",
            gib(bytes),
            gib(4 << 30)
        )
    } else {
        format!("{}: each app gets up to {}.", gib(bytes), gib(per_app))
    }
}

/// `hw.memsize` said nothing.
pub fn doctor_memory_unknown() -> String {
    "paddock cannot read the memory of this Mac: apps get 4 GiB each.".into()
}

/// The Virtualization entitlement of `paddock-vm`: present, missing, or
/// no helper at all.
pub fn doctor_entitlement(present: Option<bool>) -> String {
    match present {
        Some(true) => "paddock-vm has the Virtualization entitlement.".into(),
        Some(false) => {
            "paddock-vm lacks the Virtualization entitlement. Install paddock again with Homebrew."
                .into()
        }
        None => "paddock-vm is not installed next to paddock. Install paddock again with Homebrew."
            .into(),
    }
}

/// The two VM slots of macOS.
pub fn doctor_slots(used: usize, total: usize) -> String {
    if used >= total {
        format!("{used} of {total} macOS VMs run. Stop one before you start another app.")
    } else {
        format!("{used} of {total} macOS VMs run.")
    }
}

#[cfg(test)]
#[path = "launcher_tests.rs"]
mod tests;
