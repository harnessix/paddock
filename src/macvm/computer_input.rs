//! The input mapping of computer use, the pure part: key names to macOS
//! virtual key codes, the parse of `cmd+shift+t`, and the plan of low
//! events that one [`Input`] becomes. The guest agent posts the plan with
//! CGEvent (`guest/computer_macos.rs`); the tests read it on every system.
//!
//! The key codes are the positions of the ANSI keyboard (`kVK_ANSI_*`).
//! A letter key also carries its character, so an app that reads the
//! characters of a shortcut sees the letter that was named. Text goes in
//! as Unicode strings, never as key codes: it comes out the same on any
//! keyboard layout of the guest.

use super::computer::{Button, Combo, Input, Mod};
use super::proto::Point;
use super::text;

/// The most UTF-16 units of one Unicode key event: macOS drops the rest.
pub const UNICODE_CHUNK: usize = 20;
/// The pause between two typed chunks, in milliseconds.
pub const TYPE_PAUSE_MS: u32 = 8;
/// The pause between the down and the up of a click, and between keys.
pub const PRESS_MS: u32 = 15;
/// The longest gap between two clicks of a double click.
pub const CLICK_GAP_MS: u32 = 40;
/// The step of a drag in points, and its pause.
pub const DRAG_STEP: f64 = 12.0;
pub const DRAG_PAUSE_MS: u32 = 8;
/// The most clicks of one click action.
pub const CLICKS_MAX: u32 = 3;
/// The most lines of one scroll action, each way.
pub const SCROLL_MAX: i32 = 200;
/// The most keys of one key action.
pub const KEYS_MAX: usize = 64;

/// `CGEventFlags` bits.
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CTRL: u64 = 0x0004_0000;
pub const FLAG_OPTION: u64 = 0x0008_0000;
pub const FLAG_CMD: u64 = 0x0010_0000;
pub const FLAG_FN: u64 = 0x0080_0000;

/// The key code of the modifier keys (the left ones).
pub fn mod_code(m: Mod) -> u16 {
    match m {
        Mod::Cmd => 55,
        Mod::Shift => 56,
        Mod::Option => 58,
        Mod::Ctrl => 59,
        Mod::Fn => 63,
    }
}

/// The flag of a modifier.
pub fn mod_flag(m: Mod) -> u64 {
    match m {
        Mod::Cmd => FLAG_CMD,
        Mod::Shift => FLAG_SHIFT,
        Mod::Option => FLAG_OPTION,
        Mod::Ctrl => FLAG_CTRL,
        Mod::Fn => FLAG_FN,
    }
}

/// The flags of a set of modifiers.
pub fn flags(mods: &[Mod]) -> u64 {
    mods.iter().fold(0, |f, m| f | mod_flag(*m))
}

/// A modifier by one of its names.
pub fn parse_mod(name: &str) -> Option<Mod> {
    Some(match name.to_ascii_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" | "⌘" => Mod::Cmd,
        "shift" | "⇧" => Mod::Shift,
        "opt" | "option" | "alt" | "⌥" => Mod::Option,
        "ctrl" | "control" | "ctl" | "⌃" => Mod::Ctrl,
        "fn" | "function" => Mod::Fn,
        _ => return None,
    })
}

/// The letters and digits in key code order of the ANSI layout.
const LETTERS: [(char, u16); 36] = [
    ('a', 0),
    ('s', 1),
    ('d', 2),
    ('f', 3),
    ('h', 4),
    ('g', 5),
    ('z', 6),
    ('x', 7),
    ('c', 8),
    ('v', 9),
    ('b', 11),
    ('q', 12),
    ('w', 13),
    ('e', 14),
    ('r', 15),
    ('y', 16),
    ('t', 17),
    ('1', 18),
    ('2', 19),
    ('3', 20),
    ('4', 21),
    ('6', 22),
    ('5', 23),
    ('9', 25),
    ('7', 26),
    ('8', 28),
    ('0', 29),
    ('o', 31),
    ('u', 32),
    ('i', 34),
    ('p', 35),
    ('l', 37),
    ('j', 38),
    ('k', 40),
    ('n', 45),
    ('m', 46),
];

