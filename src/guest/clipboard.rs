//! The guest pasteboard for the agent, macOS only: the reads of the 10 Hz
//! poll while the host says a session window is key, the write of
//! `clipboard.set`, and the `guest-files` transfer of a set too big for the
//! control line.

use super::files;
use super::platform::Fail;
use crate::macvm::clipboard::Item;
use crate::macvm::pasteboard;
use crate::macvm::proto::{ErrorCode, FileKind};
use crate::macvm::text;
use std::os::unix::net::UnixStream;

pub fn count() -> u64 {
    pasteboard::count()
}

pub fn read() -> Vec<Item> {
    pasteboard::read()
}

/// Set the guest pasteboard; the new change count, which the poll skips.
pub fn write(items: &[Item]) -> Result<u64, Fail> {
    pasteboard::write(items).ok_or_else(|| {
        (
            ErrorCode::Io,
            text::io_failed("cannot write the pasteboard", ""),
        )
    })
}

/// Send the JSON of a set over `guest-files` as `clip-<count>.json`; the
/// transfer name for the `clipboard` event.
pub fn send(
    token: &str,
    count: u64,
    json: &[u8],
    dial: impl FnOnce() -> std::io::Result<UnixStream>,
) -> Result<String, Fail> {
    let name = format!("clip-{count}.json"); // not user text
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, json).map_err(|e| {
        (
            ErrorCode::Io,
            text::io_failed("cannot write the clipboard file", &e.to_string()),
        )
    })?;
    let r = files::send(token, FileKind::Clipboard, &path, dial);
    let _ = std::fs::remove_file(&path);
    r.map(|()| name)
}
