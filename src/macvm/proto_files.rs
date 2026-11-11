//! The file transfer types of the wire protocol (`proto.rs`): the
//! pasteboard item of a `clipboard` event, and the header line of a
//! `guest-files` (port 1032) transfer. `proto` re-exports them.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One pasteboard item on the wire: each type with its bytes as base64.
/// `clipboard::Item` is the decoded form.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PasteItem {
    pub types: BTreeMap<String, String>,
}

/// What a `guest-files` (port 1032) transfer carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// The agent log, for `log.fetch`.
    Log,
    /// A crash report of the app.
    Crash,
    Clipboard,
    Drop,
}

/// The header line of a `guest-files` transfer. The bytes follow it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    pub token: String,
    pub kind: FileKind,
    pub name: String,
    pub size: u64,
}

impl FileHeader {
    pub fn kind(&self) -> String {
        format!("{:?} {} ({} bytes)", self.kind, self.name, self.size) // not user text
    }
}
