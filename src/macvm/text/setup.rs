//! The texts of `paddock macvm setup`, `status` and `remove` that
//! text.rs does not hold: the state lines of the setup driver, the
//! manifest errors, the status lines and the lines of the framework side.
//! The question of the download is in text.rs, the texts of the guided
//! window in panel.rs.

use super::{gb, gib, left_text, shown};
use crate::macvm::golden::State;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---- The setup driver ---------------------------------------------------

/// The line when a state starts.
pub fn state_starts(state: State) -> String {
    let what = match state {
        State::Fetch => "the restore image",
        State::Install => "the install of macOS (10 to 20 minutes)",
        State::Seed => "the seed folder for the guest",
        State::Guided => "the first boot in the paddock window",
        State::Trust => "the Accessibility permission of the guest agent",
        State::Configure => "the configuration of the guest (2 minutes)",
        State::Seal => "the seal",
        State::Sealed => "done",
    };
    format!(
        "step {} of {}, {}: {what}",
        state.number(),
        State::STEPS,
        state.name()
    )
}

/// The line when a setup continues an earlier one.
pub fn setup_continues(state: State) -> String {
    format!(
        "an earlier setup stopped in step {} ({}). paddock continues there.",
        state.number(),
        state.name()
    )
}

/// The line when a setup finds the partial files of a crashed one.
pub fn setup_crashed(state: State, partials: &[PathBuf]) -> String {
    let names: Vec<String> = partials
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    format!(
        "an earlier setup crashed in step {} ({}) and left {}. paddock removed them and runs the step again.",
        state.number(),
        state.name(),
        names.join(", ")
    )
}

/// The line when the user stops in a state that a later run continues.
pub fn setup_stopped(state: State) -> String {
    format!(
        "the setup stopped in step {} ({}). Run `paddock macvm setup` again to continue there.",
        state.number(),
        state.name()
    )
}

/// The line when a state fails.
pub fn state_failed(state: State, reason: &str) -> String {
    format!(
        "step {} ({}) failed: {}. Run `paddock macvm setup` again to retry it.",
        state.number(),
        state.name(),
        reason.trim_end_matches('.')
    )
}

/// The refusal when the volume has too little room for a setup step.
pub fn setup_no_space(needed: u64, free: u64, what: &str, volume: &Path) -> String {
    format!(
        "not enough space on {}: {what} needs {}, {} is free. Make room, then run `paddock macvm setup` again.",
        shown(volume),
        gb(needed),
        gb(free)
    )
}

/// The refusal when the paddock keyring cannot keep the guest password.
pub fn keyring_unavailable(dir: &Path, reason: &str) -> String {
    format!(
        "the paddock keyring at {} is not available ({reason}). paddock keeps the password of the guest account there, so the setup did not start.",
        shown(dir)
    )
}

/// The refusal when `--ipsw PATH` names no file.
pub fn ipsw_missing(path: &Path) -> String {
    format!(
        "--ipsw {} is not a file. Name a macOS restore image (.ipsw), or leave --ipsw out to download the newest one.",
        shown(path)
    )
}

/// The line when a sealed golden is already there.
pub fn golden_already(version: &str) -> String {
    format!(
        "the golden is sealed already (macOS {version}). `paddock macvm update` builds a new one, `paddock macvm remove` removes it."
    )
}

/// A restore image of this version is already in the `ipsw` folder.
pub fn image_already_there(path: &Path) -> String {
    format!("the restore image {} is already there", shown(path))
}

/// The first download came up short: paddock tries once more.
pub fn download_short(got: u64, want: u64) -> String {
    format!("the download has {got} bytes, the server said {want}: paddock downloads it again")
}

/// The second download came up short too.
pub fn download_short_twice(got: u64, want: u64) -> String {
    format!("the download has {got} bytes twice, the server said {want}")
}

/// The downloaded image failed `VZMacOSRestoreImage.load`.
pub fn image_check_failed(path: &Path, reason: &str) -> String {
    format!(
        "the restore image {} failed its check: {reason}",
        shown(path)
    )
}

/// The manifest names no restore image for the install.
pub fn image_not_named() -> String {
    "the manifest names no restore image".into()
}

/// The restore image of the manifest is gone.
pub fn image_gone(path: &Path) -> String {
    format!("the restore image {} is gone", shown(path))
}

/// The restore image is not the size the download said.
pub fn image_size_differs(path: &Path, got: u64, want: u64) -> String {
    format!(
        "the restore image {} has {got} bytes, the download said {want}",
        shown(path)
    )
}

/// The installer ended without one of its four files.
pub fn installer_left_no(path: &Path) -> String {
    format!("the installer left no {}", shown(path))
}

/// The install finished.
pub fn installed(version: &str, disk: &Path, used: u64) -> String {
    format!(
        "installed macOS {version} into {} ({} used)",
        shown(disk),
        gib(used)
    )
}

/// One image did not take the seal.
pub fn seal_failed(path: &Path, reason: &str) -> String {
    format!("cannot seal {}: {reason}", shown(path))
}

