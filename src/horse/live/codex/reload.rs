//! Load the thread of a custody again from its rollout file, after
//! paddock swapped the file for another branch under the same thread id.
//!
//! codex app-server keeps a loaded thread in memory and appends to its
//! rollout through a writer that it holds open. A `thread/resume` of a
//! thread that is loaded attaches to the copy in memory and does not read
//! the file. So paddock first drops the thread: `thread/unsubscribe`, and
//! the server unloads a thread without subscribers and says
//! `thread/closed`. Then the file is swapped, under the writer lock of the
//! thread, and `thread/resume` reads it.
//!
//! `thread/unsubscribe` and `thread/closed` are taken from the app-server
//! protocol; the method list of the pinned version that paddock checked
//! does not show them, so paddock fails closed here: when the server does
//! not know the call, answers another status, or does not close the
//! thread in time, paddock does not touch the file, the thread stays (or
//! is attached again), and the custody goes on with the swap refused. A
//! restart of the app-server child with the same id would drop the copy
//! in memory as well (the cache key is the thread id, not the process),
//! but the run owns that process: that restart is the next run, and a
//! stopped session swaps on disk (`paddock horse swap --codex-home`).

use super::{wire, Codex, Pending, Wait};
use crate::horse::live::harness::Reloaded;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// Where a reload is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// `thread/unsubscribe` is on the way.
    Unsubscribing,
    /// The server dropped the subscription; `thread/closed` is due.
    Closing,
    /// The thread is not in the memory of the server.
    Closed,
    /// The server keeps the thread: why.
    Kept(String),
}

impl<W: Write> Codex<W> {
    /// The answer to `thread/unsubscribe`.
    pub(super) fn unsubscribed(&mut self, r: std::result::Result<Value, String>) -> Result<()> {
        let next = match r {
            Err(e) => Step::Kept(format!("codex app-server cannot drop the thread: {e}")),
            Ok(v) => match v.get("status").and_then(Value::as_str) {
                Some("unsubscribed") => Step::Closing,
                Some("notLoaded") => Step::Closed,
                Some(s) => Step::Kept(format!(
                    "codex app-server answered thread/unsubscribe with status {s}, so other clients may hold the thread"
                )),
                None => Step::Kept("codex app-server answered thread/unsubscribe without a status".into()),
            },
        };
        // `thread/closed` can come before the answer.
        if self.reloading == Some(Step::Unsubscribing) {
            self.reloading = Some(next);
        }
        Ok(())
    }

    /// `thread/closed` of the thread of this custody.
    pub(super) fn thread_closed(&mut self) {
        match self.reloading {
            Some(Step::Unsubscribing | Step::Closing) => self.reloading = Some(Step::Closed),
            Some(_) => {}
            None => self.say(
                "paddock: codex app-server closed the thread of this custody; the next entry waits",
            ),
        }
    }

    /// [`crate::horse::live::harness::Live::reload`] for Codex.
    pub(super) fn reload_thread(
        &mut self,
        session: &str,
        write: &mut dyn FnMut(&Path) -> Result<()>,
    ) -> Result<Reloaded> {
        if session != self.ctx.session {
            bail!("session {session} is not in this custody");
        }
        let Some(thread) = self.thread.clone() else {
            bail!("no thread is open");
        };
        if let Some(side) = &self.side {
            let written = Err(anyhow!(
                "side branch {} of thread {thread} is open; close it first (paddock horse fork {} --close)",
                side.session,
                side.session
            ));
            return Ok(Reloaded {
                native: thread,
                written,
            });
        }
        if self.state() != Wait::Idle {
            let written = Err(anyhow!("thread {thread} is not at a safe point"));
            return Ok(Reloaded {
                native: thread,
                written,
            });
        }
        self.reloading = Some(Step::Unsubscribing);
        let p = wire::thread_unsubscribe(&thread);
        self.send(wire::THREAD_UNSUBSCRIBE, p, Pending::Unsubscribe)?;
        let deadline = Instant::now() + self.ctx.reply_timeout;
        let written = loop {
            match self.reloading.clone() {
                Some(Step::Closed) => {
                    let root = self.ctx.sessions_root.clone();
                    break write(&root);
                }
                Some(Step::Kept(why)) => {
                    self.reloading = None;
                    self.say(&format!("paddock: {why}; the rollout did not change"));
                    return Ok(Reloaded {
                        native: thread,
                        written: Err(anyhow!("{why}; the rollout did not change")),
                    });
                }
                _ if Instant::now() >= deadline => {
                    break Err(anyhow!(
                        "codex app-server did not close thread {thread} within {} s; the rollout did not change",
                        self.ctx.reply_timeout.as_secs()
                    ));
                }
                _ => {}
            }
            if self.read(self.ctx.tick)? == Wait::Closed {
                bail!("codex app-server exited while paddock reloaded thread {thread}");
            }
        };
        // Load the thread from its file. A resume that comes back with
        // another id fails in `opened`, and that ends the custody.
        self.thread = None;
        self.busy = None;
        self.rollout = None;
        let model = self.ctx.model.clone();
        let p = wire::thread_params(&self.ctx.cwd, &self.policy, model.as_deref(), Some(&thread));
        self.send(
            wire::THREAD_RESUME,
            p,
            Pending::Thread {
                resume: Some(thread.clone()),
            },
        )?;
        self.settle()?;
        self.reloading = None;
        let st = self.ctx.store.clone();
        self.head = st.session(&self.ctx.session)?.head;
        let native = self.thread.clone().unwrap_or_default();
        if written.is_ok() {
            self.say(&format!(
                "paddock: codex thread {native} loaded again from its swapped rollout"
            ));
        }
        Ok(Reloaded { native, written })
    }
}
