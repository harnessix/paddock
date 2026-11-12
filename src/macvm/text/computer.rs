//! The texts of computer use: the refusals of the guest agent
//! (`guest/computer.rs`), the errors of the host connection
//! (`computer_client.rs`), the choice of the session and the hints of
//! `paddock mac` (`computer_host.rs`). No text holds typed text.

use std::path::Path;

// ---- The target ----------------------------------------------------------

/// App scope without a running app.
pub fn cu_no_app() -> String {
    "the app of this session does not run now. Start it again with its Dock icon, or use --desktop for the whole guest display".into()
}

/// App scope without a window on the screen.
pub fn cu_no_window() -> String {
    "the app has no window on the guest screen now: it may be minimized or still starting. Wait a moment, or use --desktop".into()
}

pub fn cu_outside_display(x: f64, y: f64, w: u32, h: u32) -> String {
    format!("the point {x:.0},{y:.0} is outside the guest display ({w}x{h} points)")
}

/// A window of another app covers the point of an app scope action.
pub fn cu_covered(x: f64, y: f64, owner: &str) -> String {
    let who = if owner.is_empty() {
        "another app"
    } else {
        owner
    };
    format!("a window of {who} covers the point {x:.0},{y:.0}: the action would not reach the app. Take a new screenshot, or use --desktop")
}

pub fn cu_outside_app(x: f64, y: f64) -> String {
    format!("the point {x:.0},{y:.0} is not on a window of the app. Take a new screenshot, or use --desktop")
}

pub fn cu_locked() -> String {
    "the guest screen is locked: paddock sends no input to a locked screen. paddock unlocks it when you use a window of the session".into()
}

pub fn cu_no_accessibility() -> String {
    "the guest agent has no Accessibility permission, so it cannot send input or read elements. Run `paddock macvm setup` again to build a golden image that has it".into()
}

pub fn cu_no_screen_recording() -> String {
    "the guest agent has no Screen Recording permission, so it cannot capture the screen. Run `paddock macvm setup` again to build a golden image that has it".into()
}

pub fn cu_not_front() -> String {
    "the app did not come to the front within 1.5 seconds, so the keys would go to another app. Click a window of the app first".into()
}

pub fn cu_element_id_invalid(id: &str) -> String {
    format!("{id:?} is not an element id: an id is WINDOW:PATH, as `paddock mac ax` prints it")
}

pub fn cu_element_window_gone(window: u32) -> String {
    format!("window {window} of the element is not on the screen now. Read the elements again with `paddock mac ax`")
}

/// The element at the path is gone.
pub fn cu_element_gone(id: &str) -> String {
    format!("the element {id} is gone. Read the elements again with `paddock mac ax`")
}

/// The element has another role now.
pub fn cu_element_role(id: &str, want: &str, now: &str) -> String {
    format!("the element {id} is a {now} now, not a {want}: nothing was pressed. Read the elements again")
}

pub fn cu_press_failed(id: &str, code: i32) -> String {
    format!("the element {id} did not take the press (Accessibility error {code})")
}

pub fn cu_ax_timeout() -> String {
    "the app did not answer Accessibility within its time: it may be busy. Try again in a moment"
        .into()
}

pub fn cu_ax_failed(code: i32) -> String {
    format!("the Accessibility read failed (error {code})")
}

// ---- The input checks ------------------------------------------------------

pub fn cu_key_empty() -> String {
    "no key: name one, for example return, cmd+s or shift+tab".into()
}

pub fn cu_key_unknown(key: &str) -> String {
    format!("{key:?} is not a key paddock knows. Keys: a-z, 0-9, punctuation, return, tab, space, delete, escape, forwarddelete, home, end, pageup, pagedown, left, right, up, down, f1-f15")
}

pub fn cu_mod_unknown(m: &str, combo: &str) -> String {
    format!("{m:?} in {combo:?} is not a modifier. Modifiers: cmd, shift, option (alt), ctrl, fn")
}

