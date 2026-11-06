//! Delivery through paddock's pty relay, for a TUI with no push path.
//!
//! paddock already relays the terminal of a run through a pty that it owns
//! (src/relay.rs), and a tap of the relay can read the screen and send
//! keys. In horse mode a [`PtyTap`] keeps the screen of the TUI in a
//! `vt100` parser, and the custody of the run types a queued entry into
//! the pty when the TUI is idle:
//!
//! 1. The harness said that its turn is done (a `stop` signal: for Codex
//!    its `notify` program, for Claude Code its `Stop` hook), and no
//!    newer user row is in its file. The custody of each harness
//!    (codex.rs, claude.rs) owns this part.
//! 2. The screen shows the idle prompt row of the profile, with no input
//!    in it (`empty_input`).
//! 3. No row shows the busy text of the profile, and the screen did not
//!    change for `quiet_ms`.
//!
//! All three. A screen alone is never enough. The patterns come from the
//! `[horse.pty]` table of the harness profile, for one pinned version; a
//! profile without it refuses ([`PtyIdle::of`]), and the run falls back
//! to final-only. Nothing reads the screen beyond this test, and screen
//! bytes are never stored.
//!
//! While paddock types an entry, the keys of the user wait: the tap
//! writes them after the submit key, in order. An urgent entry (`paddock
//! horse append --interrupt`) first gets the harness's interrupt key.

pub mod claude;
pub mod codex;

use crate::horse::inbox::Entry;
use crate::horse::store::{Hash, Store};
use crate::relay::{Log, Tap};
use anyhow::{bail, Context as _, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::io;
use std::os::fd::RawFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use codex::{run, CustodyCtx, Report, Timing};

/// `[horse]` of a harness profile.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileHorse {
    /// The idle screen of the TUI, for delivery through the relay.
    pub pty: Option<PtyTable>,
}

/// `[horse.pty]`: how the TUI of one version looks when it is idle.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PtyTable {
    /// The harness version that these patterns were taken from.
    pub version: String,
    /// The input row starts with this text, after its leading blanks.
    pub prompt: String,
    /// The rest of the input row is blank or dim (a placeholder): the
    /// user typed nothing into it.
    #[serde(default)]
    pub empty_input: bool,
    /// A row with one of these texts, compared without case: a turn runs.
    #[serde(default)]
    pub busy: Vec<String>,
    /// The screen must not change for this long.
    #[serde(default = "quiet_default")]
    pub quiet_ms: u64,
    /// The key that stops the running turn (a key name of screens.rs).
    pub interrupt_key: String,
    #[serde(default = "submit_default")]
    pub submit_key: String,
    /// The TUI takes a bracketed paste as one input.
    #[serde(default)]
    pub bracketed_paste: bool,
    /// The wait between the text and the submit key, so the TUI does not
    /// read a fast submit key as part of a paste.
    #[serde(default = "gap_default")]
    pub submit_gap_ms: u64,
}

fn quiet_default() -> u64 {
    1500
}
fn submit_default() -> String {
    "enter".into()
}
fn gap_default() -> u64 {
    200
}

/// The idle test and the keys of one TUI, from its profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyIdle {
    pub prompt: String,
    pub empty_input: bool,
    /// Lower case.
    pub busy: Vec<String>,
    pub quiet: Duration,
    pub interrupt_key: Vec<u8>,
    pub submit_key: Vec<u8>,
    pub bracketed_paste: bool,
    pub submit_gap: Duration,
}

impl PtyIdle {
    /// The patterns of `harness` at `version`, from its profile. Refuses
    /// when the profile has no `[horse.pty]` table for that version.
    pub fn of(profile: &crate::profile::Profile, harness: &str, version: &str) -> Result<PtyIdle> {
        let refuse = || format!("horse: no pty delivery pattern for {harness} {version}");
        let t = profile
            .horse
            .as_ref()
            .and_then(|h| h.pty.as_ref())
            .filter(|t| t.version == version)
            .with_context(refuse)?;
        Self::from_table(t).with_context(refuse)
    }

