//! The view without the terminal: frames in, bytes out; terminal bytes
//! in, actions out. `run.rs` connects it to a real terminal, the tests to
//! a buffer. Every text that comes from the guest passes [`clean`] before
//! it reaches the terminal.

use super::diff::{Gate, Verdict};
use super::encode;
use super::fit::{self, Placement, Term};
use super::keys::{Event, Parser};
use super::map::{Action, Mapper, Out};
use super::png;
use super::sixel;
use super::source::{Frame, Max};
use super::term::Protocol;
use crate::macvm::text;

/// The view state.
pub struct Screen {
    pub protocol: Protocol,
    /// Wrap image escapes for tmux passthrough.
    pub wrap: bool,
    pub term: Term,
    pub label: String,
    pub mouse_pixels: bool,
    pub kitty_keys: bool,
    gate: Gate,
    mapper: Mapper,
    parser: Parser,
    last: Option<Frame>,
    place: Option<Placement>,
    /// The kitty id on the screen, an index into `KITTY_IDS`.
    kitty_slot: Option<usize>,
    /// The line of the status bar after the label.
    pub note: String,
    pub drawn: u64,
}

/// What one call asks of the caller.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub bytes: Vec<u8>,
    pub actions: Vec<Action>,
    pub leave: bool,
}

