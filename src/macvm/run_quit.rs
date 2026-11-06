//! The quit path of one app session: the hello timeout, the begin of a
//! quit, the inputs to the state machine of `quit.rs` and its actions.
//! macOS only. The session itself is in `run.rs`.

use super::proto::Request;
use super::quit::{Action, Config as QuitConfig, Input, Quit};
use super::run::{Event, Session, Why, HELLO_COLD, HELLO_WARM};
use super::run_vm::say;
use super::session::{self, Start};
use super::text;
use super::vz_mac_agent::AgentEvent;
use std::time::{Duration, Instant};

/// At the exit, the most time that the file of a `log.fetch` still gets.
const LOG_WAIT: Duration = Duration::from_secs(5);

impl Session<'_> {
    /// No `hello` in time: a warm start retries once as a cold boot, a
    /// cold start ends with the console log path.
    pub(super) fn hello_timeout(&mut self) {
        let path = self.plan.profile.console_log();
        if self.start == Start::Warm && !self.retried_cold {
            say(&text::hello_restore_cold());
            self.events
                .record("hello", &text::hello_restore_cold(), &[]);
            self.retried_cold = true;
            self.stop();
            return;
        }
        let wait = if self.start == Start::Warm {
            HELLO_WARM
        } else {
            HELLO_COLD
        };
        say(&text::hello_timeout(wait, &path));
        self.events
            .record("error", &text::hello_timeout(wait, &path), &[]);
        self.retried_cold = true;
        self.quitting = true;
        self.failed = true;
        // No agent and no save: the machine stops the VM at once, and the
        // stamp says the session was not clean.
        self.quit = Quit::new(QuitConfig::new(
            &self.plan.app,
            false,
            self.plan.memory_bytes,
        ));
        self.feed(Input::Begin { agent: false });
    }

    /// The start cannot go on (a locked data disk): the VM stops at once,
    /// nothing is saved, the stamp says not clean, exit 1.
    pub(super) fn end_start_without_save(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.failed = true;
        // The agent log says why: it comes while the VM stops.
        self.fetch_log();
        self.quit = Quit::new(QuitConfig::new(
            &self.plan.app,
            false,
            self.plan.memory_bytes,
        ));
        self.feed(Input::Begin { agent: false });
    }

    /// After the stop of a restored VM that gave no hello: boot cold.
    pub(super) fn after_hello_timeout_stop(&mut self) {
        let _ = session::remove(&self.plan.profile, session::What::State);
        self.start = Start::Cold;
        self.boot();
    }

    pub(super) fn begin_quit(&mut self, why: &str) {
        // A frozen session resumes first (`run_calls.rs`), then quits.
        if self.freeze_quit(why) {
            self.begin_quit_now(why);
        }
    }

    /// The quit path itself, on a VM that runs.
    pub(super) fn begin_quit_now(&mut self, why: &str) {
        say(&text::quit_begin(why));
        self.quitting = true;
        self.note_open_drops();
        let agent = self.agent.is_some();
        // A prepare that still runs ends for this start; a saved state of
        // a home on the system disk is never made (`data_start.rs`).
        let cfg = QuitConfig::new(&self.plan.app, self.plan.save, self.plan.memory_bytes);
        let (save, actions) = self.data.at_quit(&super::data_start::AtQuit {
            save: self.plan.save,
            prepared: self.stamp.data_prepared,
            password: self.password.is_some(),
            free: session::free_bytes(&self.plan.profile.dir),
            needs: cfg.save_needs(),
        });
        self.data_actions(actions);
        if self.plan.save && !save && self.quit.phase() == super::quit::Phase::Idle {
            self.quit = Quit::new(QuitConfig::new(
                &self.plan.app,
                false,
                self.plan.memory_bytes,
            ));
        }
        self.fetch_log();
        self.feed(Input::Begin { agent });
    }

    /// `log.fetch` to the agent, once per session: the log comes over the
    /// files port while the app quits, and `run_files.rs` puts it at
    /// `logs/agent.log`. A timeout or an error is one event.
    fn fetch_log(&mut self) {
        if self.agent.is_some() && !self.log_wanted && self.send(Request::LogFetch, Why::LogFetch) {
            self.log_wanted = true;
        }
    }

    /// The quit is done: the file of the `log.fetch` gets [`LOG_WAIT`]
    /// more, so a VM that stopped right after the transfer still leaves
    /// `logs/agent.log`. Every other event is dropped.
    pub(super) fn wait_for_log(&mut self, rx: &std::sync::mpsc::Receiver<Event>) {
        if !self.log_wanted {
            return;
        }
        let came = super::logs::wait_for(rx, LOG_WAIT, |ev| {
            if let Event::Agent(AgentEvent::File { kind, name, path }) = ev {
                self.guest_file(kind, name, path);
            }
            !self.log_wanted
        });
        if !came {
            self.log_wanted = false;
            self.events
                .record("files", &text::log_fetch_missing(LOG_WAIT.as_secs()), &[]);
        }
    }

    /// One input to the quit machine, and its actions.
    pub(super) fn feed(&mut self, input: Input) {
        let actions = self.quit.step(input);
        self.act(actions);
    }

    /// Run the actions of the quit machine.
    fn act(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::SendQuit { force } => {
                    let app = self.plan.app.clone();
                    if !self.send(Request::AppQuit { app, force }, Why::AppQuit) {
                        // No agent after all: go on as if the app were gone.
                        self.feed(Input::AppExited);
                    }
                }
                Action::Deadline(d) => self.quit_deadline = Some(Instant::now() + d),
                Action::CloseWindows => self.close_windows(),
                Action::CheckSpace => {
                    let free = session::free_bytes(&self.plan.profile.dir);
                    self.feed(Input::Space(free));
                }
                Action::Pause => self.pause(),
                Action::Save => self.save(),
                Action::CommitState => {
                    let partial = session::partial(&self.plan.profile.state());
                    if let Err(e) = session::commit(&partial) {
                        say(&text::state_commit_failed(&e.to_string()));
                    }
                }
                Action::RemovePartial => {
                    let _ = std::fs::remove_file(session::partial(&self.plan.profile.state()));
                }
                Action::Resume => self.resume(false),
                Action::SendShutdown => {
                    // The guest password for `sudo -S` in the agent; the
                    // app has quit, so the agent takes it (`shutdown.rs`).
                    let Some(password) = self.password.clone() else {
                        // No password, no `sudo`: the VM stops at once.
                        let why = text::shutdown_no_session_password();
                        self.feed(Input::ShutdownRefused(why));
                        continue;
                    };
                    if !self.send(Request::Shutdown { password }, Why::Shutdown) {
                        self.feed(Input::Timeout);
                    }
                }
                Action::Stop => self.stop(),
                Action::Stamp { clean, saved } => {
                    let secs = now_secs();
                    if let Err(e) =
                        session::finish(&self.plan.profile, &mut self.stamp, clean, saved, secs)
                    {
                        say(&text::stamp_write_failed(&format!("{e:#}")));
                    }
                }
                Action::Line(l) => say(&l),
                Action::Event { kind, text } => self.events.record(&kind, &text, &[]),
                Action::Suspended => self.enter_suspended(),
                Action::SuspendRefused(line) => self.suspend_refused(line),
                Action::Exit(code) => {
                    self.sweep_drops();
                    self.exit = Some(if self.failed { 1 } else { code });
                }
            }
        }
    }

    /// End without the quit path: the stamp, then exit.
    pub(super) fn end(&mut self, clean: bool, code: i32) {
        let _ = session::finish(&self.plan.profile, &mut self.stamp, clean, None, now_secs());
        self.exit = Some(code);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