pub fn cu_click_count(count: u32, max: u32) -> String {
    format!("{count} clicks: a click has 1 to {max}")
}

pub fn cu_scroll_zero() -> String {
    "a scroll of 0 lines does nothing: give an amount".into()
}

pub fn cu_scroll_too_far(max: i32) -> String {
    format!("a scroll has at most {max} lines each way")
}

pub fn cu_scroll_direction(d: &str) -> String {
    format!("{d:?} is not a direction: use up, down, left or right")
}

pub fn cu_type_empty() -> String {
    "no text to type".into()
}

pub fn cu_type_too_long(max: usize) -> String {
    format!("the text is longer than {max} characters: type it in parts")
}

pub fn cu_keys_too_many(max: usize) -> String {
    format!("more than {max} keys in one call: send them in parts")
}

pub fn cu_point_invalid() -> String {
    "a coordinate is not a number".into()
}

pub fn cu_outside_image(x: f64, y: f64, w: u32, h: u32) -> String {
    format!("the pixel {x:.0},{y:.0} is outside the screenshot, which is {w}x{h}. Coordinates are pixels of the last screenshot of the same target")
}

pub fn cu_stale() -> String {
    "the windows of the target changed size since the last screenshot, so its pixels no longer match the screen. Take a new screenshot".into()
}

pub fn cu_max_size_invalid(t: &str) -> String {
    format!("{t:?} is not a size: use WIDTHxHEIGHT (1280x800), one number for both, agent, or full")
}

// ---- The wire --------------------------------------------------------------

pub fn cu_call_unknown(e: &str) -> String {
    format!("the guest agent does not know this call: {e}. `paddock macvm update` brings the agent of this paddock")
}

pub fn cu_line_too_long(max: usize) -> String {
    format!("a message is longer than {max} bytes")
}

pub fn cu_bad_message(e: &str) -> String {
    format!("the guest agent sent a message paddock cannot read: {e}")
}

pub fn cu_payload_too_big(len: u64, max: u64) -> String {
    format!("the guest agent announced {len} bytes, more than {max}")
}

pub fn cu_session_unreachable(control: &Path, e: &str) -> String {
    format!(
        "cannot reach the session at {}: {e}. `paddock app list` shows whether it runs",
        control.display()
    )
}

pub fn cu_port_refused(why: &str) -> String {
    format!("the session cannot reach the computer-use port of its guest: {why}. The guest agent may be older than this paddock: quit the app and start it again")
}

pub fn cu_closed() -> String {
    "the connection to the guest closed".into()
}

pub fn cu_no_hello() -> String {
    "the guest agent did not say hello on the computer-use port".into()
}

pub fn cu_version(got: u32, want: u32) -> String {
    format!("the guest agent speaks computer use version {got}, this paddock version {want}. Quit the app and start it again: the session updates the agent")
}

pub fn cu_send_failed(e: &str) -> String {
    format!("cannot send to the guest: {e}")
}

pub fn cu_read_failed(e: &str) -> String {
    format!("cannot read from the guest: {e}")
}

pub fn cu_no_answer() -> String {
    "the guest agent did not answer in time".into()
}

pub fn cu_wrong_reply(op: &str) -> String {
    format!("the guest agent answered {op} with something else")
}

pub fn cu_capture_failed(e: &str) -> String {
    format!("the capture of the guest screen failed: {e}")
}

pub fn cu_capture_unsupported() -> String {
    "this guest has no ScreenCaptureKit screenshot call (macOS 14 or later): build a newer golden image with `paddock macvm setup`".into()
}

pub fn cu_png_failed() -> String {
    "the capture could not be encoded as PNG".into()
}

pub fn cu_post_failed() -> String {
    "the guest did not take the input event".into()
}

pub fn cu_activate_failed(pid: u32) -> String {
    format!("cannot bring the app (pid {pid}) to the front")
}

// ---- The session -----------------------------------------------------------

