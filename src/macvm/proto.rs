//! The wire protocol between the host shim (`paddock-vm app`) and the guest
//! agent `paddock-guest`, on the `guest-control` vsock port (1030).
//!
//! One JSON object per line, [`LINE_MAX`] bytes at most. The guest dials
//! the host and speaks first with `hello`. The host answers with `token`.
//! After that the guest sends events (`"ev"`) and the host sends requests
//! (`"op"`). Every request carries a `req` id and gets exactly one `reply`
//! event with that id. Every request has a timeout, in [`timeout`], and the
//! table has no wildcard arm, so a new request fails to compile without
//! an entry.
//!
//! Versions are a major and a minor. Equal majors talk, any minors. A
//! different agent version makes the host send `agent.update` first, and
//! only an agent that cannot update itself is refused, with the text that
//! names `paddock macvm update`.
//!
//! `kind()` of every message is the text for the logs: it carries no
//! token, no password, no clipboard text and no typed text.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

pub use super::axmirror::Node as AxNode;
pub use super::menu::MenuItem;
pub use super::proto_files::{FileHeader, FileKind, PasteItem};
pub use super::proto_setup::{
    check_hello, check_hello_sha, ConfigureStep, HelloAction, StepResult,
};
pub use super::proxy_header::{Identity, ProxyHeader, HEADER_VERSION};
pub use super::version::Version;
pub use crate::vm::proto::{read_line, read_raw_line, write_line};

/// The longest line, without the newline.
pub const LINE_MAX: usize = 256 * 1024;
/// The serial of the data disk: the host sets it on the virtio block device
/// of `data.img`, the agent finds the disk by it.
pub const DATA_DISK_SERIAL: &str = "paddock-data";
/// Where the virtiofs shares of a session appear in the guest, by name.
pub const SHARE_ROOT: &str = "/Volumes/My Shared Files";
/// The protocol version of this build.
pub const VERSION: Version = Version { major: 1, minor: 1 };

/// Older agents close the connection on unknown requests.
pub fn supports_clock_set(v: Version) -> bool {
    v.major == 1 && v.minor >= 1
}
/// The hello of a cold boot comes within this time.
pub const HELLO_COLD: Duration = Duration::from_secs(90);
/// The hello of a restored session comes within this time.
pub const HELLO_WARM: Duration = Duration::from_secs(30);
/// The agent waits this long for the `token` after its `hello`.
pub const TOKEN_WAIT: Duration = Duration::from_secs(30);
/// The agent sends `health` this often.
pub const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
/// The agent reads the window list this often, between notifications.
pub const WINDOW_POLL: Duration = Duration::from_millis(100);
/// The agent dials again this long after a lost connection.
pub const RECONNECT: Duration = Duration::from_secs(2);
/// The data disk free space that makes the agent send `disk {warn: true}`.
pub const DISK_WARN: u64 = 2 << 30;
/// The second threshold: `disk {warn: false}` again.
pub const DISK_LOW: u64 = 1 << 30;

/// A window frame in guest points: the window server coordinates of the
/// guest, origin top left.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Frame { x, y, w, h }
    }

    /// True when every edge is within `eps` points of the other frame.
    pub fn near(&self, o: &Frame, eps: f64) -> bool {
        (self.x - o.x).abs() <= eps
            && (self.y - o.y).abs() <= eps
            && (self.w - o.w).abs() <= eps
            && (self.h - o.h).abs() <= eps
    }
}

/// A point in guest points.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The size of the guest display in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub w: u32,
    pub h: u32,
}

/// What kind of window a guest window is. The host gives each its own
/// host window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Normal,
    Floating,
    Menu,
    Popover,
    Tooltip,
    Sheet,
    Hidden,
}

impl Level {
    /// Every level, for the tests of the mapping.
    pub const ALL: [Level; 7] = [
        Level::Normal,
        Level::Floating,
        Level::Menu,
        Level::Popover,
        Level::Tooltip,
        Level::Sheet,
        Level::Hidden,
    ];
}

/// One guest window, as the agent reports it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Window {
    /// The window server number of the guest.
    pub id: u32,
    pub pid: u32,
    /// The bundle id of the owner, or `system` for a guest prompt.
    pub app: String,
    pub title: String,
    pub frame: Frame,
    pub level: Level,
    /// The window this one belongs to: a sheet, a popover, an input
    /// method candidate list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    pub minimized: bool,
    pub focused: bool,
    pub resizable: bool,
    pub movable: bool,
    /// The height of the title bar from the top of the window, in points,
    /// with the toolbar of a unified window. A press there and a move
    /// start a host drag. None from an older agent or without
    /// Accessibility: the host takes [`TITLE_BAR`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_bar: Option<f64>,
    /// The buttons in the title bar (the window buttons, the toolbar
    /// items), relative to the top left of the window: a press there goes
    /// to the guest, never to a host drag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Frame>,
    /// The corner radius that the agent measured on the window's image, in
    /// points. None until it is measured, or without Screen Recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// The window is in the guest's full screen (`AXFullScreen`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub full_screen: bool,
}

