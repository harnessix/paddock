//! Host-led drag and resize of the session windows, pure, with tests on
//! Linux. `windows_lead.rs` runs it on AppKit, `run_lead.rs` in the
//! session.
//!
//! A press in the title bar of a forwarded window, away from its buttons,
//! starts a drag of the host window after the mouse moves [`DRAG_SLOP`]
//! points: the window server moves the host window at once, and the view
//! keeps its offset from the guest window's last known frame, so the
//! content moves with the frame. A press on an edge starts a host resize.
//! A click without a move, a double click (the guest's zoom) and a press
//! on a button go to the guest as before.
//!
//! The guest follows with `window.move`: one in flight per window, plus
//! the latest frame pending ([`Leads`]). Until the guest reports the frame
//! that the host asked for, its reports move only the view, never the
//! host window, so the host frame does not jump back. Each frame that the
//! host asks for passes the screen gap, size and display rules of
//! `crop.rs` first ([`rule`]).

use super::crop::{self, Display, Rect, ScreenSet};
use super::proto::{Frame, Level};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The mouse moves this far (points) before a title bar press is a drag.
pub const DRAG_SLOP: f64 = 3.0;
/// The width of the resize grip inside each edge, in points.
pub const EDGE: f64 = 5.0;
/// The smallest size of a host resize, in points: the guest refuses
/// smaller sizes of its own, and the host follows its answer.
pub const MIN_W: f64 = 80.0;
pub const MIN_H: f64 = 40.0;
/// A guest frame within this many points of the host's is the same place.
pub const SAME: f64 = 2.0;
/// After the last reply, a guest frame that is not the host's is taken
/// as the guest's answer only after this time: the window server list of
/// the guest can lag one poll behind the move.
pub const GRACE: Duration = Duration::from_millis(300);

/// The edges that a resize moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub fn any(&self) -> bool {
        self.left || self.right || self.top || self.bottom
    }
}

/// What a left press on a session window starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// The guest gets the press, as before.
    Pass,
    /// A host drag once the mouse moves; a click without a move goes to
    /// the guest.
    Drag,
    /// A host resize.
    Resize(Edges),
}

/// What the host knows of a window's chrome, for the hit test.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chrome {
    /// The title bar height in points.
    pub title_bar: f64,
    /// The buttons in the title bar, relative to the top left.
    pub controls: Vec<Frame>,
    pub movable: bool,
    pub resizable: bool,
    /// The corner radius that the agent measured, in points.
    pub radius: Option<f64>,
}

/// The press at point `(x, y)` of a window of size `(w, h)`, both in
/// points from the top left of the window, y down. `clicks` is the click
/// count of the press. Only `normal` windows lead: a menu, a popover or a
/// sheet gets every press.
pub fn press(
    level: Level,
    chrome: &Chrome,
    w: f64,
    h: f64,
    x: f64,
    y: f64,
    clicks: isize,
) -> Press {
    if level != Level::Normal || x < 0.0 || y < 0.0 || x > w || y > h {
        return Press::Pass;
    }
    let on_control = chrome
        .controls
        .iter()
        .any(|c| x >= c.x && x <= c.x + c.w && y >= c.y && y <= c.y + c.h);
    if on_control {
        return Press::Pass;
    }
    if chrome.resizable && clicks < 2 {
        let edges = Edges {
            left: x < EDGE,
            right: x > w - EDGE,
            top: y < EDGE,
            bottom: y > h - EDGE,
        };
        if edges.any() {
            return Press::Resize(edges);
        }
    }
    if chrome.movable && clicks < 2 && y <= chrome.title_bar {
        return Press::Drag;
    }
    Press::Pass
}

/// True when the mouse moved far enough from the press for a drag.
pub fn past_slop(start: (f64, f64), now: (f64, f64)) -> bool {
    let (dx, dy) = (now.0 - start.0, now.1 - start.1);
    dx * dx + dy * dy >= DRAG_SLOP * DRAG_SLOP
}

/// The host frame of a resize: `start` (host points, y up) with the edges
/// moved by the mouse delta `(dx, dy)` (host points, y up), at least
/// [`MIN_W`] by [`MIN_H`]. The opposite edge stays where it was.
pub fn resized(start: Rect, edges: Edges, dx: f64, dy: f64) -> Rect {
    let mut r = start;
    if edges.right {
        r.w = (start.w + dx).max(MIN_W);
    }
    if edges.left {
        r.w = (start.w - dx).max(MIN_W);
        r.x = start.x + start.w - r.w;
    }
    if edges.top {
        r.h = (start.h + dy).max(MIN_H);
    }
    if edges.bottom {
        r.h = (start.h - dy).max(MIN_H);
        r.y = start.y + start.h - r.h;
    }
    r
}

