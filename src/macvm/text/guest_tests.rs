//! One test per function of text/guest.rs, named as the function.

use super::super::guest as t;
use std::path::Path;
use std::time::Duration;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn agent_cannot_update() {
    let s = t::agent_cannot_update("0.1.0", "1.0", "0.2.1", "2.0");
    has(
        &s,
        &[
            "version 0.1.0 (protocol 1.0)",
            "0.2.1 (protocol 2.0)",
            "paddock macvm update",
        ],
    );
}

#[test]
fn not_app_bundle() {
    has(
        &t::not_app_bundle(Path::new("/x/A.app")),
        &["/x/A.app is not an app bundle"],
    );
}

#[test]
fn launch_failed() {
    has(
        &t::launch_failed(Path::new("/x/A.app"), "e"),
        &["cannot launch /x/A.app: e"],
    );
}

#[test]
fn launch_timeout() {
    has(
        &t::launch_timeout(Path::new("/x/A.app"), Duration::from_secs(30)),
        &["Launch Services for /x/A.app within 30 s"],
    );
}

#[test]
fn no_running_app() {
    assert_eq!(t::no_running_app(7), "no running app with pid 7");
}

#[test]
fn quit_not_taken() {
    has(&t::quit_not_taken(7), &["app 7 did not take the quit"]);
}

#[test]
fn app_not_running() {
    assert_eq!(
        t::app_not_running("com.openai.chat"),
        "com.openai.chat does not run"
    );
}

#[test]
fn no_window() {
    assert_eq!(t::no_window(5), "no window 5");
}

#[test]
fn window_no_ax_element() {
    has(
        &t::window_no_ax_element(5),
        &["window 5 has no Accessibility element"],
    );
}

#[test]
fn ax_timeout() {
    has(
        &t::ax_timeout("Raise", Duration::from_secs(1)),
        &["answer the Raise within 1 s"],
    );
}

#[test]
fn ax_failed() {
    assert_eq!(t::ax_failed("Move", -25204), "Move failed: AXError -25204");
}

#[test]
fn ax_element_gone() {
    assert_eq!(
        t::ax_element_gone(&[0, 2]),
        "no element at [0, 2] in the window now"
    );
}

#[test]
fn ax_element_changed() {
    assert_eq!(
        t::ax_element_changed(&[0, 2], "AXButton", "AXGroup"),
        "the element at [0, 2] is a AXGroup now, not a AXButton"
    );
}

#[test]
fn ax_value_not_a_number() {
    assert_eq!(
        t::ax_value_not_a_number("abc"),
        "the element holds a number, not \"abc\""
    );
}

#[test]
fn cannot_run_command() {
    assert_eq!(
        t::cannot_run_command("diskutil", "e"),
        "cannot run diskutil: e"
    );
}

#[test]
fn command_failed() {
    assert_eq!(
        t::command_failed("diskutil mount", "busy"),
        "diskutil mount failed: busy"
    );
}

#[test]
fn verify_no_run() {
    assert_eq!(t::verify_no_run("e"), "cannot run the verify: e");
}

#[test]
fn step_timed_out() {
    has(
        &t::step_timed_out("out", Duration::from_secs(60)),
        &["out\ntimed out after 60 s"],
    );
}

#[test]
fn io_failed() {
    assert_eq!(
        t::io_failed("cannot open the file", "e"),
        "cannot open the file: e"
    );
}

#[test]
fn cannot_write_named() {
    assert_eq!(t::cannot_write_named("/x", "e"), "cannot write /x: e");
}

#[test]
fn cannot_read_named() {
    assert_eq!(t::cannot_read_named("/x", "e"), "cannot read /x: e");
}

#[test]
fn data_step_now() {
    assert_eq!(
        t::data_step_now("verify the volume"),
        "data disk step: verify the volume"
    );
}

#[test]
fn no_data_disk() {
    has(&t::no_data_disk("paddock-data"), &["serial paddock-data"]);
}

#[test]
fn verify_result() {
    assert_eq!(t::verify_result(true, None, "ok"), "verify ok: ok");
    assert_eq!(
        t::verify_result(false, None, "e"),
        "verify failed, cannot run the repair: e"
    );
    assert_eq!(
        t::verify_result(false, Some(Ok("r")), ""),
        "verify failed, repair ok: r"
    );
    assert_eq!(
        t::verify_result(false, Some(Err("r")), ""),
        "verify failed, repair failed: r"
    );
}

#[test]
fn data_volume_no_mount() {
    has(&t::data_volume_no_mount("x"), &["did not mount: x"]);
}

#[test]
fn input_source_restored() {
    has(
        &t::input_source_restored("Pinyin - Simplified"),
        &["Pinyin - Simplified", "restored"],
    );
}

#[test]
fn input_source_select_failed() {
    has(
        &t::input_source_select_failed("com.apple.keylayout.US", -50),
        &["com.apple.keylayout.US", "OSStatus -50"],
    );
}

#[test]
fn dialog_press_failed() {
    has(
        &t::dialog_press_failed(-25204),
        &["Ignore", "AXError -25204"],
    );
}

#[test]
fn input_source_fallback() {
    has(
        &t::input_source_fallback(
            "com.apple.inputmethod.SCIM.ITABC",
            "com.apple.keylayout.ABC",
            Some(-50),
        ),
        &[
            "com.apple.inputmethod.SCIM.ITABC",
            "OSStatus -50",
            "using com.apple.keylayout.ABC",
        ],
    );
    has(
        &t::input_source_fallback("x", "com.apple.keylayout.US", None),
        &["the guest does not have it"],
    );
}

#[test]
fn update_hash_mismatch() {
    has(
        &t::update_hash_mismatch("aa", "bb"),
        &["hash aa", "named bb"],
    );
}

#[test]
fn host_write_failed() {
    assert_eq!(t::host_write_failed("e"), "cannot write to the host: e");
}

#[test]
fn socket_clone_failed() {
    assert_eq!(t::socket_clone_failed("e"), "cannot clone the socket: e");
}

#[test]
fn no_token_first() {
    assert_eq!(
        t::no_token_first("ping"),
        "no token: the host sent ping first"
    );
}

#[test]
fn no_token_timeout() {
    has(
        &t::no_token_timeout(Duration::from_secs(30)),
        &["within 30 s of the hello"],
    );
}

#[test]
fn vsock_no_answer() {
    has(
        &t::vsock_no_answer(1030, Duration::from_secs(10)),
        &["port 1030 within 10 s"],
    );
}
