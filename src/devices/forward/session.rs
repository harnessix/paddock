//! The session answers of remote callers.
//!
//! A sandbox keeps "allow for this session" in the memory of its paddock
//! process, which lives as long as the run. A remote caller over ssh
//! gets a new `paddock device serve-remote` process per request, so its
//! answer lives in a file instead: one entry per peer and device, with
//! the time it ends. `SESSION_HOURS` after the answer, the card shows
//! again. The file is on the serving machine, written after the user's
//! own answer there, and keyed by the transport identity, never by what
//! the caller said about itself. Removing the file forgets every answer.

use crate::env::HostEnv;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How long "allow for this session" and "deny for this session" hold
/// for a remote caller.
pub const SESSION_HOURS: u64 = 8;

/// The file: `<state>/devices/remote-sessions.json`.
pub fn path(env: &HostEnv) -> PathBuf {
    env.paddock_state()
        .join("devices")
        .join("remote-sessions.json")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    allow: bool,
    /// Unix time when the answer ends.
    until: u64,
}

fn key(peer: &str, device: &str) -> String {
    format!("{peer}\t{device}")
}

fn read(env: &HostEnv) -> BTreeMap<String, Entry> {
    std::fs::read_to_string(path(env))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The answer that holds for `device` from `peer`: allow (true), deny
/// (false), or None when the card must show.
pub fn load(env: &HostEnv, peer: &str, device: &str) -> Option<bool> {
    let now = crate::state::now();
    read(env)
        .get(&key(peer, device))
        .filter(|e| e.until > now)
        .map(|e| e.allow)
}

/// Keep the answer for `SESSION_HOURS`. A failure to write is a log line:
/// the answer still holds in this process.
pub fn save(env: &HostEnv, peer: &str, device: &str, allow: bool) {
    let now = crate::state::now();
    let mut all = read(env);
    all.retain(|_, e| e.until > now);
    all.insert(
        key(peer, device),
        Entry {
            allow,
            until: now + SESSION_HOURS * 3600,
        },
    );
    let p = path(env);
    let written = p
        .parent()
        .map(crate::privfs::dir)
        .unwrap_or(Ok(()))
        .and_then(|()| {
            crate::privfs::write_atomic(&p, serde_json::to_vec_pretty(&all).unwrap_or_default())
        });
    if let Err(e) = written {
        log::warn!("cannot keep the session answer in {}: {e}", p.display());
    }
}

/// Forget every answer.
pub fn forget(env: &HostEnv) -> std::io::Result<()> {
    match std::fs::remove_file(path(env)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}
