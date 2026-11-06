//! The input sources of the shim `paddock-vm app`, macOS only: the host
//! facts for `input::InputSync` and the run of its actions. A host select
//! or restore is a `TISSelectInputSource` (`tis.rs`), a guest set is the
//! `input.set` request, the badge goes on the session windows, and after
//! every batch the saved host source goes into the session record, so
//! `paddock app list` restores it when the shim dies (item 21 of the
//! checklist). `run.rs` holds the session.

use super::input::{Action, HostState, InputSync};
use super::proto::Request;
use super::run::{Session, Why};
use super::tis;
use objc2_app_kit::NSWorkspace;
use std::time::{Duration, Instant};

/// After the last session window resigned key, the restore waits this
/// long for another session window to become key, so a click from one
/// window of the session to another does not flip the host's source
/// twice. The Mac check `restore` allows 200 ms.
pub const RESIGN_GRACE: Duration = Duration::from_millis(100);

/// The input state of the session, in the shell.
pub(super) struct InputShell {
    pub(super) sync: InputSync,
    /// When the pending resign fires.
    resign_at: Option<Instant>,
    /// The badge this side set, so a clear touches only its own.
    badge: Option<String>,
}

impl InputShell {
    pub(super) fn new() -> Self {
        InputShell {
            sync: InputSync::new(),
            resign_at: None,
            badge: None,
        }
    }
}

/// The host facts: the enabled sources, the current one and the two
/// accessibility settings that the guest mirrors.
fn host_state() -> HostState {
    let ws = NSWorkspace::sharedWorkspace();
    HostState {
        sources: tis::enabled_sources(),
        current: tis::current().unwrap_or_default(),
        reduce_motion: ws.accessibilityDisplayShouldReduceMotion(),
        increase_contrast: ws.accessibilityDisplayShouldIncreaseContrast(),
    }
}

impl Session<'_> {
    /// The `hello` of the agent: the guest gets the host's sources.
    pub(super) fn input_hello(&mut self) {
        let host = host_state();
        let actions = self.shell.input.sync.on_hello(&host);
        self.run_input(actions);
    }

    /// A session window became key.
    pub(super) fn input_became_key(&mut self) {
        self.shell.input.resign_at = None;
        if self.shell.input.sync.is_key() {
            return;
        }
        let host = host_state();
        let actions = self.shell.input.sync.on_key(&host);
        self.run_input(actions);
    }

    /// A session window resigned key: the restore follows after the
    /// grace, unless another session window takes the key.
    pub(super) fn input_resigned(&mut self) {
        if self.shell.input.sync.is_key() {
            self.shell.input.resign_at = Some(Instant::now() + RESIGN_GRACE);
        }
    }

    /// The pending resign, from the tick of the run loop.
    pub(super) fn input_tick(&mut self) {
        let Some(at) = self.shell.input.resign_at else {
            return;
        };
        if Instant::now() < at {
            return;
        }
        self.shell.input.resign_at = None;
        if self.shell.windows.key_id().is_some() {
            return;
        }
        let actions = self.shell.input.sync.on_resign();
        self.run_input(actions);
    }

    /// The host's current source changed.
    pub(super) fn input_host_changed(&mut self) {
        if !self.shell.input.sync.is_key() {
            return;
        }
        let host = host_state();
        let actions = self.shell.input.sync.on_host_change(&host);
        self.run_input(actions);
    }

    /// A lone Globe key on a session window.
    pub(super) fn input_globe(&mut self) {
        if !self.shell.input.sync.is_key() {
            return;
        }
        let host = host_state();
        let actions = self.shell.input.sync.on_globe(&host);
        self.run_input(actions);
    }

    /// The reply of the guest to `input.set`.
    pub(super) fn input_reply(&mut self, missing: &[String]) {
        if !missing.is_empty() {
            self.events.record(
                "input",
                "sources missing in the guest",
                &[("count", missing.len() as f64)],
            );
        }
        let actions = self.shell.input.sync.on_guest_reply(missing);
        self.run_input(actions);
    }

    /// `input.set` failed or timed out.
    pub(super) fn input_failed(&mut self) {
        let actions = self.shell.input.sync.on_guest_error();
        self.run_input(actions);
    }

    /// The end of the shim: the host gets its source back.
    pub(super) fn input_exit(&mut self) {
        let actions = self.shell.input.sync.on_exit();
        self.run_input(actions);
    }

    fn run_input(&mut self, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        for a in actions {
            self.events.record("input", &a.kind(), &[]);
            match a {
                Action::HostSelect(id) | Action::Restore(id) => {
                    if let Err(e) = tis::select(&id) {
                        log::warn!("macvm: {e}");
                        self.events.record("error", &e, &[]);
                    }
                }
                Action::GuestSet {
                    sources,
                    current,
                    reduce_motion,
                    increase_contrast,
                } => {
                    self.send(
                        Request::InputSet {
                            sources,
                            current,
                            reduce_motion,
                            increase_contrast,
                        },
                        Why::InputSet,
                    );
                }
                Action::Badge(b) => self.input_badge(b),
            }
        }
        let saved = self.shell.input.sync.saved().unwrap_or("").to_string();
        let differs = self.shell.input.sync.guest_differs();
        self.update_record(|v| {
            v["input_source"] = serde_json::json!(saved);
            v["guest_input"] = serde_json::json!(differs);
        });
    }

    /// The badge of this side: a text replaces the badge; a clear removes
    /// the badge only while it is still this side's.
    fn input_badge(&mut self, badge: Option<String>) {
        match badge {
            Some(text) => {
                self.shell.input.badge = Some(text.clone());
                self.set_badge(Some(text));
            }
            None => {
                if let Some(old) = self.shell.input.badge.take() {
                    if self.shell.badge() == Some(old.as_str()) {
                        self.set_badge(None);
                    }
                }
            }
        }
    }
}
