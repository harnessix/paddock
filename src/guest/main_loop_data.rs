//! `data.prepare` in the main loop: the prepare runs in a thread of its
//! own, so the agent answers `app.quit`, `log.fetch` and `ping` while the
//! home is copied. What must not run beside it waits or is refused:
//!
//! - `shutdown` cancels the prepare and waits for its end (a cancel stops
//!   the copy at once, and the link of the account is never cut in half),
//!   then shuts the guest down and answers.
//! - `app.launch`, `app.copy`, `configure`, `agent.update` and
//!   `agent.restart` get `busy`.
//! - A second `data.prepare` (the host after a restart of the connection)
//!   takes over the running one: its reply goes to the new request.

use super::{Msg, Session};
use crate::guest::data_copy::Watch;
use crate::guest::platform::Platform;
use crate::macvm::proto::{ErrorCode, Event, Reply, ReplyData, Request};
use crate::macvm::text;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Where the reply of a running prepare goes: the request and the
/// connection that wait for it.
struct Target {
    req: u64,
    tx: Sender<Msg>,
    waker: crate::guest::platform::Waker,
}

/// A `data.prepare` that runs in its thread.
pub(super) struct PrepareRun {
    cancel: Arc<AtomicBool>,
    /// The thread ended and its connection was gone: nobody gets the
    /// reply.
    orphan: Arc<AtomicBool>,
    target: Arc<Mutex<Target>>,
}

impl PrepareRun {
    /// Until its reply is handled in [`Session::prepared`], unless the
    /// reply had nowhere to go.
    fn running(&self) -> bool {
        !self.orphan.load(Ordering::SeqCst)
    }
}

/// What the gate says about a request while a prepare runs.
pub(super) enum Gate {
    /// Not touched by the prepare: dispatch it.
    Go,
    /// Answer it with this now.
    Reply(Reply),
    /// The answer comes later.
    Later,
}

impl<P: Platform> Session<'_, P> {
    /// True while a `data.prepare` thread runs. A run whose reply went to
    /// a connection that is gone is cleared here.
    pub(super) fn prepare_running(&mut self) -> bool {
        match &self.st.prepare {
            Some(run) if run.running() => true,
            Some(_) => {
                self.st.prepare = None;
                self.st.shutdown_after_prepare = None;
                false
            }
            None => false,
        }
    }

    /// The requests that a running prepare holds back.
    pub(super) fn prepare_gate(&mut self, req: u64, request: &Request) -> Gate {
        if !self.prepare_running() {
            return Gate::Go;
        }
        match request {
            Request::Shutdown { password } => {
                log::info!("shutdown while the data disk prepares: the prepare stops first");
                if let Some(run) = &self.st.prepare {
                    run.cancel.store(true, Ordering::SeqCst);
                }
                self.st.shutdown_after_prepare = Some((req, password.clone()));
                Gate::Later
            }
            Request::DataPrepare { .. } => {
                if let Some(run) = &self.st.prepare {
                    let mut t = run.target.lock().unwrap_or_else(|e| e.into_inner());
                    log::info!("data.prepare again: req {req} takes the running one");
                    *t = Target {
                        req,
                        tx: self.tx.clone(),
                        waker: self.waker.clone(),
                    };
                }
                Gate::Later
            }
            Request::AppLaunch { .. }
            | Request::AppCopy { .. }
            | Request::Configure { .. }
            | Request::AgentUpdate { .. }
            | Request::AgentRestart => Gate::Reply(Reply::error(
                req,
                ErrorCode::Busy,
                text::data_prepare_busy(),
            )),
            _ => Gate::Go,
        }
    }

    /// `data.prepare` with no prepare running: the job starts in its
    /// thread, and its reply comes as [`Msg::Prepared`].
    pub(super) fn data_prepare(&mut self, req: u64, password: &str, data_id: &str) {
        let job = self.p.data_prepare(password, data_id);
        let cancel = Arc::new(AtomicBool::new(false));
        let orphan = Arc::new(AtomicBool::new(false));
        let target = Arc::new(Mutex::new(Target {
            req,
            tx: self.tx.clone(),
            waker: self.waker.clone(),
        }));
        let writer = self.writer.clone();
        let watch = Watch {
            cancel: cancel.clone(),
            progress: Box::new(move |text, wait| {
                log::info!("{text}");
                let ev = Event::DataProgress {
                    text,
                    wait_secs: wait.as_secs(),
                };
                if let Err(e) = writer.send(&ev) {
                    log::warn!("cannot send the progress: {e}");
                }
            }),
        };
        let (t, o) = (target.clone(), orphan.clone());
        let spawned = std::thread::Builder::new()
            .name("data-prepare".into())
            .spawn(move || {
                let result = job(&watch);
                let t = t.lock().unwrap_or_else(|e| e.into_inner());
                let reply = match result {
                    Ok(verify) => Reply::data(t.req, ReplyData::DataPrepare { verify }),
                    Err((code, text)) => Reply::error(t.req, code, text),
                };
                if t.tx.send(Msg::Prepared(reply)).is_err() {
                    log::warn!("data.prepare ended after its connection");
                    o.store(true, Ordering::SeqCst);
                }
                (t.waker)();
            });
        match spawned {
            Ok(_) => {
                self.st.prepare = Some(PrepareRun {
                    cancel,
                    orphan,
                    target,
                })
            }
            Err(e) => {
                let r = Reply::error(
                    req,
                    ErrorCode::Internal,
                    text::cannot_run_command("data-prepare", &e.to_string()),
                );
                let _ = self.reply(r);
            }
        }
    }

    /// The prepare thread ended: its reply, then the shutdown that waited.
    pub(super) fn prepared(&mut self, reply: Reply) -> Result<(), String> {
        self.st.prepare = None;
        self.reply(reply)?;
        if let Some((req, password)) = self.st.shutdown_after_prepare.take() {
            let r = self.shutdown(req, &password);
            self.reply(r)?;
        }
        Ok(())
    }

    /// `shutdown` with the guest password. The password goes to `sudo -S`
    /// only with no app running, as for `configure`. An error reply lets
    /// the host stop the VM.
    pub(super) fn shutdown(&mut self, req: u64, password: &str) -> Reply {
        if self.st.follow.app_alive() {
            return Reply::error(req, ErrorCode::NotInSession, "an app runs in the guest");
        }
        match self.p.shutdown(password) {
            Ok(()) => Reply::ok(req),
            Err((code, text)) => {
                log::warn!("the shutdown failed: {text}");
                Reply::error(req, code, text)
            }
        }
    }
}
