//! A warm side branch of a session that a run holds, asked from another
//! process.
//!
//! `paddock horse fork --warm` (open) and `paddock horse fork --close`
//! (close) write a request (`codex-side/<session>.json` in the store,
//! named by the session that the run holds); the Codex custody of the run
//! reads it at its next safe point, acts, and writes the answer into the
//! same file; the asking process reads the answer and removes the file. A
//! request whose asking process is gone is dropped unanswered.

use crate::horse::guard::Verdict;
use crate::horse::store::Store;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Act {
    /// Fork the live thread, ephemeral, at the head of the session.
    Open,
    /// End the custody of the side branch.
    Close,
}

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
    /// The session that the run holds: the parent of the side branch.
    pub session: String,
    pub act: Act,
    /// The asking process.
    pub pid: u32,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The store session of the side branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// The ephemeral thread of the side branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// The turns that the side branch ran (the answer to a close).
    #[serde(default)]
    pub turns: usize,
}

fn path(session: &str) -> String {
    format!("codex-side/{session}.json")
}

pub(crate) fn alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only checks that the process exists.
    pid != 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

/// The request for a session, if there is one.
pub fn read(store: &Store, session: &str) -> Result<Option<Request>> {
    match store.read_ref(&path(session))? {
        None => Ok(None),
        Some(b) => Ok(Some(
            serde_json::from_slice(&b).context("a side branch request is damaged")?,
        )),
    }
}

/// Ask the run that holds `session` to open or close its side branch.
/// `side` names the side session of a close.
pub fn ask(store: &Store, session: &str, act: Act, side: Option<&str>) -> Result<Request> {
    let _l = store.lock()?;
    if let Some(old) = read(store, session)? {
        if old.state == State::Asked && alive(old.pid) {
            bail!(
                "a side branch request for session {session} waits already (process {}); try again when it ends",
                old.pid
            );
        }
    }
    let r = Request {
        id: crate::horse::new_id("side")?,
        session: session.into(),
        act,
        pid: std::process::id(),
        state: State::Asked,
        error: None,
        side: side.map(String::from),
        thread: None,
        verdict: None,
        turns: 0,
    };
    store.write_ref(&path(session), &serde_json::to_vec_pretty(&r)?)?;
    Ok(r)
}

/// The answer to request `id`, once the custody wrote it.
pub fn answer(store: &Store, session: &str, id: &str) -> Result<Option<Request>> {
    match read(store, session)? {
        Some(r) if r.id != id => {
            bail!("the side branch request for session {session} was replaced")
        }
        Some(r) if r.state == State::Asked => Ok(None),
        Some(r) => Ok(Some(r)),
        None => bail!("the side branch request for session {session} is gone"),
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

/// Write the answer of the custody. The asking process may have given up
/// and removed the request: then nothing is written.
pub(super) fn write(store: &Store, r: &Request) -> Result<()> {
    let _l = store.lock()?;
    if read(store, &r.session)?.is_some_and(|x| x.id == r.id) {
        store.write_ref(&path(&r.session), &serde_json::to_vec_pretty(r)?)?;
    }
    Ok(())
}
