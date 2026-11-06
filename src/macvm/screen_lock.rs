//! The lock of the guest screen during an app session. The agent reports
//! the lock state in each `health` and in a `screen.lock` event the moment
//! it changes (`guest/main_loop_screen.rs`). On a lock the session records
//! when it happened, the last input in the guest and the uptime, then
//! types the guest password and Return through the keyboard of the VM
//! (`run_screen.rs`), and waits [`CONFIRM`] for a report that says
//! unlocked. At most [`TRIES`] tries per lock, then one line for the user.
//!
//! Nothing is typed unless the newest report says locked and is at most
//! [`FRESH`] old; the typing thread asks again before each key, so a
//! screen that unlocks meanwhile gets no more keys. This file is the pure
//! state machine; it is tested on Linux.

use super::guided_ui::Played;
use super::text;
use std::time::{Duration, Instant};

/// The tries of the password for one lock.
pub const TRIES: u32 = 3;
/// How long a try waits for a report that says unlocked.
pub const CONFIRM: Duration = Duration::from_secs(10);
/// A report older than this is not enough to type on.
pub const FRESH: Duration = Duration::from_secs(5);

/// What a report says besides the state: the wall time of the host
/// (RFC 3339), the uptime of the guest, the seconds since its last input.
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub at: String,
    pub uptime: u64,
    pub idle_secs: Option<u64>,
}

/// What the session does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// An event of kind `screen` in the event log.
    Event(String),
    /// A line for the user, and an event of kind `screen`.
    Line(String),
    /// Type the password and Return now: this try.
    Type(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Unlocked,
    /// The keys of this try go now.
    Typing(u32),
    /// The keys of this try went; a report must say unlocked by then.
    Waiting(u32, Instant),
    /// The tries are spent, or paddock cannot type: until an unlock.
    GaveUp,
}

/// The lock fold of one session.
#[derive(Debug)]
pub struct Unlocker {
    phase: Phase,
    locked: bool,
    seen: Option<Instant>,
    has_password: bool,
}

impl Unlocker {
    pub fn new(has_password: bool) -> Self {
        Unlocker {
            phase: Phase::Unlocked,
            locked: false,
            seen: None,
            has_password,
        }
    }

    /// The newest report says locked.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// The newest report says locked and is fresh: keys may go now.
    pub fn may_type(&self, now: Instant) -> bool {
        self.locked
            && self
                .seen
                .is_some_and(|t| now.saturating_duration_since(t) <= FRESH)
    }

    /// A try runs or waits for its answer.
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Typing(_) | Phase::Waiting(..))
    }

    /// A report of the agent: a `screen.lock` event or a `health`.
    pub fn report(&mut self, locked: bool, facts: &Facts, now: Instant) -> Vec<Action> {
        self.locked = locked;
        self.seen = Some(now);
        match (locked, self.phase) {
            (true, Phase::Unlocked) => {
                let mut out = vec![Action::Event(text::screen_locked(
                    &facts.at,
                    facts.idle_secs,
                    facts.uptime,
                ))];
                if !self.has_password {
                    self.phase = Phase::GaveUp;
                    out.push(Action::Line(text::screen_no_password()));
                    return out;
                }
                out.extend(self.start_try(1));
                out
            }
            (true, _) => Vec::new(),
            (false, Phase::Unlocked) => Vec::new(),
            (false, Phase::Typing(n) | Phase::Waiting(n, _)) => {
                self.phase = Phase::Unlocked;
                vec![Action::Event(text::screen_unlocked(n))]
            }
            (false, Phase::GaveUp) => {
                self.phase = Phase::Unlocked;
                vec![Action::Event(text::screen_unlocked(0))]
            }
        }
    }

    fn start_try(&mut self, n: u32) -> Vec<Action> {
        self.phase = Phase::Typing(n);
        vec![
            Action::Event(text::screen_typing(n, TRIES)),
            Action::Type(n),
        ]
    }

    /// The typing of the try ended.
    pub fn typed(&mut self, played: Played, now: Instant) -> Vec<Action> {
        let Phase::Typing(n) = self.phase else {
            return Vec::new();
        };
        match played {
            Played::All => {
                self.phase = Phase::Waiting(n, now + CONFIRM);
                Vec::new()
            }
            Played::Stopped => {
                // The flag said unlocked; the report that set it ends the
                // try. Until it comes the try waits as if it went.
                self.phase = Phase::Waiting(n, now + CONFIRM);
                vec![Action::Event(text::screen_typing_stopped())]
            }
            Played::Failed => self.give_up(text::screen_no_keyboard()),
        }
    }

    /// paddock cannot type at all (a character with no key, no keyboard):
    /// one line, and no try until the next lock.
    pub fn give_up(&mut self, line: String) -> Vec<Action> {
        self.phase = Phase::GaveUp;
        vec![Action::Line(line)]
    }

    /// The time passed: a try whose wait ended without an unlock starts
    /// the next one, or after the last one the line for the user. A stale
    /// report starts no try: the next fresh one does.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let Phase::Waiting(n, until) = self.phase else {
            return Vec::new();
        };
        if now < until {
            return Vec::new();
        }
        if !self.locked {
            self.phase = Phase::Unlocked;
            return vec![Action::Event(text::screen_unlocked(n))];
        }
        if n >= TRIES {
            return self.give_up(text::screen_stays_locked(n));
        }
        if !self.may_type(now) {
            return Vec::new();
        }
        self.start_try(n + 1)
    }
}

#[cfg(test)]
#[path = "screen_lock_tests.rs"]
mod tests;