/// The punctuation keys of the ANSI layout, by character and by name.
const PUNCT: [(char, &str, u16); 11] = [
    ('=', "equal", 24),
    ('-', "minus", 27),
    (']', "rightbracket", 30),
    ('[', "leftbracket", 33),
    ('\'', "quote", 39),
    (';', "semicolon", 41),
    ('\\', "backslash", 42),
    (',', "comma", 43),
    ('/', "slash", 44),
    ('.', "period", 47),
    ('`', "grave", 50),
];

/// The named keys.
const NAMED: [(&str, u16); 39] = [
    ("return", 36),
    ("enter", 36),
    ("tab", 48),
    ("space", 49),
    ("delete", 51),
    ("backspace", 51),
    ("escape", 53),
    ("esc", 53),
    ("forwarddelete", 117),
    ("del", 117),
    ("home", 115),
    ("end", 119),
    ("pageup", 116),
    ("pagedown", 121),
    ("left", 123),
    ("right", 124),
    ("down", 125),
    ("up", 126),
    ("f1", 122),
    ("f2", 120),
    ("f3", 99),
    ("f4", 118),
    ("f5", 96),
    ("f6", 97),
    ("f7", 98),
    ("f8", 100),
    ("f9", 101),
    ("f10", 109),
    ("f11", 103),
    ("f12", 111),
    ("f13", 105),
    ("f14", 107),
    ("f15", 113),
    ("help", 114),
    ("keypadenter", 76),
    ("capslock", 57),
    ("volumeup", 72),
    ("volumedown", 73),
    ("mute", 74),
];

/// The key code of a key by its name or its character, and the
/// character it types without modifiers when it has one.
pub fn key_code(name: &str) -> Option<(u16, Option<char>)> {
    let lower = name.to_ascii_lowercase();
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if let Some((_, code)) = LETTERS.iter().find(|(l, _)| *l == c) {
            return Some((*code, Some(c)));
        }
        if let Some((_, _, code)) = PUNCT.iter().find(|(p, _, _)| *p == c) {
            return Some((*code, Some(c)));
        }
    }
    let bare: String = lower.chars().filter(|c| *c != '_' && *c != ' ').collect();
    if let Some((c, _, code)) = PUNCT.iter().find(|(_, n, _)| *n == bare) {
        return Some((*code, Some(*c)));
    }
    if bare == "plus" {
        return Some((24, Some('=')));
    }
    let (_, code) = NAMED.iter().find(|(n, _)| *n == bare)?;
    let typed = match bare.as_str() {
        "space" => Some(' '),
        _ => None,
    };
    Some((*code, typed))
}

/// One combo from its text: modifiers and a key joined by `+`, as
/// `cmd+shift+t`, `ctrl+left`, `return`. A lone `+` is the key `=`
/// with shift.
pub fn parse_combo(text: &str) -> Result<Combo, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(text::cu_key_empty());
    }
    if t == "+" {
        return Ok(Combo {
            mods: vec![Mod::Shift],
            key: "=".into(),
        });
    }
    let parts: Vec<&str> = t.split('+').collect();
    let (key, mods) = match parts.split_last() {
        Some((k, m)) if !k.is_empty() => (*k, m),
        // `cmd++`: the key is `+` itself.
        _ if t.ends_with("++") => ("plus", &parts[..parts.len() - 2]),
        _ => return Err(text::cu_key_unknown(t)),
    };
    let mut out = Vec::new();
    for m in mods {
        match parse_mod(m.trim()) {
            Some(m) if !out.contains(&m) => out.push(m),
            Some(_) => {}
            None => return Err(text::cu_mod_unknown(m, t)),
        }
    }
    let key = key.trim();
    if key_code(key).is_none() {
        // A modifier alone is a key press too: `shift`.
        if let (Some(m), true) = (parse_mod(key), out.is_empty()) {
            return Ok(Combo {
                mods: vec![m],
                key: String::new(),
            });
        }
        return Err(text::cu_key_unknown(key));
    }
    out.sort();
    Ok(Combo {
        mods: out,
        key: key.to_string(),
    })
}

