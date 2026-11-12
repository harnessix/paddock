//! Computer use of a guest session: the wire of the `guest-computer` vsock
//! port (1034) between the host and the guest agent, and the arithmetic
//! that both sides share. Plain Rust: the tests run on every system.
//!
//! The host reaches the port through the control socket of the session
//! (`connect guest-computer`, see `vm::bridge`). The agent listens on the
//! port in the guest and takes connections from the host only (CID 2).
//! Each connection is independent: `paddock mac`, `paddock mcp mac` and
//! `paddock mac view` each open their own.
//!
//! The framing: one JSON object per line, at most [`LINE_MAX`] bytes. A
//! line with `"bytes": N` is followed by exactly N raw bytes (a PNG, or
//! the JSON of an accessibility snapshot), at most [`PAYLOAD_MAX`].
//!
//! - The agent speaks first: [`Msg::Hello`].
//! - The host sends [`Call`]s: an `id` and an `op` ([`Op`]). Every call
//!   gets exactly one [`Msg::Reply`] with that id, in the order of the
//!   calls.
//! - The frame contract: `frames.subscribe {target, max_fps}` answers
//!   `ok`, and from then on the agent sends [`Msg::Frame`] lines, each with
//!   a PNG, a sequence number and the size, only when the picture changed,
//!   at most `max_fps` per second. The first frame after the subscribe
//!   always comes. `frames.stop` ends it: no frame follows its reply, and
//!   the reply names the last sequence number. A second subscribe on the
//!   same connection replaces the first, and its numbers start at 1 again.
//!   Other calls (input, status) may share the connection with a
//!   subscription; their replies come between whole frames.
//!
//! The target of every call is the whole guest display ([`Scope::Desktop`])
//! or the windows of the app of the session ([`Scope::App`]): the capture
//! shows only those windows, cropped to them, and input outside them is
//! refused.
//!
//! Coordinates: the agent works in guest points (the window server space
//! of the guest, origin top left). An image has a [`Geometry`]: the region
//! of the guest it shows and its size in pixels. The host maps a pixel of
//! the image to a guest point with [`Geometry::to_guest`] and sends points.

use super::proto::{Display, Frame, Point};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The version of this wire, in the hello.
pub const VERSION: u32 = 1;
/// The longest line, without the newline.
pub const LINE_MAX: usize = 256 * 1024;
/// The largest payload after a line: a PNG of a 6K display fits.
pub const PAYLOAD_MAX: u64 = 96 << 20;
/// The lowest and the highest `max_fps` of a subscription.
pub const FPS_MIN: f64 = 0.2;
pub const FPS_MAX: f64 = 30.0;
/// The most nodes of one accessibility snapshot, and the default.
pub const AX_NODES_MAX: u32 = 5000;
pub const AX_NODES_DEFAULT: u32 = 1500;
/// The deepest snapshot, and the default depth.
pub const AX_DEPTH_MAX: u32 = 40;
pub const AX_DEPTH_DEFAULT: u32 = 12;
/// The longest text of one `type` call, in characters.
pub const TYPE_MAX: usize = 20_000;

/// What a call works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// The whole guest display, every window on it.
    #[default]
    Desktop,
    /// Only the windows of the app of the session (its sheets, menus and
    /// panels included).
    App,
}

impl Scope {
    pub fn word(self) -> &'static str {
        match self {
            Scope::Desktop => "desktop",
            Scope::App => "app",
        }
    }
}

/// The target of a call, on the wire `{"scope": "app"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Target {
    pub scope: Scope,
}

impl Target {
    pub const DESKTOP: Target = Target {
        scope: Scope::Desktop,
    };
    pub const APP: Target = Target { scope: Scope::App };
}

/// The largest image that the host wants, in pixels. The agent scales
/// the image down to fit, keeping its aspect; it never scales up past the
/// native pixels of the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaxSize {
    pub width: u32,
    pub height: u32,
    /// The most pixels of the image, width times height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixels: Option<u64>,
}