pub fn cu_no_session() -> String {
    "no app session of the macOS guest runs. Start one with `paddock app run NAME --backend macvm`"
        .into()
}

pub fn cu_session_not_running(arg: &str, running: &[String]) -> String {
    if running.is_empty() {
        format!("{arg} does not run. Start it with `paddock app run {arg} --backend macvm`")
    } else {
        format!(
            "{arg} does not run. Running: {}",
            running.join(", ") // not user text
        )
    }
}

pub fn cu_sessions_many(running: &[String]) -> String {
    format!(
        "more than one session runs: {}. Name one with --app NAME",
        running.join(", ") // not user text
    )
}

pub fn cu_instance_needs_app() -> String {
    "--instance needs --app NAME".into()
}

pub fn cu_hint_screen_recording() -> String {
    "the guest agent has no Screen Recording permission: screenshots fail. Build a new golden image with `paddock macvm setup`".into()
}

pub fn cu_hint_accessibility() -> String {
    "the guest agent has no Accessibility permission: input and elements fail".into()
}

pub fn cu_hint_locked() -> String {
    "the guest screen is locked: input is refused until it unlocks".into()
}

// ---- The MCP server and the commands ---------------------------------------

pub fn cu_arg_missing(name: &str) -> String {
    format!("the argument {name} is missing")
}

pub fn cu_button_invalid(b: &str) -> String {
    format!("{b:?} is not a mouse button: use left, right or middle")
}

pub fn cu_tool_unknown(name: &str) -> String {
    format!("{name:?} is not a tool of paddock mac: tools/list names them")
}

pub fn cu_method_unknown(m: &str) -> String {
    format!("{m:?} is not a method of this MCP server")
}

pub fn cu_suspended(arg: &str) -> String {
    format!("{arg} is suspended: its guest does not run. Resume it with `paddock app run {arg}`")
}

/// The instructions of the MCP server for the model.
pub fn cu_mcp_instructions() -> String {
    "These tools drive a macOS guest of paddock, never the host. Work in a loop: observe with mac_screenshot (and mac_elements for the controls), act with one tool, then verify with a new screenshot (or screenshot_after=true) before the next step. Coordinates are pixels of the last screenshot of the same target. Without desktop=true the tools see and touch only the app's windows. A refusal names its reason (covered, outside, stale, locked): follow it, do not repeat the same call.".into()
}

// ---- The host broker (a sandbox) -------------------------------------------

pub fn cu_args_not_object() -> String {
    "the arguments of a mac request must be a JSON object".into()
}

/// The summary of the approval card.
pub fn cu_card_summary(name: &str, app_only: bool) -> String {
    match (name.is_empty(), app_only) {
        (true, _) => "use the macOS guest session (the whole guest display)".into(),
        (false, true) => format!("use {name} in the macOS guest"),
        (false, false) => format!("use the whole guest display of {name} in the macOS guest"),
    }
}

/// The detail of the approval card: the target, the session (its name
/// and pid) and the action, and what a grant does and does not reach.
pub fn cu_card_detail(name: &str, session: &str, app_only: bool, what: &str) -> String {
    let target = match (name.is_empty(), app_only) {
        (true, _) => {
            format!("the only running session of the macOS guest ({session}), its whole display")
        }
        (false, true) => format!("the windows of {name} in the macOS guest ({session})"),
        (false, false) => {
            format!("the whole guest display of {name} in the macOS guest ({session})")
        }
    };
    let reach = if app_only {
        format!("Clicks land only on windows of {name} that no other window covers. Keys and text go only to {name} while it is the front app of the guest, and known shortcuts of the system (cmd+space, cmd+tab, Mission Control, the Help menu, the menu bar and the like) are refused. A shortcut that the user changed or added, or one of a service, is not known and is not refused. Clicks on the menu bar and drags with the right button or ctrl are refused, and while a menu is open only escape goes. A menu that a key of the app opens is seen only after the guest shows it, so the next key of the same request can reach it, and from a menu the arrow keys reach the Apple menu (Lock Screen, Log Out, Restart) and Services")
    } else {
        "This reaches every app of the guest and the host folders that are shared with this session"
            .to_string()
    };
    format!("Computer use on {target}: {what}. {reach}. The host pointer and keyboard are never used, but while a window of this session is key on the host, the host clipboard is copied into the guest, so a paste in the guest can read it. Allow for this session covers every screenshot, click and key on this target until the sandbox ends; another session asks again.")
}

