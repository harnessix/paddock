//! The texts of the guest agent `paddock-guest` (src/guest) and of the
//! protocol: the detail text of an error reply, which the host logs next
//! to its own line, and the one refusal of the hello.

use super::shown;
use std::path::Path;
use std::time::Duration;

/// The host refuses an agent of another major version that cannot
/// update itself.
pub fn agent_cannot_update(agent: &str, agent_proto: &str, host: &str, host_proto: &str) -> String {
    format!(
        "the guest agent is version {agent} (protocol {agent_proto}) and this paddock is {host} (protocol {host_proto}), and the guest cannot update itself: rebuild the guest with `paddock macvm update`"
    )
}

// ---- Apps ---------------------------------------------------------------

pub fn not_app_bundle(path: &Path) -> String {
    format!("{} is not an app bundle in the guest", shown(path))
}

pub fn launch_failed(path: &Path, reason: &str) -> String {
    format!("cannot launch {}: {reason}", shown(path))
}

pub fn launch_timeout(path: &Path, wait: Duration) -> String {
    format!(
        "no answer from Launch Services for {} within {} s",
        shown(path),
        wait.as_secs()
    )
}

pub fn no_running_app(pid: u32) -> String {
    format!("no running app with pid {pid}")
}

pub fn quit_not_taken(pid: u32) -> String {
    format!("the app {pid} did not take the quit request")
}

pub fn app_not_running(app: &str) -> String {
    format!("{app} does not run")
}

// ---- Windows and Accessibility ------------------------------------------

pub fn no_window(id: u32) -> String {
    format!("no window {id}")
}

pub fn window_no_ax_element(id: u32) -> String {
    format!("window {id} has no Accessibility element")
}

/// The app did not answer an Accessibility request in time.
pub fn ax_timeout(op: &str, wait: Duration) -> String {
    format!(
        "the app did not answer the {op} within {} s",
        wait.as_secs()
    )
}

pub fn ax_failed(op: &str, code: i32) -> String {
    format!("{op} failed: AXError {code}")
}

/// An `ax.press` or `ax.set` names a path the window has no element at.
pub fn ax_element_gone(path: &[usize]) -> String {
    format!("no element at {path:?} in the window now")
}

/// The element at the path is another role than the host saw: nothing
/// was pressed.
pub fn ax_element_changed(path: &[usize], expected: &str, now: &str) -> String {
    format!("the element at {path:?} is a {now} now, not a {expected}")
}

/// An `ax.set` with text for an element that holds a number.
pub fn ax_value_not_a_number(value: &str) -> String {
    format!("the element holds a number, not {value:?}")
}

// ---- Commands in the guest ----------------------------------------------

/// A command did not start.
pub fn cannot_run_command(command: &str, error: &str) -> String {
    format!("cannot run {command}: {error}")
}

/// A command ran and failed, with its output.
pub fn command_failed(command: &str, output: &str) -> String {
    format!("{command} failed: {output}")
}

pub fn verify_no_run(error: &str) -> String {
    format!("cannot run the verify: {error}")
}

pub fn step_timed_out(output: &str, wait: Duration) -> String {
    format!("{output}\ntimed out after {} s", wait.as_secs())
}

/// A file or socket operation failed, with the step that failed.
pub fn io_failed(what: &str, error: &str) -> String {
    format!("{what}: {error}")
}

pub fn cannot_write_named(name: &str, error: &str) -> String {
    format!("cannot write {name}: {error}")
}

pub fn cannot_read_named(name: &str, error: &str) -> String {
    format!("cannot read {name}: {error}")
}

/// The step of `data.prepare` that runs now, for the host.
pub fn data_step_now(step: &str) -> String {
    format!("data disk step: {step}")
}

// ---- The data disk ------------------------------------------------------

pub fn no_data_disk(serial: &str) -> String {
    format!("no disk with the serial {serial} in the guest")
}

/// The result of the data volume check after an unclean session.
pub fn verify_result(verify_ok: bool, repair: Option<Result<&str, &str>>, last: &str) -> String {
    match (verify_ok, repair) {
        (true, _) => format!("verify ok: {last}"),
        (false, None) => format!("verify failed, cannot run the repair: {last}"),
        (false, Some(Ok(r))) => format!("verify failed, repair ok: {r}"),
        (false, Some(Err(r))) => format!("verify failed, repair failed: {r}"),
    }
}

pub fn data_volume_no_mount(last: &str) -> String {
    format!("the data volume did not mount: {last}")
}

/// The agent could not press Ignore on a disk dialog.
pub fn dialog_press_failed(code: i32) -> String {
    format!("the press of Ignore failed: AXError {code}")
}

// ---- Input, update, transport -------------------------------------------

pub fn input_source_select_failed(source: &str, status: i32) -> String {
    format!("cannot select the input source {source}: OSStatus {status}")
}

/// The guest cannot select the host's source and selects a plain layout
/// instead: one info line, not an error.
pub fn input_source_fallback(source: &str, layout: &str, status: Option<i32>) -> String {
    let why = match status {
        None => "the guest does not have it".to_string(),
        Some(r) => format!("OSStatus {r}"),
    };
    format!("the input source {source} is not available in the guest ({why}): using {layout}")
}

/// `paddock app list` found a dead session that had switched the host's
/// input source, and put it back (item 21 of the checklist).
pub fn input_source_restored(name: &str) -> String {
    format!("input source {name} restored: a session ended without putting it back")
}

pub fn update_hash_mismatch(got: &str, want: &str) -> String {
    format!("the copied agent has hash {got}, the host named {want}")
}

pub fn host_write_failed(error: &str) -> String {
    format!("cannot write to the host: {error}")
}

pub fn socket_clone_failed(error: &str) -> String {
    format!("cannot clone the socket: {error}")
}

/// The host sent another request before the token.
pub fn no_token_first(kind: &str) -> String {
    format!("no token: the host sent {kind} first")
}

pub fn no_token_timeout(wait: Duration) -> String {
    format!("no token within {} s of the hello", wait.as_secs())
}

pub fn vsock_no_answer(port: u32, wait: Duration) -> String {
    format!(
        "no answer from the host on vsock port {port} within {} s",
        wait.as_secs()
    )
}

/// The clock command failed without exposing its output.
pub fn clock_set_failed(code: Option<i32>) -> String {
    format!("clock.set failed (exit {code:?})")
}

#[cfg(test)]
#[path = "guest_tests.rs"]
mod tests;