    pub fn from_table(t: &PtyTable) -> Result<PtyIdle> {
        let key = |name: &str, what: &str| {
            crate::screens::key_bytes(name)
                .with_context(|| format!("[horse.pty] {what} {name:?} is not a key name"))
        };
        if t.prompt.trim().is_empty() {
            bail!("[horse.pty] prompt is empty");
        }
        if t.busy.iter().any(|b| b.trim().is_empty()) {
            bail!("[horse.pty] busy has an empty text");
        }
        Ok(PtyIdle {
            prompt: t.prompt.trim().to_string(),
            empty_input: t.empty_input,
            busy: t.busy.iter().map(|b| b.to_lowercase()).collect(),
            quiet: Duration::from_millis(t.quiet_ms),
            interrupt_key: key(&t.interrupt_key, "interrupt_key")?,
            submit_key: key(&t.submit_key, "submit_key")?,
            bracketed_paste: t.bracketed_paste,
            submit_gap: Duration::from_millis(t.submit_gap_ms),
        })
    }
}

/// One row of the screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    /// The text of the row without its dim cells (a placeholder is dim).
    pub plain: String,
}

/// The screen as the idle test reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub rows: Vec<Row>,
}

impl Screen {
    /// A TUI that draws its own cursor (Claude Code) shows an empty input
    /// row as the first letter of the placeholder in inverse, then the
    /// rest of the placeholder dim. So an inverse cell right before a dim
    /// cell is part of the placeholder, and a blank in `plain`. Text that
    /// the user typed is never dim.
    pub fn of(s: &vt100::Screen) -> Screen {
        let (rows, cols) = (s.size().0.get(), s.size().1.get());
        let rows = (0..rows)
            .map(|r| {
                // (text, dim, inverse) of each cell.
                let cells: Vec<(&str, bool, bool)> = (0..cols)
                    .filter_map(|c| s.cell(r, c))
                    .filter(|cell| !cell.is_wide_continuation())
                    .map(|cell| {
                        let t = if cell.has_contents() {
                            cell.contents()
                        } else {
                            " "
                        };
                        (t, cell.dim(), cell.inverse())
                    })
                    .collect();
                let mut row = Row::default();
                for (i, (t, dim, inverse)) in cells.iter().enumerate() {
                    row.text.push_str(t);
                    let next_dim = cells
                        .get(i + 1)
                        .is_some_and(|(n, d, _)| *d && !n.trim().is_empty());
                    if *dim || (*inverse && next_dim) {
                        row.plain
                            .extend(std::iter::repeat_n(' ', t.chars().count()));
                    } else {
                        row.plain.push_str(t);
                    }
                }
                row
            })
            .collect();
        Screen { rows }
    }

    /// The screen after `bytes`, for tests and fixtures.
    pub fn from_bytes(rows: u16, cols: u16, bytes: &[u8]) -> Screen {
        let mut p = parser(rows, cols);
        p.process(bytes);
        Screen::of(p.screen())
    }
}

fn parser(rows: u16, cols: u16) -> vt100::Parser {
    let nz = |v: u16, d: u16| {
        std::num::NonZeroU16::new(v)
            .or(std::num::NonZeroU16::new(d))
            .unwrap_or(std::num::NonZeroU16::MIN)
    };
    vt100::Parser::new(nz(rows, 24), nz(cols, 80), 0)
}

/// The screen part of the idle test: the prompt row with no input, no
/// busy row, and no change for `quiet`. The caller adds the signal part.
pub fn is_idle(screen: &Screen, p: &PtyIdle, unchanged: Duration) -> bool {
    if unchanged < p.quiet {
        return false;
    }
    let busy = screen.rows.iter().any(|r| {
        let low = r.text.to_lowercase();
        p.busy.iter().any(|b| low.contains(b.as_str()))
    });
    if busy {
        return false;
    }
    screen.rows.iter().any(|r| {
        let lead = r.text.len() - r.text.trim_start().len();
        if !r.text[lead..].starts_with(&p.prompt) {
            return false;
        }
        if !p.empty_input {
            return true;
        }
        // `text` and `plain` have the same characters but for the dim
        // ones, which are blanks in `plain`.
        let skip = r.text[..lead].chars().count() + p.prompt.chars().count();
        r.plain.chars().skip(skip).all(char::is_whitespace)
    })
}