impl MaxSize {
    /// The image that a model reads without scaling it again: 1568
    /// pixels on the long side, 1.15 megapixels at most.
    pub const AGENT: MaxSize = MaxSize {
        width: 1568,
        height: 1568,
        pixels: Some(1_150_000),
    };

    /// At most `width` by `height`.
    pub fn of(width: u32, height: u32) -> MaxSize {
        MaxSize {
            width,
            height,
            pixels: None,
        }
    }
}

/// The size of a region in guest points.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Extent {
    pub w: f64,
    pub h: f64,
}

/// The region of the guest that an image shows, and its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Geometry {
    /// The guest point of the top left pixel.
    pub origin: Point,
    /// The size of the region in guest points.
    pub points: Extent,
    /// The size of the image in pixels.
    pub width: u32,
    pub height: u32,
}

impl Geometry {
    /// The image of `region` at `scale` pixels per point (the display
    /// scale), made smaller to fit `max` when given. A region of no area
    /// gives a 1 by 1 image.
    pub fn plan(region: Frame, scale: f64, max: Option<MaxSize>) -> Geometry {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let (rw, rh) = (region.w.max(0.0), region.h.max(0.0));
        let mut f = scale;
        if let Some(m) = max {
            if rw > 0.0 && m.width > 0 {
                f = f.min(f64::from(m.width) / rw);
            }
            if rh > 0.0 && m.height > 0 {
                f = f.min(f64::from(m.height) / rh);
            }
            if let Some(px) = m.pixels.filter(|p| *p > 0) {
                let area = rw * rh;
                if area > 0.0 {
                    f = f.min((px as f64 / area).sqrt());
                }
            }
        }
        let px = |v: f64| -> u32 { ((v * f).round() as u32).max(1) };
        Geometry {
            origin: Point {
                x: region.x,
                y: region.y,
            },
            points: Extent { w: rw, h: rh },
            width: px(rw),
            height: px(rh),
        }
    }

    /// The region in guest points.
    pub fn region(&self) -> Frame {
        Frame::new(self.origin.x, self.origin.y, self.points.w, self.points.h)
    }

    /// Pixels per point across and down.
    fn factors(&self) -> (f64, f64) {
        let fx = if self.points.w > 0.0 {
            f64::from(self.width) / self.points.w
        } else {
            1.0
        };
        let fy = if self.points.h > 0.0 {
            f64::from(self.height) / self.points.h
        } else {
            1.0
        };
        (fx, fy)
    }

    /// The guest point of pixel `(x, y)` of the image. The point is in the
    /// middle of the pixel's area at a scale above 1, so a click hits what
    /// the pixel shows.
    pub fn to_guest(&self, x: f64, y: f64) -> Point {
        let (fx, fy) = self.factors();
        Point {
            x: self.origin.x + x / fx,
            y: self.origin.y + y / fy,
        }
    }

    /// The pixel of the image that shows guest point `p`.
    pub fn to_image(&self, p: Point) -> (f64, f64) {
        let (fx, fy) = self.factors();
        ((p.x - self.origin.x) * fx, (p.y - self.origin.y) * fy)
    }

    /// A guest frame in pixels of the image.
    pub fn frame_to_image(&self, f: Frame) -> Frame {
        let (x, y) = self.to_image(Point { x: f.x, y: f.y });
        let (fx, fy) = self.factors();
        Frame::new(x, y, f.w * fx, f.h * fy)
    }

    /// True when pixel `(x, y)` is inside the image.
    pub fn holds_pixel(&self, x: f64, y: f64) -> bool {
        x >= 0.0 && y >= 0.0 && x < f64::from(self.width) && y < f64::from(self.height)
    }

    /// True when both show the same region at the same size, within half
    /// a point: an image of one is still right for the other.
    pub fn same(&self, o: &Geometry) -> bool {
        self.width == o.width && self.height == o.height && self.region().near(&o.region(), 0.5)
    }
}

