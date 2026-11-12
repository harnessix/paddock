//! Which image protocol the terminal speaks. The view asks the terminal
//! (the kitty graphics query, the primary device attributes for sixel,
//! the kitty keyboard flags, the cell size, the SGR-pixel mouse mode)
//! and reads the environment (iTerm2 has no query). Inside tmux the
//! image escapes need `allow-passthrough`: the view uses it when tmux
//! has it, and says how to turn it on when it does not. The decision is
//! pure; `run.rs` does the I/O.

use super::encode;
use crate::macvm::text;

/// An image protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Kitty,
    Iterm2,
    Sixel,
}

impl Protocol {
    pub fn name(&self) -> &'static str {
        match self {
            Protocol::Kitty => "kitty",
            Protocol::Iterm2 => "iterm2",
            Protocol::Sixel => "sixel",
        }
    }

    pub fn parse(s: &str) -> Result<Option<Protocol>, String> {
        match s {
            "auto" => Ok(None),
            "kitty" => Ok(Some(Protocol::Kitty)),
            "iterm2" => Ok(Some(Protocol::Iterm2)),
            "sixel" => Ok(Some(Protocol::Sixel)),
            _ => Err(text::view_unknown_protocol(s)),
        }
    }
}

/// The environment variables that name a terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub term: String,
    pub term_program: String,
    pub lc_terminal: String,
    /// `TMUX` is set.
    pub tmux: bool,
    /// `STY` is set: GNU screen.
    pub screen: bool,
}

impl Env {
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Env {
        let v = |k: &str| get(k).unwrap_or_default();
        Env {
            term: v("TERM"),
            term_program: v("TERM_PROGRAM"),
            lc_terminal: v("LC_TERMINAL"),
            tmux: get("TMUX").is_some_and(|s| !s.is_empty()),
            screen: get("STY").is_some_and(|s| !s.is_empty()),
        }
    }
}

/// What tmux says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tmux {
    /// `#{version}`, for example `3.4`.
    pub version: String,
    /// `allow-passthrough`: `on`, `all` or `off` (empty: tmux older than
    /// 3.3, which passes everything through).
    pub passthrough: String,
    /// `#{client_termname}` and `#{client_termtype}` of the attached client.
    pub client: String,
}

impl Tmux {
    /// The escapes reach the outer terminal.
    pub fn passes(&self) -> bool {
        match self.passthrough.as_str() {
            "on" | "all" => true,
            "" => !version_at_least(&self.version, 3, 3),
            _ => false,
        }
    }
}

fn version_at_least(v: &str, major: u32, minor: u32) -> bool {
    let digits: String = v
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut it = digits.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    (a, b) >= (major, minor)
}

/// The answers of the terminal to the probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replies {
    pub kitty: bool,
    /// The terminal answered the kitty query at all, OK or an error.
    pub kitty_answered: bool,
    /// The parameters of the primary device attributes; 4 is sixel.
    pub da1: Option<Vec<u32>>,
    /// The cell size in pixels (`CSI 16 t`).
    pub cell: Option<(u16, u16)>,
    /// The kitty keyboard protocol answered `CSI ? flags u`.
    pub kitty_keys: bool,
    /// The SGR-pixel mouse mode (1016) is known to the terminal.
    pub mouse_pixels: bool,
    /// Bytes that were not a reply: keys typed during the probe.
    pub rest: Vec<u8>,
}

/// The queries of the probe. The device attributes come last: every
/// terminal answers them in order, so outside tmux their answer ends the
/// wait. In tmux the kitty query goes through passthrough to the outer
/// terminal, and tmux answers the rest itself at once, so the answer of
/// tmux comes before the kitty reply: see [`probe_complete`].
pub fn probe(wrap: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if wrap {
        out.extend(encode::tmux_wrap(encode::KITTY_QUERY));
    } else {
        out.extend_from_slice(encode::KITTY_QUERY);
    }
    out.extend_from_slice(b"\x1b[?u\x1b[16t\x1b[?1016$p\x1b[c");
    out
}

/// Parse the replies in `buf`. Returns them, and true once the device
/// attributes arrived. Outside tmux that ends the probe; in tmux the
/// probe also waits for the kitty reply ([`probe_complete`]).
pub fn parse_replies(buf: &[u8]) -> (Replies, bool) {
    let mut r = Replies::default();
    let mut done = false;
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != 0x1b || i + 1 >= buf.len() {
            r.rest.push(buf[i]);
            i += 1;
            continue;
        }
        match buf[i + 1] {
            b'_' => {
                // APC ... ST
                let Some(end) = find(&buf[i..], b"\x1b\\") else {
                    break;
                };
                let body = &buf[i + 2..i + end];
                if body.starts_with(b"G") && contains(body, b"i=31") {
                    r.kitty_answered = true;
                    if body.ends_with(b";OK") {
                        r.kitty = true;
                    }
                }
                i += end + 2;
            }
            b'[' => {
                let mut j = i + 2;
                while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                    j += 1;
                }
                if j >= buf.len() {
                    break;
                }
                let body = &buf[i + 2..j];
                let fin = buf[j];
                let text = String::from_utf8_lossy(body);
                match (fin, text.strip_prefix('?')) {
                    (b'c', Some(p)) => {
                        r.da1 = Some(p.split(';').filter_map(|x| x.parse().ok()).collect());
                        done = true;
                    }
                    (b'u', Some(_)) => r.kitty_keys = true,
                    (b'y', Some(p)) if p.starts_with("1016;") => {
                        let v = p.trim_start_matches("1016;").trim_end_matches('$');
                        r.mouse_pixels = matches!(v, "1" | "2" | "3" | "4");
                    }
                    (b't', None) => {
                        let n: Vec<u16> = text.split(';').filter_map(|x| x.parse().ok()).collect();
                        if n.len() == 3 && n[0] == 6 && n[1] > 0 && n[2] > 0 {
                            r.cell = Some((n[2], n[1]));
                        }
                    }
                    _ => r.rest.extend_from_slice(&buf[i..=j]),
                }
                i = j + 1;
            }
            _ => {
                r.rest.push(buf[i]);
                i += 1;
            }
        }
    }
    (r, done)
}