/// The question without a terminal: the command with `--yes`.
pub fn ask_without_terminal(question: &str) -> String {
    format!("{question}\nRun `paddock macvm setup --yes` to go on.")
}

/// The prompt of a question on the terminal.
pub fn ask_prompt(question: &str) -> String {
    format!("{question} [y/N] ")
}

/// Another setup holds the lock.
pub fn setup_lock_held(lock: &Path) -> String {
    format!(
        "another `paddock macvm setup` runs (lock {}). Let it finish, or stop it, then run this one again.",
        shown(lock)
    )
}

/// Steps of the configure request failed or did not prove themselves.
pub fn configure_failed(names: &[String]) -> String {
    format!(
        "these configure steps failed in the guest: {}. The guest log has the output. Run `paddock macvm setup` again to retry the step.",
        names.join(", ")
    )
}

/// The agent answered the configure request without results.
pub fn configure_no_result() -> String {
    "the guest agent answered the configure request without step results".into()
}

/// The agent binary is not next to paddock-vm and not in the layout.
pub fn agent_missing(what: &str) -> String {
    format!(
        "{what}: no paddock-guest binary next to paddock-vm and none in macvm/agent/. Build it with packaging/macos/build-guest.sh, then run `paddock macvm setup` again."
    )
}

// ---- The manifest and the layout ----------------------------------------

/// The manifest is not TOML.
pub fn manifest_not_toml(path: &Path) -> String {
    format!("{} is not valid TOML", shown(path))
}

/// The manifest has another schema. `found` is `None` when it has none.
pub fn manifest_schema(path: &Path, found: Option<i64>, known: u32) -> String {
    format!(
        "{} has manifest schema {}, this paddock knows schema {known}: remove the golden with `paddock macvm remove` and run `paddock macvm setup` again",
        shown(path),
        found.map_or("none".to_string(), |s| s.to_string())
    )
}

/// The manifest names a state that this paddock does not know.
pub fn manifest_unknown_state(path: &Path, state: &str) -> String {
    format!(
        "{} names the unknown state {state:?}: remove the golden with `paddock macvm remove` and run `paddock macvm setup` again",
        shown(path)
    )
}

/// The manifest is TOML but not a golden manifest.
pub fn manifest_invalid(path: &Path) -> String {
    format!("{} is not a golden manifest", shown(path))
}

pub fn manifest_unserializable() -> String {
    "cannot serialize the manifest".into()
}

/// `PADDOCK_MACVM_FAKE_FREE` is not a size.
pub fn fake_free_invalid(text: &str) -> String {
    format!("PADDOCK_MACVM_FAKE_FREE={text:?} is not a size such as 1G")
}

/// `promote_new` without a `golden.new`.
pub fn no_new_golden(dir: &Path) -> String {
    format!("there is no new golden in {}", shown(dir))
}

/// `rollback` without a `golden.prev`.
pub fn no_prev_golden(dir: &Path) -> String {
    format!("there is no previous golden in {}", shown(dir))
}

/// The golden could not step aside for the previous one.
pub fn cannot_move_aside(dir: &Path) -> String {
    format!("cannot move {} aside", shown(dir))
}

// ---- Status and remove --------------------------------------------------

pub fn status_head(root: &Path) -> String {
    format!("macOS guest in {}:", shown(root))
}

pub fn status_golden_none() -> String {
    "  golden   none. Run `paddock macvm setup` to build it.".into()
}

pub fn status_golden_unreadable(error: &str) -> String {
    format!("  golden   {error}")
}

pub fn status_golden_unsealed(version: &str, state: State) -> String {
    format!(
        "  golden   macOS {version}, setup stopped in step {} ({}). Run `paddock macvm setup` to continue.",
        state.number(),
        state.name()
    )
}

pub fn status_golden_sealed(version: &str, sealed: &str, size: u64, flags_ok: bool) -> String {
    format!(
        "  golden   macOS {version}, sealed {sealed}, {}{}",
        gib(size),
        if flags_ok {
            ""
        } else {
            ", the seal flags are missing"
        }
    )
}

pub fn status_golden_older(golden: &str, host: &str) -> String {
    format!("  note     the golden ({golden}) is older than the host (macOS {host}): paddock macvm update")
}

pub fn status_host(version: &str) -> String {
    format!("  host     macOS {version}")
}

pub fn status_profiles_none() -> String {
    "  profiles none".into()
}

pub fn status_profile(app: &str, home: &str, size: u64) -> String {
    format!("  profile  {app}/{home} {}", gib(size))
}

pub fn status_sessions_none() -> String {
    "  sessions none".into()
}

pub fn status_session(id: &str, pid: u32, profile: &str) -> String {
    format!("  session  {id} (pid {pid}) {profile}")
}

/// The refusal of `remove` while a session runs.
pub fn remove_while_running(pid: u32) -> String {
    format!("a session runs (pid {pid}): quit it first, then run `paddock macvm remove` again.")
}

/// The question before the golden goes.
pub fn remove_golden_question(version: &str, size: u64) -> String {
    format!(
        "paddock macvm remove deletes the golden (macOS {version}, {}). The profiles stay unless you add --profiles.",
        gib(size)
    )
}