/// The text as it can go into a terminal: no control characters but the
/// newline and the tab, so an entry can never send a key of its own (an
/// escape sequence that ends a bracketed paste, a submit). A `\r\n` or a
/// lone `\r` is a newline.
pub fn terminal_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.chars()
        .filter(|c| *c == '\n' || *c == '\t' || !(c.is_control()))
        .collect::<String>()
        .trim_end()
        .to_string()
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// The writes that type `text` and submit it: the text (one bracketed
/// paste, or line by line with Ctrl-J between the lines), then the submit
/// key. A wait of `submit_gap` goes between two writes.
pub fn keys(text: &str, p: &PtyIdle) -> Vec<Vec<u8>> {
    let text = terminal_text(text);
    let mut out = Vec::new();
    if p.bracketed_paste {
        let mut b = PASTE_START.to_vec();
        b.extend_from_slice(text.as_bytes());
        b.extend_from_slice(PASTE_END);
        out.push(b);
    } else {
        for (i, line) in text.split('\n').enumerate() {
            let mut b = Vec::new();
            if i > 0 {
                b.push(b'\n');
            }
            b.extend_from_slice(line.as_bytes());
            out.push(b);
        }
    }
    out.push(p.submit_key.clone());
    out
}

type Writer = Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

struct ScreenState {
    parser: Option<vt100::Parser>,
    changed: Instant,
    caller: RawFd,
}

/// The tap of the relay for horse mode: it keeps the screen, and it is the
/// one writer to the pty for the keys of the user and of paddock.
pub struct PtyTap {
    screen: Mutex<ScreenState>,
    /// `None` before `attach` and after `finish`.
    write: Mutex<Option<Writer>>,
}

impl PtyTap {
    pub fn new() -> Arc<PtyTap> {
        Arc::new(PtyTap {
            screen: Mutex::new(ScreenState {
                parser: None,
                changed: Instant::now(),
                caller: -1,
            }),
            write: Mutex::new(None),
        })
    }

    /// Attach to a writer of the pty and a screen of `rows` x `cols`
    /// without a terminal, for tests.
    pub fn attach_with(
        &self,
        rows: u16,
        cols: u16,
        w: impl FnMut(&[u8]) -> io::Result<()> + Send + 'static,
    ) {
        let mut st = self.screen.lock().unwrap();
        st.parser = Some(parser(rows, cols));
        st.changed = Instant::now();
        drop(st);
        *self.write.lock().unwrap() = Some(Box::new(w));
    }

    /// The screen now, and how long it is unchanged. `None` before the
    /// relay attached the tap (the run has no terminal).
    pub fn screen(&self) -> Option<(Screen, Duration)> {
        let st = self.screen.lock().unwrap();
        let p = st.parser.as_ref()?;
        Some((Screen::of(p.screen()), st.changed.elapsed()))
    }

    /// The tap can write to the pty.
    pub fn attached(&self) -> bool {
        self.write.lock().unwrap().is_some()
    }

    /// Type `text` and submit it. The keys of the user wait until the
    /// submit key went out.
    pub fn deliver(&self, text: &str, p: &PtyIdle) -> io::Result<()> {
        self.send(&keys(text, p), p.submit_gap)
    }

    /// Send the interrupt key.
    pub fn interrupt(&self, p: &PtyIdle) -> io::Result<()> {
        self.send(std::slice::from_ref(&p.interrupt_key), Duration::ZERO)
    }

    fn send(&self, writes: &[Vec<u8>], gap: Duration) -> io::Result<()> {
        let mut w = self.write.lock().unwrap();
        let w = w
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "the relay is gone"))?;
        for (i, b) in writes.iter().enumerate() {
            if i > 0 && !gap.is_zero() {
                std::thread::sleep(gap);
            }
            w(b)?;
        }
        Ok(())
    }

    /// Feed harness output without a relay, for tests.
    pub fn output_bytes(&self, data: &[u8]) {
        Tap::output(self, data);
    }
}