/// The probe has all its answers. Outside tmux the device attributes
/// come last. In tmux, tmux answers them itself before the outer terminal
/// answers the wrapped kitty query, so the probe also waits for the kitty
/// reply. A terminal without the kitty protocol never answers it: then
/// the probe ends at its deadline.
pub fn probe_complete(r: &Replies, wrap: bool) -> bool {
    r.da1.is_some() && (!wrap || r.kitty_answered)
}

/// Read the replies to [`probe`] with `read` (one read of at most the
/// given time, empty when nothing came) until [`probe_complete`] or
/// until `wait` passed.
pub fn collect_replies(
    wrap: bool,
    wait: std::time::Duration,
    mut read: impl FnMut(std::time::Duration) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Replies> {
    let start = std::time::Instant::now();
    let mut buf = Vec::new();
    while start.elapsed() < wait {
        let left = wait.saturating_sub(start.elapsed());
        buf.extend(read(left.min(std::time::Duration::from_millis(20)))?);
        if probe_complete(&parse_replies(&buf).0, wrap) {
            break;
        }
    }
    Ok(parse_replies(&buf).0)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle).is_some()
}

/// The decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// None: the terminal cannot show images.
    pub protocol: Option<Protocol>,
    /// Wrap image escapes for tmux passthrough.
    pub wrap: bool,
    /// Lines for the user: why a protocol was or was not chosen.
    pub notes: Vec<String>,
}

fn iterm_named(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("iterm") || s.contains("wezterm")
}

/// Choose the protocol. `forced` is `--protocol`.
pub fn choose(env: &Env, tmux: Option<&Tmux>, r: &Replies, forced: Option<Protocol>) -> Choice {
    let mut notes = Vec::new();
    let sixel = r.da1.as_ref().is_some_and(|p| p.contains(&4));
    let iterm = iterm_named(&env.term_program)
        || iterm_named(&env.lc_terminal)
        || tmux.is_some_and(|t| iterm_named(&t.client));
    if env.screen && !env.tmux {
        notes.push(
            "GNU screen does not pass image escapes to the terminal. Run the view outside screen, or in tmux 3.3 or later."
                .into(),
        );
        return Choice {
            protocol: forced.filter(|p| *p == Protocol::Sixel && sixel),
            wrap: false,
            notes,
        };
    }
    let passes = tmux.is_none_or(|t| t.passes());
    let in_tmux = tmux.is_some();
    if in_tmux && !passes {
        notes.push(
            "tmux does not pass image escapes to the terminal (allow-passthrough is off). Turn it on with: tmux set -g allow-passthrough on"
                .into(),
        );
    }
    if let Some(p) = forced {
        // The user knows the terminal; only tmux can still stop it.
        if in_tmux && !passes && p != Protocol::Sixel {
            return Choice {
                protocol: None,
                wrap: false,
                notes,
            };
        }
        return Choice {
            protocol: Some(p),
            wrap: in_tmux && p != Protocol::Sixel,
            notes,
        };
    }
    let protocol = if r.kitty && passes {
        Some(Protocol::Kitty)
    } else if iterm && passes {
        Some(Protocol::Iterm2)
    } else if sixel {
        // In tmux, a sixel answer comes from tmux itself, which draws
        // sixel images (tmux 3.4 built with sixel), so no wrap.
        Some(Protocol::Sixel)
    } else {
        None
    };
    if protocol.is_none() && !(in_tmux && !passes) {
        notes.push(text::view_no_protocol(&describe(env, tmux)));
    }
    Choice {
        protocol,
        wrap: in_tmux && matches!(protocol, Some(Protocol::Kitty | Protocol::Iterm2)),
        notes,
    }
}

/// A short name of the terminal for the notes.
pub fn describe(env: &Env, tmux: Option<&Tmux>) -> String {
    let base = [&env.term_program, &env.lc_terminal, &env.term]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "unknown".into());
    match tmux {
        Some(t) if !t.client.trim().is_empty() => {
            text::view_tmux(&t.version, Some(t.client.trim()))
        }
        Some(t) => text::view_tmux(&t.version, None),
        None => base,
    }
}
