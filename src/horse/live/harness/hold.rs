//! When a custody that was asked to end can close.
//!
//! The end of the user's input (Ctrl-D, or stdin from /dev/null) does not
//! drop what waits for the main session. The custody closes at a safe
//! point of the main session only when its inbox is empty and no hitched
//! child of it owes its result. Until then the driver delivers inbox
//! entries at the safe points. An entry that waits for a run that ended
//! waits for a resume, which may never come: the DNA entry of a new
//! session, a hot-load, or the result of a hitched child. Each queued
//! entry holds the end until it went to the harness once: an entry that
//! the harness refused stays in the inbox, but it does not hold the end.
//!
//! A safe point does not prove that the harness has nothing left to do.
//! dsh counts as idle from the handshake on, before it read anything. A
//! harness that is still busy (in `paddock-host spawn --hitch`, for
//! example) reads the input of paddock only later. The queued DNA entry
//! goes out first, and dsh answers it only after the busy step, so the
//! next safe point sees the hitched child.

use super::{Driver, Live};
use anyhow::Result;

impl Driver {
    /// The custody cannot close yet: an entry that did not go out yet
    /// waits in the inbox of the main session, or a hitched child of the
    /// main session owes its result. With [`super::Ends::WhenDone`], also: the main session is
    /// not known. Says once on stderr why the run holds its end.
    pub(super) fn waiting(&mut self, live: &dyn Live) -> Result<bool> {
        let Some(m) = live.main_session() else {
            self.holding = self.drain;
            return Ok(self.holding);
        };
        let owed = crate::horse::hitch::owed::owed(&self.store, &self.horse, &m)?.len();
        if owed > self.owed {
            let what = match owed {
                1 => "the result of its hitched child reaches".to_string(),
                n => format!("the results of its {n} hitched children reach"),
            };
            eprintln!(
                "horse: the run ends when {what} its session. Close the input again to end at once"
            );
        }
        self.owed = owed;
        let inbox = self.store.inbox(&m)?;
        let queued = inbox.iter().filter(|e| !self.tried.contains(&e.id)).count();
        if queued > 0 && owed == 0 && !self.drain && !self.said_queued {
            self.said_queued = true;
            let what = match queued {
                1 => "the queued entry reaches".to_string(),
                n => format!("the {n} queued entries reach"),
            };
            eprintln!(
                "horse: the run ends when {what} session {m}. Close the input again to end at once"
            );
        }
        self.holding = owed > 0 || queued > 0;
        Ok(self.holding)
    }
}
