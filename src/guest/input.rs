//! The input sources of the guest: `input.set` enables the host's sources
//! with `TISEnableInputSource`, selects the current one with
//! `TISSelectInputSource`, reports the ids the guest lacks, and writes the
//! two accessibility defaults. A source that the guest cannot select gives
//! the ABC layout instead, with one info line (`input_fallback.rs`). The Text Input Sources calls are in
//! `macvm::tis`, shared with the shim. The candidate windows of the input
//! method's process are the work of the window model
//! (`windows::Kind::InputMethod`): while the app is the front app, the agent
//! follows the input method processes and reports their windows with level
//! `popover` and the key window as parent.

use super::input_fallback::{self, Choice};
use super::platform::Fail;
use crate::macvm::proto::ErrorCode;
use crate::macvm::text;
use crate::macvm::tis::{self, List};
use std::sync::Mutex;

/// The sources whose fallback line was written.
static FELL_BACK: Mutex<Vec<String>> = Mutex::new(Vec::new());
use std::process::Command;

/// The current keyboard input source of the guest.
pub fn current() -> Option<String> {
    tis::current()
}

/// The ids of the enabled input sources.
pub fn enabled() -> Vec<String> {
    List::new(false)
        .entries
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Disable one input source (the check script restores the Mac).
pub fn disable(id: &str) -> bool {
    tis::disable(id)
}

/// `input.set`: enable `sources`, select `current`, write the two
/// defaults. The ids the guest does not have come back.
pub fn set(
    sources: &[String],
    current: &str,
    reduce_motion: bool,
    increase_contrast: bool,
) -> Result<Vec<String>, Fail> {
    let all = List::new(true);
    let mut missing = Vec::new();
    for id in sources {
        match all.enable(id) {
            Some(0) => {}
            Some(r) => log::warn!("cannot enable the input source {id}: OSStatus {r}"),
            None => missing.push(id.clone()),
        }
    }
    let choice = input_fallback::choose(current, |id| all.enable(id), |id| all.select(id));
    match choice {
        Choice::Selected => {}
        Choice::Fallback { layout, status } => {
            if input_fallback::first_time(&FELL_BACK, current) {
                log::info!("{}", text::input_source_fallback(current, &layout, status));
            }
            if !missing.iter().any(|m| m == current) {
                missing.push(current.to_string());
            }
        }
        Choice::Failed(Some(r)) => {
            return Err((ErrorCode::Io, text::input_source_select_failed(current, r)));
        }
        Choice::Failed(None) => {
            if !missing.iter().any(|m| m == current) {
                missing.push(current.to_string());
            }
        }
    }
    for (key, on) in [
        ("reduceMotion", reduce_motion),
        ("increaseContrast", increase_contrast),
    ] {
        let status = Command::new("/usr/bin/defaults")
            .args([
                "write",
                "com.apple.universalaccess",
                key,
                "-bool",
                if on { "true" } else { "false" },
            ])
            .status();
        if let Err(e) = status {
            log::warn!("cannot write the {key} default: {e}");
        }
    }
    Ok(missing)
}
