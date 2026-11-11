//! A request to end the custody of a session, from another process.
//!
//! `ctl disconnect` writes `disconnect/<session>` in the store. The custody
//! loop of the run that holds the session takes the request and closes
//! like a [`super::harness::Control::Close`]: at the next safe point.

use super::harness::Live;
use crate::horse::Store;
use anyhow::{bail, Result};

fn path(session: &str) -> String {
    format!("disconnect/{session}")
}

/// Ask the run that holds `session` to end its custody at the next safe
/// point. A session in no live run is an error.
pub fn ask(store: &Store, session: &str) -> Result<()> {
    let held = super::records(store)?
        .iter()
        .any(|r| r.sessions.iter().any(|s| s.session == session));
    if !held {
        bail!("session {session} is in no live horse-mode run");
    }
    store.write_ref(
        &path(session),
        crate::horse::now_ms().to_string().as_bytes(),
    )
}

/// Whether a session that `live` holds has a request: the requests are
/// gone then.
pub fn take(store: &Store, live: &dyn Live) -> Result<bool> {
    let mut asked = false;
    for s in live.sessions() {
        if store.read_ref(&path(&s.session))?.is_some() {
            store.remove_ref(&path(&s.session))?;
            asked = true;
        }
    }
    Ok(asked)
}

#[cfg(test)]
#[path = "disconnect_tests.rs"]
mod tests;
