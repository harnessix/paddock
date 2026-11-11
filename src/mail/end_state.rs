//! Reconcile end markers with complete, sequenced events in the up log.
use super::{Dir, Message};
use crate::env::HostEnv;
use std::path::Path;

/// Whether an end is proven, unfinished, or absent.
#[derive(Debug, PartialEq)]
pub enum EndState {
    Absent,
    Complete(Box<Message>),
    /// A run that may still be writing its end.
    Pending,
    /// The marker or log is incomplete, so its outcome is unknown.
    Incomplete,
    /// The marker and log disagree, or the log has multiple end events.
    Conflict,
}

pub(super) enum LogEnd {
    Absent,
    One(Box<Message>),
    Unfinished,
    Conflict,
}

fn tail_is_end(bytes: &[u8]) -> bool {
    if bytes.last().is_none_or(|b| *b == b'\n') {
        return false;
    }
    let tail = bytes.rsplit(|b| *b == b'\n').next().unwrap_or_default();
    serde_json::from_slice::<Message>(tail).is_ok_and(|m| m.seq != 0 && m.is_end())
}

/// Under the mailbox lock, inspect all complete end events in the log.
pub(super) fn logged_end(d: &Path) -> LogEnd {
    let bytes = match std::fs::read(super::log_file(d, Dir::Up)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LogEnd::Absent,
        Err(_) => return LogEnd::Conflict,
    };
    let mut ends = super::parse_log(&bytes)
        .into_iter()
        .filter(|m| m.seq != 0 && m.is_end());
    let first = ends.next();
    let tail = tail_is_end(&bytes);
    if ends.next().is_some() || (first.is_some() && tail) {
        return LogEnd::Conflict;
    }
    match (first, tail) {
        (Some(m), false) => LogEnd::One(Box::new(m)),
        (None, true) => LogEnd::Unfinished,
        _ => LogEnd::Absent,
    }
}

pub(super) fn save_marker(d: &Path, event: &Message) {
    if let Ok(bytes) = serde_json::to_vec(event) {
        let _ = crate::privfs::write_atomic(&d.join("end"), bytes);
    }
}

/// A valid JSON tail without a newline is not yet a committed log event.
pub(super) fn has_unterminated_end(d: &Path) -> bool {
    let Ok(bytes) = std::fs::read(super::log_file(d, Dir::Up)) else {
        return false;
    };
    tail_is_end(&bytes)
}

fn same_payload(marker: &Message, event: &Message) -> bool {
    let mut marker = marker.clone();
    let mut event = event.clone();
    marker.seq = 0;
    marker.ts = 0;
    event.seq = 0;
    event.ts = 0;
    marker == event
}

fn verified_live(env: &HostEnv, id: &str) -> bool {
    let Some(info) = super::run_info(env, id) else {
        return false;
    };
    let Some(start) = info.pid_start else {
        return false;
    };
    if info.pid == 0 || info.pid > i32::MAX as u32 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", info.pid)) else {
        return false;
    };
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    let mut fields = rest.split_whitespace();
    let state = fields.next();
    let actual = fields.nth(18).and_then(|s| s.parse::<u64>().ok());
    !matches!(state, None | Some("Z" | "X")) && actual == Some(start)
}

/// Inspect under the writer's lock. An unproven marker is left untouched.
pub fn inspect_end(env: &HostEnv, id: &str) -> EndState {
    let Ok(d) = super::dir(env, id) else {
        return EndState::Absent;
    };
    if !super::exists(env, id) {
        return EndState::Absent;
    }
    let _guard = match super::try_lock(&d) {
        Ok(Some(guard)) => guard,
        Ok(None) => return EndState::Pending,
        Err(_) => return EndState::Incomplete,
    };
    let path = d.join("end");
    let marker_exists = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => true,
        Ok(_) => return EndState::Conflict,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return EndState::Conflict,
    };
    let marker = super::end(env, id);
    if marker_exists && marker.as_ref().is_none_or(|m| !m.is_end()) {
        return EndState::Conflict;
    }
    match logged_end(&d) {
        LogEnd::Conflict => EndState::Conflict,
        LogEnd::One(event) => match marker {
            Some(m) if m.seq == 0 && same_payload(&m, &event) => {
                save_marker(&d, &event);
                EndState::Complete(event)
            }
            Some(m) if m == *event => EndState::Complete(event),
            Some(_) => EndState::Conflict,
            None => {
                save_marker(&d, &event);
                EndState::Complete(event)
            }
        },
        LogEnd::Absent if !marker_exists => EndState::Absent,
        LogEnd::Absent | LogEnd::Unfinished => {
            if marker.as_ref().is_some_and(|m| m.seq != 0) {
                EndState::Incomplete
            } else if verified_live(env, id) {
                EndState::Pending
            } else {
                EndState::Incomplete
            }
        }
    }
}
