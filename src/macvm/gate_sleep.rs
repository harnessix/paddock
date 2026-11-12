//! Detect a host sleep from wall and monotonic clocks.

use std::time::Instant;

/// The host slept when the wall clock moved this much further than the
/// clock that stops in sleep.
pub const SLEEP_MIN_SECS: u64 = 30;

/// A watch for a sleep of the host, from the two clocks: the wall clock
/// runs through a sleep, `Instant` does not (`CLOCK_UPTIME_RAW` on macOS).
/// The session loop asks it every few seconds and tells the asker about a
/// wake, so a question from before the sleep is asked again.
pub struct SleepWatch {
    wall: u64,
    mono: Instant,
}

impl SleepWatch {
    pub fn new(now_wall: u64) -> Self {
        Self {
            wall: now_wall,
            mono: Instant::now(),
        }
    }

    /// Whether the two elapsed times show a sleep.
    pub fn slept(wall_elapsed: u64, mono_elapsed: u64) -> bool {
        wall_elapsed > mono_elapsed + SLEEP_MIN_SECS
    }

    /// The wake time, when the host slept since the last check.
    pub fn check(&mut self, now_wall: u64) -> Option<u64> {
        let wall = now_wall.saturating_sub(self.wall);
        let mono = self.mono.elapsed().as_secs();
        self.wall = now_wall;
        self.mono = Instant::now();
        Self::slept(wall, mono).then_some(now_wall)
    }
}
