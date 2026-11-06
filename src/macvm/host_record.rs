//! The parts of the session record (`host::Session`) that the shim writes
//! while the session runs: the shares, the last health, the start and the
//! app. `host` re-exports them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The `Running` record of a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The `paddock-vm app` shim.
    pub pid: u32,
    pub app: String,
    pub home: String,
    pub profile: PathBuf,
    /// The VM slot, written by the shim when it has one.
    #[serde(default)]
    pub slot: Option<usize>,
    /// The host input source that the shim put aside while a session
    /// window is key, written by the shim, empty when the host has its own.
    /// `paddock app list` restores it when the shim is gone (item 21 of
    /// the security checklist).
    #[serde(default)]
    pub input_source: String,
    /// Seconds since the epoch.
    pub started: u64,
    pub control: PathBuf,
    /// The session started with `--bench`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bench: bool,
    /// The badge text of the session's windows, written by the
    /// shim, shown by the marker next to the paddock mark. None when there is nothing to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// The last `health` of the guest, written by the shim, for `paddock
    /// app status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<RecordHealth>,
    /// How this session started and how long it took to the hello.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<RecordStart>,
    /// The app of the session, once it was launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_info: Option<RecordApp>,
    /// The counters of the stub network, written by the shim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<serde_json::Value>,
    /// The network mode word of the session (`ask`, `egress`, ...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub network_mode: String,
    /// The workspace folders of the session, for `paddock app status`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<RecordShare>,
    /// The menu bar replica, written by the shim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<super::menu::MenuRecord>,
    /// The Accessibility mirror, written by the shim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ax: Option<super::axmirror::AxRecord>,
    /// The Dock name of the session, `ChatGPT (paddock)`, for the menu
    /// entry of the marker.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    /// The name of the guest input source while it differs from the
    /// host's, written by the shim, for the badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_input: Option<String>,
    /// The instance of the profile: 1 is the folder `default`. An old
    /// record without the field is instance 1.
    #[serde(default = "first_instance")]
    pub instance: u32,
    /// The last time a window of the session was key, in seconds since
    /// the epoch, written by the shim. Automatic suspend takes the least
    /// recently used session.
    #[serde(default)]
    pub last_key: u64,
    /// Why the session cannot be suspended now (`dialog`, `copy`,
    /// `data_prepare`), written by the shim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<String>,
    /// The session saved its state and gave its slot back. Its shim stays
    /// in the Dock: a click on it, or `paddock app run`, resumes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suspended: bool,
    /// Why the session was suspended, for `paddock app status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend_reason: Option<String>,
}

fn first_instance() -> u32 {
    super::instance::FIRST
}

impl Session {
    /// The instance number, 1 for a record without one.
    pub fn instance(&self) -> u32 {
        self.instance.max(super::instance::FIRST)
    }

    /// The key of the instance: `chatgpt`, `chatgpt--2`.
    pub fn key(&self) -> String {
        super::instance::key(&self.app, self.instance())
    }

    /// The name of the session for the user: `chatgpt`, `chatgpt 2`.
    pub fn label(&self) -> String {
        super::instance::label(&self.app, self.instance())
    }

    /// The name of the session in a command: `chatgpt`, `chatgpt
    /// --instance 2`.
    pub fn arg(&self) -> String {
        super::instance::arg(&self.app, self.instance())
    }
}

/// The options of `paddock app run` that only the macOS guest reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuestOptions {
    /// The session honors the bench ops (`paddock macvm bench`).
    pub bench: bool,
    /// The instance of the app profile (1 is the first).
    pub instance: Option<u32>,
    /// A new instance, the next free number.
    pub new_instance: bool,
    /// Why a suspended session resumes (a click on its Dock icon).
    pub resumed_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordShare {
    pub path: PathBuf,
    pub read_only: bool,
}

/// The last `health` of the guest in the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordHealth {
    /// Seconds since the epoch when it came.
    pub t: u64,
    pub load: f64,
    pub mem_free: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_disk_free: Option<u64>,
    pub app_running: bool,
    /// The host windows of the session at that time.
    pub windows: usize,
    /// The input source of the guest, `com.apple.keylayout.US`, as the Mac
    /// check reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_source: Option<String>,
    /// The guest screen is locked, as the Mac check reads it.
    #[serde(default)]
    pub locked: bool,
}

/// The start of the session in the record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordStart {
    /// `warm`, `cold`, `fresh` or `repair`.
    pub kind: String,
    /// Seconds from the boot to the hello.
    pub secs: f64,
}

/// The app of the session in the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordApp {
    pub name: String,
    pub version: String,
}
