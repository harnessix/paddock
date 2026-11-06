//! The texts of an app session: the shim `paddock-vm app`, the parent
//! `paddock app run --backend macvm`, the locks, the quit path and the
//! lines of the start decision. The refusals, the start lines and the
//! badges of a session are in text.rs.

use super::{gib, shown};
use std::path::Path;
use std::time::Duration;

// ---- Arguments ----------------------------------------------------------

pub fn display_arg_invalid(text: &str) -> String {
    format!("{text:?} is not WxH@S, for example 2560x1600@2")
}

pub fn display_arg_range(text: &str) -> String {
    format!("{text}: the width and height are 640 to 16384 pixels, the scale 1 to 4")
}

pub fn bad_app_name(name: &str) -> String {
    format!("the app name {name:?} must be letters, digits, dots, dashes or underscores")
}

pub fn macvm_macos_only() -> String {
    "the macOS guest of app mode (--backend macvm) runs only on macOS".into()
}

// ---- The start decision -------------------------------------------------

pub fn start_fresh() -> String {
    "new profile: paddock clones the golden and cold boots".into()
}

pub fn golden_changed(old: &str, new: &str) -> String {
    format!("the golden changed from {old} to {new}: paddock backs your data up, clones again and cold boots")
}

pub fn protocol_changed(old: u32, new: u32) -> String {
    format!("the guest protocol changed from {old} to {new}: cold boot")
}

pub fn display_differs(old: &str, new: &str) -> String {
    format!("the display changed from {old} to {new}: cold boot")
}

pub fn state_damaged() -> String {
    "the saved state does not match the data disk: it is discarded, cold boot".into()
}

pub fn stamp_foreign() -> String {
    "the profile stamp is from another paddock: the saved state is discarded, cold boot".into()
}

pub fn warm_start() -> String {
    "warm start from the saved state".into()
}

pub fn cold_boot() -> String {
    "cold boot".into()
}

// ---- The profile files --------------------------------------------------

pub fn cannot_clone(from: &Path, to: &Path) -> String {
    format!("cannot clone {} to {}", shown(from), shown(to))
}

pub fn machine_id_size(len: usize) -> String {
    format!("a machine identifier of {len} bytes")
}

pub fn stamp_unreadable(path: &Path, error: &str) -> String {
    format!(
        "cannot read the stamp {} ({error}) after prepare",
        shown(path)
    )
}

pub fn cannot_remove_part(what: &str, dir: &Path) -> String {
    format!("cannot remove {what} of {}", shown(dir))
}

pub fn cannot_prepare(dir: &Path) -> String {
    format!("cannot prepare the profile {}", shown(dir))
}

// ---- The locks and slots ------------------------------------------------

pub fn slot_unreadable(index: usize, error: &str) -> String {
    format!("slot {index} (cannot open: {error})")
}

pub fn slot_io(error: &str) -> String {
    format!("cannot take a VM slot: {error}")
}

pub fn not_a_folder(path: &Path) -> String {
    format!("{} exists and is not a folder", shown(path))
}

pub fn cannot_lock(path: &Path) -> String {
    format!("cannot lock {}", shown(path))
}

// ---- The quit path ------------------------------------------------------

pub fn quit_asking(app: &str) -> String {
    format!("asking {app} to quit")
}

pub fn quit_app_not_running(app: &str) -> String {
    format!("{app} does not run in the guest: nothing to quit, the quit goes on")
}

pub fn quit_forcing(app: &str, wait: Duration) -> String {
    format!("{app} did not quit in {} s, forcing", wait.as_secs())
}

pub fn quit_force_failed(app: &str, total: Duration) -> String {
    format!(
        "{app} did not quit in {} s: the VM stops without a save",
        total.as_secs()
    )
}

pub fn pause_failed(error: &str) -> String {
    format!("the VM did not pause ({error}): the next start is a cold boot")
}

pub fn save_failed(error: &str) -> String {
    format!("the save failed ({error}): the next start is a cold boot")
}

pub fn resume_failed(error: &str) -> String {
    format!("resume failed: {error}")
}

/// The agent answered `shutdown` with an error.
pub fn shutdown_refused(why: &str) -> String {
    format!("the guest did not shut down ({why}), stopping the VM")
}

/// The guest did not stop within the wait after `shutdown`.
pub fn shutdown_timeout(wait: Duration) -> String {
    format!(
        "the guest did not stop in {} s, stopping the VM",
        wait.as_secs()
    )
}

pub fn stop_failed(error: &str) -> String {
    format!("the stop failed: {error}")
}

pub fn quit_begin(why: &str) -> String {
    format!("quit ({why})")
}

pub fn second_signal() -> String {
    "second signal: hard stop".into()
}

pub fn state_commit_failed(error: &str) -> String {
    format!("cannot commit the saved state: {error}")
}

pub fn stamp_write_failed(error: &str) -> String {
    format!("cannot write the stamp: {error}")
}

// ---- The shim -----------------------------------------------------------

/// A line of the helper `paddock-vm` in the shim log.
pub fn helper_line(text: &str) -> String {
    format!("paddock-vm: {text}")
}

pub fn restore_unsupported(error: &str) -> String {
    format!("the saved state cannot be restored here ({error}): cold boot")
}

pub fn restore_failed(error: &str) -> String {
    format!("the saved state did not restore ({error}): cold boot")
}

pub fn vm_running(kind: &str, since: Duration) -> String {
    format!(
        "the VM runs ({kind}), {} ms after the start",
        since.as_millis()
    )
}

