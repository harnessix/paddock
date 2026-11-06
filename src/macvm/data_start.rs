//! The data disk at the start of a session, as a state machine without any
//! framework call, so the tests run on Linux. `run_agent.rs` feeds it the
//! hello, the answer of `data.prepare` and its timeout, and runs the
//! actions.
//!
//! The data disk (`data.img`) holds the home of the guest account, so the
//! logins and the files of the app survive a new golden. The host formats
//! it before the first boot (`data_disk.rs`); the guest agent moves the
//! home onto it at the first hello of a profile whose stamp says
//! `data_prepared: false` (`guest/data.rs`). After that the host sends
//! nothing: the mount daemon of the guest mounts the volume at boot, and
//! the agent verifies it after an unclean end (the `data_verify` of the
//! health).
//!
//! 1. A prepared profile: the app starts at once.
//! 2. A warm start of a profile that is not prepared: the restored guest
//!    runs the app on the system disk, and a move of the home under it
//!    would break it. The app runs; the quit saves nothing
//!    ([`save_at_quit`]), so the next start boots cold and prepares.
//! 3. No guest password: no request (it needs `sudo`); the failure is in
//!    the events and the stamp, and the app starts.
//! 4. Otherwise `data.prepare {password, data_id}`, and the app waits
//!    for its answer, at most the timeout of the request. Ok: the stamp
//!    says prepared. `locked`: the disk belongs to another system disk, so
//!    the start ends without a save and no disk changes. Any other error
//!    or the timeout: the app starts with the home on the system disk, and
//!    the next cold start tries again. A new hello while the request
//!    waits (the agent restarted) sends it again, at most [`MAX_SENDS`]
//!    times: the prepare of the guest is safe to run twice.

use super::proto::{ErrorCode, Outcome, ReplyData};
use super::text;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The sends of one start: the first and two after agent restarts.
pub const MAX_SENDS: u32 = 3;
/// The host waits this long on top of the time that a progress line of
/// the guest names.
pub const PROGRESS_MARGIN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No hello yet.
    Idle,
    /// `data.prepare` is out; the app waits.
    Waiting,
    /// Decided for this start: the app may start.
    Done,
}

/// What the session knows at a hello.
#[derive(Debug, Clone)]
pub struct Hello<'a> {
    /// `data_prepared` of the stamp.
    pub prepared: bool,
    /// The start restores a saved state.
    pub warm: bool,
    pub password: Option<&'a str>,
    pub data_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `data.prepare` to the agent.
    Send { password: String, data_id: String },
    /// The app may start now (`start_app`, which runs once).
    StartApp,
    /// One line of `events.jsonl`.
    Event { kind: &'static str, text: String },
    /// One line for the user.
    Line(String),
    /// Write the stamp: the prepare flag, and the failure that the status
    /// shows (`None` clears it).
    Stamp {
        prepared: bool,
        error: Option<String>,
    },
    /// A data disk of another system disk: end the start without a save.
    /// `detail` is the text of the agent.
    EndLocked { detail: String },
    /// The pending `data.prepare` waits this long from now: the guest
    /// said how long the rest may take.
    Extend(Duration),
}

/// What the quit needs to know of the data disk.
#[derive(Debug, Clone)]
pub struct AtQuit {
    /// `[macvm] save`.
    pub save: bool,
    /// `data_prepared` of the stamp.
    pub prepared: bool,
    pub password: bool,
    /// The free bytes of the volume of the profile, and what a save needs.
    pub free: u64,
    pub needs: u64,
}

#[derive(Debug)]
pub struct DataStart {
    phase: Phase,
    sends: u32,
}

impl Default for DataStart {
    fn default() -> Self {
        Self::new()
    }
}

impl DataStart {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            sends: 0,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// A hello of the agent, the first of the start or one after a
    /// restart of the agent.
    pub fn hello(&mut self, h: &Hello<'_>) -> Vec<Action> {
        match self.phase {
            Phase::Done => return vec![Action::StartApp],
            Phase::Waiting if self.sends >= MAX_SENDS => {
                return self.failed(text::data_agent_restarts(self.sends))
            }
            Phase::Waiting => {
                // The agent restarted while it prepared: ask again.
                if let Some(pw) = h.password {
                    return self.send(pw, h.data_id);
                }
            }
            Phase::Idle => {}
        }
        if h.prepared {
            self.phase = Phase::Done;
            return vec![Action::StartApp];
        }
        if h.warm {
            self.phase = Phase::Done;
            return vec![
                Action::Event {
                    kind: "data",
                    text: text::data_prepare_warm(),
                },
                Action::StartApp,
            ];
        }
        match h.password {
            Some(pw) => self.send(pw, h.data_id),
            None => self.failed(text::data_no_password()),
        }
    }

    fn send(&mut self, password: &str, data_id: &str) -> Vec<Action> {
        self.phase = Phase::Waiting;
        self.sends += 1;
        let mut v = Vec::new();
        if self.sends == 1 {
            v.push(Action::Event {
                kind: "data",
                text: text::data_preparing(),
            });
        }
        v.push(Action::Send {
            password: password.to_string(),
            data_id: data_id.to_string(),
        });
        v
    }

