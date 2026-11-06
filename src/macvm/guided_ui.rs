//! The geometry and the typing of the guided window, platform-neutral:
//! `vz_mac_window.rs` and `vz_mac_panel.rs` only apply what this file
//! computes, so the Linux tests prove the numbers.
//!
//! - [`window_layout`]: the first frame of the window inside the visible
//!   frame of the screen (never under the Dock or the menu bar), and the
//!   frame of the guest view, which keeps the aspect of the guest display
//!   and fills the room left of the panel.
//! - [`view_frame`]: the frame of the guest view after a resize.
//! - [`stack`]: the panel rows from the top, each as tall as its text.
//! - [`key_strokes`]: a text as US ANSI key codes with shift, for the
//!   `Type ... into the guest` buttons. The guest has no clipboard before
//!   its agent runs, so the host types into the view instead.

/// A rectangle in AppKit points, origin at the bottom left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Whether `inner` lies inside this rectangle, with a little room for
    /// rounding.
    pub fn contains(&self, inner: &Rect) -> bool {
        const E: f64 = 0.5;
        inner.x >= self.x - E
            && inner.y >= self.y - E
            && inner.x + inner.w <= self.x + self.w + E
            && inner.y + inner.h <= self.y + self.h + E
    }
}

/// The width of the side panel.
pub const PANEL_W: f64 = 380.0;
/// The smallest guest view that still shows Setup Assistant readably.
pub const MIN_VIEW_W: f64 = 480.0;
pub const MIN_VIEW_H: f64 = 420.0;
/// The largest first guest view: the size of the guest display in points
/// (`vz_mac::DISPLAY`, 1240 by 1000). A bigger view only scales it up.
pub const MAX_VIEW_W: f64 = 1240.0;
/// The room kept free around the first frame inside the visible frame.
pub const SCREEN_MARGIN: f64 = 8.0;

/// What [`window_layout`] returns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowLayout {
    /// The content rectangle of the window, in screen coordinates: the
    /// argument of `initWithContentRect`.
    pub content: Rect,
    /// The smallest content size, for `setContentMinSize`.
    pub min_w: f64,
    pub min_h: f64,
    /// The guest view, in content coordinates.
    pub view: Rect,
    /// The side panel, in content coordinates.
    pub panel: Rect,
}

/// The first frame of the guided window. `visible` is
/// `NSScreen.mainScreen.visibleFrame` (the screen without the menu bar
/// and the Dock), `aspect` the width over the height of the guest
/// display, `title_bar` the height that the window frame adds to the
/// content. The window is as large as fits, up to the guest display at
/// its own size, and centered in `visible`; the whole frame, title bar
/// included, stays inside `visible`.
pub fn window_layout(visible: Rect, aspect: f64, title_bar: f64) -> WindowLayout {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let room_w = (visible.w - 2.0 * SCREEN_MARGIN - PANEL_W).max(1.0);
    let room_h = (visible.h - 2.0 * SCREEN_MARGIN - title_bar).max(1.0);
    // The largest view of the guest aspect in the room, no larger than the
    // guest display itself.
    let mut vw = room_w.min(MAX_VIEW_W).min(room_h * aspect);
    let mut vh = vw / aspect;
    // The content is never smaller than the panel needs: a screen too small
    // for the view gets a smaller view, not a window under the Dock.
    let min_h = MIN_VIEW_H.min(room_h);
    if vh < min_h {
        vh = min_h;
        vw = (vh * aspect).min(room_w);
    }
    let content_w = (vw + PANEL_W).floor();
    let content_h = vh.max(min_h).floor();
    let frame_h = content_h + title_bar;
    let x = visible.x + ((visible.w - content_w) / 2.0).max(0.0);
    let frame_y = visible.y + ((visible.h - frame_h) / 2.0).max(0.0);
    let content = Rect::new(x.floor(), frame_y.floor(), content_w, content_h);
    // The first frame is never below the smallest size: on a tiny screen
    // the smallest size is the first frame.
    WindowLayout {
        content,
        min_w: (MIN_VIEW_W + PANEL_W).min(content_w),
        min_h: MIN_VIEW_H.min(content_h),
        view: view_frame(content_w, content_h, aspect),
        panel: Rect::new(content_w - PANEL_W, 0.0, PANEL_W, content_h),
    }
}