/// The session on the card: its name and the pid of its shim.
pub fn cu_card_session(label: &str, pid: u32) -> String {
    format!("session {label}, pid {pid}")
}

/// The typed text on the card.
pub fn cu_card_typed(t: &str) -> String {
    format!("\nText to type:\n{t}")
}

/// The length of the typed text on the card, before the text.
pub fn cu_card_typed_count(n: usize) -> String {
    format!("\n{n} characters to type, every one of them shown below: \u{23CE} is a Return, \u{21E5} a Tab, \\u{{...}} another invisible character.")
}

/// A press without a role on the card.
pub fn cu_card_press_unchecked() -> String {
    "\nThe element is a position in the window, and no role was given: if the window changed since the snapshot, another element there is pressed.".into()
}

/// A sandbox asked to type more than a card can show.
pub fn cu_type_card_too_long(n: usize, max: usize) -> String {
    format!("the text has {n} characters, and an approval card shows at most {max}: type it in parts of at most {max} characters")
}

/// The session of an approved request is no longer the same one.
pub fn cu_session_changed(arg: &str) -> String {
    format!("the session {arg} that the request named stopped or started again since it was approved: send the request again")
}

/// A system shortcut in app scope.
pub fn cu_system_shortcut(combo: &str) -> String {
    format!("{combo} is a shortcut of the system, which would reach other apps of the guest: it is refused for the app's windows. Use --desktop (desktop=true) for the whole guest display")
}

/// A menu is open in app scope.
pub fn cu_menu_open() -> String {
    "a menu is open in the guest: from a menu the keys reach the Apple menu and Services, so in the app's windows only escape goes while a menu is open. Send escape, or use --desktop (desktop=true) for the whole guest display".into()
}

/// A right or ctrl drag in app scope.
pub fn cu_menu_drag() -> String {
    "a drag with the right button or with ctrl opens a context menu, and its let-go picks an item of the menu, which can reach other apps of the guest (services, Quick Actions): it is refused for the app's windows. Use the left button, or --desktop (desktop=true) for the whole guest display".into()
}

/// A right or ctrl click with a count above 1 in app scope.
pub fn cu_menu_clicks() -> String {
    "a click with the right button or with ctrl opens a context menu, and the next click of a double or triple click can pick an item of the menu, which can reach other apps of the guest (services, Quick Actions): it is refused for the app's windows. Send one click, or use --desktop (desktop=true) for the whole guest display".into()
}

/// A combo of the action opened a menu.
pub fn cu_menu_opened(done: usize, total: usize) -> String {
    format!("a menu opened in the guest after {done} of {total} steps: the rest was not sent. Send escape, or use --desktop (desktop=true)")
}

/// A point on the menu bar in app scope.
pub fn cu_menu_bar(x: f64, y: f64) -> String {
    format!("the point {x:.0},{y:.0} is on the menu bar of the guest: its Apple menu and app menu reach the whole guest, so it is refused for the app's windows. Use --desktop (desktop=true)")
}

/// The app left the front during keys or text.
pub fn cu_front_lost(done: usize, total: usize) -> String {
    format!("the app is no longer the front app of the guest after {done} of {total} steps: the rest was not sent. Take a new screenshot")
}

pub fn cu_denied(message: &str) -> String {
    format!("the host did not allow the request: {message}")
}

#[cfg(test)]
#[path = "computer_tests.rs"]
mod tests;
