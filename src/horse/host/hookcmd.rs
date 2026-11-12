//! `paddock horse hook HARNESS EVENT`: the host hook entries of an
//! adoption (hooks.rs) run this at each event of a host session.
//!
//! A hook must be quick: the user waits for it, and the harness kills it
//! after a time (Claude Code: 30 s). So:
//!
//! - `Stop` and `SessionStart` read the session of the event into the
//!   store: the file that `transcript_path` of the hook input names, or,
//!   without it, the new and changed sessions of the adoption. The read
//!   waits at most a few seconds for the lock of another sweep and stops
//!   at its deadline (adopt.rs [`adopt::Bounds`]). What it leaves, the
//!   next sweep reads.
//! - `UserPromptSubmit` reads nothing: the prompt of the user waits for it.
//!
//! Then:
//!
//! - Claude Code and Codex, `SessionStart` of a new session (`source`
//!   `startup`, or no source): the DNA of the adoption horse goes into the
//!   context of the session, as the first entry of a run session does.
//! - Claude Code, `UserPromptSubmit`: the queued entries of the session go
//!   into the context, with the prompt of the user.
//! - Claude Code, `Stop`: the queued entries of the session go back to
//!   Claude Code as the reason of a `block` decision, so the session goes
//!   on with them at once. With an empty inbox the session stops as usual.
//!
//! An entry leaves the inbox only after its text is written to the
//! harness: the output goes out and is flushed first, then each delivery
//! is recorded. A hook that dies between the two gives the entry again
//! at the next event, never zero times. Two hooks of one session at once
//! take turns (a lock per session), so an entry goes out once. A harness
//! that is not adopted (an entry that stayed behind) gets nothing.

use super::adopt::{self, Bounds};
use crate::horse::{dna, inbox::Entry, Session, Store};
use anyhow::{Context as _, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

/// Run the hook: its output for the harness, if it has one, goes to `out`.
pub fn run(
    store: &Store,
    state: &Path,
    harness: &str,
    event: &str,
    input: &[u8],
    bounds: &Bounds,
    out: &mut dyn Write,
) -> Result<()> {
    let input: Value = if input.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(input).unwrap_or(Value::Null)
    };
    let Some(a) = adopt::load(state)?.harnesses.remove(harness) else {
        return Ok(());
    };
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    if event != "UserPromptSubmit" {
        let transcript = s("transcript_path")
            .map(Path::new)
            .filter(|p| p.is_absolute());
        match adopt::sweep_hook(store, state, harness, transcript, bounds) {
            Ok(Some(w)) if w.left > 0 => log_line(
                state,
                harness,
                event,
                &format!("{} changed session(s) left for the next sweep", w.left),
            ),
            Ok(Some(_)) => {}
            Ok(None) => log_line(
                state,
                harness,
                event,
                "another sweep holds the lock; nothing read",
            ),
            // The delivery below does not need the read.
            Err(e) => log_error(state, harness, event, &e),
        }
    }
    match (harness, event) {
        ("claude" | "codex", "SessionStart") => {
            if !matches!(s("source"), None | Some("startup")) {
                return Ok(());
            }
            let horse = store.horse(&a.horse)?;
            let text = dna::render_entry(&dna::get_or_default(store, &horse.dna), "start");
            emit(
                out,
                &json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": text}}),
            )
        }
        ("claude", "UserPromptSubmit" | "Stop") => {
            let Some(native) = s("session_id") else {
                return Ok(());
            };
            let home = super::canon_home(&a.home);
            let Some(session) = store.host_session_at(harness, native, &home)? else {
                return Ok(());
            };
            let Some(_turn) = adopt::session_lock(state, &session.id, bounds.wait)? else {
                // The other hook of this session delivers the queue.
                return Ok(());
            };
            let entries = store.inbox(&session.id)?;
            if entries.is_empty() {
                return Ok(());
            }
            let text = entries
                .iter()
                .map(Entry::wire_text)
                .collect::<Vec<_>>()
                .join("\n\n");
            let v = if event == "Stop" {
                json!({"decision": "block", "reason": text})
            } else {
                json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": text}})
            };
            emit(out, &v)?;
            record(store, &session, &entries, event)
        }
        _ => Ok(()),
    }
}

/// Write the output for the harness, and flush it.
fn emit(out: &mut dyn Write, v: &Value) -> Result<()> {
    writeln!(out, "{v}").context("cannot write the output of the hook")?;
    out.flush().context("cannot write the output of the hook")
}

/// Record the delivery of each entry: it leaves the inbox.
fn record(store: &Store, s: &Session, entries: &[Entry], event: &str) -> Result<()> {
    for e in entries {
        store.record_delivery(e, s.head.as_ref(), &format!("host-hook:{event}:{}", e.id))?;
    }
    Ok(())
}

/// Append one line to the hook log: a hook never fails the harness, so
/// its errors go there.
pub fn log_error(state: &Path, harness: &str, event: &str, e: &anyhow::Error) {
    log_line(state, harness, event, &format!("{e:#}"));
}

fn log_line(state: &Path, harness: &str, event: &str, what: &str) {
    use std::os::unix::fs::OpenOptionsExt as _;
    let p = adopt::dir(state).join("hook.log");
    let _ = crate::privfs::dir(&adopt::dir(state));
    let line = format!(
        "{} {harness} {event}: {}\n",
        crate::horse::now_ms(),
        what.replace('\n', " ")
    );
    // A log that grows past 1 MiB starts again.
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&p)
    {
        let _ = f.write_all(line.as_bytes());
    }
}