    /// The answer of the agent to `data.prepare`.
    pub fn answer(&mut self, outcome: &Outcome) -> Vec<Action> {
        if self.phase != Phase::Waiting {
            return vec![];
        }
        match outcome {
            Outcome::Ok { data } => {
                self.phase = Phase::Done;
                let verify = match data {
                    Some(ReplyData::DataPrepare { verify }) => verify.trim(),
                    _ => "",
                };
                vec![
                    Action::Stamp {
                        prepared: true,
                        error: None,
                    },
                    Action::Event {
                        kind: "data",
                        text: text::data_prepared(last_line(verify)),
                    },
                    Action::StartApp,
                ]
            }
            Outcome::Error {
                code: ErrorCode::Locked,
                text: detail,
            } => {
                self.phase = Phase::Done;
                vec![
                    Action::Stamp {
                        prepared: false,
                        error: Some(detail.clone()),
                    },
                    Action::EndLocked {
                        detail: detail.clone(),
                    },
                ]
            }
            Outcome::Error { code, text: detail } => {
                let detail = text::io_failed(code.describe(), detail);
                self.failed(text::data_prepare_failed(&detail))
            }
        }
    }

    /// A progress line of the guest while `data.prepare` runs: the event,
    /// and the longer wait for its answer.
    pub fn progress(&mut self, line: &str, wait: Duration) -> Vec<Action> {
        if self.phase != Phase::Waiting {
            return vec![];
        }
        vec![
            Action::Event {
                kind: "data",
                text: line.to_string(),
            },
            Action::Extend(wait + PROGRESS_MARGIN),
        ]
    }

    /// The quit begins: whether it may save the machine state, and what
    /// to say. A prepare that still runs ends here for this start (the
    /// guest stops it at the shutdown). A save that the data disk forbids
    /// still says when the volume has too little space for it, as the save
    /// itself would.
    pub fn at_quit(&mut self, q: &AtQuit) -> (bool, Vec<Action>) {
        let mut v = Vec::new();
        if self.phase == Phase::Waiting {
            self.phase = Phase::Done;
            let line = text::data_prepare_stopped_by_quit();
            v.push(Action::Event {
                kind: "data",
                text: line.clone(),
            });
            v.push(Action::Stamp {
                prepared: false,
                error: Some(line),
            });
        }
        let save = save_at_quit(q.save, q.prepared, q.password);
        if q.save && !save {
            v.push(Action::Event {
                kind: "save",
                text: text::data_no_save_unprepared(),
            });
            if q.free < q.needs {
                let line = super::session::save_refusal(q.needs, q.free);
                v.push(Action::Line(line.clone()));
                v.push(Action::Event {
                    kind: "save",
                    text: line,
                });
            }
        }
        (save, v)
    }

    /// No answer in the time of the request.
    pub fn timeout(&mut self, wait: Duration) -> Vec<Action> {
        if self.phase != Phase::Waiting {
            return vec![];
        }
        self.failed(text::data_prepare_timeout(wait))
    }

    /// The prepare did not happen on this start: the line, the event, the
    /// stamp, and the app starts on the system disk.
    fn failed(&mut self, line: String) -> Vec<Action> {
        self.phase = Phase::Done;
        vec![
            Action::Line(line.clone()),
            Action::Event {
                kind: "error",
                text: line.clone(),
            },
            Action::Stamp {
                prepared: false,
                error: Some(line),
            },
            Action::StartApp,
        ]
    }
}

/// Whether the quit may save the machine state. A saved state of a
/// profile whose home is still on the system disk would make every later
/// start warm, and a warm start never prepares: no save then, so the next
/// start boots cold and prepares. Without a password nothing can prepare
/// it, so the save stays as configured.
pub fn save_at_quit(save: bool, prepared: bool, password: bool) -> bool {
    save && (prepared || !password)
}

/// The last line of the verify output, the one that says OK or not.
fn last_line(s: &str) -> &str {
    s.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// The file of the guest password in the keyring of `paddock macvm
/// setup`.
pub fn keyring_file(env: &crate::env::HostEnv) -> PathBuf {
    env.paddock_state()
        .join("keyring")
        .join("macvm")
        .join("guest-password")
}

/// The guest password for a session: read from the keyring, never made.
/// A missing file is `Ok(None)`: the start goes on, and the stamp and the
/// events say what does not happen without it. A damaged file is an error
/// that names it, and also no password.
pub fn session_password(file: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(s) => {
            let s = s.strip_suffix('\n').unwrap_or(&s);
            let s = s.strip_suffix('\r').unwrap_or(s);
            if super::golden::is_keyring_password(s) {
                Ok(Some(s.to_string()))
            } else {
                Err(text::keyring_password_damaged(file))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(text::keyring_password_unreadable(file, &e.to_string())),
    }
}

/// The password for the shim of `paddock app run`. Without one the user
/// gets one warning line on stderr: the data disk and the shutdown at the
/// quit need it.
pub fn start_password(env: &crate::env::HostEnv) -> Option<String> {
    let file = keyring_file(env);
    match session_password(&file) {
        Ok(Some(p)) => Some(p),
        Ok(None) => {
            eprintln!("{}", text::session_password_missing(&file));
            None
        }
        Err(e) => {
            eprintln!("{e}");
            None
        }
    }
}

#[cfg(test)]
#[path = "data_start_tests.rs"]
mod tests;