/// The guest view in a content of `w` by `h`: the largest rectangle of
/// `aspect` left of the panel, centered in that room. The bars around it
/// show the window background.
pub fn view_frame(w: f64, h: f64, aspect: f64) -> Rect {
    let room_w = (w - PANEL_W).max(1.0);
    let room_h = h.max(1.0);
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        room_w / room_h
    };
    let (vw, vh) = if room_w / room_h > aspect {
        (room_h * aspect, room_h)
    } else {
        (room_w, room_w / aspect)
    };
    Rect::new(
        ((room_w - vw) / 2.0).floor(),
        ((room_h - vh) / 2.0).floor(),
        vw.floor(),
        vh.floor(),
    )
}

/// One row of the panel: its height (0 for a hidden row) and the gap
/// under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub h: f64,
    pub gap: f64,
}

/// The panel rows from the top of a flipped view: the top of each row,
/// and the height of them all with `margin` above and below. A hidden
/// row (height 0) takes no gap either.
pub fn stack(rows: &[Row], margin: f64) -> (Vec<f64>, f64) {
    let mut y = margin;
    let mut tops = Vec::with_capacity(rows.len());
    let mut last_gap = 0.0;
    for r in rows {
        tops.push(y);
        if r.h > 0.0 {
            y += r.h + r.gap;
            last_gap = r.gap;
        }
    }
    (tops, y - last_gap + margin)
}

/// The kVK_Shift key code, for the flags change around a shifted key.
pub const SHIFT_KEY: u16 = 0x38;

/// One key of a typed text: the US ANSI virtual key code, whether shift
/// is down, the character it gives and the one without shift (the two
/// `characters` of the `NSEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub code: u16,
    pub shift: bool,
    pub ch: char,
    pub plain: char,
}

/// The US ANSI key codes (`kVK_ANSI_*` of Carbon's `Events.h`) of the
/// unshifted characters.
const PLAIN: &[(char, u16)] = &[
    ('a', 0x00),
    ('s', 0x01),
    ('d', 0x02),
    ('f', 0x03),
    ('h', 0x04),
    ('g', 0x05),
    ('z', 0x06),
    ('x', 0x07),
    ('c', 0x08),
    ('v', 0x09),
    ('b', 0x0B),
    ('q', 0x0C),
    ('w', 0x0D),
    ('e', 0x0E),
    ('r', 0x0F),
    ('y', 0x10),
    ('t', 0x11),
    ('1', 0x12),
    ('2', 0x13),
    ('3', 0x14),
    ('4', 0x15),
    ('6', 0x16),
    ('5', 0x17),
    ('=', 0x18),
    ('9', 0x19),
    ('7', 0x1A),
    ('-', 0x1B),
    ('8', 0x1C),
    ('0', 0x1D),
    (']', 0x1E),
    ('o', 0x1F),
    ('u', 0x20),
    ('[', 0x21),
    ('i', 0x22),
    ('p', 0x23),
    ('\n', 0x24),
    ('l', 0x25),
    ('j', 0x26),
    ('\'', 0x27),
    ('k', 0x28),
    (';', 0x29),
    ('\\', 0x2A),
    (',', 0x2B),
    ('/', 0x2C),
    ('n', 0x2D),
    ('m', 0x2E),
    ('.', 0x2F),
    ('\t', 0x30),
    (' ', 0x31),
    ('`', 0x32),
];

/// The shifted symbols of the US layout and their unshifted key.
const SHIFTED: &[(char, char)] = &[
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
];