impl Tap for PtyTap {
    fn attach(&self, caller: RawFd, master: RawFd, _log: Log) {
        let (rows, cols) = winsize(caller).unwrap_or((24, 80));
        let mut st = self.screen.lock().unwrap();
        st.parser = Some(parser(rows, cols));
        st.changed = Instant::now();
        st.caller = caller;
        drop(st);
        *self.write.lock().unwrap() = Some(Box::new(move |b: &[u8]| {
            crate::relay::write_all_fd(master, b)
        }));
    }

    fn output(&self, data: &[u8]) {
        let mut st = self.screen.lock().unwrap();
        if let Some(p) = st.parser.as_mut() {
            let before = (p.screen().contents(), p.screen().cursor_position());
            p.process(data);
            // Output that leaves the text as it was (a cursor that blinks,
            // a title, a color) is no change.
            if (p.screen().contents(), p.screen().cursor_position()) != before {
                st.changed = Instant::now();
            }
        }
    }

    fn write_user(&self, master: RawFd, data: &[u8]) -> io::Result<()> {
        let mut w = self.write.lock().unwrap();
        match w.as_mut() {
            Some(w) => w(data),
            None => crate::relay::write_all_fd(master, data),
        }
    }

    fn resize(&self) {
        let mut st = self.screen.lock().unwrap();
        let size = winsize(st.caller).and_then(|(r, c)| {
            Some((std::num::NonZeroU16::new(r)?, std::num::NonZeroU16::new(c)?))
        });
        if let (Some((rows, cols)), Some(p)) = (size, st.parser.as_mut()) {
            p.screen_mut().set_size(rows, cols);
            st.changed = Instant::now();
        }
    }

    fn finish(&self) {
        *self.write.lock().unwrap() = None;
    }
}

/// Write the user's keys through `tap` while another tap decides on them
/// (relay/tap.rs `Taps`): paddock's typing and the user's keys never mix.
pub fn serialized(tap: &PtyTap, f: &mut dyn FnMut() -> io::Result<()>) -> io::Result<()> {
    let _w = tap.write.lock().unwrap();
    f()
}

/// Record entry `e` as delivered, then type it into the TUI. The text that
/// went out and the session head at that time. `None` when the relay is
/// gone: the record is taken back, and the entry stays queued.
///
/// The record comes first: a recorded entry never goes out twice, also
/// after a crash between the record and the keys.
pub(crate) fn type_entry(
    store: &Store,
    tap: &PtyTap,
    idle: &PtyIdle,
    e: &Entry,
) -> Result<Option<(String, Option<Hash>)>> {
    let text = terminal_text(&e.wire_text());
    let head = store.session(&e.session)?.head;
    store.record_delivery(e, head.as_ref(), &e.id)?;
    if let Err(err) = tap.deliver(&text, idle) {
        log::warn!(
            "horse pty: cannot type entry {} into the TUI ({err}); it stays queued",
            e.id
        );
        store.retract_delivery(&e.session, &e.id)?;
        return Ok(None);
    }
    Ok(Some((text, head)))
}

/// The oldest urgent entry (`paddock horse append --interrupt`) of
/// `session` that has no delivery record and is not in `sent`: the
/// entries that got their interrupt key already.
pub(crate) fn urgent_entry(
    store: &Store,
    session: &str,
    sent: &HashSet<String>,
) -> Result<Option<Entry>> {
    let done: HashSet<String> = store
        .deliveries(session)?
        .into_iter()
        .map(|d| d.entry.id)
        .collect();
    Ok(store
        .inbox(session)?
        .into_iter()
        .find(|e| e.interrupt && !done.contains(&e.id) && !sent.contains(&e.id)))
}

fn winsize(fd: RawFd) -> Option<(u16, u16)> {
    if fd < 0 {
        return None;
    }
    // SAFETY: TIOCGWINSZ writes into a local winsize.
    unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) < 0 {
            return None;
        }
        Some((size.ws_row, size.ws_col))
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod relay_tests;

#[cfg(test)]
mod codex_relay_tests;
