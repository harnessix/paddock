//! Automatic suspend: Apple runs at most two macOS VMs per Mac, and
//! paddock holds two slots. When both slots are held by sessions of this
//! user and another session starts, paddock saves the least recently used
//! session (the one whose windows were not key for the longest time) and
//! frees its slot. A session in a modal dialog, in the copy of the app or
//! in the preparation of its data disk is never chosen: the next one is,
//! else the start is refused with one line that says why.
//!
//! The suspended session keeps its Dock face: a click on it, or `paddock
//! app run`, resumes it from the saved state, and suspends another session
//! when it needs the slot. `[macvm] auto_suspend = false` turns all of this
//! off: a third start is refused as before.
//!
//! Plain Rust over closures: the tests pass fake slots, records and
//! answers. `host.rs` passes the slot files, the session records and the
//! control socket of each shim.

use super::host::Session;
use super::text;
use std::time::{Duration, Instant};

/// How long a start waits for the slot of the session it suspended.
pub const WAIT: Duration = Duration::from_secs(30);
/// How often the start looks at the slots while it waits.
pub const POLL: Duration = Duration::from_millis(200);
/// How long the parent waits for a suspended shim to end before a resume.
pub const HANDOVER_WAIT: Duration = Duration::from_secs(10);

/// Why a session cannot be suspended now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// A sheet of the guest app is open: a save dialog, for example.
    Dialog,
    /// The copy of the app into the guest runs.
    Copy,
    /// The guest prepares the data disk.
    DataPrepare,
}

impl Busy {
    /// The word in the session record.
    pub fn word(self) -> &'static str {
        match self {
            Busy::Dialog => "dialog",
            Busy::Copy => "copy",
            Busy::DataPrepare => "data_prepare",
        }
    }

    /// The word of the record, back.
    pub fn parse(word: &str) -> Option<Self> {
        [Busy::Dialog, Busy::Copy, Busy::DataPrepare]
            .into_iter()
            .find(|b| b.word() == word)
    }

    /// What the user reads in a refusal.
    pub fn text(self) -> String {
        match self {
            Busy::Dialog => text::busy_dialog(),
            Busy::Copy => text::busy_copy(),
            Busy::DataPrepare => text::busy_data_prepare(),
        }
    }
}

/// What the shim knows of itself: the busy state of one session.
pub fn busy(dialog: bool, copying: bool, preparing: bool) -> Option<Busy> {
    if dialog {
        Some(Busy::Dialog)
    } else if copying {
        Some(Busy::Copy)
    } else if preparing {
        Some(Busy::DataPrepare)
    } else {
        None
    }
}

/// The time a session was last used: the last time one of its windows
/// was key, else its start.
pub fn last_used(s: &Session) -> u64 {
    s.last_key.max(s.started)
}

/// The sessions that may be suspended, least recently used first: live,
/// not suspended already, not `except` (the session that starts).
pub fn order<'a>(live: &'a [Session], except: &str) -> Vec<&'a Session> {
    let mut v: Vec<&Session> = live
        .iter()
        .filter(|s| !s.suspended && s.key() != except)
        .collect();
    v.sort_by_key(|s| (last_used(s), s.started, s.pid));
    v
}

/// The session to suspend: the least recently used one that is not busy.
/// `Err` lists the busy sessions, or is empty when no session of this
/// user holds a slot.
pub fn choose<'a>(live: &'a [Session], except: &str) -> Result<&'a Session, Vec<(String, Busy)>> {
    let mut busy = Vec::new();
    for s in order(live, except) {
        match s.busy.as_deref().and_then(Busy::parse) {
            None => return Ok(s),
            Some(b) => busy.push((s.label(), b)),
        }
    }
    Err(busy)
}

/// What [`free_slot`] needs from the host.
pub struct Ports<'a> {
    /// Both slots are held now.
    pub full: &'a dyn Fn() -> bool,
    /// The session records of this user whose shim lives.
    pub live: &'a dyn Fn() -> Vec<Session>,
    /// Ask one shim to suspend, with the reason. `Err` is its refusal.
    pub ask: &'a dyn Fn(&Session, &str) -> Result<(), String>,
    pub sleep: &'a dyn Fn(Duration),
    pub now: &'a dyn Fn() -> Instant,
}

/// The result of [`free_slot`].
#[derive(Debug, Clone, PartialEq)]
pub enum Freed {
    /// A slot was free already.
    Free,
    /// This session was suspended, and its slot is free now.
    Suspended(Box<Session>),
}

/// Why no slot could be freed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoSlot {
    /// No session of this user holds a slot: the line of the slot
    /// holders applies, as without automatic suspend.
    NoneOfYours,
    /// The line for the user.
    Refused(String),
}

/// Free one slot for the session `starting` (its key) of `label`: when
/// both are held, suspend the least recently used session of this user
/// that is not busy, and wait up to `wait` for its slot. A shim that
/// refuses (it became busy, its save cannot happen) passes the turn to
/// the next one.
pub fn free_slot(p: &Ports, starting: &str, label: &str, wait: Duration) -> Result<Freed, NoSlot> {
    if !(p.full)() {
        return Ok(Freed::Free);
    }
    let live = (p.live)();
    let candidates = order(&live, starting);
    if candidates.is_empty() {
        return Err(NoSlot::NoneOfYours);
    }
    let reason = text::suspend_reason_start(label);
    let mut refused: Vec<(String, String)> = Vec::new();
    for s in candidates {
        if let Some(b) = s.busy.as_deref().and_then(Busy::parse) {
            refused.push((s.label(), b.text()));
            continue;
        }
        if let Err(why) = (p.ask)(s, &reason) {
            refused.push((s.label(), why));
            continue;
        }
        let end = (p.now)() + wait;
        loop {
            if !(p.full)() {
                return Ok(Freed::Suspended(Box::new(s.clone())));
            }
            if (p.now)() >= end {
                return Err(NoSlot::Refused(text::suspend_timeout(
                    &s.label(),
                    &s.arg(),
                    wait,
                )));
            }
            (p.sleep)(POLL);
        }
    }
    Err(NoSlot::Refused(text::suspend_refused_all(label, &refused)))
}

#[cfg(test)]
#[path = "suspend_tests.rs"]
mod tests;