/// The title bar height without a report of the agent: the title bar of
/// a window without a toolbar.
pub const TITLE_BAR: f64 = 28.0;

/// The window event: the order of every window, front first, the windows
/// that are new or changed, and the ids that went away.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WindowsEvent {
    pub order: Vec<u32>,
    pub changed: Vec<Window>,
    pub removed: Vec<u32>,
}

/// The first message of the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub v: Version,
    /// The paddock version of the agent build.
    pub agent: String,
    /// The macOS version of the guest, `15.2`.
    pub macos: String,
    /// The macOS build of the guest, `24C101`.
    pub build: String,
    pub scale: f64,
    pub display: Display,
    /// False when the `launch` script of the guest cannot take an update:
    /// the host then refuses a different major with the `paddock macvm
    /// update` line.
    pub can_update: bool,
    /// The sha256 of the agent's own binary. A rebuilt agent keeps the
    /// version, so the host compares this with the agent it ships and
    /// updates the guest when they differ. An older agent sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// The health report, every [`HEALTH_INTERVAL`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// Seconds since the guest booted.
    pub uptime: u64,
    /// The one minute load average.
    pub load: f64,
    pub mem_free: u64,
    /// None while the data disk is not mounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_disk_free: Option<u64>,
    pub app_running: bool,
    /// The input source of the guest, `com.apple.keylayout.US`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_source: Option<String>,
    /// The result of the data disk check after an unclean session, until
    /// the host saw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_verify: Option<String>,
    /// True when the agent has the Accessibility permission. The guided
    /// step of the golden build waits for it.
    #[serde(default)]
    pub accessibility: bool,
    /// The guest screen is locked (`CGSSessionScreenIsLocked`).
    #[serde(default)]
    pub locked: bool,
}

/// A message from the agent to the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Event {
    Hello(Hello),
    Health(Health),
    Windows(WindowsEvent),
    /// The front application or the key window of the guest changed.
    Focus {
        app: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u32>,
    },
    /// The guest pasteboard changed while watched: the items inline, or the
    /// name of the `guest-files` transfer that carried them as JSON.
    Clipboard {
        count: u64,
        #[serde(default)]
        items: Vec<PasteItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer: Option<String>,
    },
    /// The menu bar of the app, on request and when the key window of the
    /// guest changed. `cut` says the 20 KiB cap left whole submenus out.
    Menu {
        app: String,
        items: Vec<MenuItem>,
        #[serde(default)]
        cut: bool,
    },
    AppExited {
        app: String,
        /// 0 for an end without a crash report, 1 with one: the guest
        /// cannot read the status of a process that Launch Services
        /// started.
        code: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crash_report: Option<PathBuf>,
        /// False after three exits in a minute: the host stops the
        /// relaunch and prints the crash report path.
        relaunch: bool,
    },
    Pong {
        /// The guest's monotonic clock in milliseconds.
        t: u64,
    },
    #[serde(rename = "ax.value")]
    AxValue {
        window: u32,
        text: String,
    },
    /// The Accessibility tree of a guest window, on request and after a
    /// focus change; `cut` says the cap cut it.
    #[serde(rename = "ax.tree")]
    AxTree {
        window: u32,
        nodes: Vec<AxNode>,
        #[serde(default)]
        cut: bool,
    },
    Disk {
        data_free: u64,
        warn: bool,
    },
    #[serde(rename = "data.unmountable")]
    DataUnmountable {
        text: String,
    },
    /// `data.prepare` goes on: the step or the part of the copy, and the
    /// time that the rest may take, which the host waits on top of now.
    #[serde(rename = "data.progress")]
    DataProgress {
        text: String,
        wait_secs: u64,
    },
    /// A system dialog came up in the guest (the disk dialog of
    /// diskarbitrationd): its owner, its title and buttons, and whether the
    /// agent dismissed it.
    Dialog {
        owner: String,
        text: String,
        dismissed: bool,
    },
    /// The guest screen locked or unlocked, at the poll that saw it: the
    /// uptime of the guest and the seconds since its last input.
    /// A guest program opened an http or https link: the default browser
    /// of the guest is paddock's link helper, which hands it to the agent.
    /// The host opens it in the host browser after its approval.
    #[serde(rename = "link.open")]
    LinkOpen {
        url: String,
    },
    #[serde(rename = "screen.lock")]
    ScreenLock {
        locked: bool,
        uptime: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idle_secs: Option<u64>,
    },
    Reply(Reply),
}