/// The question before the profiles go.
pub fn remove_profiles_question(count: usize, size: u64) -> String {
    format!(
        "--profiles also deletes {count} profile(s), {} with the logins and the files of their apps. paddock copies each data disk into the trash folder first, for 7 days.",
        gib(size)
    )
}

// ---- The framework side -------------------------------------------------

pub fn needs_apple_silicon() -> String {
    "the macOS guest needs a Mac with Apple silicon".into()
}

pub fn no_virtualization() -> String {
    "this Mac cannot run virtual machines".into()
}

pub fn main_thread_only(what: &str) -> String {
    format!("{what} must run on the main thread")
}

/// The parent `paddock macvm setup` is gone.
pub fn parent_ended() -> String {
    "paddock-vm: paddock macvm setup ended, stopping".into()
}

pub fn restore_image_no_url() -> String {
    "the restore image has no URL".into()
}

pub fn image_unsupported(version: &str, build: &str) -> String {
    format!("this Mac cannot run macOS {version} ({build}): the restore image has no supported configuration here")
}

pub fn hardware_model_unsupported(version: &str) -> String {
    format!("the hardware model of macOS {version} is not supported on this Mac")
}

pub fn image_lost_configuration() -> String {
    "the restore image lost its configuration".into()
}

/// The guest shut down on its own during the guided step.
pub fn guest_shut_down_in_guided() -> String {
    "paddock-vm: the guest shut down during the guided step".into()
}

pub fn guest_stopped_with_error(error: &str) -> String {
    format!("the guest stopped with an error: {error}")
}

pub fn guided_window_gone() -> String {
    "the guided window is gone".into()
}

/// The guest did not shut down for the seal.
pub fn guest_no_shutdown(wait: Duration) -> String {
    format!(
        "the guest did not shut down within {}: shut it down from its Apple menu, then run `paddock macvm setup` again",
        left_text(wait)
    )
}

pub fn main_thread_no_boot() -> String {
    "the main thread did not boot the guest".into()
}

pub fn golden_start_failed(error: &str) -> String {
    format!("cannot start the golden: {error}")
}

/// The golden stopped at the start, with the framework's error when
/// there is one.
pub fn golden_stopped_at_start(error: Option<&str>) -> String {
    match error {
        Some(e) => format!("the golden stopped at the start: {e}"),
        None => "the golden stopped at the start".into(),
    }
}

pub fn golden_start_timeout(wait: Duration) -> String {
    format!("the golden did not start within {}", left_text(wait))
}

pub fn stop_request_failed(error: &str) -> String {
    format!("paddock-vm: the stop request failed: {error}")
}

/// A file did not open for the framework, with its error.
pub fn cannot_open_for_vm(path: &Path, error: &str) -> String {
    format!("cannot open {}: {error}", shown(path))
}

/// A file of the golden could not be made, with the framework's error.
pub fn cannot_make_for_vm(path: &Path, error: &str) -> String {
    format!("cannot make {}: {error}", shown(path))
}

pub fn stub_network_failed(what: &str) -> String {
    format!("cannot {what} the stub network")
}

pub fn configuration_invalid(error: &str) -> String {
    format!("the configuration of the guest is not valid: {error}")
}

pub fn installer_no_start() -> String {
    "the installer did not start".into()
}

pub fn install_failed(error: &str) -> String {
    format!("the install of macOS failed: {error}")
}

pub fn installer_no_result() -> String {
    "the installer ended without a result".into()
}

pub fn fetch_failed(error: &str) -> String {
    format!("cannot find the newest macOS for this Mac: {error}")
}

pub fn fetch_timeout() -> String {
    "Apple did not answer the restore image request in time".into()
}

pub fn image_unusable(path: &Path, error: &str) -> String {
    format!("{} is not a usable restore image: {error}", shown(path))
}

pub fn image_load_timeout(path: &Path) -> String {
    format!("the load of {} did not end in time", shown(path))
}

pub fn downloading(url: &str) -> String {
    format!("downloading {url}")
}

pub fn download_failed(status: &str, error: &str) -> String {
    format!("the download failed ({status}): {error}")
}

/// The seed folder is ready.
pub fn seed_prepared(dir: &Path, with_agent: bool) -> String {
    let agent = if with_agent {
        "the agent, its launch script and its launchd file"
    } else {
        "the launch script and the launchd file, no agent"
    };
    format!(
        "the seed folder {} holds {agent}. The next step shares it read-only with the guest.",
        shown(dir)
    )
}

/// The terminal line of the guided step: the command that the panel
/// shows, for a person who reads the terminal.
pub fn seed_install_line(command: &str) -> String {
    format!("after the account is made, run this in Terminal of the guest. The paddock window types it for you (Type the command), and sudo asks for the password that the window shows: {command}")
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;

/// The auxiliary storage of the golden is still locked by the VM that
/// just ended.
pub fn aux_busy_wait() -> String {
    "the VM of the last step still holds the golden; paddock waits for it (at most a minute)".into()
}