/// The frame that the guest may get for a host frame that the user made:
/// a top-level window out of the gap between two screens, a window above
/// `normal` within its size, and every window inside the guest display.
/// The host window goes to this frame when the drag ends.
pub fn rule(
    level: Level,
    child: bool,
    guest: Frame,
    screens: &ScreenSet,
    display: Display,
) -> Frame {
    let mut f = crop::fit_to_limit(level, guest, screens.main());
    if !child {
        f = crop::gap_clamp_guest(f, screens);
    }
    crop::clamp_into(f, display)
}

/// One led window.
#[derive(Debug, Clone, PartialEq)]
struct Lead {
    /// The last frame the host asked for, in guest points.
    target: Frame,
    /// A `window.move` without its reply yet.
    in_flight: Option<Frame>,
    /// The newest frame not sent yet.
    pending: Option<Frame>,
    /// The user still holds the window.
    held: bool,
    /// When the host moved to `target`, for the latency line.
    moved_at: Instant,
    /// When the last reply came.
    answered_at: Option<Instant>,
}

impl Lead {
    fn busy(&self) -> bool {
        self.held || self.in_flight.is_some() || self.pending.is_some()
    }
}

/// What the host does with a guest frame of a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Follow {
    /// No lead: the host window and the view follow, as before.
    Window,
    /// A lead runs: the view follows, the host frame stays.
    View,
    /// The guest reached the host's frame and the lead ended: the view
    /// follows, the host frame stays. The time from the host move.
    Reached(Duration),
}

/// The led windows of the session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Leads {
    map: BTreeMap<u32, Lead>,
}

impl Leads {
    /// True while the host leads the window of `id`.
    pub fn leading(&self, id: u32) -> bool {
        self.map.contains_key(&id)
    }

    /// The frame the host asked for, while it leads.
    pub fn target(&self, id: u32) -> Option<Frame> {
        self.map.get(&id).map(|l| l.target)
    }

    /// The host window of `id` moved or resized to guest frame `frame`
    /// (after [`rule`]). `held` while the user holds the window. Some
    /// frame to send now in `window.move`; None when one is in flight,
    /// then the frame waits as the latest pending one.
    pub fn host_moved(&mut self, id: u32, frame: Frame, held: bool, now: Instant) -> Option<Frame> {
        let lead = self.map.entry(id).or_insert(Lead {
            target: frame,
            in_flight: None,
            pending: None,
            held,
            moved_at: now,
            answered_at: None,
        });
        lead.held = held;
        if lead.in_flight.is_some() {
            if !lead.target.near(&frame, crop::EPS) {
                lead.target = frame;
                lead.moved_at = now;
                lead.pending = Some(frame);
            }
            return None;
        }
        lead.target = frame;
        lead.moved_at = now;
        lead.pending = None;
        lead.in_flight = Some(frame);
        Some(frame)
    }

    /// The user let go of the window of `id`.
    pub fn released(&mut self, id: u32) {
        if let Some(l) = self.map.get_mut(&id) {
            l.held = false;
        }
    }

    /// The reply, an error or the timeout of the `window.move` in flight
    /// for `id`: Some pending frame to send now. A failed move counts as
    /// answered: the guest's next report, or [`Leads::expired`], puts the
    /// host window where the guest window is.
    pub fn answered(&mut self, id: u32, now: Instant) -> Option<Frame> {
        let l = self.map.get_mut(&id)?;
        l.in_flight = l.pending.take();
        l.answered_at = Some(now);
        l.in_flight
    }

    /// A guest frame of `id` arrived (`windows` event).
    pub fn guest_frame(&mut self, id: u32, frame: Frame, now: Instant) -> Follow {
        let Some(l) = self.map.get(&id) else {
            return Follow::Window;
        };
        if l.busy() {
            return Follow::View;
        }
        if frame.near(&l.target, SAME) {
            let since = now.saturating_duration_since(l.moved_at);
            self.map.remove(&id);
            return Follow::Reached(since);
        }
        if l.answered_at
            .is_some_and(|t| now.saturating_duration_since(t) < GRACE)
        {
            return Follow::View;
        }
        // The guest put the window elsewhere (its minimum size, a
        // refusal): the host follows the guest.
        self.map.remove(&id);
        Follow::Window
    }

    /// The leads that ended without the guest at the host's frame: no
    /// move in flight, the user let go, and no matching report within
    /// [`GRACE`] of the last reply. The host window of each goes back to
    /// the guest's frame.
    pub fn expired(&mut self, now: Instant) -> Vec<u32> {
        let gone: Vec<u32> = self
            .map
            .iter()
            .filter(|(_, l)| {
                !l.busy()
                    && l.answered_at
                        .is_some_and(|t| now.saturating_duration_since(t) >= GRACE)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &gone {
            self.map.remove(id);
        }
        gone
    }

    /// The window went away, or the agent restarted: no lead.
    pub fn remove(&mut self, id: u32) {
        self.map.remove(&id);
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
#[path = "lead_tests.rs"]
mod tests;