/// The answer to one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub req: u64,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// How a request ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<ReplyData>,
    },
    Error {
        code: ErrorCode,
        text: String,
    },
}

/// The payload of a successful reply, when the request has one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyData {
    /// `app.launch`: the pid of the app.
    Launched { pid: u32 },
    /// `input.set`: the input source ids that the guest does not have.
    InputSet { missing: Vec<String> },
    /// `configure`: one result per step.
    Configure { steps: Vec<StepResult> },
    /// `data.prepare`: the output of the verify after the move.
    DataPrepare { verify: String },
    /// A bench op: the pasteboard, the environment or the Dock menu.
    Text { text: String },
    /// `shares.open`: the open files per share name.
    OpenFiles { per_share: BTreeMap<String, u32> },
    /// `window.move`: the frame that the window has after the move, as
    /// Accessibility reads it back, and the time of the move in the
    /// agent in milliseconds. The host shows the frame without waiting
    /// for the next window event.
    Moved { frame: Frame, ms: f64 },
}

/// Why a request failed. The host matches on the code and shows the user
/// its own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The guest side timed out: a hung app did not answer an
    /// Accessibility call within its timeout.
    Timeout,
    /// The request runs only outside a session: `configure` and
    /// `data.prepare` while a followed app runs.
    NotInSession,
    NoSuchWindow,
    NoSuchApp,
    /// Another request of the same kind runs, or an app already runs.
    Busy,
    /// The agent has no Accessibility permission.
    Accessibility,
    /// The hash of an update does not match.
    BadHash,
    /// A formatted data disk that does not mount: untouched.
    Locked,
    /// A file or a command failed. The text has the detail.
    Io,
    /// This agent version does not implement the request.
    Unsupported,
    BadRequest,
    Internal,
}

/// A request from the host, with its id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub req: u64,
    #[serde(flatten)]
    pub request: Request,
}

