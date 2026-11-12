//! From terminal events to guest actions. Keys go to the target as keys
//! or text; mouse reports become clicks (with the click count of a
//! double click), drags and scrolls in frame coordinates. One key,
//! [`MENU_KEY`], opens the shortcut menu: there the next key gets Cmd,
//! so the Cmd combinations that the terminal keeps for itself still
//! reach the Mac. Pure: time comes in as milliseconds.

use super::fit::Placement;
use super::keys::{Button, Event, Mods, Mouse, MouseKind};

/// The key that opens the menu: Ctrl-].
pub const MENU_KEY: &str = "]";
/// Two clicks closer than this, in time and in frame pixels, are a
/// double click.
pub const DOUBLE_MS: u64 = 400;
pub const DOUBLE_PX: u32 = 6;
/// A press and release farther apart than this, in frame pixels, is a drag.
pub const DRAG_PX: u32 = 4;
/// The longest text of one `type` action.
pub const MAX_TYPE: usize = 4096;

/// A mouse button of the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestButton {
    Left,
    Right,
    Middle,
}

impl GuestButton {
    pub fn name(&self) -> &'static str {
        match self {
            GuestButton::Left => "left",
            GuestButton::Right => "right",
            GuestButton::Middle => "middle",
        }
    }
}

/// One action on the target. Points are pixels of the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Type(String),
    Key {
        key: String,
        mods: Mods,
    },
    Click {
        x: u32,
        y: u32,
        button: GuestButton,
        count: u8,
        mods: Mods,
    },
    Drag {
        from: (u32, u32),
        to: (u32, u32),
        button: GuestButton,
        mods: Mods,
    },
    /// Lines; `dy > 0` scrolls toward the end, `dx > 0` to the right.
    Scroll {
        x: u32,
        y: u32,
        dx: i32,
        dy: i32,
    },
}

/// What the view does after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Act(Action),
    MenuOpen,
    MenuClose,
    /// Draw the frame again.
    Redraw,
    /// Leave the view; the session keeps running.
    Leave,
}

#[derive(Debug, Clone, Copy)]
struct Press {
    at: (u32, u32),
    button: GuestButton,
    mods: Mods,
    moved: bool,
    last: (u32, u32),
}

/// The mapper: the menu state, the held button and the last click.
#[derive(Debug, Default)]
pub struct Mapper {
    menu: bool,
    press: Option<Press>,
    last_click: Option<(u64, (u32, u32), GuestButton, u8)>,
}

fn ctl(mods: &Mods) -> bool {
    mods.control && !mods.option && !mods.cmd && !mods.shift
}

impl Mapper {
    pub fn menu_open(&self) -> bool {
        self.menu
    }

    /// Map one event at time `now_ms`, with the frame at `place`.
    pub fn map(
        &mut self,
        ev: Event,
        place: Option<&Placement>,
        pixels: bool,
        now_ms: u64,
    ) -> Vec<Out> {
        if self.menu {
            return self.in_menu(ev);
        }
        match ev {
            Event::Key { key, mods } if key == MENU_KEY && ctl(&mods) => {
                self.menu = true;
                vec![Out::MenuOpen]
            }
            Event::Key { key, mods } => vec![Out::Act(Action::Key { key, mods })],
            Event::Text(t) | Event::Paste(t) => split_text(&t),
            Event::Mouse(m) => match place {
                Some(p) => self.mouse(m, p, pixels, now_ms),
                None => vec![],
            },
            Event::Focus(_) => vec![],
        }
    }

    fn in_menu(&mut self, ev: Event) -> Vec<Out> {
        let close = |mut v: Vec<Out>| {
            v.push(Out::MenuClose);
            v
        };
        let out = match ev {
            Event::Key { key, mods } if mods.is_empty() && key == "escape" => vec![],
            Event::Key { key, mods } if ctl(&mods) && (key == "c" || key == "d") => {
                self.menu = false;
                return vec![Out::Leave];
            }
            Event::Key { key, mods } if ctl(&mods) && key == "l" => vec![Out::Redraw],
            Event::Key { key, mods } if ctl(&mods) && key == MENU_KEY => {
                vec![Out::Act(Action::Key { key, mods })]
            }
            Event::Key { key, mods } if ctl(&mods) && key == "e" => vec![Out::Act(Action::Key {
                key: "escape".into(),
                mods: Mods {
                    option: true,
                    cmd: true,
                    ..Mods::NONE
                },
            })],
            Event::Key { key, mut mods } => {
                mods.cmd = true;
                vec![Out::Act(Action::Key { key, mods })]
            }
            Event::Text(t) => {
                let Some(c) = t.chars().next() else {
                    return vec![];
                };
                let mut mods = Mods {
                    cmd: true,
                    ..Mods::NONE
                };
                let key = if c == ' ' {
                    "space".to_string()
                } else if c.is_ascii_uppercase() {
                    mods.shift = true;
                    c.to_ascii_lowercase().to_string()
                } else {
                    c.to_string()
                };
                vec![Out::Act(Action::Key { key, mods })]
            }
            // A paste, the mouse and the focus do not choose an entry.
            _ => return vec![],
        };
        self.menu = false;
        close(out)
    }

