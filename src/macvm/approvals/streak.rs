//! Repeated failures of the card on the screen or of the pause notice.
//!
//! Every waiting connection polls the store, so the number of failed
//! polls says nothing about time: twenty waiters fail twenty times in one
//! moment. A streak counts at most one failure per second of the clock.

/// Seconds with a failure before a selection is given up.
pub(super) const STREAK_SECONDS: u32 = 5;

/// The failures of one card in a row.
#[derive(Debug, Default)]
pub(super) struct Streak {
    /// The card that failed, and the last second with a failure.
    card: Option<(String, u64)>,
    seconds: u32,
}

impl Streak {
    /// Note a failure of `card` at `now`. `true`: the card failed in
    /// [`STREAK_SECONDS`] different seconds without a success between.
    pub(super) fn fail(&mut self, card: &str, now: u64) -> bool {
        match &self.card {
            Some((id, last)) if id == card => {
                if *last != now {
                    self.seconds += 1;
                }
            }
            _ => self.seconds = 1,
        }
        self.card = Some((card.to_string(), now));
        self.seconds >= STREAK_SECONDS
    }

    /// The card could be read: a later failure starts a new streak.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_in_one_second_count_once() {
        let mut s = Streak::default();
        for _ in 0..100 {
            assert!(!s.fail("a", 10));
        }
        for now in 11..10 + u64::from(STREAK_SECONDS) - 1 {
            assert!(!s.fail("a", now));
        }
        assert!(s.fail("a", 10 + u64::from(STREAK_SECONDS) - 1));
    }

    #[test]
    fn another_card_or_a_success_starts_again() {
        let mut s = Streak::default();
        for now in 0..u64::from(STREAK_SECONDS) - 1 {
            assert!(!s.fail("a", now));
        }
        assert!(!s.fail("b", 50));
        s.clear();
        assert!(!s.fail("b", 51));
    }
}
