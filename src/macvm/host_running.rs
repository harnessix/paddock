//! The `Running` record of a guest session: the shim's pid with `backend =
//! "macvm"`, so the marker marks every host window of the session, `paddock
//! windows` lists them, and `paddock freeze` finds the session. The record
//! goes with the shim: `Running::list` removes it once the pid is gone.

use super::appcopy::HostBundle;
use super::host::{Session, BACKEND};
use crate::env::HostEnv;
use crate::state::Running;
use std::path::PathBuf;

/// The id of the guest session of an app instance, in `paddock ps`,
/// `paddock freeze` and the marks: `macvm-sample-notes`, and
/// `macvm-sample-notes--2` for instance 2. `key` is the instance key.
pub fn running_id(key: &str) -> String {
    format!("macvm-{key}") // not user text
}

/// The Dock name of the session: `Sample Notes (paddock)`, from the bundle's
/// name, else the profile name; `Sample Notes 2 (paddock)` for instance 2.
pub fn display_name(app: &str, instance: u32, bundle: Option<&HostBundle>) -> String {
    let name = bundle
        .map(|b| b.display_name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(app);
    super::shim::paddock_name(&super::instance::label(name, instance))
}

/// The record of a session. The project is the first workspace folder,
/// else the profile folder.
pub fn record(s: &Session, roots: &[PathBuf], network: &str, host_pid: u32) -> Running {
    Running {
        id: running_id(&s.key()),
        harness: s.app.clone(),
        project: roots.first().cloned().unwrap_or_else(|| s.profile.clone()),
        backend: BACKEND.to_string(),
        network: network.to_string(),
        gui: "app".into(),
        pid: s.pid,
        started: s.started,
        host_pid,
        ..Default::default()
    }
}

/// Write the record. A failure is one log line: the session runs, only
/// without marks and freeze.
pub fn register(env: &HostEnv, s: &Session, roots: &[PathBuf], network: &str) {
    let r = record(s, roots, network, std::process::id());
    if let Err(e) = r.register(env) {
        log::warn!("macvm: cannot list the session with the running sandboxes: {e:#}");
    }
}

#[cfg(test)]
#[path = "host_running_tests.rs"]
mod tests;