/// The union of frames, or None for none.
pub fn union(frames: impl IntoIterator<Item = Frame>) -> Option<Frame> {
    let mut it = frames.into_iter();
    let first = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for f in it {
        x0 = x0.min(f.x);
        y0 = y0.min(f.y);
        x1 = x1.max(f.x + f.w);
        y1 = y1.max(f.y + f.h);
    }
    Some(Frame::new(x0, y0, x1 - x0, y1 - y0))
}

/// The part of `f` on the display, or None when nothing is on it.
pub fn clip(f: Frame, display: Display) -> Option<Frame> {
    let x0 = f.x.max(0.0);
    let y0 = f.y.max(0.0);
    let x1 = (f.x + f.w).min(f64::from(display.w));
    let y1 = (f.y + f.h).min(f64::from(display.h));
    (x1 > x0 && y1 > y0).then(|| Frame::new(x0, y0, x1 - x0, y1 - y0))
}

/// True when the frame holds the point (the right and bottom edges out).
pub fn holds(f: &Frame, p: Point) -> bool {
    p.x >= f.x && p.y >= f.y && p.x < f.x + f.w && p.y < f.y + f.h
}

// ---- The messages --------------------------------------------------------

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

/// A modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mod {
    Cmd,
    Shift,
    Option,
    Ctrl,
    Fn,
}

/// One key press: the modifiers held and the key, by its name
/// (`computer_input::key_code`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Combo {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<Mod>,
    pub key: String,
}

/// One input action, in guest points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Input {
    /// `count` clicks of `button` at the point: 2 is a double click.
    Click {
        at: Point,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mods: Vec<Mod>,
    },
    /// Move the pointer, no button.
    Move { at: Point },
    /// Press at `from`, move to `to`, let go.
    Drag {
        from: Point,
        to: Point,
        #[serde(default)]
        button: Button,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mods: Vec<Mod>,
    },
    /// Scroll at the point by lines: `dy` above 0 scrolls down (toward
    /// the end), `dx` above 0 scrolls right.
    Scroll {
        at: Point,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// Type text as it is, whatever the keyboard layout of the guest.
    Type { text: String },
    /// Press keys one after the other: `cmd+shift+t`, `return`.
    Key { combos: Vec<Combo> },
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

impl Input {
    /// The point that the action starts at, when it has one.
    pub fn point(&self) -> Option<Point> {
        match self {
            Input::Click { at, .. } | Input::Move { at } | Input::Scroll { at, .. } => Some(*at),
            Input::Drag { from, .. } => Some(*from),
            Input::Type { .. } | Input::Key { .. } => None,
        }
    }

    /// Every point of the action.
    pub fn points(&self) -> Vec<Point> {
        match self {
            Input::Drag { from, to, .. } => vec![*from, *to],
            other => other.point().into_iter().collect(),
        }
    }

    /// True for typing and keys: they go to the focused window.
    pub fn is_keyboard(&self) -> bool {
        matches!(self, Input::Type { .. } | Input::Key { .. })
    }

    /// The name of the action, for the output and the logs. No text.
    pub fn word(&self) -> &'static str {
        match self {
            Input::Click { .. } => "click",
            Input::Move { .. } => "move",
            Input::Drag { .. } => "drag",
            Input::Scroll { .. } => "scroll",
            Input::Type { .. } => "type",
            Input::Key { .. } => "key",
        }
    }
}

