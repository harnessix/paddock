//! The subscription of the view. It asks the guest for frames no larger
//! than the room of the terminal (`max {width, height}`), so a Retina
//! desktop does not cross the TTY at full size, and it subscribes again
//! when the terminal size settles on a new room. A new subscription
//! starts its `seq` at 1, so the caller resets the frame gate
//! ([`super::screen::Screen::subscribed`]).

use super::source::{FrameSource, Item, Max, Target};
use std::sync::mpsc::Receiver;

/// A new room must stay the same this long before the view subscribes
/// again: a window drag gives many sizes, and each subscribe costs a
/// full first frame.
pub const SETTLE_MS: u64 = 250;
/// After a failed subscribe, the view tries the same room again after
/// this long, and each failure after that doubles the wait.
pub const RETRY_MS: u64 = 1000;
/// The longest wait between two tries of the same room.
pub const RETRY_MAX_MS: u64 = 30_000;

/// The next try of a room whose subscribe failed.
#[derive(Debug, Clone, Copy)]
struct Retry {
    room: Max,
    /// The time of the next try.
    at: u64,
    /// The wait before this try.
    wait: u64,
}

/// The frame stream of one target.
pub struct Stream {
    rx: Receiver<Item>,
    target: Target,
    max_fps: u32,
    /// The `max` of the current subscription.
    max: Max,
    /// A different room and when it was first seen.
    want: Option<(Max, u64)>,
    /// The subscribe for a room failed: when to try it again.
    retry: Option<Retry>,
}

impl Stream {
    /// Subscribe to `target` with frames of at most `max`.
    pub fn start(
        source: &mut dyn FrameSource,
        target: &Target,
        max_fps: u32,
        max: Max,
    ) -> Result<Stream, String> {
        let rx = source.subscribe(target, max_fps, max)?;
        Ok(Stream {
            rx,
            target: target.clone(),
            max_fps,
            max,
            want: None,
            retry: None,
        })
    }

    pub fn frames(&self) -> &Receiver<Item> {
        &self.rx
    }

    /// The `max` of the current subscription.
    pub fn max(&self) -> Max {
        self.max
    }

    /// The terminal has room for `room` at `now_ms`. Subscribes again once
    /// a new room held for [`SETTLE_MS`]. Returns true after a new
    /// subscription: then the frames start at `seq` 1. On an error the
    /// old stream stays, and the same room is tried again after
    /// [`RETRY_MS`], then after twice as long each time, up to
    /// [`RETRY_MAX_MS`]. Only a try returns the error, so the caller can
    /// show each one.
    pub fn room(
        &mut self,
        source: &mut dyn FrameSource,
        room: Max,
        now_ms: u64,
    ) -> Result<bool, String> {
        if room == self.max {
            self.want = None;
            self.retry = None;
            return Ok(false);
        }
        let due = match self.retry {
            Some(r) if r.room == room => now_ms >= r.at,
            _ => {
                // A new room settles first, also after a failed one.
                self.retry = None;
                match self.want {
                    Some((w, since)) if w == room => now_ms.saturating_sub(since) >= SETTLE_MS,
                    _ => {
                        self.want = Some((room, now_ms));
                        false
                    }
                }
            }
        };
        if !due {
            return Ok(false);
        }
        match source.subscribe(&self.target, self.max_fps, room) {
            Ok(rx) => {
                self.rx = rx;
                self.max = room;
                self.want = None;
                self.retry = None;
                Ok(true)
            }
            Err(e) => {
                let wait = match self.retry {
                    Some(r) => r.wait.saturating_mul(2).min(RETRY_MAX_MS),
                    None => RETRY_MS,
                };
                self.retry = Some(Retry {
                    room,
                    at: now_ms.saturating_add(wait),
                    wait,
                });
                Err(e)
            }
        }
    }
}