/// Printable text of at most `max` characters: control characters
/// (escape sequences of a hostile guest included) become spaces.
pub fn clean(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

impl Screen {
    pub fn new(protocol: Protocol, wrap: bool, term: Term, label: &str) -> Screen {
        Screen {
            protocol,
            wrap,
            term,
            label: clean(label, 80),
            mouse_pixels: false,
            kitty_keys: false,
            gate: Gate::default(),
            mapper: Mapper::default(),
            parser: Parser::new(false),
            last: None,
            place: None,
            kitty_slot: None,
            note: String::new(),
            drawn: 0,
        }
    }

    /// The frames not drawn because they did not change.
    pub fn skipped(&self) -> u64 {
        self.gate.skipped
    }

    /// The `max` for `frames.subscribe`: the room of the terminal now.
    pub fn max(&self) -> Max {
        let (width, height) = self.term.room();
        Max {
            width: width.max(1),
            height: height.max(1),
        }
    }

    /// A new subscription started: its `seq` starts at 1 again.
    pub fn subscribed(&mut self) {
        self.gate.new_stream();
    }

    pub fn placement(&self) -> Option<&Placement> {
        self.place.as_ref()
    }

    fn image(&self, seq: &[u8], out: &mut Vec<u8>) {
        if self.wrap {
            out.extend(encode::tmux_wrap(seq));
        } else {
            out.extend_from_slice(seq);
        }
    }

    /// The bytes that start the view: the alternate screen, no cursor,
    /// mouse reports, bracketed paste and, when known, kitty keys.
    pub fn enter(&mut self) -> Vec<u8> {
        self.parser = Parser::new(self.mouse_pixels);
        let mut out =
            b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?2004h".to_vec();
        if self.mouse_pixels {
            out.extend_from_slice(b"\x1b[?1016h");
        }
        if self.kitty_keys {
            out.extend_from_slice(b"\x1b[>1u");
        }
        out.extend(self.status());
        out
    }

    /// The bytes that end the view and give the terminal back as it was.
    pub fn leave(&mut self) -> Vec<u8> {
        let mut out = self.delete_images();
        if self.kitty_keys {
            out.extend_from_slice(b"\x1b[<u");
        }
        if self.mouse_pixels {
            out.extend_from_slice(b"\x1b[?1016l");
        }
        out.extend_from_slice(
            b"\x1b[?2004l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[0m\x1b[2J\x1b[?25h\x1b[?1049l",
        );
        out
    }

    fn delete_images(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.protocol == Protocol::Kitty && self.kitty_slot.take().is_some() {
            for id in encode::KITTY_IDS {
                self.image(&encode::kitty_delete(id), &mut out);
            }
        }
        out
    }

    /// The status line at the bottom row.
    pub fn status(&self) -> Vec<u8> {
        let size = self
            .last
            .as_ref()
            .map(|f| text::view_status_size(f.width, f.height))
            .unwrap_or_default();
        let mut line = text::view_status_line(&self.label, &size, self.protocol.name());
        if !self.note.is_empty() {
            line.push_str(&text::view_status_note(&clean(&self.note, 200)));
        }
        let line: String = line.chars().take(self.term.cols as usize).collect();
        format!("\x1b[{};1H\x1b[2K\x1b[7m{line}\x1b[0m", self.term.rows).into_bytes()
    }

    /// A new frame from the source.
    pub fn frame(&mut self, f: Frame) -> Vec<u8> {
        if let Err(e) = f.check() {
            self.note = text::view_frame_refused(&e);
            return self.status();
        }
        let first = self.last.is_none();
        let size_changed = self
            .last
            .as_ref()
            .is_some_and(|l| (l.width, l.height) != (f.width, f.height));
        self.last = Some(f);
        if first || size_changed {
            self.gate.invalidate();
        }
        let mut out = self.draw(false);
        if first || size_changed {
            out.extend(self.status());
        }
        out
    }

    /// Draw the last frame, if it changed or `force`.
    fn draw(&mut self, force: bool) -> Vec<u8> {
        if self.mapper.menu_open() {
            return Vec::new();
        }
        let Some(f) = self.last.as_ref() else {
            return Vec::new();
        };
        let Some(place) = fit::place(&self.term, f.width, f.height) else {
            return Vec::new();
        };
        if force {
            self.gate.invalidate();
        }
        if self.gate.check(f.seq, &f.png, &place) != Verdict::Draw {
            return Vec::new();
        }
        let moved = self.place.is_some_and(|p| p != place);
        self.place = Some(place);
        let mut out = Vec::new();
        if moved && self.protocol != Protocol::Kitty {
            // The old image stays in the cells: clear them first.
            out.extend_from_slice(b"\x1b[2J");
            out.extend(self.status());
        }
        out.extend(encode::cursor_to(&place.cells));
        match self.protocol {
            Protocol::Kitty => {
                let slot = self.kitty_slot.map_or(0, |s| 1 - s);
                for esc in encode::kitty(&f.png, encode::KITTY_IDS[slot], &place.cells) {
                    self.image(&esc, &mut out);
                }
                if let Some(old) = self.kitty_slot.replace(slot) {
                    self.image(&encode::kitty_delete(encode::KITTY_IDS[old]), &mut out);
                }
            }
            Protocol::Iterm2 => {
                let esc = encode::iterm(&f.png, &place.cells);
                self.image(&esc, &mut out);
            }
            Protocol::Sixel => match png::decode(&f.png) {
                Ok(img) => {
                    let (w, h) = place.px();
                    out.extend(sixel::encode(&sixel::scale(&img, w, h)));
                }
                Err(e) => {
                    self.note = text::view_frame_refused(&e);
                    out.extend(self.status());
                    return out;
                }
            },
        }
        self.drawn += 1;
        out
    }

    /// The terminal changed size.
    pub fn resize(&mut self, term: Term) -> Vec<u8> {
        if term == self.term {
            return Vec::new();
        }
        self.term = term;
        let mut out = self.delete_images();
        out.extend_from_slice(b"\x1b[2J");
        self.gate.cleared();
        self.place = None;
        if self.mapper.menu_open() {
            out.extend(self.menu());
            return out;
        }
        out.extend(self.draw(true));
        out.extend(self.status());
        out
    }

    /// Bytes from the terminal at time `now_ms`.
    pub fn input(&mut self, bytes: &[u8], now_ms: u64) -> Step {
        let events = self.parser.feed(bytes);
        self.events(events, now_ms)
    }

    /// The input went quiet: a lone ESC is the Escape key.
    pub fn idle(&mut self, now_ms: u64) -> Step {
        let events = self.parser.idle();
        self.events(events, now_ms)
    }

    pub fn pending(&self) -> bool {
        self.parser.pending()
    }

    fn events(&mut self, events: Vec<Event>, now_ms: u64) -> Step {
        let mut step = Step::default();
        for ev in events {
            let outs = self
                .mapper
                .map(ev, self.place.as_ref(), self.mouse_pixels, now_ms);
            for o in outs {
                match o {
                    Out::Act(a) => step.actions.push(a),
                    Out::MenuOpen => {
                        let mut b = self.delete_images();
                        b.extend_from_slice(b"\x1b[2J");
                        b.extend(self.menu());
                        self.gate.cleared();
                        step.bytes.extend(b);
                    }
                    Out::MenuClose => {
                        step.bytes.extend_from_slice(b"\x1b[2J");
                        self.gate.cleared();
                        let d = self.draw(true);
                        step.bytes.extend(d);
                        step.bytes.extend(self.status());
                    }
                    Out::Redraw => {
                        self.gate.cleared();
                    }
                    Out::Leave => {
                        step.leave = true;
                        return step;
                    }
                }
            }
        }
        step
    }

    /// An action failed in the guest: say so in the status line.
    pub fn action_failed(&mut self, e: &str) -> Vec<u8> {
        self.say(text::view_action_failed(e))
    }

    /// Put `note` in the status line.
    pub fn say(&mut self, note: String) -> Vec<u8> {
        self.note = note;
        self.status()
    }

    fn menu(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, line) in text::view_menu(&self.label).iter().enumerate() {
            if i + 1 >= self.term.rows as usize {
                break;
            }
            let line: String = line.chars().take(self.term.cols as usize).collect();
            out.extend(format!("\x1b[{};1H\x1b[2K{line}", i + 1).into_bytes());
        }
        out
    }
}