/// A low event: what one CGEvent does.
#[derive(Debug, Clone, PartialEq)]
pub enum Low {
    /// The pointer goes to the point, no button down.
    Move(Point),
    Down {
        at: Point,
        button: Button,
        clicks: u32,
        flags: u64,
    },
    Up {
        at: Point,
        button: Button,
        clicks: u32,
        flags: u64,
    },
    /// The pointer moves with the button down.
    Drag {
        at: Point,
        button: Button,
        flags: u64,
    },
    /// Scroll lines at the pointer: positive `y` scrolls up (the CGEvent
    /// sign), positive `x` scrolls left.
    Scroll { x: i32, y: i32 },
    /// A key down or up with these flags; `chars` is the character for
    /// the Unicode string of the event.
    Key {
        code: u16,
        down: bool,
        flags: u64,
        chars: Option<char>,
    },
    /// A modifier key down or up: a flags-changed event with the flags
    /// after it.
    Modifier { code: u16, down: bool, flags: u64 },
    /// A Unicode string, at most [`UNICODE_CHUNK`] UTF-16 units.
    Text(String),
    /// A pause in milliseconds.
    Wait(u32),
}

/// The text cut into chunks of at most [`UNICODE_CHUNK`] UTF-16 units,
/// never inside a character, with a return and a tab as their keys.
pub fn type_plan(text: &str) -> Vec<Low> {
    type_steps(text).concat()
}

/// [`type_plan`] in steps: one chunk, or one return or tab, each.
fn type_steps(text: &str) -> Vec<Vec<Low>> {
    let mut out: Vec<Vec<Low>> = Vec::new();
    let mut chunk = String::new();
    let mut units = 0;
    let flush = |chunk: &mut String, units: &mut usize, out: &mut Vec<Vec<Low>>| {
        if !chunk.is_empty() {
            out.push(vec![
                Low::Text(std::mem::take(chunk)),
                Low::Wait(TYPE_PAUSE_MS),
            ]);
            *units = 0;
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let key = match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                Some(36)
            }
            '\n' => Some(36),
            '\t' => Some(48),
            _ => None,
        };
        if let Some(code) = key {
            flush(&mut chunk, &mut units, &mut out);
            let mut step = Vec::new();
            press(&mut step, code, 0, None);
            out.push(step);
            continue;
        }
        let n = c.len_utf16();
        if units + n > UNICODE_CHUNK {
            flush(&mut chunk, &mut units, &mut out);
        }
        chunk.push(c);
        units += n;
    }
    flush(&mut chunk, &mut units, &mut out);
    out
}

fn press(out: &mut Vec<Low>, code: u16, flags: u64, chars: Option<char>) {
    out.push(Low::Key {
        code,
        down: true,
        flags,
        chars,
    });
    out.push(Low::Wait(PRESS_MS));
    out.push(Low::Key {
        code,
        down: false,
        flags,
        chars,
    });
}

/// The events of one combo: the modifiers down in order, the key down and
/// up with their flags, the modifiers up in the other order.
pub fn combo_plan(c: &Combo, out: &mut Vec<Low>) -> Result<(), String> {
    let mut held = 0u64;
    for m in &c.mods {
        held |= mod_flag(*m);
        out.push(Low::Modifier {
            code: mod_code(*m),
            down: true,
            flags: held,
        });
    }
    if !c.key.is_empty() {
        let (code, ch) = key_code(&c.key).ok_or_else(|| text::cu_key_unknown(&c.key))?;
        let typed = match ch {
            Some(ch) if c.mods.contains(&Mod::Shift) => Some(shifted(ch)),
            other => other,
        };
        press(out, code, held, typed);
    } else {
        out.push(Low::Wait(PRESS_MS));
    }
    for m in c.mods.iter().rev() {
        held &= !mod_flag(*m);
        out.push(Low::Modifier {
            code: mod_code(*m),
            down: false,
            flags: held,
        });
    }
    out.push(Low::Wait(PRESS_MS));
    Ok(())
}

