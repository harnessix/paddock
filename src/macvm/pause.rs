//! The freeze of a guest session in the shim (`paddock freeze` and `paddock
//! thaw`): the `pause` and `resume` lines of the control socket become
//! `VZVirtualMachine.pause` and `resume` on the main queue. Pure:
//! `run_calls.rs` executes what this fold returns.
//!
//! The rules:
//! - A second `pause` of a paused VM, and a `resume` of a running one,
//!   answer `ok` at once and call nothing.
//! - A line that comes while the framework works on the other one is
//!   refused with one line; the same one waits for the same answer.
//! - A quit of a frozen session resumes the VM first, then quits, so the
//!   app can save its work and the state can be saved.
//! - While the VM is paused, the health gap and the request timeouts do
//!   not count: the silence is paddock's, not the guest's.

use super::text;
use std::time::{Duration, Instant};

/// What the control socket or the quit path asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Pause,
    Resume,
    /// A quit of the session, with its reason for the quit line.
    Quit(String),
}

/// What the shim does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Do {
    /// `VZVirtualMachine.pause` on the main queue.
    CallPause,
    /// `VZVirtualMachine.resume` on the main queue.
    CallResume,
    /// Keep the answer channel of this line until the framework is done.
    Wait,
    /// Answer this line now.
    Answer(Result<(), String>),
    /// Answer every line that waits.
    AnswerAll(Result<(), String>),
    /// The VM is paused: the badge, the event line.
    Frozen,
    /// The VM runs again after this long: the badge goes, the deadlines
    /// move by the pause.
    Thawed(Duration),
    /// Start the quit path, with its reason.
    BeginQuit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Pausing,
    Paused(Instant),
    Resuming(Instant),
}

/// The fold of one session.
#[derive(Debug, Clone)]
pub struct Pause {
    state: State,
    /// A quit came while the VM was paused or pausing.
    quit_after: Option<String>,
    /// The session is quitting: no more pause.
    quitting: bool,
}

impl Default for Pause {
    fn default() -> Self {
        Self::new()
    }
}

impl Pause {
    pub fn new() -> Self {
        Self {
            state: State::Running,
            quit_after: None,
            quitting: false,
        }
    }

    /// The VM is paused, or on its way in or out: the watch and the
    /// request deadlines wait.
    pub fn frozen(&self) -> bool {
        self.state != State::Running
    }

    /// The paused VM: since when.
    pub fn paused_since(&self) -> Option<Instant> {
        match self.state {
            State::Paused(t) | State::Resuming(t) => Some(t),
            _ => None,
        }
    }

    /// One line of the control socket, or a quit.
    pub fn ask(&mut self, ask: Ask) -> Vec<Do> {
        match (ask, self.state) {
            (Ask::Pause, _) if self.quitting || self.quit_after.is_some() => {
                vec![Do::Answer(Err(text::pause_while_quitting()))]
            }
            (Ask::Pause, State::Running) => {
                self.state = State::Pausing;
                vec![Do::CallPause, Do::Wait]
            }
            (Ask::Pause, State::Pausing) => vec![Do::Wait],
            (Ask::Pause, State::Paused(_)) => vec![Do::Answer(Ok(()))],
            (Ask::Pause, State::Resuming(_)) => {
                vec![Do::Answer(Err(text::pause_while_resuming()))]
            }
            (Ask::Resume, State::Running) => vec![Do::Answer(Ok(()))],
            (Ask::Resume, State::Pausing) => {
                vec![Do::Answer(Err(text::resume_while_pausing()))]
            }
            (Ask::Resume, State::Paused(t)) => {
                self.state = State::Resuming(t);
                vec![Do::CallResume, Do::Wait]
            }
            (Ask::Resume, State::Resuming(_)) => vec![Do::Wait],
            (Ask::Quit(why), State::Running) => {
                self.quitting = true;
                vec![Do::BeginQuit(why)]
            }
            (Ask::Quit(why), State::Paused(t)) => {
                self.quit_after.get_or_insert(why);
                self.state = State::Resuming(t);
                vec![Do::CallResume]
            }
            // The answer of the framework brings the quit.
            (Ask::Quit(why), State::Pausing | State::Resuming(_)) => {
                self.quit_after.get_or_insert(why);
                Vec::new()
            }
        }
    }

    /// The framework answered a pause (`pause = true`) or a resume.
    pub fn done(&mut self, pause: bool, r: Result<(), String>, now: Instant) -> Vec<Do> {
        let mut v = Vec::new();
        match (pause, r) {
            (true, Ok(())) => {
                self.state = State::Paused(now);
                v.push(Do::AnswerAll(Ok(())));
                v.push(Do::Frozen);
                if self.quit_after.is_some() {
                    self.state = State::Resuming(now);
                    v.push(Do::CallResume);
                }
            }
            (true, Err(e)) => {
                self.state = State::Running;
                v.push(Do::AnswerAll(Err(text::pause_failed_freeze(&e))));
                v.extend(self.quit_now());
            }
            (false, Ok(())) => {
                let since = self.paused_since().unwrap_or(now);
                self.state = State::Running;
                v.push(Do::AnswerAll(Ok(())));
                v.push(Do::Thawed(now.saturating_duration_since(since)));
                v.extend(self.quit_now());
            }
            (false, Err(e)) => {
                // Still paused. A quit goes on: the quit path stops the VM
                // without a save when it cannot pause or save.
                let since = self.paused_since().unwrap_or(now);
                self.state = State::Paused(since);
                v.push(Do::AnswerAll(Err(text::resume_failed_freeze(&e))));
                if self.quit_after.is_some() {
                    self.state = State::Running;
                    v.extend(self.quit_now());
                }
            }
        }
        v
    }

    fn quit_now(&mut self) -> Option<Do> {
        let why = self.quit_after.take()?;
        self.quitting = true;
        Some(Do::BeginQuit(why))
    }
}

#[cfg(test)]
#[path = "pause_tests.rs"]
mod tests;
