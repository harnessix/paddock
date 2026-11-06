//! The quit path of a session as a state machine without any framework
//! call: `step(input)` gives the actions, and the shim (`run.rs`) runs
//! them. The tests drive it with a fake agent.
//!
//! 1. `app.quit` to the agent, wait up to 60 s for `app_exited`, then
//!    `app.quit {force}` and 10 s more. When the agent answers that the
//!    app does not run, the quit goes on at once.
//! 2. Close the host windows.
//! 3. When the save is on and the volume has the RAM size plus 1 GiB free:
//!    `pause`, `saveMachineStateTo` a `.partial` file, commit. On a
//!    failure remove the partial, `resume`, send `shutdown` and wait for
//!    the stop, so the disks are consistent.
//! 4. `stop`. The stamp gets `clean = true`. Exit 0.
//!
//! A second signal at any point stops the VM at once, without a save, and
//! the stamp gets `clean = false`. Exit 3.
//!
//! A suspend (`Input::Suspend`, automatic suspend of `suspend.rs`) skips
//! step 1: the app keeps running in the saved state. The windows close
//! only after the save, and the end is `Suspended` instead of `Exit`: the
//! shim gives its slot back and stays in the Dock. A suspend that cannot
//! save (save off, no space, a failed pause or save) is refused with
//! `SuspendRefused`, and the session runs on.

use super::session::{save_refusal, SAVE_EXTRA};
use super::text;
use std::time::Duration;

/// The exit code after a hard stop, as `vz.rs` uses it.
pub const EXIT_HARD_STOP: i32 = 3;
/// The wait for `app_exited` after `app.quit`.
pub const QUIT_WAIT: Duration = Duration::from_secs(60);
/// The wait after `app.quit {force}`.
pub const FORCE_WAIT: Duration = Duration::from_secs(10);
/// The wait for the guest stop after `shutdown`.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `[macvm] save`.
    pub save: bool,
    /// The memory of the VM: the saved state is about this size.
    pub ram_bytes: u64,
    /// The name of the app, for the lines.
    pub app: String,
    pub quit_wait: Duration,
    pub force_wait: Duration,
    pub shutdown_wait: Duration,
}

impl Config {
    pub fn new(app: &str, save: bool, ram_bytes: u64) -> Self {
        Self {
            save,
            ram_bytes,
            app: app.to_string(),
            quit_wait: QUIT_WAIT,
            force_wait: FORCE_WAIT,
            shutdown_wait: SHUTDOWN_WAIT,
        }
    }

    /// The free space that a save needs.
    pub fn save_needs(&self) -> u64 {
        self.ram_bytes + SAVE_EXTRA
    }
}

/// What happened, as the shim sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// A quit request: SIGTERM, Ctrl-C, the control `stop` line, the Dock.
    /// `agent` is false when no agent is connected: no `app.quit` and no
    /// `shutdown` can be sent.
    Begin {
        agent: bool,
    },
    /// The agent reported `app_exited`, or the followed pid set is empty.
    AppExited,
    /// The agent answered `app.quit` with `no_such_app`: the app does not
    /// run in the guest (a scratch profile, or the user quit it). Nothing
    /// is left to quit, so the quit goes on at once, as after `AppExited`.
    AppNotRunning,
    /// The deadline of the current step passed.
    Timeout,
    /// The free space of the volume, as the answer to `CheckSpace`.
    Space(u64),
    Paused(Result<(), String>),
    /// The save ended: the bytes of the partial file, or the error.
    Saved(Result<u64, String>),
    Resumed(Result<(), String>),
    /// The guest stopped itself (after `shutdown`).
    GuestStopped,
    /// The agent answered `shutdown` with an error (sudo refused the
    /// password, an app still runs): the text says why. The VM stops at
    /// once instead of after the shutdown wait.
    ShutdownRefused(String),
    /// The forced stop ended.
    Stopped(Result<(), String>),
    /// A second signal: no save, stop now.
    Signal,
    /// Save the state and give the slot back, without `app.quit`.
    Suspend {
        agent: bool,
    },
}