/// The character of an ANSI key with shift.
fn shifted(c: char) -> char {
    const PAIRS: [(char, char); 21] = [
        ('1', '!'),
        ('2', '@'),
        ('3', '#'),
        ('4', '$'),
        ('5', '%'),
        ('6', '^'),
        ('7', '&'),
        ('8', '*'),
        ('9', '('),
        ('0', ')'),
        ('-', '_'),
        ('=', '+'),
        ('[', '{'),
        (']', '}'),
        ('\\', '|'),
        (';', ':'),
        ('\'', '"'),
        (',', '<'),
        ('.', '>'),
        ('/', '?'),
        ('`', '~'),
    ];
    PAIRS
        .iter()
        .find(|(a, _)| *a == c)
        .map(|(_, b)| *b)
        .unwrap_or_else(|| c.to_ascii_uppercase())
}

/// The points of a drag from `a` to `b`, `b` last, at most
/// [`DRAG_STEP`] points apart, and at least 4 steps.
pub fn drag_path(a: Point, b: Point) -> Vec<Point> {
    let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
    let steps = ((d / DRAG_STEP).ceil() as usize).clamp(4, 200);
    (1..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            Point {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
            }
        })
        .collect()
}

/// The low events of one action, after the checks of its values.
pub fn plan(input: &Input) -> Result<Vec<Low>, String> {
    Ok(steps(input)?.concat())
}

/// [`plan`] in steps: one per combo of a key action, one per chunk (and
/// per return or tab) of typed text, one for a pointer action. The guest
/// checks before each step that the keys still go where they may.
pub fn steps(input: &Input) -> Result<Vec<Vec<Low>>, String> {
    let mut out = Vec::new();
    let mut steps = None;
    match input {
        Input::Move { at } => out.push(Low::Move(*at)),
        Input::Click {
            at,
            button,
            count,
            mods,
        } => {
            if *count == 0 || *count > CLICKS_MAX {
                return Err(text::cu_click_count(*count, CLICKS_MAX));
            }
            let f = flags(mods);
            out.push(Low::Move(*at));
            out.push(Low::Wait(PRESS_MS));
            for n in 1..=*count {
                out.push(Low::Down {
                    at: *at,
                    button: *button,
                    clicks: n,
                    flags: f,
                });
                out.push(Low::Wait(PRESS_MS));
                out.push(Low::Up {
                    at: *at,
                    button: *button,
                    clicks: n,
                    flags: f,
                });
                if n < *count {
                    out.push(Low::Wait(CLICK_GAP_MS));
                }
            }
        }
        Input::Drag {
            from,
            to,
            button,
            mods,
        } => {
            let f = flags(mods);
            out.push(Low::Move(*from));
            out.push(Low::Wait(PRESS_MS));
            out.push(Low::Down {
                at: *from,
                button: *button,
                clicks: 1,
                flags: f,
            });
            out.push(Low::Wait(PRESS_MS * 4));
            // The press is a step of its own: in app scope the guest looks
            // for a menu that it opened before the pointer moves and lets
            // go, so a let-go never picks an item of the menu.
            steps = Some(vec![std::mem::take(&mut out)]);
            for p in drag_path(*from, *to) {
                out.push(Low::Drag {
                    at: p,
                    button: *button,
                    flags: f,
                });
                out.push(Low::Wait(DRAG_PAUSE_MS));
            }
            out.push(Low::Wait(PRESS_MS * 4));
            out.push(Low::Up {
                at: *to,
                button: *button,
                clicks: 1,
                flags: f,
            });
            if let Some(v) = steps.as_mut() {
                v.push(std::mem::take(&mut out));
            }
        }
        Input::Scroll { at, dx, dy } => {
            if *dx == 0 && *dy == 0 {
                return Err(text::cu_scroll_zero());
            }
            if dx.abs() > SCROLL_MAX || dy.abs() > SCROLL_MAX {
                return Err(text::cu_scroll_too_far(SCROLL_MAX));
            }
            out.push(Low::Move(*at));
            out.push(Low::Wait(PRESS_MS));
            // One event per 5 lines, so the app sees a scroll, not a jump.
            let (mut left_x, mut left_y) = (*dx, *dy);
            while left_x != 0 || left_y != 0 {
                let sx = left_x.clamp(-5, 5);
                let sy = left_y.clamp(-5, 5);
                left_x -= sx;
                left_y -= sy;
                // CGEvent: positive wheel 1 scrolls up, positive wheel 2 left.
                out.push(Low::Scroll { x: -sx, y: -sy });
                out.push(Low::Wait(PRESS_MS));
            }
        }
        Input::Type { text: t } => {
            if t.is_empty() {
                return Err(text::cu_type_empty());
            }
            if t.chars().count() > super::computer::TYPE_MAX {
                return Err(text::cu_type_too_long(super::computer::TYPE_MAX));
            }
            steps = Some(type_steps(t));
        }
        Input::Key { combos } => {
            if combos.is_empty() {
                return Err(text::cu_key_empty());
            }
            if combos.len() > KEYS_MAX {
                return Err(text::cu_keys_too_many(KEYS_MAX));
            }
            let mut all = Vec::new();
            for c in combos {
                let mut step = Vec::new();
                combo_plan(c, &mut step)?;
                all.push(step);
            }
            steps = Some(all);
        }
    }
    for p in input.points() {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return Err(text::cu_point_invalid());
        }
    }
    Ok(steps.unwrap_or_else(|| vec![out]))
}