pub fn no_control_socket(error: &str) -> String {
    format!("no control socket: {error}")
}

/// The framework refused the start. With both slots free, a VM of
/// another program holds the limit.
pub fn vm_start_failed(error: &str, other_program: bool) -> String {
    if other_program {
        format!("cannot start the VM: {error}. Both paddock slots were free, so a VM of another program (Tart, UTM) runs: stop one, then try again")
    } else {
        format!("cannot start the VM: {error}")
    }
}

pub fn guest_stopped_after(since: Duration) -> String {
    format!(
        "the guest stopped, {} ms after the start",
        since.as_millis()
    )
}

pub fn vm_failed(error: &str) -> String {
    format!("the VM stopped with an error: {error}")
}

pub fn agent_dialed(agent: &str, since: Duration) -> String {
    format!(
        "the guest agent {agent} dialed in after {:.1} s",
        since.as_secs_f64()
    )
}

pub fn agent_updating(from: &str, to: &str) -> String {
    format!("the guest agent {from} updates itself to {to}")
}

pub fn agent_no_update_file(path: &Path) -> String {
    format!(
        "the guest agent is another version and {} has no hash file next to it, so it stays",
        shown(path)
    )
}

pub fn data_prepare_failed(error: &str) -> String {
    format!("the data disk was not prepared: {error}")
}

pub fn request_failed(kind: &str, error: &str) -> String {
    format!("the guest answered an error to {kind}: {error}")
}

pub fn request_timeout(kind: &str, wait: Duration) -> String {
    format!(
        "the guest did not answer {kind} within {} s",
        wait.as_secs()
    )
}

pub fn hello_restore_cold() -> String {
    "the guest agent did not dial in after the restore: cold boot".into()
}

pub fn hello_timeout(wait: Duration, console_log: &Path) -> String {
    format!(
        "the guest agent did not dial in within {} s. The console log is {}",
        wait.as_secs(),
        shown(console_log)
    )
}

pub fn no_socket_device() -> String {
    "the VM has no socket device".into()
}

pub fn no_connection() -> String {
    "no connection".into()
}

pub fn vm_cannot_stop() -> String {
    "the VM is in a state that cannot stop".into()
}

pub fn guest_port_failed(port: u32, error: &str) -> String {
    format!("cannot connect to guest port {port}: {error}")
}

pub fn guest_port_timeout(port: u32) -> String {
    format!("guest port {port} did not answer")
}

pub fn token_unreadable() -> String {
    "cannot read the token from the parent".into()
}

pub fn token_invalid() -> String {
    "the parent passed no valid session token".into()
}

pub fn token_pipe_failed(what: &str) -> String {
    format!("cannot {what} the token pipe")
}

pub fn signal_pipe_failed() -> String {
    "cannot make the signal pipe".into()
}

// ---- The parent ---------------------------------------------------------

pub fn stop_timeout(name: &str, pid: u32, wait: Duration) -> String {
    format!(
        "the session of {name} (pid {pid}) did not stop in {} s",
        wait.as_secs()
    )
}

/// One line of `paddock app list` for a profile of the macOS guest.
pub fn list_line(name: &str, state: &str) -> String {
    format!("{name:<16} macOS guest, {state}")
}

pub fn list_running(pid: u32, up: &str) -> String {
    format!("running (pid {pid}, {up})")
}

pub fn list_stopped(end: &str) -> String {
    format!("stopped ({end})")
}

// ---- The VM configuration -----------------------------------------------

pub fn hardware_model_invalid() -> String {
    "the hardware model of the golden is not valid".into()
}

pub fn hardware_model_unsupported_here() -> String {
    "the hardware model of the golden is not supported on this Mac: run `paddock macvm setup` again"
        .into()
}

pub fn machine_id_damaged(path: &Path) -> String {
    format!(
        "the machine identifier {} is damaged: `paddock app reset` makes a new one",
        shown(path)
    )
}

pub fn disk_serial_invalid(serial: &str, error: &str) -> String {
    format!("the disk serial {serial:?}: {error}")
}

pub fn share_name_invalid(name: &str, error: &str) -> String {
    format!("the share name {name:?}: {error}")
}

pub fn cpus_out_of_range(cpus: usize, min: usize, max: usize) -> String {
    format!("--cpus {cpus} is outside {min} to {max} on this Mac")
}

pub fn memory_out_of_range(mib: u64, min_mib: u64, max_mib: u64) -> String {
    format!("--memory {mib} MiB is outside {min_mib} to {max_mib} MiB on this Mac")
}

pub fn backup_skipped_used(used: u64, free: u64) -> String {
    format!(
        "backup of the data disk skipped: {} used, {} free",
        gib(used),
        gib(free)
    )
}

/// The badge while the host waits for the first `pong` after a wake.
pub fn waking() -> String {
    "waking".into()
}

/// The badge after a display change of the host.
pub fn display_changed() -> String {
    "display changed, restart for full size".into()
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

/// The agent pressed Ignore on a disk dialog of the guest.
pub fn guest_dialog_dismissed(owner: &str, detail: &str) -> String {
    format!("a disk dialog of {owner} came up in the guest, and the agent dismissed it: {detail}")
}

/// A disk dialog of the guest that the agent could not dismiss.
pub fn guest_dialog_open(owner: &str, detail: &str) -> String {
    format!(
        "a disk dialog of {owner} is open in the guest, and the agent could not dismiss it: {detail}. It shows as a window of the app `system`: choose Ignore there, never Eject"
    )
}
