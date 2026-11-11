//! The texts of the data disk at the start of a session (`data_start.rs`),
//! of its steps in the guest (`guest/data.rs`), of the guest password that
//! a session reads from the keyring, and of the serial console lines of
//! the agent (`guest/console.rs`).

use super::shown;
use std::path::Path;
use std::time::Duration;

/// The host sends `data.prepare`.
pub fn data_preparing() -> String {
    "preparing the data disk: the home of the guest moves onto it".into()
}

/// The answer of `data.prepare` was Ok. The Mac check greps for the
/// words `the data disk is prepared`.
pub fn data_prepared(verify: &str) -> String {
    if verify.is_empty() {
        "the data disk is prepared".into()
    } else {
        format!("the data disk is prepared (verify: {verify})")
    }
}

/// No answer to `data.prepare` in its time.
pub fn data_prepare_timeout(wait: Duration) -> String {
    format!(
        "the data disk was not prepared: the guest agent did not answer data.prepare in {} s. The app starts with the home on the system disk, and the next start tries again.",
        wait.as_secs()
    )
}

/// A warm start of a profile whose data disk is not prepared.
pub fn data_prepare_warm() -> String {
    "the data disk is not prepared yet: this start restores a saved state, so the quit saves nothing and the next start prepares it".into()
}

/// The quit skips the save, so the next start boots cold and prepares.
pub fn data_no_save_unprepared() -> String {
    "no save: the data disk is not prepared yet, so the next start boots cold and prepares it"
        .into()
}

/// The agent restarted again and again while it prepared the disk.
pub fn data_agent_restarts(sends: u32) -> String {
    format!("the data disk was not prepared: the guest agent restarted {sends} times during data.prepare. The app starts with the home on the system disk.")
}

/// No guest password reached the session.
pub fn data_no_password() -> String {
    "the data disk was not prepared: the session has no guest password (the keyring of `paddock macvm setup` has none). The home stays on the system disk, and the quit stops the VM without a shutdown.".into()
}

/// The quit has no password for the shutdown of the guest.
pub fn shutdown_no_session_password() -> String {
    "the session has no guest password for the shutdown".into()
}

/// The start line of `paddock app run` when the keyring has no password.
pub fn session_password_missing(file: &Path) -> String {
    format!(
        "warning: the keyring has no guest password ({}). The data disk is not prepared and the VM stops without a shutdown at the quit. `paddock macvm setup` asks for the password of the guest account.",
        shown(file)
    )
}

/// The keyring file holds no valid password.
pub fn keyring_password_damaged(file: &Path) -> String {
    format!(
        "the guest password in {} is damaged: the session has no guest password",
        shown(file)
    )
}

/// The keyring file cannot be read.
pub fn keyring_password_unreadable(file: &Path, error: &str) -> String {
    format!("cannot read the guest password in {}: {error}", shown(file))
}

/// The agent verified the data volume after an unclean end.
pub fn data_verified(result: &str) -> String {
    format!("the data volume was verified after an unclean end: {result}")
}

/// One step of `data.prepare` in the guest failed: the step and the
/// detail, so the events and the status name the step.
pub fn data_step_failed(step: &str, detail: &str) -> String {
    format!("{step}: {detail}")
}

/// The `data` row of `paddock app status`.
pub fn status_data_row(prepared: bool, error: Option<&str>) -> String {
    match (prepared, error) {
        (true, _) => "prepared: the home of the guest is on data.img".into(),
        (false, Some(e)) => format!("not prepared: {e}"),
        (false, None) => "not prepared yet: the next cold start prepares it".into(),
    }
}

/// The first line of the agent on the serial console.
pub fn console_start(version: &str, protocol: &str, pid: u32) -> String {
    format!("paddock-guest {version} starts, protocol {protocol}, pid {pid}")
}

/// The agent found no serial console device in /dev.
pub fn console_no_device(seen: &str) -> String {
    format!("no virtio serial console in /dev (seen: {seen})")
}

/// The agent could not write to the serial console.
pub fn console_write_failed(device: &str, error: &str) -> String {
    format!("cannot write to the serial console {device}: {error}")
}

/// The quit came while `data.prepare` ran in the guest: the reply of the
/// guest.
pub fn data_prepare_cancelled() -> String {
    "the quit came first: the copy of the home stopped, and the next cold start prepares the data disk again".into()
}

/// A request that must wait for the end of `data.prepare`.
pub fn data_prepare_busy() -> String {
    "the guest prepares the data disk now: try again when it is done".into()
}

/// The host saw the quit while `data.prepare` waited.
pub fn data_prepare_stopped_by_quit() -> String {
    "the quit came while the guest prepared the data disk: the guest stops the copy of the home, nothing is saved, and the next cold start prepares the disk again".into()
}

/// The copy of the home ran out of its time.
pub fn data_copy_timed_out(limit: Duration) -> String {
    format!(
        "the copy of the home did not end in {} s (60 s plus 1 s per 50 MB of the home)",
        limit.as_secs()
    )
}

/// One part of the home could not be copied.
pub fn data_copy_failed(part: &str, why: &str) -> String {
    format!("cannot copy {part}: {why}")
}

/// The progress of the copy of the home, for the host.
pub fn data_copy_progress(done: usize, parts: usize, bytes: u64, total: u64) -> String {
    format!(
        "copying the home: {done} of {parts} parts, {} of {}",
        super::gib(bytes),
        super::gib(total)
    )
}

/// The size of the home and the time that its copy gets.
pub fn data_copy_measured(bytes: Option<u64>, limit: Duration) -> String {
    match bytes {
        Some(b) => format!(
            "the home holds {} outside the guarded folders: the copy may take {}",
            super::gib(b),
            super::left_text(limit)
        ),
        None => format!(
            "du could not measure the home: the copy may take {}",
            super::left_text(limit)
        ),
    }
}

/// The end of the copy: the parts, the files that could not be read and
/// the guarded folders left empty.
pub fn data_copy_done(parts: usize, denied: usize, skipped: &[String]) -> String {
    let mut s = format!("copied the home: {parts} parts");
    if denied > 0 {
        s.push_str(&format!(", {denied} files could not be read (skipped)"));
    }
    if !skipped.is_empty() {
        s.push_str(&format!(
            ", left empty because macOS guards them: {}",
            skipped.join(", ")
        ));
    }
    s
}

/// The agent log did not come over the files port at the quit.
pub fn log_fetch_missing(secs: u64) -> String {
    format!("the agent log did not come in {secs} s: logs/agent.log is from an earlier session or missing")
}

#[cfg(test)]
#[path = "datadisk_tests.rs"]
mod tests;

/// The restart of the guest user's preferences daemon after the home moved
/// failed.
pub fn cfprefsd_restart_failed(reason: &str) -> String {
    format!("the preferences daemon of the guest user did not restart after the home moved, so apps may not save their settings until the next start: {reason}")
}
