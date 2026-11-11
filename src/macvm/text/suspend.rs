//! The texts of automatic suspend and of the instances of a profile.

use std::time::Duration;

// ---- Instances -----------------------------------------------------------

/// `--instance` got a value that is not a number from 1 to `max`.
pub fn instance_invalid(value: &str, max: u32) -> String {
    format!("--instance takes a number from 1 to {max}, not '{value}'. Instance 1 is the first profile of the app")
}

/// Both `--instance` and `--new`.
pub fn instance_and_new() -> String {
    "use --instance N or --new, not both: --new picks the next free number".into()
}

/// `--new` found every number in use.
pub fn instances_full(max: u32) -> String {
    format!("the app has {max} instances, the most paddock makes. Remove one with paddock app reset NAME --instance N --all")
}

// ---- Why a session cannot be suspended -----------------------------------

pub fn busy_dialog() -> String {
    "a dialog of the app is open (a save dialog, for example)".into()
}

pub fn busy_copy() -> String {
    "the copy of the app into the guest runs".into()
}

pub fn busy_data_prepare() -> String {
    "the guest prepares its data disk".into()
}

/// The shim is quitting, or suspended already.
pub fn suspend_while_quitting() -> String {
    "the session is quitting or suspended already".into()
}

/// The shim is frozen by `paddock freeze`.
pub fn suspend_frozen() -> String {
    "the session is frozen (paddock freeze). paddock thaw resumes it".into()
}

/// `[macvm] save = false`: no saved state, so no suspend.
pub fn suspend_save_off() -> String {
    "the save is off ([macvm] save = false), so the session cannot be suspended without a loss"
        .into()
}

/// The save of a suspend failed; the VM runs again.
pub fn suspend_save_failed() -> String {
    "the save of the machine state failed, so the session runs on".into()
}

/// The event of a suspend that saved.
pub fn suspend_saved() -> String {
    "suspended: the machine state is saved and the VM slot is free".into()
}

// ---- The start that frees a slot -----------------------------------------

/// The reason that the suspended session records.
pub fn suspend_reason_start(label: &str) -> String {
    format!("both VM slots were in use and {label} started")
}

/// The line of the start after a suspend.
pub fn suspended_for(old: &str, old_arg: &str, new: &str) -> String {
    format!("{old} is suspended to free a VM slot for {new}. Click its Dock icon, or run paddock app run {old_arg}, to resume it")
}

/// The suspended session did not give its slot back in time.
pub fn suspend_timeout(label: &str, arg: &str, wait: Duration) -> String {
    format!(
        "{label} did not free its VM slot within {} s. paddock app status {arg} shows what it does; try again",
        wait.as_secs()
    )
}

/// No session could be suspended: each one with its reason.
pub fn suspend_refused_all(label: &str, refused: &[(String, String)]) -> String {
    let each: Vec<String> = refused
        .iter()
        .map(|(name, why)| format!("{name}: {why}"))
        .collect();
    format!(
        "{label} cannot start: both VM slots are in use and no session can be suspended now ({}). Quit one of them, then start {label} again",
        each.join("; ")
    )
}

// ---- Suspended sessions --------------------------------------------------

/// The shim line when it is suspended.
pub fn suspended_line(label: &str, arg: &str) -> String {
    format!("{label} is suspended: its state is saved and its VM slot is free. Click its Dock icon, or run paddock app run {arg}, to resume it")
}

/// `paddock app list`: the state of a suspended session.
pub fn list_suspended(pid: u32) -> String {
    format!("suspended (pid {pid}, state saved; a click on its Dock icon resumes it)")
}

/// The Dock menu entry of a suspended session.
pub fn dock_resume(app: &str) -> String {
    format!("Resume {app} (suspended)")
}

/// The resume reason of a click on the Dock icon.
pub fn resume_reason_dock() -> String {
    "a click on its Dock icon".into()
}

/// The resume reason of `paddock app run`.
pub fn resume_reason_run() -> String {
    "paddock app run".into()
}

/// The event of a suspend, with its reason.
pub fn suspend_event(reason: &str) -> String {
    format!("suspend requested: {reason}")
}

/// The event and line of a resume, with its reason.
pub fn resume_event(reason: &str) -> String {
    format!("resumed from the saved state after {reason}")
}

/// The suspended shim did not end before the resume.
pub fn handover_timeout(label: &str, arg: &str, pid: u32) -> String {
    format!("the suspended session of {label} (pid {pid}) did not end in time. Stop it with paddock app stop {arg}, then start it again")
}

/// The resume of a Dock click could not start `paddock app run`.
pub fn resume_spawn_failed(e: &str) -> String {
    format!("cannot start the resume ({e}). Run paddock app run in a terminal")
}

#[cfg(test)]
#[path = "suspend_tests.rs"]
mod tests;
