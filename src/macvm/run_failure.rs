//! The failure matrix in the shim, macOS only: the executor of the steps of
//! `failure.rs`, the tick that feeds the fold (the health gap, the wake,
//! the marker) and the disk watch of `space.rs`. `run.rs` holds the
//! session.

use super::failure::{self, Step};
use super::quit::{Config as QuitConfig, Quit};
use super::run::{Session, Why};
use super::run_vm::say;
use super::session;
use super::text;
use std::time::{Duration, Instant};

/// How often the shim looks for a dead marker.
const MARKER_CHECK: Duration = Duration::from_secs(60);

impl Session<'_> {
    /// The per-tick feed of the fold: the sleep of the host, the health
    /// gap stages, the wake pings and the marker.
    pub(super) fn failure_tick(&mut self, now: Instant) {
        if !self.quitting && !self.freeze.frozen() {
            // A sleep of the host: the wake row of the failure matrix.
            let wall = crate::state::now();
            if let Some(woke) = self.sleep.check(wall) {
                self.woke_at = Some(woke);
                let slept = Duration::from_secs(woke.saturating_sub(self.sleep_began));
                self.events.record(
                    "sleep",
                    &text::sleep_line(),
                    &[("secs", slept.as_secs_f64())],
                );
                let steps = self.watch.woke(slept, now);
                self.execute(steps);
            }
            self.sleep_began = wall;
            let steps = self.watch.tick(now);
            self.execute(steps);
            if now.duration_since(self.marker_checked) >= MARKER_CHECK {
                self.marker_checked = now;
                let steps = failure::respond(&failure::Failure::MarkerDied, self.watch.ctx());
                self.execute(steps);
            }
        }
    }

    /// The disk space of the volume (the full disk row), every 10 s:
    /// each level once, when it is reached.
    pub(super) fn disk_tick(&mut self) {
        // The disk space of the volume (the full disk row): each
        // level once, when it is reached.
        if !self.quitting {
            let dir = self.plan.profile.dir.clone();
            let level = super::space::watch(&session::free_bytes, &dir);
            if level.is_some() && level > self.disk_level {
                self.disk_level = level;
                if let Some(l) = level {
                    let steps = failure::respond(&failure::Failure::DiskFull(l), self.watch.ctx());
                    self.execute(steps);
                }
            }
        }
    }

    /// Execute the steps of the failure fold. `ColdBoot` needs no step of
    /// its own: the `Stopped` event of a VM that is not quitting boots cold
    /// with the same arguments (`after_hello_timeout_stop`).
    pub(super) fn execute(&mut self, steps: Vec<Step>) {
        for step in steps {
            match step {
                Step::Badge(b) => self.set_badge(Some(b)),
                Step::ClearBadge => self.set_badge(None),
                Step::DockEntry(failure::Entry::RestartGuest) => {
                    self.shell.dock.update(|s| s.silent = true)
                }
                Step::DockEntry(failure::Entry::OpenAgain) => self.app_exited_dock(),
                Step::Send(req) => {
                    self.send(req, Why::Other);
                }
                Step::StopVm { .. } => {
                    self.shell.dock.update(|s| s.silent = false);
                    self.last_health = None;
                    self.stop();
                }
                Step::ColdBoot => {}
                Step::Discard(what) => {
                    let _ = session::remove(&self.plan.profile, what);
                }
                Step::Backup(_) => {}
                Step::Line(l) => say(&l),
                Step::Event { kind, text } => self.events.record(kind, &text, &[]),
                Step::CloseWindows => self.close_windows(),
                Step::CloseTunnels => {
                    let n = crate::proxy::close_idle_tunnels();
                    log::info!("macvm: closed {n} idle tunnels at the wake");
                }
                Step::DenyStale => {
                    if let (Some(woke), Some(asker)) = (self.woke_at, self.firewall.asker()) {
                        asker.note_wake(woke);
                    }
                }
                Step::DropAgent => {
                    self.agent = None;
                    self.why.clear();
                }
                Step::Relaunch(allowed) => {
                    self.relaunch_allowed = allowed;
                    if !allowed {
                        self.shell.dock.update(|s| s.app_exited = false);
                    }
                }
                Step::SkipSave => {
                    if !self.quitting {
                        self.quit = Quit::new(QuitConfig::new(
                            &self.plan.app,
                            false,
                            self.plan.memory_bytes,
                        ));
                    }
                }
                // `mark::spawn` starts nothing while a marker runs.
                Step::RestartMarker => crate::mark::spawn(self.env),
                Step::Exit(code) => self.end(false, code),
            }
        }
    }
}
