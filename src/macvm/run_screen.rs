//! The guest screen lock and the login keychain in the app session,
//! macOS only. The fold of `screen_lock.rs` decides; this file records
//! its events, types the password through the keyboard of the VM
//! (`vz_mac_input.rs`, with the key pacing of the guided window), and
//! sends `keychain.unlock` before the app starts. The password goes to no
//! log, no event and no argv.

use super::guided_ui::{self, Played, KEY_GAP_MS};
use super::proto::{ErrorCode, Outcome, ReplyData, Request};
use super::run::{Event, Session, Why};
use super::run_vm::say;
use super::screen_lock::{Action, Facts, Unlocker};
use super::text;
use dispatch2::DispatchQueue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The lock state of the session.
pub(super) struct Screen {
    unlocker: Unlocker,
    /// The newest report says locked: the typing thread reads it before
    /// each key.
    flag: Arc<AtomicBool>,
}

impl Screen {
    pub(super) fn new(has_password: bool) -> Self {
        Screen {
            unlocker: Unlocker::new(has_password),
            flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Session<'_> {
    /// A report of the agent: `screen.lock` (with the idle time) or a
    /// `health`.
    pub(super) fn screen_report(&mut self, locked: bool, uptime: u64, idle_secs: Option<u64>) {
        self.screen.flag.store(locked, Ordering::SeqCst);
        let facts = Facts {
            at: crate::waiting::format_rfc3339(crate::state::now()),
            uptime,
            idle_secs,
        };
        let actions = self.screen.unlocker.report(locked, &facts, Instant::now());
        self.screen_actions(actions, Some(&facts));
    }

    /// The agent connection is gone: no key goes until a new report.
    pub(super) fn screen_agent_gone(&mut self) {
        self.screen.flag.store(false, Ordering::SeqCst);
    }

    /// The wait of a try ended.
    pub(super) fn screen_tick(&mut self, now: Instant) {
        if !self.screen.unlocker.busy() {
            return;
        }
        let actions = self.screen.unlocker.tick(now);
        self.screen_actions(actions, None);
    }

    /// The typing thread ended.
    pub(super) fn screen_typed(&mut self, played: Played) {
        let actions = self.screen.unlocker.typed(played, Instant::now());
        self.screen_actions(actions, None);
    }

    fn screen_actions(&mut self, actions: Vec<Action>, facts: Option<&Facts>) {
        for a in actions {
            match a {
                Action::Event(line) => {
                    log::info!("macvm: {line}");
                    let mut nums = Vec::new();
                    if let Some(f) = facts {
                        nums.push(("uptime", f.uptime as f64));
                        if let Some(i) = f.idle_secs {
                            nums.push(("idle", i as f64));
                        }
                    }
                    self.events.record("screen", &line, &nums);
                }
                Action::Line(line) => {
                    say(&line);
                    self.events.record("screen", &line, &[]);
                }
                Action::Type(attempt) => self.screen_type(attempt),
            }
        }
    }

    /// Type the password and Return into the guest on a thread of its
    /// own, paced as the Type buttons of the guided window. The lock is
    /// checked right before: the fold asked for a fresh locked report, and
    /// the thread reads the flag before each key.
    fn screen_type(&mut self, attempt: u32) {
        let now = Instant::now();
        if self.quitting || !self.screen.unlocker.may_type(now) {
            log::info!("macvm: no password typed (try {attempt}): the screen is not reported locked now or the session quits");
            let actions = self.screen.unlocker.typed(Played::Stopped, now);
            return self.screen_actions(actions, None);
        }
        let Some(password) = self.password.as_deref() else {
            let actions = self.screen.unlocker.give_up(text::screen_no_password());
            return self.screen_actions(actions, None);
        };
        let strokes = match guided_ui::key_strokes(&format!("{password}\n")) {
            Ok(s) => s,
            Err(_) => {
                // The character is part of the password: not named.
                let actions = self.screen.unlocker.give_up(text::screen_cannot_type());
                return self.screen_actions(actions, None);
            }
        };
        let actions = guided_ui::key_actions(&strokes);
        let vm = self.vm.clone();
        let flag = self.screen.flag.clone();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("screen-unlock".into())
            .spawn(move || {
                let played = guided_ui::play_keys(
                    &actions,
                    || flag.load(Ordering::SeqCst),
                    |a| {
                        let (code, down) = a.code_down();
                        let vm = vm.clone();
                        let sent = Arc::new(AtomicBool::new(false));
                        let done = sent.clone();
                        DispatchQueue::main().exec_sync(move || {
                            done.store(
                                super::vz_mac_input::key_vm(&vm.0, code, down),
                                Ordering::SeqCst,
                            )
                        });
                        sent.load(Ordering::SeqCst)
                    },
                    || std::thread::sleep(Duration::from_millis(KEY_GAP_MS)),
                );
                let _ = tx.send(Event::Typed(played));
            });
        if let Err(e) = spawned {
            log::warn!("macvm: cannot start the typing thread: {e}");
            let actions = self.screen.unlocker.typed(Played::Failed, now);
            self.screen_actions(actions, None);
        }
    }

    /// `keychain.unlock` before the app starts: the login keychain of the
    /// guest is unlocked after the home moved, and after a warm start.
    pub(super) fn keychain_request(&mut self) {
        let Some(password) = self.password.clone() else {
            self.events
                .record("keychain", &text::keychain_no_password(), &[]);
            return;
        };
        self.send(Request::KeychainUnlock { password }, Why::Keychain);
    }

    /// The reply of `keychain.unlock`, or of `bench.keychain`.
    pub(super) fn keychain_reply(&mut self, outcome: Outcome, bench: bool) {
        match outcome {
            Outcome::Ok {
                data: Some(ReplyData::Text { text: line }),
            } => {
                let line = if bench {
                    format!("bench: {line}") // not user text
                } else {
                    line
                };
                log::info!("macvm: keychain: {line}");
                self.events.record("keychain", &line, &[]);
            }
            Outcome::Ok { .. } => {}
            Outcome::Error {
                code: ErrorCode::Unsupported,
                text: detail,
            } => {
                // An agent without the keychain calls.
                log::info!("macvm: keychain: {detail}");
                self.events.record("keychain", &detail, &[]);
            }
            Outcome::Error { text: detail, .. } => {
                let line = text::keychain_failed(&detail);
                say(&line);
                self.events.record("keychain", &line, &[]);
            }
        }
    }
}
