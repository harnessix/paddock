//! The bytes that a terminal sends, as events: text, keys with their
//! modifiers (legacy escapes and the kitty keyboard protocol), SGR mouse
//! reports in cells or pixels, bracketed paste and focus. Pure, fed in
//! chunks: a sequence split across two reads waits for its rest.

/// The modifiers of a key or a mouse event, as the guest names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    /// Alt or Meta on the terminal.
    pub option: bool,
    pub control: bool,
    /// Super on the terminal (the kitty keyboard protocol reports it).
    pub cmd: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        option: false,
        control: false,
        cmd: false,
    };

    /// The xterm modifier parameter: 1 plus the bits shift 1, alt 2,
    /// ctrl 4, super 8 (meta 32 counts as alt).
    pub fn from_param(p: u32) -> Mods {
        let b = p.saturating_sub(1);
        Mods {
            shift: b & 1 != 0,
            option: b & 2 != 0 || b & 32 != 0,
            control: b & 4 != 0,
            cmd: b & 8 != 0,
        }
    }

    /// The modifier bits of an SGR mouse report.
    fn from_mouse(b: u32) -> Mods {
        Mods {
            shift: b & 4 != 0,
            option: b & 8 != 0,
            control: b & 16 != 0,
            cmd: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Mods::NONE
    }

    /// The names in the order of macOS menus: control, option, shift, cmd.
    pub fn names(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.control {
            v.push("control");
        }
        if self.option {
            v.push("option");
        }
        if self.shift {
            v.push("shift");
        }
        if self.cmd {
            v.push("cmd");
        }
        v
    }
}

/// A mouse button of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    /// No button: a motion report.
    None,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Motion,
}

/// One mouse report. `x` and `y` are 0-based: cells, or pixels when
/// `pixels` is true (SGR-pixel mode 1016).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub button: Button,
    pub x: u32,
    pub y: u32,
    pub mods: Mods,
}

/// One event of the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Printable text, typed or pasted with no modifiers.
    Text(String),
    /// A key with a guest name (`return`, `left`, `f5`, `a`) and modifiers.
    Key {
        key: String,
        mods: Mods,
    },
    Mouse(Mouse),
    /// A bracketed paste.
    Paste(String),
    /// The terminal window gained or lost the focus.
    Focus(bool),
}

/// The longest pending escape sequence; a longer one is dropped.
const MAX_SEQ: usize = 64;
/// The longest terminal string (APC, DCS, OSC) that waits for its end.
/// The parser drops a longer one up to here, and then drops the rest of
/// it as it comes, up to its end.
const MAX_STRING: usize = 1 << 16;
/// CAN and SUB: they cancel a string (ECMA-48).
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
/// The longest bracketed paste that the view sends on.
pub const MAX_PASTE: usize = 1 << 20;

/// The parser. Feed it bytes with [`Parser::feed`]. When no byte comes
/// for a short while, call [`Parser::idle`]: a lone ESC is the Escape key.
#[derive(Debug, Default)]
pub struct Parser {
    buf: Vec<u8>,
    paste: Option<Vec<u8>>,
    /// A string longer than [`MAX_STRING`] goes on: its bytes are dropped
    /// up to its end, a byte that no reply holds, or a quiet input.
    skip: bool,
    /// The SGR mouse coordinates are pixels.
    pub pixels: bool,
}

fn key(name: &str, mods: Mods) -> Event {
    Event::Key {
        key: name.to_string(),
        mods,
    }
}

