//! The progress of the guided step, platform-neutral: which panel steps
//! are done after each signal, and which waits on the person each signal
//! ends. `vz_mac_session.rs` feeds it the agent's messages and the
//! `Continue` button, and marks the panel and the event log by what it
//! returns.
//!
//! The panel steps: 1 language, 2 skips, 3 account, 4 seed (the install
//! command in Terminal), 5 Accessibility, 6 done. The agent runs only
//! after the person ran the install command, so its `hello` proves steps
//! 1 to 4, and its health with the Accessibility permission proves step
//! 5. Step 6 is marked when the configure step ends.

use super::setup_events::{ACCOUNT_CREATED, SEED_INSTALLED, TRUSTED};
use super::text::PanelStep;

/// What the guided step saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// The agent dialed in.
    Hello,
    /// A health report, with the Accessibility permission or not.
    Health(bool),
    /// The `Continue` button: the next wait is done by hand.
    Continue,
    /// The guest booted with its account made: the provisioning options.
    Desktop,
}

/// The panel steps done, 0 to 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub done: usize,
}

/// The change that a signal made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Change {
    /// The panel steps 1 to `mark` are done now; `None` when nothing moved.
    pub mark: Option<usize>,
    /// The waits on the person that ended, in order.
    pub person: Vec<&'static str>,
}

const ACCOUNT: usize = PanelStep::Account.number() as usize;
const SEED: usize = PanelStep::Seed.number() as usize;
const ACCESSIBILITY: usize = PanelStep::Accessibility.number() as usize;
const DONE: usize = PanelStep::Done.number() as usize;

/// The person wait that ends with a panel step, if one does.
fn person_kind(step: usize) -> Option<&'static str> {
    match step {
        ACCOUNT => Some(ACCOUNT_CREATED),
        SEED => Some(SEED_INSTALLED),
        ACCESSIBILITY => Some(TRUSTED),
        _ => None,
    }
}

impl Progress {
    /// The change of one signal.
    pub fn on(&mut self, signal: Signal) -> Change {
        let to = match signal {
            Signal::Hello => SEED,
            Signal::Health(true) => ACCESSIBILITY,
            Signal::Health(false) => return Change::default(),
            Signal::Continue if self.done < ACCOUNT => ACCOUNT,
            Signal::Continue => (self.done + 1).min(ACCESSIBILITY),
            Signal::Desktop => ACCOUNT,
        };
        self.advance(to)
    }

    /// A step goes back to the person at panel step `at` (the agent of a
    /// seeded disk did not start): the steps before it are done, except
    /// the language and the skips of Setup Assistant, which only a
    /// hand-over at the account leaves to the person.
    pub fn hand_over(&mut self, at: PanelStep) -> Change {
        match at {
            PanelStep::Seed => self.advance(ACCOUNT),
            PanelStep::Accessibility | PanelStep::Done => self.advance(SEED),
            _ => Change::default(),
        }
    }

    /// The configure step ended: the last panel step is done.
    pub fn configured(&mut self) -> Change {
        self.advance(DONE)
    }

    fn advance(&mut self, to: usize) -> Change {
        if to <= self.done {
            return Change::default();
        }
        let person = (self.done + 1..=to).filter_map(person_kind).collect();
        self.done = to;
        Change {
            mark: Some(to),
            person,
        }
    }

    /// The guided step is over: Accessibility is on.
    pub fn finished(&self) -> bool {
        self.done >= ACCESSIBILITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_path_marks_account_and_seed_at_hello_then_trusted() {
        let mut p = Progress::default();
        assert_eq!(p.on(Signal::Health(false)), Change::default());
        let c = p.on(Signal::Hello);
        assert_eq!(c.mark, Some(4));
        assert_eq!(c.person, vec![ACCOUNT_CREATED, SEED_INSTALLED]);
        assert!(!p.finished());
        assert_eq!(p.on(Signal::Hello), Change::default(), "a second hello");
        let c = p.on(Signal::Health(true));
        assert_eq!(c.mark, Some(5));
        assert_eq!(c.person, vec![TRUSTED]);
        assert!(p.finished());
        let c = p.configured();
        assert_eq!(c.mark, Some(6));
        assert!(c.person.is_empty());
    }

    #[test]
    fn continue_steps_through_each_wait_by_hand() {
        let mut p = Progress::default();
        let marks: Vec<(Option<usize>, Vec<&str>)> = (0..4)
            .map(|_| {
                let c = p.on(Signal::Continue);
                (c.mark, c.person)
            })
            .collect();
        assert_eq!(
            marks,
            vec![
                (Some(3), vec![ACCOUNT_CREATED]),
                (Some(4), vec![SEED_INSTALLED]),
                (Some(5), vec![TRUSTED]),
                (None, vec![]),
            ]
        );
        assert!(p.finished());
    }

    #[test]
    fn continue_then_hello_counts_each_wait_once() {
        let mut p = Progress::default();
        assert_eq!(p.on(Signal::Continue).person, vec![ACCOUNT_CREATED]);
        assert_eq!(p.on(Signal::Hello).person, vec![SEED_INSTALLED]);
        assert_eq!(p.on(Signal::Continue).person, vec![TRUSTED]);
    }

    #[test]
    fn the_desktop_marks_the_account_and_a_hand_over_marks_the_steps_before() {
        let mut p = Progress::default();
        assert_eq!(p.on(Signal::Desktop).person, vec![ACCOUNT_CREATED]);
        assert_eq!(p.hand_over(PanelStep::Seed), Change::default());
        let c = p.hand_over(PanelStep::Accessibility);
        assert_eq!((c.mark, c.person), (Some(4), vec![SEED_INSTALLED]));
        let mut p = Progress::default();
        assert_eq!(p.hand_over(PanelStep::Account), Change::default());
        assert_eq!(p.done, 0);
    }

    #[test]
    fn a_health_with_accessibility_before_hello_ends_every_wait() {
        let mut p = Progress::default();
        assert_eq!(
            p.on(Signal::Health(true)).person,
            vec![ACCOUNT_CREATED, SEED_INSTALLED, TRUSTED]
        );
    }
}