/// A message from the host to the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The answer to `hello`: the session token and the host's version.
    Token {
        token: String,
        v: Version,
        /// The session started with `--bench`: the bench ops are honored.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        bench: bool,
        /// The host's wall clock, in milliseconds since 1970. A guest that
        /// resumed from a saved state keeps the time of the save; the
        /// Retained for compatibility with older agents.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        now_ms: Option<u64>,
    },
    /// Set the guest clock using the session password.
    #[serde(rename = "clock.set")]
    ClockSet {
        unix_ms: u64,
        password: String,
    },
    #[serde(rename = "window.move")]
    WindowMove {
        id: u32,
        frame: Frame,
    },
    #[serde(rename = "window.raise")]
    WindowRaise {
        id: u32,
    },
    #[serde(rename = "window.minimize")]
    WindowMinimize {
        id: u32,
    },
    #[serde(rename = "window.close")]
    WindowClose {
        id: u32,
    },
    #[serde(rename = "window.focus")]
    WindowFocus {
        id: u32,
    },
    #[serde(rename = "app.launch")]
    AppLaunch {
        path: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    #[serde(rename = "app.quit")]
    AppQuit {
        app: String,
        force: bool,
    },
    #[serde(rename = "app.copy")]
    AppCopy {
        from: PathBuf,
        to: PathBuf,
        version: String,
        /// The guest removes `com.apple.quarantine` from the copy. True
        /// only when the host bundle carried the user's approval
        /// (`appcopy::quarantined_unapproved`).
        #[serde(default)]
        unquarantine: bool,
    },
    /// The host pasteboard for the guest: the items inline, or the guest
    /// path of a JSON file of them under the `drop` share.
    #[serde(rename = "clipboard.set")]
    ClipboardSet {
        count: u64,
        #[serde(default)]
        items: Vec<PasteItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transfer: Option<PathBuf>,
    },
    /// Poll the guest pasteboard at 10 Hz and send `clipboard` on a
    /// change: on while a session window is key on the host.
    #[serde(rename = "clipboard.watch")]
    ClipboardWatch {
        on: bool,
    },
    Drop {
        window: u32,
        point: Point,
        paths: Vec<PathBuf>,
    },
    #[serde(rename = "display.set")]
    DisplaySet {
        w: u32,
        h: u32,
        scale: f64,
    },
    #[serde(rename = "data.prepare")]
    DataPrepare {
        password: String,
        data_id: String,
    },
    Configure {
        password: String,
        steps: Vec<ConfigureStep>,
    },
    Ping {
        t: u64,
    },
    #[serde(rename = "agent.update")]
    AgentUpdate {
        path: PathBuf,
        sha256: String,
        version: String,
    },
    #[serde(rename = "agent.restart")]
    AgentRestart,
    #[serde(rename = "input.set")]
    InputSet {
        sources: Vec<String>,
        current: String,
        reduce_motion: bool,
        increase_contrast: bool,
    },
    #[serde(rename = "log.fetch")]
    LogFetch,
    #[serde(rename = "disk.low")]
    DiskLow,
    /// The menu bar of the app now, as a `menu` event before the reply.
    Menu {
        app: String,
    },
    /// Press the item with this Accessibility path of the last `menu`
    /// event (`kAXPressAction`).
    #[serde(rename = "menu.press")]
    MenuPress {
        path: Vec<usize>,
    },
    /// Send `ax.value` events for the focused text field (the bench).
    #[serde(rename = "ax.watch")]
    AxWatch {
        on: bool,
    },
    /// The tree of this window now, as an `ax.tree` event before the reply.
    #[serde(rename = "ax.tree")]
    AxTree {
        window: u32,
    },
    /// `kAXPressAction` on the element at this path, if its role is still `role`.
    #[serde(rename = "ax.press")]
    AxPress {
        window: u32,
        path: Vec<usize>,
        role: String,
    },
    /// Set `kAXValueAttribute` of the element at this path of the window.
    #[serde(rename = "ax.set")]
    AxSet {
        window: u32,
        path: Vec<usize>,
        role: String,
        value: String,
    },
    /// Shut the guest down: `sudo -S shutdown -h now` with the guest
    /// password on stdin (`guest/shutdown.rs`). Refused while an app runs.
    /// An older host sends no password: the agent then answers an error.
    Shutdown {
        #[serde(default)]
        password: String,
    },
    /// Unlock the login keychain of the account with the guest password
    /// (never on a command line), and point the search list and the
    /// default keychain at the home of the data disk (`guest/keychain.rs`).
    #[serde(rename = "keychain.unlock")]
    KeychainUnlock {
        password: String,
    },
    /// The bench ops, honored only in a `--bench` session
    /// (`bench::op_gate`). A 60 Hz counter in the text area of the window
    /// for `secs` seconds, for `bench frames`.
    #[serde(rename = "bench.animate")]
    BenchAnimate {
        window: u32,
        #[serde(default = "default_animate_secs")]
        secs: u32,
    },
    /// Focus the window and make sure that it has a text area, for
    /// `bench input` (TextEdit).
    #[serde(rename = "bench.text")]
    BenchText {
        window: u32,
    },
    /// The agent stops answering, for the hang row of the failure matrix.
    #[serde(rename = "bench.hang")]
    BenchHang,
    /// Start a child process of the agent, for the checklist.
    #[serde(rename = "bench.spawn_child")]
    BenchSpawnChild {
        argv: Vec<String>,
    },
    /// Read the guest pasteboard as text (`get`), or set it (`set`).
    #[serde(rename = "bench.pasteboard")]
    BenchPasteboard {
        action: String,
        #[serde(default)]
        text: String,
    },
    /// The environment of the agent, names and values, for the checklist.
    #[serde(rename = "bench.env")]
    BenchEnv,
    /// The items of the Dock menu of the app in the guest, as text.
    #[serde(rename = "bench.dock_menu")]
    BenchDockMenu,
    /// The open files of the guest per share under [`SHARE_ROOT`], as
    /// `ReplyData::OpenFiles` (`lsof`), for `paddock app share NAME list`.
    #[serde(rename = "shares.open")]
    SharesOpen,
    /// Lock the guest screen now, for the lock check.
    #[serde(rename = "bench.lock")]
    BenchLock,
    /// The state of the login keychain, its search list and its default.
    #[serde(rename = "bench.keychain")]
    BenchKeychain,
    /// Forward one host connection of a login callback: connect to
    /// `127.0.0.1:port` (then `[::1]:port`) in the guest, and only then dial
    /// the `guest-login` port with the token line and the ticket line.
    #[serde(rename = "login.forward")]
    LoginForward {
        port: u16,
        ticket: String,
    },
}

fn default_animate_secs() -> u32 {
    4
}