/// A combo as text: `cmd+shift+t`, `space`.
pub fn combo_text(c: &Combo) -> String {
    let mut parts: Vec<&str> = c
        .mods
        .iter()
        .map(|m| match m {
            Mod::Cmd => "cmd",
            Mod::Shift => "shift",
            Mod::Option => "option",
            Mod::Ctrl => "ctrl",
            Mod::Fn => "fn",
        })
        .collect();
    if !c.key.is_empty() {
        parts.push(&c.key);
    }
    parts.join("+")
}

/// True for a combo that the system takes, not the front app: it opens
/// Spotlight, the app switcher, Mission Control, the Dock, Force Quit,
/// the screenshot tools, VoiceOver, the Help menu or the menu bar
/// (`ctrl+f2`), or locks or logs out the session.
/// In app scope such a combo would reach other apps of the guest, so the
/// guest refuses it. The test is on the key code that [`key_code`] gives,
/// so every name of a key (`space`, `s_pace`, `SPACE`, ` `) is the same
/// key. This is a list of the known shortcuts of the system: a shortcut
/// that the user changed or that a service adds is not in it.
pub fn system_shortcut(c: &Combo) -> bool {
    let has = |m: Mod| c.mods.contains(&m);
    let (cmd, ctrl, opt, shift, fnk) = (
        has(Mod::Cmd),
        has(Mod::Ctrl),
        has(Mod::Option),
        has(Mod::Shift),
        has(Mod::Fn),
    );
    // A modifier alone: fn (or globe) starts dictation, the emoji
    // picker or the input source.
    if c.key.is_empty() {
        return fnk;
    }
    let Some((code, _)) = key_code(&c.key) else {
        // Not a key: the plan refuses it before this.
        return false;
    };
    let letter = |ch: char| LETTERS.iter().any(|(l, k)| *l == ch && *k == code);
    let digit = LETTERS
        .iter()
        .any(|(l, k)| l.is_ascii_digit() && *k == code);
    let punct = |ch: char| PUNCT.iter().any(|(p, _, k)| *p == ch && *k == code);
    let named = |name: &str| NAMED.iter().any(|(n, k)| *n == name && *k == code);
    let fkey = NAMED.iter().any(|(n, k)| {
        *k == code
            && n.len() >= 2
            && n.starts_with('f')
            && n[1..].bytes().all(|b| b.is_ascii_digit())
    });
    // Spotlight, Finder search, the input source, the emoji picker.
    if named("space") {
        return cmd || ctrl || opt || fnk;
    }
    // The app switcher.
    if named("tab") {
        return cmd;
    }
    // Force Quit.
    if named("escape") {
        return cmd && opt;
    }
    // Lock the screen, log out.
    if letter('q') {
        return (cmd && ctrl) || (cmd && shift);
    }
    // The screenshot tools.
    if cmd && shift && ['3', '4', '5', '6'].into_iter().any(letter) {
        return true;
    }
    // Mission Control, App Exposé, the Spaces.
    if ["left", "right", "up", "down"].into_iter().any(named) {
        return ctrl;
    }
    if ctrl && digit {
        return true;
    }
    // Zoom and inverted colors: `=` is also `plus`.
    if cmd && opt && (letter('8') || punct('=') || punct('-')) {
        return true;
    }
    // The Dock.
    if letter('d') && cmd && opt {
        return true;
    }
    // fn with a letter or a digit: the Dock, the Control Center, the
    // Notification Center, Quick Note, the emoji picker.
    if fnk && LETTERS.iter().any(|(_, k)| *k == code) {
        return true;
    }
    // The Help menu of the app (cmd+?): from an open menu the arrow keys
    // reach the Apple menu (Lock Screen, Log Out, Restart) and Services.
    if cmd && shift && punct('/') {
        return true;
    }
    // Keyboard focus on the Dock or the menu bar, VoiceOver, the
    // accessibility shortcuts; Show Desktop and Dashboard.
    if fkey {
        return cmd || ctrl || opt || fnk || named("f11") || named("f12");
    }
    false
}

