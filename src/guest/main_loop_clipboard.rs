//! The clipboard of the agent's main loop: the poll of the guest pasteboard
//! while a session window is key on the host, `clipboard.set` and the
//! transfer file of a big set.

use super::Session;
use crate::guest::platform::Platform;
use crate::macvm::clipboard::{self, Payload};
use crate::macvm::proto::{ErrorCode, Event, PasteItem, Reply, SHARE_ROOT};
use crate::macvm::text;
use std::path::{Path, PathBuf};

impl<P: Platform> Session<'_, P> {
    /// The guest pasteboard, at the window poll rate (10 Hz) while the host
    /// says a session window is key. A change goes inline up to 64 KiB, as a
    /// `guest-files` transfer up to 16 MiB, and the marked items never.
    pub(super) fn clipboard_poll(&mut self) -> Result<(), String> {
        if !self.st.clipboard_watch {
            return Ok(());
        }
        let count = self.p.clipboard_count();
        if self.st.clip_seen == Some(count) {
            return Ok(());
        }
        self.st.clip_seen = Some(count);
        let items = self.p.clipboard_read();
        let items = clipboard::convert(items, &mut |t| self.p.tiff_to_png(t));
        let kept = match clipboard::prepare(&items) {
            Ok(kept) if kept.is_empty() => return Ok(()),
            Ok(kept) => kept,
            Err(badge) => {
                log::info!("the clipboard change {count} stays in the guest: {badge}");
                return Ok(());
            }
        };
        let payload = Payload { items: kept };
        if payload.inline() {
            return self.writer.send(&Event::Clipboard {
                count,
                items: clipboard::to_wire(&payload.items),
                transfer: None,
            });
        }
        let Some(token) = self.st.token.clone() else {
            return Ok(());
        };
        let json =
            serde_json::to_vec(&clipboard::to_wire(&payload.items)).map_err(|e| e.to_string())?;
        match self.p.send_clipboard(&token, count, &json) {
            Ok(name) => self.writer.send(&Event::Clipboard {
                count,
                items: Vec::new(),
                transfer: Some(name),
            }),
            Err((_, text)) => {
                log::warn!("the clipboard transfer failed: {text}");
                Ok(())
            }
        }
    }

    /// `clipboard.set`: the items of the line, or of the JSON file under
    /// the drop share that the host wrote for a big set.
    pub(super) fn clipboard_set(
        &mut self,
        req: u64,
        items: Vec<PasteItem>,
        transfer: Option<PathBuf>,
    ) -> Reply {
        let items = match transfer {
            None => clipboard::from_wire(&items),
            Some(path) => match read_transfer(&path) {
                Ok(items) => items,
                Err(text) => return Reply::error(req, ErrorCode::BadRequest, text),
            },
        };
        match self.p.clipboard_write(&items) {
            Ok(count) => {
                self.st.clip_seen = Some(count);
                Reply::ok(req)
            }
            Err((code, text)) => Reply::error(req, code, text),
        }
    }
}

/// The most bytes of a clipboard transfer file: 16 MiB of data as base64
/// in JSON.
const TRANSFER_FILE_MAX: u64 = clipboard::TRANSFER_MAX * 3 / 2 + 1024;

/// The items of a clipboard transfer file of the host. The path must be
/// under the `drop` share.
fn read_transfer(path: &Path) -> Result<Vec<clipboard::Item>, String> {
    let drop = Path::new(SHARE_ROOT).join("drop");
    if !crate::macvm::shares::normalize(path).starts_with(&drop) {
        return Err(text::io_failed(
            "the clipboard file is not under the drop share",
            "",
        ));
    }
    let size = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| text::io_failed("cannot read the clipboard file", &e.to_string()))?;
    if size > TRANSFER_FILE_MAX {
        return Err(text::clipboard_too_large(size));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| text::io_failed("cannot read the clipboard file", &e.to_string()))?;
    let wire: Vec<PasteItem> = serde_json::from_slice(&bytes)
        .map_err(|e| text::io_failed("the clipboard file is not JSON", &e.to_string()))?;
    Ok(clipboard::from_wire(&wire))
}