/// A request of the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    /// The display, the permissions, the app and its windows.
    #[serde(rename = "status")]
    Status {
        #[serde(default)]
        target: Target,
    },
    /// One PNG of the target: `Data::Shot` and the PNG bytes.
    #[serde(rename = "screenshot")]
    Screenshot {
        #[serde(default)]
        target: Target,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<MaxSize>,
        /// Draw the pointer into the image.
        #[serde(default)]
        cursor: bool,
    },
    /// One input action into the guest session.
    #[serde(rename = "input")]
    Input {
        #[serde(default)]
        target: Target,
        action: Input,
    },
    /// The accessibility snapshot of the target: `Data::Ax` and the JSON
    /// of the [`Element`]s as bytes.
    #[serde(rename = "ax")]
    Ax {
        #[serde(default)]
        target: Target,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_nodes: Option<u32>,
    },
    /// `AXPress` on an element of the last snapshot, by its id. With a
    /// role, only when the element still has it.
    #[serde(rename = "ax.press")]
    AxPress {
        #[serde(default)]
        target: Target,
        element: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    /// Start the frame stream of the target.
    #[serde(rename = "frames.subscribe")]
    FramesSubscribe {
        #[serde(default)]
        target: Target,
        max_fps: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<MaxSize>,
        /// Draw the pointer into the frames.
        #[serde(default = "yes")]
        cursor: bool,
    },
    /// End the frame stream.
    #[serde(rename = "frames.stop")]
    FramesStop,
}

impl Op {
    /// The name of the op, for the logs.
    pub fn word(&self) -> &'static str {
        match self {
            Op::Status { .. } => "status",
            Op::Screenshot { .. } => "screenshot",
            Op::Input { .. } => "input",
            Op::Ax { .. } => "ax",
            Op::AxPress { .. } => "ax.press",
            Op::FramesSubscribe { .. } => "frames.subscribe",
            Op::FramesStop => "frames.stop",
        }
    }

    /// How long the host waits for the reply.
    pub fn timeout(&self) -> Duration {
        let ms = match self {
            Op::Status { .. } | Op::FramesStop => 5_000,
            Op::Screenshot { .. } => 20_000,
            Op::Input { action, .. } => match action {
                // 10 ms per event of up to 20 characters, and the margin.
                Input::Type { text } => 10_000 + 2 * text.chars().count() as u64,
                Input::Key { combos } => 10_000 + 50 * combos.len() as u64,
                _ => 10_000,
            },
            Op::Ax { .. } => 30_000,
            Op::AxPress { .. } => 10_000,
            Op::FramesSubscribe { .. } => 10_000,
        };
        Duration::from_millis(ms)
    }
}

/// A request with its id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// The first line of the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub v: u32,
    /// The guest display in points.
    pub display: Display,
    /// Pixels per point of the guest display.
    pub scale: f64,
    pub screen_recording: bool,
    pub accessibility: bool,
}

/// Why a call failed. The text says it for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The app of the session does not run (app scope).
    NoApp,
    /// The app has no window on the screen (app scope).
    NoWindow,
    /// The point is outside the display, or outside the app's windows.
    Outside,
    /// Another window covers the point (app scope).
    Covered,
    /// The app cannot be brought to the front for keys (app scope).
    NotFront,
    /// The guest screen is locked: no input.
    Locked,
    /// The agent has no Screen Recording permission.
    ScreenRecording,
    /// The agent has no Accessibility permission.
    Accessibility,
    /// The element of an `ax.press` is gone, or has another role now.
    NoElement,
    BadRequest,
    /// The capture failed.
    Capture,
    /// Posting an input event failed.
    Input,
    Timeout,
    /// The agent of the guest does not have this call.
    Unsupported,
    /// The windows of the target changed size since the screenshot that
    /// the coordinates are from (host side only).
    Stale,
    /// The session or its guest cannot be reached (host side only).
    Unreachable,
    /// The host did not allow the request of a sandbox (host side only).
    Denied,
    Internal,
}

impl Code {
    pub fn word(self) -> &'static str {
        match self {
            Code::NoApp => "no_app",
            Code::NoWindow => "no_window",
            Code::Outside => "outside",
            Code::Covered => "covered",
            Code::NotFront => "not_front",
            Code::Locked => "locked",
            Code::ScreenRecording => "screen_recording",
            Code::Accessibility => "accessibility",
            Code::NoElement => "no_element",
            Code::BadRequest => "bad_request",
            Code::Capture => "capture",
            Code::Input => "input",
            Code::Timeout => "timeout",
            Code::Unsupported => "unsupported",
            Code::Stale => "stale",
            Code::Unreachable => "unreachable",
            Code::Denied => "denied",
            Code::Internal => "internal",
        }
    }
}

