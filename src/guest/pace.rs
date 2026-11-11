//! The window poll around `window.move`, pure, with tests on Linux.
//!
//! The reply of a `window.move` carries the frame that the window has
//! after the move, so the host needs no poll for it. A lone move still
//! polls at once: the other windows of the app may follow it. During a
//! live resize the host sends one move after the reply of the last one,
//! and a full poll after each (the window list, the process table and
//! every window's Accessibility facts) would delay the next move. So in
//! a streak of moves the poll waits up to [`STREAK_POLL`], and the
//! Accessibility notifications of the moves wait with it.

use std::time::{Duration, Instant};

/// A move this soon after the one before is part of a streak.
pub const MOVE_STREAK: Duration = Duration::from_millis(150);
/// In a streak, the poll comes at most this long after a move.
pub const STREAK_POLL: Duration = Duration::from_millis(60);

/// When the last `window.move` ran.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MovePace {
    last: Option<Instant>,
}

impl MovePace {
    /// A `window.move` ran at `now`. The time of the next poll: now for
    /// a lone move, else the poll that is due, but not later than
    /// [`STREAK_POLL`] from now.
    pub fn moved(&mut self, now: Instant, next_poll: Instant) -> Instant {
        let streak = self.streak(now);
        self.last = Some(now);
        if streak {
            next_poll.min(now + STREAK_POLL)
        } else {
            now
        }
    }

    /// True while a streak of moves runs.
    pub fn streak(&self, now: Instant) -> bool {
        self.last
            .is_some_and(|t| now.saturating_duration_since(t) < MOVE_STREAK)
    }

    /// The time of the next poll after an Accessibility notification at
    /// `now`: at once, unless a streak runs.
    pub fn dirty(&self, now: Instant, next_poll: Instant) -> Instant {
        if self.streak(now) {
            next_poll.min(now + STREAK_POLL)
        } else {
            now
        }
    }
}

#[cfg(test)]
#[path = "pace_tests.rs"]
mod tests;