/// The timeout of each request. No wildcard arm: a new request fails to
/// compile without an entry.
pub fn timeout(r: &Request) -> Duration {
    let secs = match r {
        Request::Token { .. } => 5,
        Request::ClockSet { .. } => 20,
        Request::WindowMove { .. }
        | Request::WindowRaise { .. }
        | Request::WindowMinimize { .. }
        | Request::WindowClose { .. }
        | Request::WindowFocus { .. } => 2,
        Request::AppLaunch { .. } => 30,
        Request::AppQuit { force: false, .. } => 60,
        Request::AppQuit { force: true, .. } => 10,
        Request::AppCopy { .. } => 300,
        Request::ClipboardSet { .. } => 5,
        Request::ClipboardWatch { .. } => 2,
        Request::Drop { .. } => 10,
        Request::DisplaySet { .. } => 5,
        Request::DataPrepare { .. } => 120,
        Request::Configure { steps, .. } => super::configure::request_secs(steps.len()),
        Request::Ping { .. } => 5,
        Request::AgentUpdate { .. } => 30,
        Request::AgentRestart => 5,
        Request::InputSet { .. } => 5,
        Request::LogFetch => 20,
        Request::DiskLow => 5,
        Request::Menu { .. } | Request::MenuPress { .. } => 2,
        Request::AxWatch { .. } => 2,
        Request::AxTree { .. } | Request::AxPress { .. } | Request::AxSet { .. } => 2,
        Request::Shutdown { .. } => 30,
        Request::KeychainUnlock { .. } => 20,
        Request::BenchAnimate { secs, .. } => u64::from(*secs) + 5,
        Request::BenchText { .. } | Request::BenchPasteboard { .. } | Request::BenchEnv => 5,
        Request::BenchHang => 2,
        Request::BenchSpawnChild { .. } | Request::BenchDockMenu => 10,
        Request::SharesOpen => 15,
        Request::BenchLock => 5,
        Request::BenchKeychain => 10,
        Request::LoginForward { .. } => 10,
    };
    Duration::from_secs(secs)
}

impl Reply {
    pub fn ok(req: u64) -> Self {
        Reply {
            req,
            outcome: Outcome::Ok { data: None },
        }
    }

    pub fn data(req: u64, data: ReplyData) -> Self {
        Reply {
            req,
            outcome: Outcome::Ok { data: Some(data) },
        }
    }

    pub fn error(req: u64, code: ErrorCode, text: impl fmt::Display) -> Self {
        Reply {
            req,
            outcome: Outcome::Error {
                code,
                text: text.to_string().replace(['\n', '\r'], " "),
            },
        }
    }

    /// A short text for the logs.
    pub fn kind(&self) -> String {
        match &self.outcome {
            Outcome::Ok { data: None } => format!("reply {} ok", self.req), // not user text
            Outcome::Ok { data: Some(d) } => format!("reply {} ok with {}", self.req, d.kind()), // not user text
            Outcome::Error { code, text } => format!("reply {} error {code:?}: {text}", self.req), // not user text
        }
    }
}

impl Outcome {
    /// The frame and the agent's milliseconds of a `window.move` reply
    /// that has them.
    pub fn moved_with_ms(&self) -> Option<(Frame, f64)> {
        match self {
            Outcome::Ok {
                data: Some(ReplyData::Moved { frame, ms }),
            } => Some((*frame, *ms)),
            _ => None,
        }
    }

    /// The frame of a `window.move` reply that has one.
    pub fn moved(&self) -> Option<Frame> {
        self.moved_with_ms().map(|(f, _)| f)
    }
}

impl ReplyData {
    fn kind(&self) -> String {
        match self {
            ReplyData::Launched { pid } => format!("pid {pid}"), // not user text
            ReplyData::InputSet { missing } => format!("{} missing sources", missing.len()), // not user text
            ReplyData::Configure { steps } => format!("{} step results", steps.len()), // not user text
            ReplyData::DataPrepare { .. } => "data verify".into(),
            ReplyData::Text { text } => format!("{} bytes of text", text.len()), // not user text
            ReplyData::OpenFiles { per_share } => format!("open files of {}", per_share.len()), // not user text
            ReplyData::Moved { frame: f, ms } => {
                let (x, y, w, h) = (f.x, f.y, f.w, f.h);
                format!("frame {x},{y} {w}x{h} in {ms:.1} ms") // not user text
            }
        }
    }
}

impl Call {
    pub fn kind(&self) -> String {
        format!("req {} {}", self.req, self.request.kind()) // not user text
    }
}

#[path = "proto_kind.rs"]
mod kind;
pub use kind::link_kind;

#[cfg(test)]
#[path = "proto_tests.rs"]
mod tests;

/// The wall clock now, in milliseconds since 1970, for clock synchronization.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