/// True for a drag that opens a context menu at the press: the right
/// button, or ctrl with any button. The menu opens under the pointer and
/// the let-go picks the item at the end of the drag, which can be a
/// service that reaches other apps (Search With Google, Quick Actions).
/// In app scope the guest refuses such a drag.
pub fn menu_drag(input: &Input) -> bool {
    matches!(input, Input::Drag { button, mods, .. }
        if *button == Button::Right || mods.contains(&Mod::Ctrl))
}

/// True for a click of the right button, or with ctrl, with a count
/// above 1. The first click opens a context menu, and when the menu does
/// not fit on the display macOS moves it under the pointer: the next
/// click picks an item. In app scope the guest refuses such a click.
pub fn menu_clicks(input: &Input) -> bool {
    matches!(input, Input::Click { button, mods, count, .. }
        if *count > 1 && (*button == Button::Right || mods.contains(&Mod::Ctrl)))
}

/// The keys that give `?` with shift on the common layouts: `/` of the US
/// and British layouts, the key right of `0` on the German, Swiss, Nordic,
/// Italian and Spanish layouts (`-` on the US layout), and the key of `,`
/// on the French and Belgian layouts (`m` on the US layout).
const HELP_KEYS: [u16; 3] = [44, 27, 46];

/// True for the shortcut of the Help menu (cmd+?) on the layout of the
/// guest. `char_of(code, shift)` gives the character of a key with cmd,
/// and with shift when it is true, on the current layout, or None when the
/// layout cannot say. Then each key that gives `?` on a common layout
/// counts. From the Help menu the arrow keys reach the Apple menu.
pub fn help_combo(c: &Combo, char_of: impl FnOnce(u16, bool) -> Option<char>) -> bool {
    if !c.mods.contains(&Mod::Cmd) {
        return false;
    }
    let Some((code, _)) = key_code(&c.key) else {
        return false;
    };
    let shift = c.mods.contains(&Mod::Shift);
    match char_of(code, shift) {
        Some(ch) => ch == '?',
        None => shift && HELP_KEYS.contains(&code),
    }
}

/// True for the escape key with no modifier, by any name of the key: it
/// closes an open menu and reaches nothing else.
pub fn is_escape(c: &Combo) -> bool {
    c.mods.is_empty() && key_code(&c.key).is_some_and(|(code, _)| code == ESCAPE)
}

/// The key code of escape.
const ESCAPE: u16 = 53;

/// The count of events that post (the waits do not).
pub fn event_count(plan: &[Low]) -> u32 {
    plan.iter().filter(|l| !matches!(l, Low::Wait(_))).count() as u32
}

#[cfg(test)]
#[path = "computer_input_tests.rs"]
mod tests;
