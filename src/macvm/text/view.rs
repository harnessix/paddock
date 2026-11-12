//! The texts of the terminal view, `paddock mac view`: the status line,
//! the shortcut menu, the probe report, the checks of a frame and the
//! notes of the terminal probe.

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;

/// The hint in the status line.
pub fn view_status_hint() -> &'static str {
    "Ctrl-] menu"
}

/// The size of the last frame in the status line.
pub fn view_status_size(width: u32, height: u32) -> String {
    format!(" {width}x{height}")
}

/// The status line: the target, the frame size, the protocol and the hint.
pub fn view_status_line(label: &str, size: &str, protocol: &str) -> String {
    format!(" {label}{size} | {protocol} | {} ", view_status_hint())
}

/// The last note at the end of the status line.
pub fn view_status_note(note: &str) -> String {
    format!("| {note} ")
}

pub fn view_frame_refused(e: &str) -> String {
    format!("a frame was refused: {e}")
}

pub fn view_action_failed(e: &str) -> String {
    format!("the guest did not take the input: {e}")
}

pub fn view_resize_failed(e: &str) -> String {
    format!("the guest did not take the new size, so frames keep the old size: {e}")
}

/// The lines of the shortcut menu.
pub fn view_menu(label: &str) -> Vec<String> {
    [
        format!("paddock mac view: {label}"),
        "  Ctrl-C  leave the view (the session keeps running)".into(),
        "  Ctrl-D  leave the view too".into(),
        "  Esc     close this menu".into(),
        "  Ctrl-L  draw the picture again".into(),
        "  Ctrl-]  send Ctrl-] to the Mac".into(),
        "  Ctrl-E  Cmd-Option-Esc (Force Quit Applications)".into(),
        String::new(),
        "Any other key goes to the Mac with Cmd:".into(),
        "  q  Cmd-Q quit the app         w  Cmd-W close the window".into(),
        "  c  Cmd-C copy                 v  Cmd-V paste".into(),
        "  x  Cmd-X cut                  z  Cmd-Z undo (Z: Cmd-Shift-Z redo)".into(),
        "  a  Cmd-A select all           f  Cmd-F find".into(),
        "  t  Cmd-T new tab              n  Cmd-N new window".into(),
        "  ,  Cmd-, settings             h  Cmd-H hide the app".into(),
        "  Tab    Cmd-Tab switch apps    Space  Cmd-Space Spotlight".into(),
        "  `      Cmd-` next window      arrows Cmd-arrow keys".into(),
        "  Alt-key  Cmd-Option-key       Ctrl-key  Cmd-Control-key".into(),
    ]
    .into_iter()
    .collect()
}

/// After the view ends.
pub fn view_left(label: &str, again: &str) -> String {
    format!("The view of {label} ended. The session keeps running. Run `{again}` to see it again.")
}

/// The terminal shows no images, so no view started. `windows`: the
/// target is an app session, whose windows show on the host desktop.
pub fn view_no_images(notes: &[String], label: &str, windows: bool) -> String {
    let mut s =
        String::from("paddock mac view did not start: no image protocol works in this terminal.");
    for n in notes {
        s.push('\n');
        s.push_str(n);
    }
    if windows {
        s.push_str(&format!(
            "\nThe windows of {label} stay in their normal windows on the host desktop."
        ));
    }
    s.push_str("\nTo force a protocol, use --protocol kitty, iterm2 or sixel. The exit code is 3.");
    s
}

pub fn view_not_a_terminal() -> String {
    "paddock mac view needs a terminal: stdin and stdout must be a TTY.".into()
}

pub fn view_needs_macos() -> String {
    "paddock mac view shows a macOS guest, so it runs on a Mac with Apple silicon. Use --fake-source to try the terminal side on this system.".into()
}

pub fn view_stream_missing() -> String {
    "The terminal view does not read the frame stream of the guest agent (frames.subscribe) yet. Use paddock mac screenshot to see the guest.".into()
}

pub fn view_target_needs_app() -> String {
    "paddock mac view: --instance and --desktop need --app NAME. Without --app the view shows the whole guest desktop.".into()
}