/// A failed call: the code and the text for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub code: Code,
    pub text: String,
}

impl Failure {
    pub fn new(code: Code, text: impl Into<String>) -> Self {
        Failure {
            code,
            text: text.into().replace(['\n', '\r'], " "),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for Failure {}

/// One window of the target, in a status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub frame: Frame,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub minimized: bool,
}

/// The answer of `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub display: Display,
    pub scale: f64,
    pub screen_recording: bool,
    pub accessibility: bool,
    pub locked: bool,
    /// The bundle id of the front app of the guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_app: Option<String>,
    /// The bundle id of the app of the session, once it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub app_running: bool,
    /// The app is the front app of the guest.
    #[serde(default)]
    pub app_front: bool,
    /// The windows of the target, front first: the app's in app scope,
    /// the app's too in desktop scope.
    #[serde(default)]
    pub windows: Vec<WindowInfo>,
    /// The image of the target at its native size now, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
}

/// What an input did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Done {
    /// The events posted.
    pub events: u32,
    /// The app was brought to the front first (app scope keys).
    #[serde(default)]
    pub activated: bool,
}

/// One accessibility element of a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// `WINDOW:PATH`, the window id and the child indices from the
    /// window down, `4512:0.3.1`. `ax.press` takes it.
    pub id: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subrole: String,
    /// The title, else the description, else the help text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The frame in guest points.
    pub frame: Frame,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    /// 0 for a window, 1 for its children.
    pub depth: u32,
}

/// The id of an element: its window and its path.
pub fn element_id(window: u32, path: &[usize]) -> String {
    let p: Vec<String> = path.iter().map(|i| i.to_string()).collect();
    format!("{window}:{}", p.join(".")) // not user text
}

/// The window and the path of an element id, or None for another text.
pub fn parse_element_id(id: &str) -> Option<(u32, Vec<usize>)> {
    let (w, p) = id.trim().split_once(':')?;
    let window: u32 = w.parse().ok()?;
    if p.is_empty() {
        return Some((window, Vec::new()));
    }
    let path: Option<Vec<usize>> = p.split('.').map(|s| s.parse().ok()).collect();
    Some((window, path?))
}

/// The payload of a successful reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Data {
    Status(Status),
    /// `screenshot`: the geometry of the PNG that follows.
    Shot(Geometry),
    Input(Done),
    /// `ax`: the geometry of the target now, the count of elements in the
    /// JSON that follows, and whether the limits cut the read.
    Ax {
        geometry: Geometry,
        count: u32,
        cut: bool,
    },
    /// `ax.press`: the role and the label of the element pressed.
    Pressed {
        role: String,
        label: String,
    },
    /// `frames.subscribe`: the geometry of the first frame.
    Subscribed(Geometry),
    /// `frames.stop`: the last sequence number sent, 0 for none.
    Stopped {
        last_seq: u64,
    },
}

/// How a call ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Data>,
        /// The bytes after the line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes: Option<u64>,
    },
    Error(Failure),
}

/// One frame of a subscription: the PNG of `bytes` follows the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameHeader {
    /// 1 for the first frame of the subscription, one more for each.
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub geometry: Geometry,
    pub bytes: u64,
}

/// A message of the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum Msg {
    Hello(Hello),
    Reply {
        id: u64,
        #[serde(flatten)]
        outcome: Outcome,
    },
    Frame(FrameHeader),
}

impl Msg {
    /// The bytes that follow the line.
    pub fn bytes(&self) -> u64 {
        match self {
            Msg::Reply {
                outcome: Outcome::Ok { bytes, .. },
                ..
            } => bytes.unwrap_or(0),
            Msg::Frame(f) => f.bytes,
            _ => 0,
        }
    }
}

// ---- The frame stream -----------------------------------------------------

pub use super::computer_frames::{frame_interval, Differ};

#[cfg(test)]
#[path = "computer_tests.rs"]
mod tests;
