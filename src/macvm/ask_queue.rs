//! The queue of the firewall questions of one session: which question is
//! on the screen, which wait behind it, how many may come up per minute,
//! and when asking pauses.
//!
//! The rules:
//! - At most one question of a session is on the screen (in the approvals
//!   store, where the marker and `paddock approvals` see it). Every other
//!   open question waits in the queue, in the order of its first ask.
//! - At most [`RATE_MAX`] questions come up on the screen in any
//!   [`RATE_WINDOW`]. A question past that waits silently in the queue.
//! - When more than [`GUARD_MAX`] questions are open, asking pauses: the
//!   question on the screen goes back to the front of the queue, and the
//!   session shows one notice instead. After "ask one by one" the guard
//!   counts from the questions open at that time.
//!
//! Pure: the time is a number, the caller does the files.

use super::approvals::Triple;
use std::collections::VecDeque;

/// Questions that may come up on the screen in one [`RATE_WINDOW`].
pub const RATE_MAX: usize = 3;
/// The window of the rate limit, in seconds.
pub const RATE_WINDOW: u64 = 60;
/// More open questions than this pause the asking of a session.
pub const GUARD_MAX: usize = 20;

/// What the caller does with the store after a change of the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Put this question on the screen.
    Show(Triple),
    /// Take this question off the screen; it waits in the queue again.
    Withdraw(Triple),
    /// Asking paused: show the notice.
    Pause,
}

#[derive(Debug, Default)]
pub struct Queue {
    /// The open questions that are not on the screen, oldest first.
    waiting: VecDeque<Triple>,
    shown: Option<Triple>,
    /// When the questions of the last window came up.
    came_up: VecDeque<u64>,
    paused: bool,
    /// The open questions at the last "ask one by one".
    base: usize,
    /// [`RATE_MAX`], or another limit for a test.
    rate_max: Option<usize>,
}

impl Queue {
    /// A queue with another rate limit than [`RATE_MAX`].
    pub fn with_rate(max: usize) -> Self {
        Self {
            rate_max: Some(max),
            ..Self::default()
        }
    }

    /// The open questions: on the screen and waiting.
    pub fn open(&self) -> usize {
        self.waiting.len() + usize::from(self.shown.is_some())
    }

    /// The open questions besides the one on the screen.
    pub fn more(&self) -> usize {
        self.waiting.len()
    }

