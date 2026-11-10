//! dsh can answer `session/prompt` with an error. It then starts no turn
//! and sends no `session.status`, so the session is idle again at once.
//! The refused entry stays first in the inbox. So that it does not go out
//! again on every tick, the session takes no entry for [`RETRY`]. After
//! [`LIMIT`] refusals of one entry, the entry leaves the inbox: one
//! `horse:` line on stderr, and an `append.dropped` event in the history
//! of the horse. An event, and not a delivery record: a delivery record
//! says that the harness took the entry, which it did not. The event
//! keeps the entry and the last answer of dsh, so the history shows
//! what was dropped and why.

use super::{Custodian, HARNESS};
use crate::horse::inbox::Entry;
use crate::horse::run::{append_event, RunHorse};
use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

/// The wait before a refused entry goes out again.
pub const RETRY: Duration = Duration::from_secs(30);
/// The refusals of one entry after which it leaves the inbox.
pub const LIMIT: u32 = 3;

/// The refusals of the entries of one custody.
#[derive(Debug)]
pub(super) struct Refusals {
    pub(super) retry: Duration,
    /// Refusals by entry id.
    count: HashMap<String, u32>,
    /// By store session: no entry goes out before this time.
    until: HashMap<String, Instant>,
}

impl Default for Refusals {
    fn default() -> Self {
        Refusals {
            retry: RETRY,
            count: HashMap::new(),
            until: HashMap::new(),
        }
    }
}

impl Refusals {
    /// The session waits after a refusal.
    pub(super) fn waits(&self, session: &str) -> bool {
        self.until.get(session).is_some_and(|t| Instant::now() < *t)
    }

    /// The entry went out and dsh took it.
    pub(super) fn taken(&mut self, e: &Entry) {
        self.count.remove(&e.id);
        self.until.remove(&e.session);
    }
}

impl<W: Write, O: Write> Custodian<W, O> {
    /// Wait this long before a refused entry goes out again.
    pub fn retry_wait(mut self, d: Duration) -> Self {
        self.refusals.retry = d;
        self
    }

    /// dsh refused entry `e` with `why`.
    pub(super) fn refused(&mut self, e: Entry, why: &str) -> Result<()> {
        let r = &mut self.refusals;
        let n = r.count.entry(e.id.clone()).or_default();
        *n += 1;
        let n = *n;
        r.until.insert(e.session.clone(), Instant::now() + r.retry);
        if n < LIMIT {
            self.say(&format!(
                "paddock: dsh did not take append {}: {why}; it stays in the inbox",
                e.id
            ));
            return Ok(());
        }
        // The next entry need not wait for this one.
        r.count.remove(&e.id);
        r.until.remove(&e.session);
        if !self.set.store.drop_entry(&e.session, &e.id)? {
            return Ok(());
        }
        eprintln!(
            "horse: dsh refused append {} of session {} {n} times, so it left the inbox: {why}",
            e.id, e.session
        );
        let rh = RunHorse {
            horse: self.set.horse.clone(),
            session: e.session.clone(),
            harness: HARNESS.into(),
        };
        append_event(
            &self.set.store,
            &rh,
            &json!({
                "type": "append.dropped",
                "session": e.session,
                "entry": e,
                "refusals": n,
                "why": why,
                "at_ms": crate::horse::now_ms(),
            }),
        )?;
        Ok(())
    }
}
