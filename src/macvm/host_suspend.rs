//! The parent side of automatic suspend (`suspend.rs`): before a start,
//! free a VM slot by a suspend of the least recently used session, and
//! before a resume, end the suspended shim of the profile so the new
//! shim takes its lock and its record. Plain code over the session
//! records, the slot files and the control sockets; the tests use a
//! folder and fake shims.

use super::events::Events;
use super::host::{self, Session};
use super::layout::Layout;
use super::session::Profile;
use super::suspend::{self, Freed, NoSlot};
use super::text;
use std::path::Path;
use std::time::{Duration, Instant};

/// `[macvm] auto_suspend`, on by default.
pub fn auto_suspend(user: Option<&crate::manifest::MacvmCfg>) -> bool {
    user.and_then(|m| m.auto_suspend).unwrap_or(true)
}

/// The live records of this user's sessions.
pub fn live(sessions: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<Session> {
    host::sessions(sessions)
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| alive(s.pid))
        .collect()
}

/// Ask a shim to suspend over its control socket. Its answer is `ok`, or
/// `error` with the reason it cannot.
pub fn ask(s: &Session, reason: &str) -> Result<(), String> {
    let line = format!("suspend {}", one_line(reason)); // not user text
    crate::vm::host::control(&s.control, &line)
        .map(|_| ())
        .map_err(|e| {
            let e = format!("{e:#}"); // not user text
            e.strip_prefix("the VM: ").unwrap_or(&e).to_string()
        })
}

/// A reason as one line of the control socket.
pub fn one_line(reason: &str) -> String {
    reason
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect()
}

/// Free a slot for the session `key` of `label`, when both are held. The
/// lines for the user: the suspended session, or nothing. `Err` is the
/// refusal line. `full` and `ask` are the slot files and the control
/// socket on a Mac, fakes in the tests.
pub fn make_room(
    sessions: &Path,
    key: &str,
    label: &str,
    full: &dyn Fn() -> bool,
    ask: &dyn Fn(&Session, &str) -> Result<(), String>,
    wait: Duration,
) -> Result<Vec<String>, Option<String>> {
    let live_now = || live(sessions, &host::alive);
    let sleep = |d: Duration| std::thread::sleep(d);
    let now = Instant::now;
    let ports = suspend::Ports {
        full,
        live: &live_now,
        ask,
        sleep: &sleep,
        now: &now,
    };
    match suspend::free_slot(&ports, key, label, wait) {
        Ok(Freed::Free) => Ok(vec![]),
        Ok(Freed::Suspended(s)) => Ok(vec![text::suspended_for(&s.label(), &s.arg(), label)]),
        // The slot refusal of the shim names the holders.
        Err(NoSlot::NoneOfYours) => Err(None),
        Err(NoSlot::Refused(line)) => Err(Some(line)),
    }
}

/// End the suspended shim of a profile before its resume: a stop signal,
/// which a suspended shim answers with its exit, then wait for it. The
/// profile records the resume and its reason.
pub fn handover(
    s: &Session,
    reason: &str,
    alive: &dyn Fn(u32) -> bool,
    signal: &dyn Fn(u32),
    wait: Duration,
) -> Result<(), String> {
    signal(s.pid);
    let end = Instant::now() + wait;
    while alive(s.pid) {
        if Instant::now() >= end {
            return Err(text::handover_timeout(&s.label(), &s.arg(), s.pid));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let profile = Profile::new(&s.profile);
    if profile.dir.is_dir() {
        Events::open(&profile).record("resume", &text::resume_event(reason), &[]);
    }
    Ok(())
}

/// SIGTERM to a shim.
pub fn term(pid: u32) {
    if pid > 0 {
        // SAFETY: a stop signal to the shim that the record names.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
}

/// The running or suspended session of an instance, with the handover
/// done for a suspended one. `Ok(Some)`: a session runs, the start is
/// refused with its line.
pub fn before_start(
    layout: &Layout,
    app: &str,
    home: &str,
    resumed_by: Option<&str>,
) -> Result<Option<Session>, String> {
    let Some(s) = host::running(layout, app, home) else {
        return Ok(None);
    };
    if !s.suspended {
        return Ok(Some(s));
    }
    let reason = resumed_by
        .map(str::to_string)
        .unwrap_or_else(text::resume_reason_run);
    handover(&s, &reason, &host::alive, &term, suspend::HANDOVER_WAIT)?;
    Ok(None)
}

#[cfg(test)]
#[path = "host_suspend_tests.rs"]
mod tests;
