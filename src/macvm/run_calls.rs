//! The calls on the VM outside the start, macOS only: `pause`, `save` and
//! `stop` of the quit path, and the freeze of `paddock freeze` and `paddock
//! thaw`, which `pause.rs` folds. Every call runs on the main queue, and
//! its answer comes back as an event of the loop. `run.rs` holds the
//! session.

use super::pause::{Ask, Do};
use super::run::{Event, Session};
use super::run_vm::{say, OnQueue};
use super::session;
use super::text;
use crate::vm::vz::{error_text, url};
use block2::RcBlock;
use objc2_foundation::NSError;
use std::sync::mpsc::Sender;
use std::time::Instant;

/// The answer channel of one `pause` or `resume` line.
pub(super) type Reply = Sender<Result<String, String>>;

/// The freeze of the session: the fold and the lines that wait.
#[derive(Default)]
pub(super) struct Freeze {
    pub(super) fold: super::pause::Pause,
    waiting: Vec<Reply>,
}

impl Freeze {
    pub(super) fn frozen(&self) -> bool {
        self.fold.frozen()
    }
}

impl Session<'_> {
    pub(super) fn pause(&self) {
        let vm = self.vm.clone();
        let tx = self.tx.clone();
        self.queue.exec_async(move || {
            let done = RcBlock::new(move |err: *mut NSError| {
                let _ = tx.send(Event::Paused(error_text(err)));
            });
            unsafe { vm.0.pauseWithCompletionHandler(&done) };
        });
    }

    pub(super) fn save(&self) {
        let partial = session::partial(&self.plan.profile.state());
        let _ = std::fs::remove_file(&partial);
        let vm = self.vm.clone();
        let tx = self.tx.clone();
        let target = match url(&partial) {
            Ok(u) => OnQueue(u),
            Err(e) => {
                let _ = tx.send(Event::Saved(Err(e.to_string())));
                return;
            }
        };
        self.queue.exec_async(move || {
            let done = RcBlock::new(move |err: *mut NSError| {
                let _ = tx.send(Event::Saved(error_text(err)));
            });
            unsafe {
                vm.0.saveMachineStateToURL_completionHandler(target.get(), &done)
            };
        });
    }

    pub(super) fn stop(&self) {
        let vm = self.vm.clone();
        let tx = self.tx.clone();
        self.queue.exec_async(move || {
            let vm = &vm.0;
            if !unsafe { vm.canStop() } {
                let _ = tx.send(Event::Stopped(Err(text::vm_cannot_stop())));
                return;
            }
            let done = RcBlock::new(move |err: *mut NSError| {
                let _ = tx.send(Event::Stopped(error_text(err)));
            });
            unsafe { vm.stopWithCompletionHandler(&done) };
        });
    }

    /// A `pause` or `resume` line of the control socket.
    pub(super) fn freeze_line(&mut self, pause: bool, reply: Reply) {
        if pause && self.quitting {
            let _ = reply.send(Err(text::pause_while_quitting()));
            return;
        }
        let ask = if pause { Ask::Pause } else { Ask::Resume };
        let steps = self.freeze.fold.ask(ask);
        self.freeze_steps(steps, Some(reply));
    }

    /// The quit path of a frozen session resumes the VM first. True when
    /// the quit begins now, false when it waits for the resume.
    pub(super) fn freeze_quit(&mut self, why: &str) -> bool {
        let steps = self.freeze.fold.ask(Ask::Quit(why.to_string()));
        let now = steps.iter().any(|s| matches!(s, Do::BeginQuit(_)));
        if !now {
            say(&text::thaw_before_quit(&self.plan.app));
        }
        let steps = steps
            .into_iter()
            .filter(|s| !matches!(s, Do::BeginQuit(_)))
            .collect();
        self.freeze_steps(steps, None);
        now
    }

    /// The framework answered a pause or a resume of the freeze.
    pub(super) fn freeze_done(&mut self, pause: bool, r: Result<(), String>) {
        let steps = self.freeze.fold.done(pause, r, Instant::now());
        self.freeze_steps(steps, None);
    }

    fn freeze_steps(&mut self, steps: Vec<Do>, mut reply: Option<Reply>) {
        for step in steps {
            match step {
                Do::CallPause => self.freeze_call(true),
                Do::CallResume => self.freeze_call(false),
                Do::Wait => self.freeze.waiting.extend(reply.take()),
                Do::Answer(r) => {
                    if let Some(tx) = reply.take() {
                        let _ = tx.send(r.map(|_| String::new()));
                    }
                }
                Do::AnswerAll(r) => {
                    for tx in self.freeze.waiting.drain(..) {
                        let _ = tx.send(r.clone().map(|_| String::new()));
                    }
                    if let Err(e) = &r {
                        say(e);
                        self.events.record("freeze", e, &[]);
                    }
                }
                Do::Frozen => {
                    self.events.record("freeze", &text::frozen_line(), &[]);
                    self.set_badge(Some(text::frozen_badge()));
                }
                Do::Thawed(d) => {
                    let line = text::thawed_line(d.as_secs());
                    self.events
                        .record("thaw", &line, &[("secs", d.as_secs_f64())]);
                    // The pause was paddock's: the clocks of the guest
                    // start again from now.
                    self.watch.thawed(Instant::now());
                    if let Some(agent) = &mut self.agent {
                        agent.delay(d);
                    }
                    if let Some(h) = &mut self.hello_deadline {
                        *h += d;
                    }
                    if self.shell.badge() == Some(text::frozen_badge().as_str()) {
                        self.set_badge(None);
                    }
                }
                Do::BeginQuit(why) => self.begin_quit_now(&why),
            }
        }
    }

    /// `VZVirtualMachine.pause` or `resume` on the main queue, answered
    /// by `Event::Frozen`.
    fn freeze_call(&self, pause: bool) {
        let vm = self.vm.clone();
        let tx = self.tx.clone();
        self.queue.exec_async(move || {
            let vm = &vm.0;
            let can = if pause {
                unsafe { vm.canPause() }
            } else {
                unsafe { vm.canResume() }
            };
            if !can {
                let _ = tx.send(Event::Frozen(pause, Err(text::vm_state_refused())));
                return;
            }
            let done = RcBlock::new(move |err: *mut NSError| {
                let _ = tx.send(Event::Frozen(pause, error_text(err)));
            });
            if pause {
                unsafe { vm.pauseWithCompletionHandler(&done) };
            } else {
                unsafe { vm.resumeWithCompletionHandler(&done) };
            }
        });
    }
}