/// The input that the agent's answer to `app.quit` gives, if any. A
/// success says nothing yet: the `app_exited` event follows. `no_such_app`
/// means the app does not run in the guest, so nothing is left to quit.
/// Any other error waits for the deadline, which forces the quit.
pub fn quit_answer(outcome: &super::proto::Outcome) -> Option<Input> {
    use super::proto::{ErrorCode, Outcome};
    match outcome {
        Outcome::Error {
            code: ErrorCode::NoSuchApp,
            ..
        } => Some(Input::AppNotRunning),
        _ => None,
    }
}

/// What the shim does, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    SendQuit {
        force: bool,
    },
    /// Wait this long for the next input, then feed `Timeout`.
    Deadline(Duration),
    CloseWindows,
    CheckSpace,
    Pause,
    /// `saveMachineStateTo` the partial state file.
    Save,
    /// Rename the partial state file to `state.vzs`.
    CommitState,
    RemovePartial,
    Resume,
    SendShutdown,
    /// `stop` the VM at once.
    Stop,
    /// Write the stamp. `saved` is the length of the saved state.
    Stamp {
        clean: bool,
        saved: Option<u64>,
    },
    /// One line for the user.
    Line(String),
    /// One event log line.
    Event {
        kind: String,
        text: String,
    },
    /// Release the slot, the lock and the record, then exit.
    Exit(i32),
    /// The state is saved and the VM stopped: give the slot back and stay
    /// in the Dock as a suspended session.
    Suspended,
    /// The suspend cannot happen: the line says why. The session runs on.
    SuspendRefused(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Quitting,
    Forcing,
    Pausing,
    Saving,
    Resuming,
    ShuttingDown {
        saved: Option<u64>,
    },
    Stopping {
        clean: bool,
        saved: Option<u64>,
        code: i32,
    },
    Done {
        clean: bool,
        code: i32,
    },
}

pub struct Quit {
    cfg: Config,
    phase: Phase,
    agent: bool,
    /// Set by `Signal`: the stop is a hard stop.
    hard: bool,
    /// The run is a suspend, not a quit.
    suspend: bool,
    /// A quit came while the suspend ran: the end is a quit.
    quit_after: bool,
}

impl Quit {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            phase: Phase::Idle,
            agent: false,
            hard: false,
            suspend: false,
            quit_after: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn done(&self) -> bool {
        matches!(self.phase, Phase::Done { .. })
    }