impl Parser {
    pub fn new(pixels: bool) -> Parser {
        Parser {
            pixels,
            ..Parser::default()
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(bytes);
        self.drain(false)
    }

    /// The input went quiet: what is pending is complete.
    pub fn idle(&mut self) -> Vec<Event> {
        self.drain(true)
    }

    /// True while bytes wait for the rest of a sequence, or the rest of
    /// a long string is dropped.
    pub fn pending(&self) -> bool {
        !self.buf.is_empty() || self.skip
    }

    fn drain(&mut self, idle: bool) -> Vec<Event> {
        let mut out = Vec::new();
        let mut text = String::new();
        let mut i = 0;
        let buf = std::mem::take(&mut self.buf);
        while i < buf.len() {
            if let Some(p) = self.paste.as_mut() {
                // Inside a paste: everything up to ESC [ 201 ~ is text.
                let rest = &buf[i..];
                if let Some(end) = find(rest, b"\x1b[201~") {
                    if p.len() < MAX_PASTE {
                        p.extend_from_slice(&rest[..end.min(MAX_PASTE - p.len())]);
                    }
                    let s = String::from_utf8_lossy(p).into_owned();
                    self.paste = None;
                    flush(&mut text, &mut out);
                    out.push(Event::Paste(s));
                    i += end + 6;
                    continue;
                }
                // Keep a possible partial end marker for the next read.
                let keep = partial_suffix(rest, b"\x1b[201~");
                let take = rest.len() - keep;
                if p.len() < MAX_PASTE {
                    p.extend_from_slice(&rest[..take.min(MAX_PASTE - p.len())]);
                }
                self.buf = rest[take..].to_vec();
                flush(&mut text, &mut out);
                return out;
            }
            if self.skip {
                match skip_string(&buf[i..], idle) {
                    Skip::End(n) => {
                        self.skip = false;
                        i += n;
                    }
                    Skip::More(n) => {
                        // Keep a lone ESC: it can start ST.
                        self.buf = buf[i + n..].to_vec();
                        flush(&mut text, &mut out);
                        return out;
                    }
                }
                continue;
            }
            let b = buf[i];
            if b == 0x1b {
                match self.escape(&buf[i..], idle) {
                    Parsed::Done(n, ev) => {
                        flush(&mut text, &mut out);
                        out.extend(ev);
                        i += n;
                    }
                    Parsed::More => {
                        flush(&mut text, &mut out);
                        self.buf = buf[i..].to_vec();
                        return out;
                    }
                }
                continue;
            }
            if b < 0x20 || b == 0x7f {
                flush(&mut text, &mut out);
                out.push(control(b));
                i += 1;
                continue;
            }
            // UTF-8 text.
            let len = utf8_len(b);
            if i + len > buf.len() {
                if idle {
                    i = buf.len();
                    continue;
                }
                flush(&mut text, &mut out);
                self.buf = buf[i..].to_vec();
                return out;
            }
            if let Ok(s) = std::str::from_utf8(&buf[i..i + len]) {
                text.push_str(s);
            }
            i += len;
        }
        if idle {
            // A quiet input ends a long string, so it cannot swallow the
            // keys after it.
            self.skip = false;
        }
        flush(&mut text, &mut out);
        out
    }

    fn escape(&mut self, s: &[u8], idle: bool) -> Parsed {
        if s.len() == 1 {
            return if idle {
                Parsed::Done(1, vec![key("escape", Mods::NONE)])
            } else {
                Parsed::More
            };
        }
        match s[1] {
            b'[' => self.csi(s, idle),
            b'O' => {
                if s.len() < 3 {
                    return if idle {
                        Parsed::Done(2, vec![key("o", alt())])
                    } else {
                        Parsed::More
                    };
                }
                let name = match s[2] {
                    b'A' => "up",
                    b'B' => "down",
                    b'C' => "right",
                    b'D' => "left",
                    b'H' => "home",
                    b'F' => "end",
                    b'P' => "f1",
                    b'Q' => "f2",
                    b'R' => "f3",
                    b'S' => "f4",
                    b'M' => "return",
                    _ => return Parsed::Done(3, vec![]),
                };
                Parsed::Done(3, vec![key(name, Mods::NONE)])
            }
            // APC, DCS, OSC, SOS and PM: a terminal reply (a late kitty
            // graphics answer, a color report), never keys for the guest.
            b'_' | b'P' | b']' | b'X' | b'^' => self.string(s, idle),
            0x1b => Parsed::Done(1, vec![key("escape", Mods::NONE)]),
            b => {
                // ESC then a key: the terminal's Alt (Meta sends ESC).
                if b < 0x20 || b == 0x7f {
                    let mut ev = control(b);
                    if let Event::Key { mods, .. } = &mut ev {
                        mods.option = true;
                    }
                    return Parsed::Done(2, vec![ev]);
                }
                let len = utf8_len(b);
                if s.len() < 1 + len {
                    return if idle {
                        Parsed::Done(s.len(), vec![])
                    } else {
                        Parsed::More
                    };
                }
                let ch = String::from_utf8_lossy(&s[1..1 + len]).into_owned();
                Parsed::Done(1 + len, vec![char_key(&ch, alt())])
            }
        }
    }

    fn csi(&mut self, s: &[u8], idle: bool) -> Parsed {
        // ESC [ params intermediates final, final in 0x40..=0x7e.
        let mut j = 2;
        while j < s.len() && !(0x40..=0x7e).contains(&s[j]) {
            j += 1;
            if j > MAX_SEQ {
                return Parsed::Done(j, vec![]);
            }
        }
        if j >= s.len() {
            return if idle {
                Parsed::Done(s.len(), vec![])
            } else {
                Parsed::More
            };
        }
        let fin = s[j];
        let body = &s[2..j];
        let n = j + 1;
        if body.first() == Some(&b'<') && (fin == b'M' || fin == b'm') {
            return Parsed::Done(n, self.mouse(&body[1..], fin == b'M').into_iter().collect());
        }
        if body == b"200" && fin == b'~' {
            self.paste = Some(Vec::new());
            return Parsed::Done(n, vec![]);
        }
        if body.is_empty() && (fin == b'I' || fin == b'O') {
            return Parsed::Done(n, vec![Event::Focus(fin == b'I')]);
        }
        if body
            .first()
            .is_some_and(|c| !c.is_ascii_digit() && *c != b';')
        {
            // A private reply (?...), not a key.
            return Parsed::Done(n, vec![]);
        }
        let text = String::from_utf8_lossy(body);
        let params: Vec<&str> = text.split(';').collect();
        let num = |k: usize| -> u32 {
            params
                .get(k)
                .and_then(|p| p.split(':').next())
                .and_then(|p| p.parse().ok())
                .unwrap_or(0)
        };
        let mods = Mods::from_param(num(1));
        let ev = match fin {
            b'A' => key("up", mods),
            b'B' => key("down", mods),
            b'C' => key("right", mods),
            b'D' => key("left", mods),
            b'H' => key("home", mods),
            b'F' => key("end", mods),
            b'P' => key("f1", mods),
            b'Q' => key("f2", mods),
            b'R' => key("f3", mods),
            b'S' => key("f4", mods),
            b'Z' => key(
                "tab",
                Mods {
                    shift: true,
                    ..mods
                },
            ),
            b'~' => match num(0) {
                1 | 7 => key("home", mods),
                2 => key("help", mods),
                3 => key("forward-delete", mods),
                4 | 8 => key("end", mods),
                5 => key("page-up", mods),
                6 => key("page-down", mods),
                11 => key("f1", mods),
                12 => key("f2", mods),
                13 => key("f3", mods),
                14 => key("f4", mods),
                15 => key("f5", mods),
                17 => key("f6", mods),
                18 => key("f7", mods),
                19 => key("f8", mods),
                20 => key("f9", mods),
                21 => key("f10", mods),
                23 => key("f11", mods),
                24 => key("f12", mods),
                _ => return Parsed::Done(n, vec![]),
            },
            b'u' => match kitty_key(num(0), mods) {
                Some(ev) => ev,
                None => return Parsed::Done(n, vec![]),
            },
            _ => return Parsed::Done(n, vec![]),
        };
        Parsed::Done(n, vec![ev])
    }

    fn mouse(&self, body: &[u8], press: bool) -> Option<Event> {
        let text = std::str::from_utf8(body).ok()?;
        let mut it = text.split(';').map(|p| p.parse::<u32>().ok());
        let (b, x, y) = (it.next()??, it.next()??, it.next()??);
        let motion = b & 32 != 0;
        let low = b & 3;
        let button = if b & 64 != 0 {
            match low {
                0 => Button::WheelUp,
                1 => Button::WheelDown,
                2 => Button::WheelLeft,
                _ => Button::WheelRight,
            }
        } else {
            match low {
                0 => Button::Left,
                1 => Button::Middle,
                2 => Button::Right,
                _ => Button::None,
            }
        };
        let kind = if motion {
            MouseKind::Motion
        } else if press {
            MouseKind::Press
        } else {
            MouseKind::Release
        };
        Some(Event::Mouse(Mouse {
            kind,
            button,
            x: x.saturating_sub(1),
            y: y.saturating_sub(1),
            mods: Mods::from_mouse(b),
        }))
    }

    /// A string sequence ESC `_`, `P`, `]`, `X` or `^` up to ST (ESC and
    /// a backslash) or BEL: a terminal reply (a late kitty graphics
    /// answer, a color report), dropped whole. It waits for its end
    /// across reads.
    ///
    /// The introducer is also legacy Alt with `_`, `P`, `]`, `X` or `^`.
    /// A byte that no reply holds (an ESC that is not ST, another C0
    /// control, DEL) shows that the user typed it: the introducer is
    /// then that Alt key, and the bytes after it are keys too.
    ///
    /// CAN or SUB cancels the string: it is dropped with them. If the
    /// input goes quiet first, the introducer alone is its Alt key, and
    /// an unfinished string is dropped, so a broken reply cannot swallow
    /// the keys after it. A string longer than [`MAX_STRING`] is dropped
    /// up to there, and the parser drops the rest of it as it comes.
    fn string(&mut self, s: &[u8], idle: bool) -> Parsed {
        let typed = || {
            let ch = (s[1] as char).to_string();
            Parsed::Done(2, vec![char_key(&ch, alt())])
        };
        for (j, &b) in s.iter().enumerate().skip(2) {
            if j >= MAX_STRING {
                self.skip = true;
                return Parsed::Done(j, vec![]);
            }
            match b {
                0x07 => return Parsed::Done(j + 1, vec![]),
                CAN | SUB => return Parsed::Done(j + 1, vec![]),
                0x1b => match s.get(j + 1) {
                    Some(b'\\') => return Parsed::Done(j + 2, vec![]),
                    Some(_) => return typed(),
                    // ESC last: ST or the Escape key, the next byte says.
                    None if !idle => return Parsed::More,
                    None => return typed(),
                },
                b if not_in_reply(b) => return typed(),
                _ => {}
            }
        }
        if !idle {
            return Parsed::More;
        }
        if s.len() == 2 {
            return typed();
        }
        Parsed::Done(s.len(), vec![])
    }
}

/// A byte that no terminal reply holds inside its string: a C0 control
/// other than the ends BEL, ESC, CAN and SUB, or DEL. A typed key (Return,
/// Tab, Backspace, a control key) sends it.
fn not_in_reply(b: u8) -> bool {
    (b < 0x20 && !matches!(b, 0x07 | 0x1b | CAN | SUB)) || b == 0x7f
}

enum Skip {
    /// The long string ended after this many bytes; the rest is input.
    End(usize),
    /// All but the last this many bytes are dropped; the string goes on.
    More(usize),
}

/// The rest of a string longer than [`MAX_STRING`]: drop it up to BEL,
/// ST, CAN or SUB, which go with it. An ESC that is not ST and another
/// byte that no reply holds end it too, but stay input: a key typed
/// after a broken reply. A quiet input ends it with what is there.
fn skip_string(s: &[u8], idle: bool) -> Skip {
    for (j, &b) in s.iter().enumerate() {
        match b {
            0x07 | CAN | SUB => return Skip::End(j + 1),
            0x1b => match s.get(j + 1) {
                Some(b'\\') => return Skip::End(j + 2),
                Some(_) => return Skip::End(j),
                None if idle => return Skip::End(j),
                None => return Skip::More(j),
            },
            b if not_in_reply(b) => return Skip::End(j),
            _ => {}
        }
    }
    if idle {
        Skip::End(s.len())
    } else {
        Skip::More(s.len())
    }
}

enum Parsed {
    Done(usize, Vec<Event>),
    More,
}

fn alt() -> Mods {
    Mods {
        option: true,
        ..Mods::NONE
    }
}

fn flush(text: &mut String, out: &mut Vec<Event>) {
    if !text.is_empty() {
        out.push(Event::Text(std::mem::take(text)));
    }
}

fn utf8_len(b: u8) -> usize {
    match b {
        0xf0..=0xf7 => 4,
        0xe0..=0xef => 3,
        0xc0..=0xdf => 2,
        _ => 1,
    }
}

/// A key of one character with modifiers: letters are lower case, and an
/// upper case letter adds shift.
fn char_key(ch: &str, mut mods: Mods) -> Event {
    let mut c = ch.chars();
    if let (Some(first), None) = (c.next(), c.next()) {
        if first.is_ascii_uppercase() {
            mods.shift = true;
            return key(&first.to_ascii_lowercase().to_string(), mods);
        }
        if first == ' ' {
            return key("space", mods);
        }
    }
    key(ch, mods)
}

/// A C0 control byte as a key.
fn control(b: u8) -> Event {
    let ctl = Mods {
        control: true,
        ..Mods::NONE
    };
    match b {
        0x0d | 0x0a => key("return", Mods::NONE),
        0x09 => key("tab", Mods::NONE),
        0x7f | 0x08 => key("delete", Mods::NONE),
        0x00 => key("space", ctl),
        0x01..=0x1a => key(&((b - 1 + b'a') as char).to_string(), ctl),
        0x1c => key("\\", ctl),
        0x1d => key("]", ctl),
        0x1e => key("6", ctl),
        0x1f => key("-", ctl),
        _ => key("escape", Mods::NONE),
    }
}

/// A key of the kitty keyboard protocol: `CSI code ; mods u`.
fn kitty_key(code: u32, mods: Mods) -> Option<Event> {
    let name = match code {
        13 => "return",
        9 => "tab",
        27 => "escape",
        127 | 8 => "delete",
        32 => "space",
        // The keypad digits.
        57399..=57408 => {
            return Some(key(
                &char::from(b'0' + (code - 57399) as u8).to_string(),
                mods,
            ))
        }
        57414 => "return",
        57441..=57454 => return None, // a modifier key alone
        _ => {
            let c = char::from_u32(code)?;
            if c.is_control() {
                return None;
            }
            return Some(char_key(&c.to_string(), mods));
        }
    };
    Some(key(name, mods))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The length of the longest suffix of `hay` that starts `needle`.
fn partial_suffix(hay: &[u8], needle: &[u8]) -> usize {
    (1..needle.len())
        .rev()
        .find(|&k| hay.len() >= k && hay[hay.len() - k..] == needle[..k])
        .unwrap_or(0)
}