pub fn view_stream_ended(label: &str, why: &str) -> String {
    format!("The picture of {label} stopped: {why}")
}

pub fn view_too_small() -> String {
    "The terminal is too small: the view needs at least 2 rows.".into()
}

/// `--fake-source` ended: the count of the recorded actions.
pub fn view_fake_actions(count: usize) -> String {
    format!("The fake source got {count} actions.")
}

/// The target of the view is the desktop of the only running guest.
pub fn view_guest_desktop() -> String {
    "the guest desktop".into()
}

/// The target of the view is the desktop of an app's guest.
pub fn view_desktop_of(name: &str) -> String {
    format!("the desktop of {name}")
}

/// An app name with its instance number, for the second instance and up.
pub fn view_instance(name: &str, n: u32) -> String {
    format!("{name} (instance {n})")
}

/// The size that the guest states differs from the size of the PNG.
pub fn view_frame_size_mismatch(width: u32, height: u32, png_w: u32, png_h: u32) -> String {
    format!("the frame says {width}x{height} but its PNG is {png_w}x{png_h}")
}

/// A frame is larger than the byte limit.
pub fn view_png_too_big(bytes: usize, max: usize) -> String {
    format!("the frame has {bytes} bytes, more than the limit of {max}")
}

/// A side of a frame is 0 or above the limit.
pub fn view_png_bad_side(width: u32, height: u32, max_side: u32) -> String {
    format!("the frame is {width}x{height} pixels, outside 1 to {max_side} per side")
}

/// A frame has more pixels than the limit.
pub fn view_png_too_many_pixels(width: u32, height: u32, max: u64) -> String {
    format!("the frame is {width}x{height} pixels, more than {max} pixels")
}

/// The color type and the bit depth of a PNG do not go together.
pub fn view_png_bad_color(color: u8, depth: u8) -> String {
    format!("the PNG has color type {color} with depth {depth}, which is not valid")
}

/// A PNG row names an unknown filter.
pub fn view_png_bad_filter(filter: u8) -> String {
    format!("a PNG row has filter type {filter}")
}

/// `--image-protocol` names no known protocol.
pub fn view_unknown_protocol(s: &str) -> String {
    format!("unknown image protocol {s:?}: use auto, kitty, iterm2 or sixel")
}

/// The probe found no image protocol in the terminal.
pub fn view_no_protocol(terminal: &str) -> String {
    format!(
        "This terminal ({terminal}) answers no image protocol: the view needs the kitty graphics protocol, iTerm2 inline images or sixel."
    )
}

/// The terminal name of a tmux session: the version and the client.
pub fn view_tmux(version: &str, client: Option<&str>) -> String {
    match client {
        Some(client) => format!("tmux {version} in {client}"),
        None => format!("tmux {version}"),
    }
}

/// What the probe found: the values of a probe, one line each.
pub struct ViewProbe<'a> {
    pub protocol: &'a str,
    pub tmux_wrap: bool,
    pub kitty: bool,
    pub sixel: bool,
    pub kitty_keys: bool,
    pub mouse_pixels: bool,
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
    pub guessed: bool,
    pub term: &'a str,
    pub term_program: &'a str,
}

/// The lines of `paddock mac view --probe`.
pub fn view_probe_lines(p: &ViewProbe) -> Vec<String> {
    vec![
        format!("protocol: {}", p.protocol),
        format!(
            "tmux passthrough: {}",
            if p.tmux_wrap { "used" } else { "not used" }
        ),
        format!("kitty graphics: {}", p.kitty),
        format!("sixel: {}", p.sixel),
        format!("kitty keyboard: {}", p.kitty_keys),
        format!("mouse in pixels: {}", p.mouse_pixels),
        format!(
            "terminal: {} cols x {} rows, cell {}x{} px{}",
            p.cols,
            p.rows,
            p.cell_w,
            p.cell_h,
            if p.guessed { " (a guess)" } else { "" }
        ),
        format!("TERM={} TERM_PROGRAM={}", p.term, p.term_program),
    ]
}
