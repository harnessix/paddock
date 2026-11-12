//! `paddock --launcher-protocol`: the read-only query with which paddock
//! for Mac identifies the CLI that it talks to.
//!
//! The query is one exact root option. `main` answers it after the tack
//! exec wrapper and before every other step: no host detection, no config,
//! no refresh, no cleanup. The answer reads no file and starts nothing. It
//! is one JSON object on one line, at most [`MAX_REPLY`] bytes, and the exit
//! status is 0:
//!
//! ```text
//! {"protocol":"paddock.mac-launcher/v1","features":["approvals-v1",...]}
//! ```
//!
//! Releases before the query (0.2.2 and older) reject the option in the
//! root parser with exit status 2, before any harness starts.
//!
//! The features name the JSON outputs that the launcher reads. A feature
//! is listed only when the output exists in this build:
//!
//! - `approvals-v1`: `paddock approvals --json` prints one object with
//!   `"v":1` and `"api_version":"paddock.approvals/v1"`.
//! - `app-list-v1`: `paddock app list --json` prints `{"v":1,"profiles":[...]}`.
//!   The list can be incomplete: the output has no `profiles_complete`
//!   field, so the feature `app-list-v1-completeness` is not listed.
//! - `events-v1`: `paddock events --follow --json` prints lines of the
//!   schema `eventstream::schema` with `"v":1`, and the `stream` kind with
//!   `started` and `lost`.
//! - `macvm-status-v1`: `paddock macvm status --json` prints one object with
//!   `"v":1`.
//! - `setup-progress-v1`: `paddock macvm setup --json` prints lines of kind
//!   `setup` with `"v":1`. Lines of the helper pass through as they are, so
//!   a reader checks the `v` of each line.
//!
//! These outputs exist only where app mode runs: macOS on Apple silicon.
//! Other builds answer with the same protocol and no features, so the
//! launcher refuses them.

use std::ffi::OsString;
use std::io::Write;

/// The one argument of the query.
pub const OPTION: &str = "--launcher-protocol";

/// The protocol name and version of the answer.
pub const PROTOCOL: &str = "paddock.mac-launcher/v1";

/// The longest answer. The launcher reads at most 16 KiB.
pub const MAX_REPLY: usize = 1024;

/// The features of a build with app mode, in sorted order.
pub const MAC_FEATURES: &[&str] = &[
    "app-list-v1",
    "approvals-v1",
    "events-v1",
    "macvm-status-v1",
    "setup-progress-v1",
];

/// The features of this build.
pub fn features() -> &'static [&'static str] {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        MAC_FEATURES
    } else {
        &[]
    }
}

/// The bytes of the answer for `features`, with the final newline.
pub fn reply(features: &[&str]) -> Vec<u8> {
    let list = serde_json::to_string(features).expect("a list of strings is JSON");
    let bytes = format!("{{\"protocol\":\"{PROTOCOL}\",\"features\":{list}}}\n").into_bytes();
    debug_assert!(bytes.len() <= MAX_REPLY);
    bytes
}

/// What `main` does with the arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Not the query: the normal start.
    Normal,
    /// `paddock --launcher-protocol` and nothing else: answer.
    Answer,
    /// The option with other root arguments: refuse, start nothing.
    Refuse,
}

/// The route of `argv`, with the program name at index 0.
pub fn route(argv: &[OsString]) -> Route {
    if argv.len() == 2 && argv[1] == OPTION {
        return Route::Answer;
    }
    // Only the root options count. After a subcommand or `--`, the word
    // belongs to that command, for example to a harness.
    for arg in argv.iter().skip(1) {
        let Some(arg) = arg.to_str() else {
            break;
        };
        if arg == "--" || arg == "-" || !arg.starts_with('-') {
            break;
        }
        if arg == OPTION || arg.starts_with("--launcher-protocol=") {
            return Route::Refuse;
        }
    }
    Route::Normal
}

/// Answers the query, or runs `normal` for every other command line.
/// Returns the exit status.
pub fn dispatch(
    argv: &[OsString],
    out: &mut dyn Write,
    err: &mut dyn Write,
    normal: impl FnOnce() -> i32,
) -> i32 {
    match route(argv) {
        Route::Normal => normal(),
        Route::Answer => {
            let sent = out.write_all(&reply(features())).and_then(|()| out.flush());
            i32::from(sent.is_err())
        }
        Route::Refuse => {
            let _ = writeln!(err, "paddock: {OPTION} must be the only argument");
            2
        }
    }
}

#[cfg(test)]
#[path = "launcher_protocol_tests.rs"]
mod tests;
