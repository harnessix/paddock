//! The escape sequences that put a frame into the terminal: the kitty
//! graphics protocol, iTerm2 inline images, and the tmux passthrough
//! around both. Pure byte builders with golden tests. Sixel is in
//! `sixel.rs`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

/// The base64 bytes of one kitty chunk (the protocol allows 4096).
pub const KITTY_CHUNK: usize = 4096;
/// The kitty image ids of the view. Two ids alternate, so the new frame
/// is on the screen before the old one goes: no blank frame between.
pub const KITTY_IDS: [u32; 2] = [0x7064_0001, 0x7064_0002];

/// Where and how large a frame is drawn, in terminal cells, 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cells {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

/// Move the cursor to `cells`' top left corner.
pub fn cursor_to(cells: &Cells) -> Vec<u8> {
    format!("\x1b[{};{}H", cells.row + 1, cells.col + 1).into_bytes()
}

/// A kitty graphics transmission of `png` with image `id`, shown at the
/// cursor over `cells.cols` x `cells.rows` cells. `q=2` keeps the
/// terminal quiet, `C=1` keeps the cursor where it is. Each escape is a
/// separate item, so tmux passthrough can wrap them one by one.
pub fn kitty(png: &[u8], id: u32, cells: &Cells) -> Vec<Vec<u8>> {
    let b64 = STANDARD.encode(png);
    let parts: Vec<&[u8]> = b64.as_bytes().chunks(KITTY_CHUNK).collect();
    let mut out = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        let more = u8::from(i + 1 < parts.len());
        let mut esc = Vec::with_capacity(part.len() + 80);
        if i == 0 {
            esc.extend_from_slice(
                format!(
                    "\x1b_Ga=T,f=100,t=d,i={id},p=1,q=2,C=1,c={},r={},m={more};",
                    cells.cols, cells.rows
                )
                .as_bytes(),
            );
        } else {
            esc.extend_from_slice(format!("\x1b_Gm={more},q=2;").as_bytes());
        }
        esc.extend_from_slice(part);
        esc.extend_from_slice(b"\x1b\\");
        out.push(esc);
    }
    out
}

/// Delete kitty image `id` and free its data.
pub fn kitty_delete(id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").into_bytes()
}

/// The kitty query of the probe: a 1x1 RGB image that is never shown.
/// A terminal with the protocol answers `\x1b_Gi=31;OK\x1b\\`.
pub const KITTY_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// An iTerm2 inline image of `png` over `cells`, with the cursor left in
/// place. The view computes the fit, so the aspect is not kept again.
pub fn iterm(png: &[u8], cells: &Cells) -> Vec<u8> {
    let mut out = format!(
        "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0;doNotMoveCursor=1:",
        png.len(),
        cells.cols,
        cells.rows
    )
    .into_bytes();
    out.extend_from_slice(STANDARD.encode(png).as_bytes());
    out.push(0x07);
    out
}

/// Wrap one escape sequence for tmux (`allow-passthrough on`): every ESC
/// inside is doubled.
pub fn tmux_wrap(seq: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(seq.len() + 16);
    out.extend_from_slice(b"\x1bPtmux;");
    for &b in seq {
        if b == 0x1b {
            out.push(0x1b);
        }
        out.push(b);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}