    /// Feed one input, get the actions.
    pub fn step(&mut self, input: Input) -> Vec<Action> {
        match (self.phase, input) {
            (Phase::Done { .. }, _) => vec![],
            (_, Input::Signal) => self.hard_stop(),
            // A guest that stops on its own in any phase ends the session.
            (_, Input::GuestStopped) => self.finish(!self.hard, None, 0),
            (Phase::Idle, Input::Suspend { agent }) => self.begin_suspend(agent),
            (_, Input::Suspend { .. }) => vec![],
            (_, Input::Begin { .. }) if self.suspend => {
                self.quit_after = true;
                vec![]
            }
            (Phase::Idle, Input::Begin { agent }) => {
                self.agent = agent;
                if agent {
                    self.phase = Phase::Quitting;
                    vec![
                        Action::Event {
                            kind: "quit".into(),
                            text: text::quit_asking(&self.cfg.app),
                        },
                        Action::SendQuit { force: false },
                        Action::Deadline(self.cfg.quit_wait),
                    ]
                } else {
                    let mut v = vec![Action::Event {
                        kind: "quit".into(),
                        text: "no guest agent: the app cannot be asked to quit".into(),
                    }];
                    v.extend(self.after_app_quit());
                    v
                }
            }
            (Phase::Idle, _) => vec![],
            (Phase::Quitting, Input::AppExited) | (Phase::Forcing, Input::AppExited) => {
                self.after_app_quit()
            }
            (Phase::Quitting, Input::AppNotRunning) | (Phase::Forcing, Input::AppNotRunning) => {
                let mut v = vec![Action::Event {
                    kind: "quit".into(),
                    text: text::quit_app_not_running(&self.cfg.app),
                }];
                v.extend(self.after_app_quit());
                v
            }
            (Phase::Quitting, Input::Timeout) => {
                self.phase = Phase::Forcing;
                vec![
                    Action::Event {
                        kind: "quit".into(),
                        text: text::quit_forcing(&self.cfg.app, self.cfg.quit_wait),
                    },
                    Action::SendQuit { force: true },
                    Action::Deadline(self.cfg.force_wait),
                ]
            }
            (Phase::Forcing, Input::Timeout) => {
                let total = self.cfg.quit_wait + self.cfg.force_wait;
                let mut v = vec![Action::Line(text::quit_force_failed(&self.cfg.app, total))];
                v.extend(self.stop(false, None, 0));
                v
            }
            (Phase::Pausing, Input::Space(free)) => self.space(free),
            (Phase::Pausing, Input::Paused(Ok(()))) => {
                self.phase = Phase::Saving;
                vec![Action::Save]
            }
            (Phase::Pausing, Input::Paused(Err(e))) if self.suspend => {
                self.suspend_refused(text::pause_failed(&e))
            }
            (Phase::Pausing, Input::Paused(Err(e))) => {
                let mut v = vec![Action::Line(text::pause_failed(&e))];
                v.extend(self.no_save());
                v
            }
            (Phase::Saving, Input::Saved(Ok(len))) => {
                let mut v = vec![
                    Action::CommitState,
                    Action::Event {
                        kind: "save".into(),
                        text: "saved the machine state".into(),
                    },
                ];
                if self.suspend {
                    v.push(Action::CloseWindows);
                }
                v.extend(self.stop(true, Some(len), 0));
                v
            }
            (Phase::Saving, Input::Saved(Err(e))) => {
                self.phase = Phase::Resuming;
                vec![
                    Action::RemovePartial,
                    Action::Line(text::save_failed(&e)),
                    Action::Event {
                        kind: "save".into(),
                        text: text::save_failed(&e),
                    },
                    Action::Resume,
                ]
            }
            (Phase::Resuming, Input::Resumed(Ok(()))) if self.suspend => {
                self.suspend_refused(text::suspend_save_failed())
            }
            (Phase::Resuming, Input::Resumed(Ok(()))) => self.shutdown(None),
            (Phase::Resuming, Input::Resumed(Err(e))) => {
                let mut v = vec![Action::Event {
                    kind: "error".into(),
                    text: text::resume_failed(&e),
                }];
                v.extend(self.stop(false, None, 0));
                v
            }
            (Phase::ShuttingDown { saved }, Input::ShutdownRefused(why)) => {
                let mut v = vec![Action::Event {
                    kind: "error".into(),
                    text: text::shutdown_refused(&why),
                }];
                v.extend(self.stop(false, saved, 0));
                v
            }
            (Phase::ShuttingDown { saved }, Input::Timeout) => {
                let mut v = vec![Action::Event {
                    kind: "error".into(),
                    text: text::shutdown_timeout(self.cfg.shutdown_wait),
                }];
                v.extend(self.stop(false, saved, 0));
                v
            }
            (Phase::Stopping { clean, saved, code }, Input::Stopped(r)) => {
                let mut v = Vec::new();
                if let Err(e) = r {
                    v.push(Action::Event {
                        kind: "error".into(),
                        text: text::stop_failed(&e),
                    });
                }
                v.extend(self.finish(clean, saved, code));
                v
            }
            _ => vec![],
        }
    }

    /// After the app is gone: close the windows, then save or shut down.
    fn after_app_quit(&mut self) -> Vec<Action> {
        let mut v = vec![Action::CloseWindows];
        if self.cfg.save {
            self.phase = Phase::Pausing;
            v.push(Action::CheckSpace);
        } else {
            v.push(Action::Event {
                kind: "save".into(),
                text: "save is off: no machine state saved".into(),
            });
            v.extend(self.no_save());
        }
        v
    }

