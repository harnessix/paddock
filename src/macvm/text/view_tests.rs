use super::*;

#[test]
fn view_status_hint() {
    assert_eq!(super::view_status_hint(), "Ctrl-] menu");
}

#[test]
fn view_status_size() {
    assert_eq!(super::view_status_size(480, 300), " 480x300");
}

#[test]
fn view_status_line() {
    assert_eq!(
        super::view_status_line("the guest desktop", " 480x300", "kitty"),
        " the guest desktop 480x300 | kitty | Ctrl-] menu "
    );
}

#[test]
fn view_status_note() {
    assert_eq!(super::view_status_note("a note"), "| a note ");
}

#[test]
fn view_frame_refused() {
    assert_eq!(
        super::view_frame_refused("the frame is not a PNG"),
        "a frame was refused: the frame is not a PNG"
    );
}

#[test]
fn view_action_failed() {
    assert_eq!(
        super::view_action_failed("no agent"),
        "the guest did not take the input: no agent"
    );
}

#[test]
fn view_resize_failed() {
    assert_eq!(
        super::view_resize_failed("no agent"),
        "the guest did not take the new size, so frames keep the old size: no agent"
    );
}

#[test]
fn view_menu() {
    let lines = super::view_menu("ChatGPT");
    assert_eq!(lines[0], "paddock mac view: ChatGPT");
    assert!(lines.contains(&"  Ctrl-C  leave the view (the session keeps running)".to_string()));
    assert!(lines.contains(&"  Esc     close this menu".to_string()));
    assert_eq!(lines.len(), 18);
}

#[test]
fn view_left() {
    assert_eq!(
        super::view_left("ChatGPT", "paddock mac view --app ChatGPT"),
        "The view of ChatGPT ended. The session keeps running. Run `paddock mac view --app ChatGPT` to see it again."
    );
}

#[test]
fn view_no_images() {
    let notes = vec!["This terminal (xterm) answers no image protocol.".to_string()];
    let s = super::view_no_images(&notes, "ChatGPT", true);
    assert!(s.starts_with(
        "paddock mac view did not start: no image protocol works in this terminal.\nThis terminal (xterm)"
    ));
    assert!(
        s.contains("\nThe windows of ChatGPT stay in their normal windows on the host desktop.")
    );
    assert!(s.ends_with(
        "\nTo force a protocol, use --protocol kitty, iterm2 or sixel. The exit code is 3."
    ));
    assert!(!super::view_no_images(&[], "ChatGPT", false).contains("ChatGPT"));
}

#[test]
fn view_not_a_terminal() {
    assert_eq!(
        super::view_not_a_terminal(),
        "paddock mac view needs a terminal: stdin and stdout must be a TTY."
    );
}

#[test]
fn view_needs_macos() {
    let s = super::view_needs_macos();
    assert!(s.contains("Mac with Apple silicon"), "{s}");
    assert!(s.contains("--fake-source"), "{s}");
}

#[test]
fn view_stream_missing() {
    let s = super::view_stream_missing();
    assert!(s.contains("frames.subscribe"), "{s}");
}

#[test]
fn view_target_needs_app() {
    let s = super::view_target_needs_app();
    assert!(s.contains("--app NAME"), "{s}");
}

#[test]
fn view_stream_ended() {
    assert_eq!(
        super::view_stream_ended("ChatGPT", "the guest stopped"),
        "The picture of ChatGPT stopped: the guest stopped"
    );
}

#[test]
fn view_too_small() {
    assert_eq!(
        super::view_too_small(),
        "The terminal is too small: the view needs at least 2 rows."
    );
}

#[test]
fn view_fake_actions() {
    assert_eq!(
        super::view_fake_actions(3),
        "The fake source got 3 actions."
    );
}

#[test]
fn view_guest_desktop() {
    assert_eq!(super::view_guest_desktop(), "the guest desktop");
}

#[test]
fn view_desktop_of() {
    assert_eq!(super::view_desktop_of("ChatGPT"), "the desktop of ChatGPT");
}

#[test]
fn view_instance() {
    assert_eq!(super::view_instance("ChatGPT", 2), "ChatGPT (instance 2)");
}

#[test]
fn view_frame_size_mismatch() {
    assert_eq!(
        super::view_frame_size_mismatch(480, 300, 640, 400),
        "the frame says 480x300 but its PNG is 640x400"
    );
}

#[test]
fn view_png_too_big() {
    assert_eq!(
        super::view_png_too_big(70_000_000, 67_108_864),
        "the frame has 70000000 bytes, more than the limit of 67108864"
    );
}

#[test]
fn view_png_bad_side() {
    assert_eq!(
        super::view_png_bad_side(0, 300, 16_384),
        "the frame is 0x300 pixels, outside 1 to 16384 per side"
    );
}

#[test]
fn view_png_too_many_pixels() {
    assert_eq!(
        super::view_png_too_many_pixels(10_000, 10_000, 40_000_000),
        "the frame is 10000x10000 pixels, more than 40000000 pixels"
    );
}

#[test]
fn view_png_bad_color() {
    assert_eq!(
        super::view_png_bad_color(3, 16),
        "the PNG has color type 3 with depth 16, which is not valid"
    );
}

#[test]
fn view_png_bad_filter() {
    assert_eq!(super::view_png_bad_filter(9), "a PNG row has filter type 9");
}

#[test]
fn view_unknown_protocol() {
    assert_eq!(
        super::view_unknown_protocol("png"),
        "unknown image protocol \"png\": use auto, kitty, iterm2 or sixel"
    );
}

#[test]
fn view_no_protocol() {
    assert_eq!(
        super::view_no_protocol("xterm"),
        "This terminal (xterm) answers no image protocol: the view needs the kitty graphics protocol, iTerm2 inline images or sixel."
    );
}

#[test]
fn view_tmux() {
    assert_eq!(super::view_tmux("3.4", Some("kitty")), "tmux 3.4 in kitty");
    assert_eq!(super::view_tmux("3.4", None), "tmux 3.4");
}

#[test]
fn view_probe_lines() {
    let lines = super::view_probe_lines(&ViewProbe {
        protocol: "kitty",
        tmux_wrap: false,
        kitty: true,
        sixel: false,
        kitty_keys: true,
        mouse_pixels: false,
        cols: 80,
        rows: 24,
        cell_w: 10,
        cell_h: 20,
        guessed: true,
        term: "xterm-kitty",
        term_program: "",
    });
    assert_eq!(
        lines,
        [
            "protocol: kitty",
            "tmux passthrough: not used",
            "kitty graphics: true",
            "sixel: false",
            "kitty keyboard: true",
            "mouse in pixels: false",
            "terminal: 80 cols x 24 rows, cell 10x20 px (a guess)",
            "TERM=xterm-kitty TERM_PROGRAM=",
        ]
    );
}
