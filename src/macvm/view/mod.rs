//! `paddock mac view`: a guest desktop, or the windows of one app
//! session, inline in the terminal. The guest agent streams PNG frames
//! only on change (`frames.subscribe`); the view draws them with the
//! kitty graphics protocol, iTerm2 inline images or sixel, fits them to
//! the terminal, and sends keys and the mouse back as actions on the
//! target. Leaving the view leaves the session running.
//!
//! Everything but `run.rs` is pure and tested on Linux with fakes:
//! `term` (the detection), `encode` and `sixel` (the protocols, golden
//! bytes), `png` and `inflate` (the bounded decoder for sixel), `fit`,
//! `diff` (draw only changed frames), `keys` and `map` (the input),
//! `screen` (the view without a terminal) and `stream` (the subscription,
//! sized to the terminal).

pub mod diff;
pub mod encode;
pub mod fit;
pub mod inflate;
pub mod keys;
pub mod map;
pub mod png;
#[cfg(unix)]
pub mod run;
pub mod screen;
pub mod sixel;
pub mod source;
pub mod stream;
pub mod term;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