    fn mouse(&mut self, m: Mouse, p: &Placement, pixels: bool, now: u64) -> Vec<Out> {
        let at = if pixels {
            p.point_px(m.x, m.y)
        } else {
            p.point_cell(m.x, m.y)
        };
        let button = match m.button {
            Button::Left => Some(GuestButton::Left),
            Button::Right => Some(GuestButton::Right),
            Button::Middle => Some(GuestButton::Middle),
            _ => None,
        };
        match (m.kind, m.button) {
            (
                MouseKind::Press,
                Button::WheelUp | Button::WheelDown | Button::WheelLeft | Button::WheelRight,
            ) => {
                let Some((x, y)) = at else { return vec![] };
                let (mut dx, mut dy) = match m.button {
                    Button::WheelUp => (0, -1),
                    Button::WheelDown => (0, 1),
                    Button::WheelLeft => (-1, 0),
                    _ => (1, 0),
                };
                if m.mods.shift {
                    // Shift turns the wheel sideways, as on the Mac.
                    std::mem::swap(&mut dx, &mut dy);
                }
                vec![Out::Act(Action::Scroll { x, y, dx, dy })]
            }
            (MouseKind::Press, _) => {
                if let (Some(at), Some(button)) = (at, button) {
                    self.press = Some(Press {
                        at,
                        button,
                        mods: m.mods,
                        moved: false,
                        last: at,
                    });
                }
                vec![]
            }
            (MouseKind::Motion, _) => {
                if let (Some(pr), Some(at)) = (self.press.as_mut(), at) {
                    pr.last = at;
                    if far(pr.at, at, DRAG_PX) {
                        pr.moved = true;
                    }
                }
                vec![]
            }
            (MouseKind::Release, _) => {
                let Some(pr) = self.press.take() else {
                    return vec![];
                };
                // A release outside the frame ends at the last point inside.
                let to = at.unwrap_or(pr.last);
                if pr.moved || far(pr.at, to, DRAG_PX) {
                    self.last_click = None;
                    return vec![Out::Act(Action::Drag {
                        from: pr.at,
                        to,
                        button: pr.button,
                        mods: pr.mods,
                    })];
                }
                let count = match self.last_click {
                    Some((t, p0, b, n))
                        if b == pr.button
                            && now.saturating_sub(t) <= DOUBLE_MS
                            && !far(p0, pr.at, DOUBLE_PX) =>
                    {
                        if n >= 3 {
                            1
                        } else {
                            n + 1
                        }
                    }
                    _ => 1,
                };
                self.last_click = Some((now, pr.at, pr.button, count));
                vec![Out::Act(Action::Click {
                    x: pr.at.0,
                    y: pr.at.1,
                    button: pr.button,
                    count,
                    mods: pr.mods,
                })]
            }
        }
    }
}

fn far(a: (u32, u32), b: (u32, u32), d: u32) -> bool {
    a.0.abs_diff(b.0) > d || a.1.abs_diff(b.1) > d
}

/// Text as `type` actions of at most [`MAX_TYPE`] bytes, split on
/// character boundaries. A newline in a paste is a Return key.
fn split_text(t: &str) -> Vec<Out> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let push = |cur: &mut String, out: &mut Vec<Out>| {
        if !cur.is_empty() {
            out.push(Out::Act(Action::Type(std::mem::take(cur))));
        }
    };
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            push(&mut cur, &mut out);
            out.push(Out::Act(Action::Key {
                key: "return".into(),
                mods: Mods::NONE,
            }));
            continue;
        }
        if c.is_control() && c != '\t' {
            continue;
        }
        if cur.len() + c.len_utf8() > MAX_TYPE {
            push(&mut cur, &mut out);
        }
        cur.push(c);
    }
    push(&mut cur, &mut out);
    out
}
