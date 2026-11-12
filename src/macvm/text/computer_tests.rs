//! One test per function of text/computer.rs, named as the function.

use super::super::computer as t;
use std::path::Path;

#[test]
fn cu_no_app() {
    let s = t::cu_no_app();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_no_window() {
    let s = t::cu_no_window();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_outside_display() {
    assert_eq!(
        t::cu_outside_display(-1.0, 5.0, 1440, 900),
        "the point -1,5 is outside the guest display (1440x900 points)"
    );
}

#[test]
fn cu_covered() {
    let s = t::cu_covered(10.0, 20.0, "Finder");
    assert!(s.contains("Finder") && s.contains("10,20"), "{s}");
    assert!(t::cu_covered(1.0, 2.0, "").contains("another app"));
}

#[test]
fn cu_outside_app() {
    let s = t::cu_outside_app(12.0, 12.0);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_locked() {
    let s = t::cu_locked();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_no_accessibility() {
    let s = t::cu_no_accessibility();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_no_screen_recording() {
    let s = t::cu_no_screen_recording();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_not_front() {
    let s = t::cu_not_front();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_element_id_invalid() {
    let s = t::cu_element_id_invalid("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_element_window_gone() {
    let s = t::cu_element_window_gone(7);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&7.to_string()), "{s}");
}

#[test]
fn cu_element_gone() {
    let s = t::cu_element_gone("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_element_role() {
    let s = t::cu_element_role("1:0.1", "AXButton", "AXGroup");
    assert!(
        s.contains("AXGroup") && s.contains("AXButton") && s.contains("nothing was pressed"),
        "{s}"
    );
}

#[test]
fn cu_press_failed() {
    let s = t::cu_press_failed("x", 5);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_ax_timeout() {
    let s = t::cu_ax_timeout();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_ax_failed() {
    let s = t::cu_ax_failed(5);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&5.to_string()), "{s}");
}

#[test]
fn cu_key_empty() {
    let s = t::cu_key_empty();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_key_unknown() {
    let s = t::cu_key_unknown("zz");
    assert!(s.contains("\"zz\"") && s.contains("return"), "{s}");
}

#[test]
fn cu_mod_unknown() {
    let s = t::cu_mod_unknown("hyper", "hyper+a");
    assert!(s.contains("\"hyper\"") && s.contains("cmd, shift"), "{s}");
}

#[test]
fn cu_click_count() {
    assert_eq!(t::cu_click_count(4, 3), "4 clicks: a click has 1 to 3");
}

#[test]
fn cu_scroll_zero() {
    let s = t::cu_scroll_zero();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_scroll_too_far() {
    let s = t::cu_scroll_too_far(5);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&5.to_string()), "{s}");
}

#[test]
fn cu_scroll_direction() {
    let s = t::cu_scroll_direction("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_type_empty() {
    let s = t::cu_type_empty();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_type_too_long() {
    let s = t::cu_type_too_long(9);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&9.to_string()), "{s}");
}

#[test]
fn cu_keys_too_many() {
    let s = t::cu_keys_too_many(9);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&9.to_string()), "{s}");
}

#[test]
fn cu_point_invalid() {
    let s = t::cu_point_invalid();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_outside_image() {
    let s = t::cu_outside_image(2000.0, 10.0, 1280, 800);
    assert!(s.contains("2000,10") && s.contains("1280x800"), "{s}");
}

#[test]
fn cu_stale() {
    let s = t::cu_stale();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_max_size_invalid() {
    let s = t::cu_max_size_invalid("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_call_unknown() {
    let s = t::cu_call_unknown("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_line_too_long() {
    let s = t::cu_line_too_long(9);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&9.to_string()), "{s}");
}

#[test]
fn cu_bad_message() {
    let s = t::cu_bad_message("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_payload_too_big() {
    let s = t::cu_payload_too_big(10, 5);
    assert!(s.contains("10 bytes") && s.contains("5"), "{s}");
}

#[test]
fn cu_session_unreachable() {
    let s = t::cu_session_unreachable(Path::new("/run/c.sock"), "refused");
    assert!(s.contains("/run/c.sock") && s.contains("refused"), "{s}");
}

#[test]
fn cu_port_refused() {
    let s = t::cu_port_refused("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_closed() {
    let s = t::cu_closed();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_no_hello() {
    let s = t::cu_no_hello();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_version() {
    let s = t::cu_version(2, 1);
    assert!(s.contains("version 2") && s.contains("version 1"), "{s}");
}

#[test]
fn cu_send_failed() {
    let s = t::cu_send_failed("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_read_failed() {
    let s = t::cu_read_failed("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_no_answer() {
    let s = t::cu_no_answer();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_wrong_reply() {
    let s = t::cu_wrong_reply("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_capture_failed() {
    let s = t::cu_capture_failed("x");
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains('x'), "{s}");
}

#[test]
fn cu_capture_unsupported() {
    let s = t::cu_capture_unsupported();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_png_failed() {
    let s = t::cu_png_failed();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_post_failed() {
    let s = t::cu_post_failed();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_activate_failed() {
    let s = t::cu_activate_failed(7);
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
    assert!(s.contains(&7.to_string()), "{s}");
}

#[test]
fn cu_no_session() {
    let s = t::cu_no_session();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_session_not_running() {
    let s = t::cu_session_not_running("notes", &[]);
    assert!(s.contains("paddock app run notes --backend macvm"), "{s}");
    let s = t::cu_session_not_running(
        "notes",
        &["mail".to_string(), "web --instance 2".to_string()],
    );
    assert!(s.contains("Running: mail, web --instance 2"), "{s}");
}

#[test]
fn cu_sessions_many() {
    let s = t::cu_sessions_many(&["a".to_string(), "b".to_string()]);
    assert!(s.contains("a, b") && s.contains("--app"), "{s}");
}

#[test]
fn cu_instance_needs_app() {
    let s = t::cu_instance_needs_app();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_hint_screen_recording() {
    let s = t::cu_hint_screen_recording();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_hint_accessibility() {
    let s = t::cu_hint_accessibility();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_hint_locked() {
    let s = t::cu_hint_locked();
    assert!(!s.is_empty() && !s.contains('\n'), "{s}");
}

#[test]
fn cu_arg_missing() {
    assert_eq!(t::cu_arg_missing("x"), "the argument x is missing");
}

#[test]
fn cu_button_invalid() {
    assert!(t::cu_button_invalid("big").contains("left, right or middle"));
}

#[test]
fn cu_tool_unknown() {
    assert!(t::cu_tool_unknown("mac_fly").contains("\"mac_fly\""));
}

#[test]
fn cu_method_unknown() {
    assert!(t::cu_method_unknown("x/y").contains("\"x/y\""));
}

#[test]
fn cu_suspended() {
    let s = t::cu_suspended("notes --instance 2");
    assert!(s.contains("paddock app run notes --instance 2"), "{s}");
}

#[test]
fn cu_mcp_instructions() {
    let s = t::cu_mcp_instructions();
    assert!(
        s.contains("never the host") && s.contains("mac_screenshot"),
        "{s}"
    );
}

#[test]
fn cu_args_not_object() {
    assert!(t::cu_args_not_object().contains("JSON object"));
}

#[test]
fn cu_card_summary() {
    assert_eq!(
        t::cu_card_summary("notes", true),
        "use notes in the macOS guest"
    );
    assert!(t::cu_card_summary("notes", false).contains("whole guest display of notes"));
    assert!(t::cu_card_summary("", true).contains("whole guest display"));
}

#[test]
fn cu_card_detail() {
    let s = t::cu_card_detail("notes", "notes, pid 42", true, "mac_click at 1,2");
    assert!(
        s.contains("windows of notes")
            && s.contains("mac_click at 1,2")
            && s.contains("pid 42")
            && s.contains("known shortcuts of the system")
            && s.contains("is not known and is not refused")
            && s.contains("host clipboard is copied into the guest"),
        "{s}"
    );
    // It never claims that the host stays untouched: the clipboard moves.
    assert!(!s.contains("Nothing on the host"), "{s}");
    assert!(!s.contains("and no other window"), "{s}");
    assert!(t::cu_card_detail("", "n, pid 1", false, "x").contains("only running session"));
    let d = t::cu_card_detail("n", "n, pid 1", false, "x");
    assert!(d.contains("whole guest display of n"), "{d}");
    assert!(d.contains("host folders that are shared"), "{d}");
}

#[test]
fn cu_card_session() {
    assert_eq!(t::cu_card_session("notes 2", 42), "session notes 2, pid 42");
}

#[test]
fn cu_card_typed() {
    assert_eq!(t::cu_card_typed("hi"), "\nText to type:\nhi");
}

#[test]
fn cu_card_typed_count() {
    let s = t::cu_card_typed_count(12);
    assert!(s.contains("12 characters") && s.contains('\u{23CE}'), "{s}");
}

#[test]
fn cu_card_press_unchecked() {
    assert!(t::cu_card_press_unchecked().contains("another element"));
}

#[test]
fn cu_type_card_too_long() {
    let s = t::cu_type_card_too_long(1200, 1000);
    assert!(s.contains("1200") && s.contains("at most 1000"), "{s}");
}

#[test]
fn cu_session_changed() {
    assert!(t::cu_session_changed("notes --instance 2").contains("notes --instance 2"));
}

#[test]
fn cu_system_shortcut() {
    let s = t::cu_system_shortcut("cmd+space");
    assert!(s.contains("cmd+space") && s.contains("--desktop"), "{s}");
}

#[test]
fn cu_menu_open() {
    let s = t::cu_menu_open();
    assert!(s.contains("only escape") && s.contains("--desktop"), "{s}");
}

#[test]
fn cu_menu_opened() {
    assert!(t::cu_menu_opened(1, 3).contains("after 1 of 3 steps"));
}

#[test]
fn cu_menu_bar() {
    let s = t::cu_menu_bar(10.0, 5.0);
    assert!(s.contains("10,5") && s.contains("menu bar"), "{s}");
}

#[test]
fn cu_menu_drag() {
    let s = t::cu_menu_drag();
    assert!(s.contains("context menu") && s.contains("--desktop"), "{s}");
}

#[test]
fn cu_menu_clicks() {
    let s = t::cu_menu_clicks();
    assert!(s.contains("context menu") && s.contains("one click"), "{s}");
}

#[test]
fn cu_front_lost() {
    assert!(t::cu_front_lost(2, 5).contains("after 2 of 5 steps"));
}

#[test]
fn cu_denied() {
    assert!(t::cu_denied("no").contains("did not allow the request: no"));
}
