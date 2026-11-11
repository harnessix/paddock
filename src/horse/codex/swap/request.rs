//! A swap of a session that a run holds, asked from another process.
//!
//! `paddock horse swap` writes a request (`swap-requests/<session>.json`
//! in the store); the custody of the run reads it in its loop, swaps at
//! the next safe point ([`super::swap_live`]) and writes the answer into
//! the same file; the asking process reads the answer and removes the
//! file. A request whose asking process is gone is dropped unanswered.

use super::{swap_live, Failure, LiveOpts};
use crate::horse::guard::Verdict;
use crate::horse::live::harness::Live;
use crate::horse::store::{Hash, Store};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Asked,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub session: String,
    pub to: Hash,
    pub interrupt: bool,
    pub wait_ms: u64,
    /// The asking process.
    pub pid: u32,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The custody ended with the failure.
    #[serde(default)]
    pub ended: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Hash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default)]
    pub waited_ms: u64,
}

fn path(session: &str) -> String {
    format!("swap-requests/{session}.json")
}

fn alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only checks that the process exists.
    pid != 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

/// The request of a session, if there is one.
pub fn read(store: &Store, session: &str) -> Result<Option<Request>> {
    match store.read_ref(&path(session))? {
        None => Ok(None),
        Some(b) => Ok(Some(
            serde_json::from_slice(&b).context("a swap request is damaged")?,
        )),
    }
}

/// Ask the run that holds `session` to swap it to `to`.
pub fn ask(
    store: &Store,
    session: &str,
    to: &Hash,
    interrupt: bool,
    wait: Duration,
) -> Result<Request> {
    let _l = store.lock()?;
    if let Some(old) = read(store, session)? {
        if old.state == State::Asked && alive(old.pid) {
            bail!(
                "a swap of session {session} waits already (process {}); try again when it ends",
                old.pid
            );
        }
    }
    let r = Request {
        id: crate::horse::new_id("swap")?,
        session: session.into(),
        to: to.clone(),
        interrupt,
        wait_ms: wait.as_millis() as u64,
        pid: std::process::id(),
        state: State::Asked,
        error: None,
        ended: false,
        verdict: None,
        from: None,
        kept: None,
        thread: None,
        waited_ms: 0,
    };
    store.write_ref(&path(session), &serde_json::to_vec_pretty(&r)?)?;
    Ok(r)
}

/// The answer to request `id`, once the custody wrote it. A request that
/// another one replaced is an error.
pub fn answer(store: &Store, session: &str, id: &str) -> Result<Option<Request>> {
    match read(store, session)? {
        Some(r) if r.id != id => bail!("the swap request of session {session} was replaced"),
        Some(r) if r.state == State::Asked => Ok(None),
        Some(r) => Ok(Some(r)),
        None => bail!("the swap request of session {session} is gone"),
    }
}

/// Remove request `id`; another request stays.
pub fn remove(store: &Store, session: &str, id: &str) -> Result<()> {
    let _l = store.lock()?;
    if read(store, session)?.is_some_and(|r| r.id == id) {
        store.remove_ref(&path(session))?;
    }
    Ok(())
}

fn write(store: &Store, r: &Request) -> Result<()> {
    let _l = store.lock()?;
    // The asking process may have given up and removed it.
    if read(store, &r.session)?.is_some_and(|x| x.id == r.id) {
        store.write_ref(&path(&r.session), &serde_json::to_vec_pretty(r)?)?;
    }
    Ok(())
}

/// Serve the swap requests of the sessions that `live` holds. An error
/// ends the custody: the request has its answer then.
pub fn serve(store: &Store, live: &mut dyn Live, tick: Duration) -> Result<()> {
    for s in live.sessions() {
        let Some(mut r) = read(store, &s.session)? else {
            continue;
        };
        if r.state != State::Asked {
            continue;
        }
        if !alive(r.pid) {
            log::info!(
                "horse: the swap request {} of session {} has no asking process; dropped",
                r.id,
                s.session
            );
            remove(store, &s.session, &r.id)?;
            continue;
        }
        let opts = LiveOpts {
            interrupt: r.interrupt,
            wait: Duration::from_millis(r.wait_ms),
            tick,
        };
        match swap_live(store, &s.session, &r.to, live, &opts) {
            Ok(rep) => {
                r.state = State::Done;
                r.verdict = Some(rep.verdict);
                r.from = rep.file.from;
                r.kept = rep.file.kept;
                r.thread = Some(rep.thread);
                r.waited_ms = rep.waited.as_millis() as u64;
                write(store, &r)?;
            }
            Err(Failure::Refused(e)) => {
                r.state = State::Failed;
                r.error = Some(format!("{e:#}"));
                write(store, &r)?;
            }
            Err(Failure::Custody(e)) => {
                r.state = State::Failed;
                r.error = Some(format!("{e:#}"));
                r.ended = true;
                write(store, &r)?;
                return Err(e);
            }
        }
    }
    Ok(())
}