fn plain_code(c: char) -> Option<u16> {
    PLAIN.iter().find(|(k, _)| *k == c).map(|(_, code)| *code)
}

/// The key of one character on the US layout, `None` when the US layout
/// has no key for it (`é`, `€`, a control character other than the line
/// end and the tab).
pub fn key_of(c: char) -> Option<Stroke> {
    if let Some(code) = plain_code(c) {
        return Some(Stroke {
            code,
            shift: false,
            ch: c,
            plain: c,
        });
    }
    let plain = if c.is_ascii_uppercase() {
        c.to_ascii_lowercase()
    } else {
        SHIFTED.iter().find(|(s, _)| *s == c)?.1
    };
    Some(Stroke {
        code: plain_code(plain)?,
        shift: true,
        ch: c,
        plain,
    })
}

/// The keys of a whole text, or the first character that has no key. A
/// text is typed whole or not at all, so a person never gets half a
/// password in a field.
pub fn key_strokes(text: &str) -> Result<Vec<Stroke>, char> {
    text.chars().map(|c| key_of(c).ok_or(c)).collect()
}

/// One event that the host sends to the guest view for a typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// A flags change: the left shift key goes down.
    ShiftDown,
    /// A `keyDown` of the stroke.
    Down(Stroke),
    /// A `keyUp` of the stroke.
    Up(Stroke),
    /// A flags change: the shift key goes up.
    ShiftUp,
}

impl KeyAction {
    /// The virtual key code of the event and whether the key goes down.
    pub fn code_down(self) -> (u16, bool) {
        match self {
            KeyAction::ShiftDown => (SHIFT_KEY, true),
            KeyAction::ShiftUp => (SHIFT_KEY, false),
            KeyAction::Down(s) => (s.code, true),
            KeyAction::Up(s) => (s.code, false),
        }
    }
}

/// The events of a text, in order: each key goes down and up, and a
/// shifted key has the shift key down around it, as a real keyboard
/// sends it. Shift is released after each key, so a lost event never
/// leaves the guest with shift held for the rest of the text.
pub fn key_actions(strokes: &[Stroke]) -> Vec<KeyAction> {
    let mut out = Vec::with_capacity(strokes.len() * 2);
    for s in strokes {
        if s.shift {
            out.push(KeyAction::ShiftDown);
        }
        out.push(KeyAction::Down(*s));
        out.push(KeyAction::Up(*s));
        if s.shift {
            out.push(KeyAction::ShiftUp);
        }
    }
    out
}

/// The pause between two events: the guest keyboard is a USB device of
/// the framework, and a burst can lose keys in a busy guest.
pub const KEY_GAP_MS: u64 = 15;

/// How a [`play_keys`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Played {
    /// Every action went.
    All,
    /// `go` said no before a key: the keys before it went whole.
    Stopped,
    /// `send` failed: the keyboard of the VM is not reachable.
    Failed,
}

/// Send the actions of a text in order, with `pause` after each. `go` is
/// asked before each key (a shift press or a plain key press), never in
/// the middle of one, so a stop never leaves a key or shift held down in
/// the guest. The Type buttons of the guided window and the unlock of a
/// locked guest screen (`run_screen.rs`) both type with it.
pub fn play_keys(
    actions: &[KeyAction],
    mut go: impl FnMut() -> bool,
    mut send: impl FnMut(KeyAction) -> bool,
    mut pause: impl FnMut(),
) -> Played {
    for a in actions {
        let starts_key = match a {
            KeyAction::ShiftDown => true,
            KeyAction::Down(s) => !s.shift,
            KeyAction::Up(_) | KeyAction::ShiftUp => false,
        };
        if starts_key && !go() {
            return Played::Stopped;
        }
        if !send(*a) {
            return Played::Failed;
        }
        pause();
    }
    Played::All
}

#[cfg(test)]
#[path = "guided_ui_tests.rs"]
mod tests;