    pub fn shown(&self) -> Option<&Triple> {
        self.shown.as_ref()
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    #[cfg(test)]
    pub(super) fn force_pause(&mut self) {
        self.paused = true;
    }

    /// Every open question, the one on the screen first.
    pub fn all(&self) -> Vec<Triple> {
        self.shown
            .iter()
            .chain(self.waiting.iter())
            .cloned()
            .collect()
    }

    /// A new question waits at the end.
    pub fn add(&mut self, t: Triple) {
        if self.shown.as_ref() != Some(&t) && !self.waiting.contains(&t) {
            self.waiting.push_back(t);
        }
    }

    /// A question is answered or decided: it leaves the queue.
    pub fn remove(&mut self, t: &Triple) {
        if self.shown.as_ref() == Some(t) {
            self.shown = None;
        }
        self.waiting.retain(|w| w != t);
        self.base = self.base.min(self.open());
    }

    /// Every open question leaves, for "deny all".
    pub fn take_all(&mut self) -> Vec<Triple> {
        let all = self.all();
        self.shown = None;
        self.waiting.clear();
        self.paused = false;
        self.base = 0;
        all
    }

    /// "Ask one by one": the asking goes on, and the guard counts from the
    /// questions open now.
    pub fn resume(&mut self) {
        self.paused = false;
        self.base = self.open();
    }

    /// Restore the displayed question if closing its card failed. The next
    /// pump retries the transition before showing the pause notice.
    pub fn undo_pause(&mut self, t: &Triple) {
        self.waiting.retain(|w| w != t);
        self.shown = Some(t.clone());
        self.paused = false;
    }

    /// Bring the screen up to date at `now`: pause on too many questions,
    /// else put the next question up when the screen is free and the rate
    /// allows it.
    pub fn pump(&mut self, now: u64) -> Vec<Step> {
        let mut steps = Vec::new();
        if !self.paused && self.open() > self.base + GUARD_MAX {
            self.paused = true;
            if let Some(t) = self.shown.take() {
                self.waiting.push_front(t.clone());
                steps.push(Step::Withdraw(t));
            }
            steps.push(Step::Pause);
        }
        if self.paused || self.shown.is_some() {
            return steps;
        }
        while self
            .came_up
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= RATE_WINDOW)
        {
            self.came_up.pop_front();
        }
        if self.came_up.len() >= self.rate_max.unwrap_or(RATE_MAX) {
            return steps;
        }
        if let Some(t) = self.waiting.pop_front() {
            self.came_up.push_back(now);
            self.shown = Some(t.clone());
            steps.push(Step::Show(t));
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(host: &str) -> Triple {
        Triple {
            identity: None,
            app: "com.example.notes".into(),
            host: host.into(),
            port: 443,
        }
    }

    #[test]
    fn one_question_on_the_screen_the_rest_wait_in_order() {
        let mut q = Queue::default();
        for h in ["a", "b", "c"] {
            q.add(t(h));
        }
        q.add(t("a"));
        assert_eq!(
            q.open(),
            3,
            "a second ask of the same triple is no new question"
        );
        assert_eq!(q.pump(100), vec![Step::Show(t("a"))]);
        assert_eq!(q.pump(101), vec![], "the screen is taken");
        assert_eq!((q.shown(), q.more()), (Some(&t("a")), 2));
        q.remove(&t("a"));
        assert_eq!(q.pump(102), vec![Step::Show(t("b"))]);
        // A waiting question that a rule decided leaves without a panel.
        q.remove(&t("c"));
        q.remove(&t("b"));
        assert_eq!(q.pump(103), vec![]);
        assert_eq!(q.open(), 0);
    }

    #[test]
    fn at_most_three_come_up_per_minute() {
        let mut q = Queue::default();
        for h in ["a", "b", "c", "d", "e"] {
            q.add(t(h));
        }
        let mut shown = Vec::new();
        for now in 0..10 {
            for s in q.pump(now) {
                if let Step::Show(x) = s {
                    shown.push(x.host.clone());
                    q.remove(&x);
                }
            }
        }
        assert_eq!(shown, ["a", "b", "c"], "the fourth waits silently");
        assert_eq!(q.pump(59), vec![]);
        assert_eq!(
            q.pump(60),
            vec![Step::Show(t("d"))],
            "a minute after the first"
        );
    }

    #[test]
    fn too_many_open_questions_pause_with_one_notice() {
        let mut q = Queue::default();
        q.add(t("first"));
        assert_eq!(q.pump(0), vec![Step::Show(t("first"))]);
        for i in 0..GUARD_MAX {
            q.add(t(&format!("h{i}")));
        }
        assert_eq!(q.open(), GUARD_MAX + 1);
        assert_eq!(
            q.pump(1),
            vec![Step::Withdraw(t("first")), Step::Pause],
            "the panel goes, one notice comes"
        );
        assert!(q.paused());
        assert_eq!(q.shown(), None);
        q.add(t("more"));
        assert_eq!(q.pump(2), vec![], "paused: no panel, no second notice");
        assert_eq!(
            q.all()[0],
            t("first"),
            "the withdrawn question waits in front"
        );
        // Ask one by one: the queue goes on, from the front, and the guard
        // waits for twenty more.
        q.resume();
        assert_eq!(q.pump(120), vec![Step::Show(t("first"))]);
        for i in 0..GUARD_MAX {
            q.add(t(&format!("x{i}")));
        }
        assert!(!q.pump(121).contains(&Step::Pause));
        q.add(t("one-too-many"));
        assert!(q.pump(122).contains(&Step::Pause));
        // Deny all: nothing stays.
        let all = q.take_all();
        assert_eq!(all.len(), GUARD_MAX * 2 + 3);
        assert_eq!((q.open(), q.paused()), (0, false));
    }

    #[test]
    fn answers_lower_the_guard_base() {
        let mut q = Queue::default();
        for i in 0..5 {
            q.add(t(&format!("h{i}")));
        }
        q.resume();
        for i in 0..5 {
            q.remove(&t(&format!("h{i}")));
        }
        for i in 0..=GUARD_MAX {
            q.add(t(&format!("n{i}")));
        }
        assert!(q.pump(0).contains(&Step::Pause), "the base fell to zero");
    }
}
