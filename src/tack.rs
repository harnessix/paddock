//! tack: environment variables whose real value only allowlisted programs
//! receive. Every other process sees a reference, such as `tack://GH_TOKEN`.
//!
//! This module holds the parts that work without a service:
//!
//! - `reference`: the string that a process sees instead of the value.
//! - `config`: the `[tack]` table of the user configuration. A project
//!   `paddock.toml` can never name it (manifest.rs refuses it).
//! - `program`: the identity of an allowlisted program (path, content
//!   hash, macOS signer), and the programs that can never be allowlisted.
//! - `store`: the values, in the system keyring or behind a password
//!   manager command. Values live in locked memory (`credentials::secret`).
//! - `lines` and `import`: `paddock tack import` moves values out of shell
//!   startup files, environment.d and .env files into the store, and puts
//!   references in their place.
//! - `report`: `paddock tack list`, the `paddock explain` lines and the
//!   `paddock doctor` block.
//!
//! The service that starts allowlisted programs with the value (tackd)
//! uses `config::Tack` for the policy, `program::check` for the program
//! identity and `store::open` for the value. Nothing here starts a
//! program with a value yet.

pub mod audit;
pub mod client;
pub mod config;
pub mod daemon;
pub mod ident;
pub mod import;
pub mod lines;
pub mod peer;
pub mod policy;
pub mod procs;
pub mod program;
pub mod proto;
pub mod reference;
pub mod report;
pub mod service;
pub mod status;
pub mod store;

pub use reference::Reference;

use crate::env::HostEnv;
use std::path::PathBuf;

/// The folder of tack's own state: the event log and the backups.
pub fn state_dir(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("tack")
}

/// The event log: one JSON object per line, names and paths only.
pub fn log_file(env: &HostEnv) -> PathBuf {
    state_dir(env).join("log.jsonl")
}

/// The folder of the backups that `paddock tack import` makes. The backups
/// hold the old file, with its plain values: they are private (0600).
pub fn backup_dir(env: &HostEnv) -> PathBuf {
    state_dir(env).join("backups")
}

/// Append one event to the log. A log that cannot be written is reported
/// in the debug log and does not stop the command.
pub fn log_event(env: &HostEnv, mut event: serde_json::Value) {
    use std::io::Write;
    event["time"] = crate::state::now().into();
    // A crash can leave a last line without its end: start a new line, so
    // that this event stays readable.
    let file = log_file(env);
    let cut = std::fs::read(&file).is_ok_and(|b| b.last().is_some_and(|&c| c != b'\n'));
    let line = format!("{}{event}\n", if cut { "\n" } else { "" });
    let written = crate::privfs::append(&file).and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        log::debug!("cannot write {}: {e}", file.display());
    }
}

/// The events of the log, oldest first. Lines that do not parse (a write
/// that a crash cut short) are skipped.
pub fn events(env: &HostEnv) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(log_file(env)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// A path for messages: `~/` for the home folder.
pub fn show(env: &HostEnv, p: &std::path::Path) -> String {
    match p.strip_prefix(&env.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_skips_a_line_that_a_crash_cut_short() {
        let t = tempfile::tempdir().unwrap();
        let env = crate::privfs::tests::env_in(t.path());
        log_event(&env, serde_json::json!({"event": "import", "name": "A"}));
        let mut f = crate::privfs::append(&log_file(&env)).unwrap();
        std::io::Write::write_all(&mut f, b"{\"event\": \"imp").unwrap();
        log_event(&env, serde_json::json!({"event": "import", "name": "B"}));
        let names: Vec<String> = events(&env)
            .iter()
            .filter_map(|e| e["name"].as_str().map(String::from))
            .collect();
        assert_eq!(names, ["A", "B"]);
    }
}