    /// The answer to `CheckSpace`.
    pub fn space(&mut self, free: u64) -> Vec<Action> {
        if self.phase != Phase::Pausing {
            return vec![];
        }
        let needs = self.cfg.save_needs();
        if free < needs && self.suspend {
            return self.suspend_refused(save_refusal(needs, free));
        }
        if free < needs {
            let line = save_refusal(needs, free);
            let mut v = vec![
                Action::Line(line.clone()),
                Action::Event {
                    kind: "save".into(),
                    text: line,
                },
            ];
            v.extend(self.no_save());
            return v;
        }
        vec![Action::Pause]
    }

    /// No save: shut the guest down through the agent, so the disks are
    /// consistent, or stop at once without one.
    fn no_save(&mut self) -> Vec<Action> {
        if self.agent {
            self.shutdown(None)
        } else {
            self.stop(false, None, 0)
        }
    }

    fn shutdown(&mut self, saved: Option<u64>) -> Vec<Action> {
        self.phase = Phase::ShuttingDown { saved };
        vec![
            Action::SendShutdown,
            Action::Deadline(self.cfg.shutdown_wait),
        ]
    }

    fn stop(&mut self, clean: bool, saved: Option<u64>, code: i32) -> Vec<Action> {
        self.phase = Phase::Stopping { clean, saved, code };
        vec![Action::Stop]
    }

    /// The start of a suspend: the space check, then the pause and the
    /// save. With the save off, the suspend is refused at once.
    fn begin_suspend(&mut self, agent: bool) -> Vec<Action> {
        self.agent = agent;
        if !self.cfg.save {
            return vec![Action::SuspendRefused(text::suspend_save_off())];
        }
        self.suspend = true;
        self.quit_after = false;
        self.phase = Phase::Pausing;
        vec![Action::CheckSpace]
    }

    /// The suspend ends without a save: back to a running session, and to
    /// the quit when one came in the meantime.
    fn suspend_refused(&mut self, line: String) -> Vec<Action> {
        self.suspend = false;
        self.phase = Phase::Idle;
        let mut v = vec![
            Action::Event {
                kind: "suspend".into(),
                text: line.clone(),
            },
            Action::SuspendRefused(line),
        ];
        if std::mem::take(&mut self.quit_after) {
            v.extend(self.step(Input::Begin { agent: self.agent }));
        }
        v
    }

    fn hard_stop(&mut self) -> Vec<Action> {
        self.hard = true;
        let mut v = vec![Action::Event {
            kind: "quit".into(),
            text: "second signal: the VM stops now, without a save".into(),
        }];
        v.extend(self.stop(false, None, EXIT_HARD_STOP));
        v
    }

    fn finish(&mut self, clean: bool, saved: Option<u64>, code: i32) -> Vec<Action> {
        let clean = clean && !self.hard;
        let saved = if self.hard { None } else { saved };
        self.phase = Phase::Done { clean, code };
        let mut v = Vec::new();
        if self.hard {
            v.push(Action::RemovePartial);
        }
        v.push(Action::Stamp { clean, saved });
        if self.suspend && clean && saved.is_some() && !self.quit_after {
            v.push(Action::Event {
                kind: "suspend".into(),
                text: text::suspend_saved(),
            });
            v.push(Action::Suspended);
            return v;
        }
        v.push(Action::Event {
            kind: "stop".into(),
            text: match (clean, saved) {
                (true, Some(_)) => "quit with save".into(),
                (true, None) => "quit without save".into(),
                (false, _) => "stopped without save".into(),
            },
        });
        v.push(Action::Exit(code));
        v
    }
}

#[cfg(test)]
#[path = "quit_suspend_tests.rs"]
mod suspend_tests;
#[cfg(test)]
#[path = "quit_tests.rs"]
mod tests;
