//! Where each host window of a session is: the guest frame of its window
//! plus an offset of its own. Pure, with tests on Linux.
//!
//! The guest window stays where the guest put it. When the person drags
//! a host window, only the host window moves, and its offset (the host
//! origin minus the guest origin, in guest points) changes. The view
//! inside the host window shows the guest window at its guest position,
//! so a click or a key goes to the right place without any map.
//!
//! - A new top-level window opens where the guest put it: offset zero.
//! - A child window (a menu, a popover, a sheet, a tooltip, an input
//!   method candidate list) takes the offset of its anchor: the parent
//!   that the guest names, else the top-level window under its top left
//!   corner ([`anchor_of`]).
//! - A move of the guest (the app moves its window, or the guest zooms
//!   it) moves the host window by the same delta: the offset stays.
//! - The screen gap and display rules of `crop.rs` apply to the host
//!   frame. When a rule moves a host frame, only the offset changes, and
//!   the guest gets no `window.move`.

use super::crop::{self, Display, ScreenSet, EPS};
use super::order::{Change, Windows};
use super::proto::{Frame, Level};
use std::collections::BTreeMap;

/// The host origin minus the guest origin of a window, in guest points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Offset {
    pub dx: f64,
    pub dy: f64,
}

impl Offset {
    pub const ZERO: Offset = Offset { dx: 0.0, dy: 0.0 };

    /// The offset that puts guest frame `guest` at host frame `host`
    /// (both in guest points).
    pub fn between(host: Frame, guest: Frame) -> Offset {
        Offset {
            dx: host.x - guest.x,
            dy: host.y - guest.y,
        }
    }

    /// The host frame (in guest points) of guest frame `guest`.
    pub fn apply(self, guest: Frame) -> Frame {
        Frame::new(guest.x + self.dx, guest.y + self.dy, guest.w, guest.h)
    }

    /// True when both parts are within [`EPS`] of the other offset.
    pub fn near(self, o: Offset) -> bool {
        (self.dx - o.dx).abs() < EPS && (self.dy - o.dy).abs() < EPS
    }
}

/// The offset of a top-level window after the rules: the host frame of
/// `guest` at `offset` goes out of the gap between two screens and into
/// the guest display, and the offset follows the host frame.
pub fn ruled(guest: Frame, offset: Offset, screens: &ScreenSet, display: Display) -> Offset {
    let host = offset.apply(guest);
    let f = crop::clamp_into(crop::gap_clamp_guest(host, screens), display);
    if f.near(&host, EPS) {
        offset
    } else {
        Offset::between(f, guest)
    }
}

/// One window that a new child may anchor to: a top-level window of the
/// session, front first in [`anchor_of`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub id: u32,
    pub pid: u32,
    pub frame: Frame,
    pub minimized: bool,
    /// The guest's key window.
    pub focused: bool,
}

/// The kinds of window that belong to another window even when the guest
/// names no parent.
pub fn is_attached(level: Level) -> bool {
    matches!(
        level,
        Level::Menu | Level::Popover | Level::Tooltip | Level::Sheet
    )
}

/// The window whose offset a new window takes, or None for a top-level
/// window. The parent that the guest names comes first. A menu, popover,
/// tooltip or sheet without one takes the front-most top-level window of
/// its process that holds its top left corner, then one of any process,
/// then the guest's key window, then the front-most window: a context
/// menu opens at the click, inside the window that got it.
pub fn anchor_of(
    id: u32,
    pid: u32,
    level: Level,
    parent: Option<u32>,
    frame: Frame,
    tops: &[Candidate],
) -> Option<u32> {
    if let Some(p) = parent.filter(|p| *p != id) {
        return Some(p);
    }
    if !is_attached(level) {
        return None;
    }
    let shown = || tops.iter().filter(|c| c.id != id && !c.minimized);
    let holds = |c: &&Candidate| {
        frame.x >= c.frame.x - EPS
            && frame.y >= c.frame.y - EPS
            && frame.x <= c.frame.x + c.frame.w + EPS
            && frame.y <= c.frame.y + c.frame.h + EPS
    };
    shown()
        .filter(|c| c.pid == pid)
        .find(holds)
        .or_else(|| shown().find(holds))
        .or_else(|| shown().find(|c| c.focused))
        .or_else(|| shown().next())
        .map(|c| c.id)
}

/// The top-level windows of the fold, front first, for [`anchor_of`].
pub fn candidates(order: &Windows) -> Vec<Candidate> {
    order
        .ids()
        .iter()
        .filter_map(|id| order.get(*id).map(|e| (*id, e)))
        .filter(|(_, e)| e.parent.is_none() && !is_attached(e.level) && e.level != Level::Hidden)
        .map(|(id, e)| Candidate {
            id,
            pid: e.pid,
            frame: e.frame,
            minimized: e.minimized,
            focused: e.focused,
        })
        .collect()
}

/// The place of one window.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    /// The offset of a top-level window. A child keeps the last offset
    /// of its anchor here, for the time after the anchor closed.
    offset: Offset,
    anchor: Option<u32>,
}

/// The longest chain of anchors that [`Places::root`] follows: a cycle in
/// the guest's report ends there.
const DEPTH: usize = 8;

/// The places of the session's windows, by guest window id.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Places {
    map: BTreeMap<u32, Place>,
}

impl Places {
    /// A new window: a top-level one (`anchor` None) at `offset`, a
    /// child at the offset of its anchor.
    pub fn open(&mut self, id: u32, anchor: Option<u32>, offset: Offset) {
        let anchor = anchor.filter(|a| *a != id && self.map.contains_key(a));
        let offset = match anchor {
            Some(a) => self.offset(a),
            None => offset,
        };
        self.map.insert(id, Place { offset, anchor });
    }

    /// The window went. A child of it keeps the offset it had, as a
    /// top-level window of its own.
    pub fn close(&mut self, id: u32) {
        let gone = self.offset(id);
        self.map.remove(&id);
        for p in self.map.values_mut() {
            if p.anchor == Some(id) {
                p.anchor = None;
                p.offset = gone;
            }
        }
    }

    pub fn contains(&self, id: u32) -> bool {
        self.map.contains_key(&id)
    }

    /// True for a window that takes the offset of another one.
    pub fn is_child(&self, id: u32) -> bool {
        self.map.get(&id).is_some_and(|p| p.anchor.is_some())
    }

    /// The top-level window at the end of the anchors of `id`.
    pub fn root(&self, id: u32) -> u32 {
        let mut at = id;
        for _ in 0..DEPTH {
            match self.map.get(&at).and_then(|p| p.anchor) {
                Some(a) if self.map.contains_key(&a) => at = a,
                _ => break,
            }
        }
        at
    }

    /// The offset of `id`: the one of its root. Zero for a window that
    /// the map does not know.
    pub fn offset(&self, id: u32) -> Offset {
        let root = self.root(id);
        self.map
            .get(&root)
            .or_else(|| self.map.get(&id))
            .map_or(Offset::ZERO, |p| p.offset)
    }

    /// Set the offset of `id`, on its root: every window anchored to it
    /// moves with it. True when the offset changed.
    pub fn set(&mut self, id: u32, offset: Offset) -> bool {
        let root = self.root(id);
        let Some(p) = self.map.get_mut(&root) else {
            return false;
        };
        let changed = !p.offset.near(offset);
        p.offset = offset;
        changed
    }

    /// The host frame of `id` (in guest points) for its guest frame
    /// `guest`. A child goes into the guest display as well: a menu of a
    /// window that the person put near a screen edge stays on the screen.
    pub fn host(&self, id: u32, guest: Frame, display: Display) -> Frame {
        let f = self.offset(id).apply(guest);
        if self.is_child(id) {
            crop::clamp_into(f, display)
        } else {
            f
        }
    }

    /// Every window whose root is `id`, `id` not included.
    pub fn anchored(&self, id: u32) -> Vec<u32> {
        self.map
            .keys()
            .copied()
            .filter(|w| *w != id && self.root(*w) == id)
            .collect()
    }

    /// The changes of one guest event, after the fold: a new window gets
    /// its anchor or its ruled offset, a closed one goes, and a guest
    /// move of a top-level window keeps the offset unless a rule moves
    /// the host frame. The ids of the windows that go to a new place
    /// besides the `Open` and `Move` changes themselves: the windows
    /// anchored to a window whose offset a rule changed.
    pub fn follow(
        &mut self,
        changes: &[Change],
        order: &Windows,
        screens: &ScreenSet,
        display: Display,
    ) -> Vec<u32> {
        let mut again = Vec::new();
        for c in changes {
            match c {
                Change::Open(w) => {
                    let tops = candidates(order);
                    let anchor = anchor_of(w.id, w.pid, w.level, w.parent, w.frame, &tops);
                    let offset = if anchor.is_none() && !super::order::full_screen_of(w) {
                        ruled(w.frame, Offset::ZERO, screens, display)
                    } else {
                        Offset::ZERO
                    };
                    self.open(w.id, anchor, offset);
                }
                Change::Close(id) => self.close(*id),
                Change::Move(id, guest) => {
                    if self.is_child(*id) || order.get(*id).is_some_and(|e| e.full_screen) {
                        continue;
                    }
                    let new = ruled(*guest, self.offset(*id), screens, display);
                    if self.set(*id, new) {
                        again.extend(self.anchored(*id));
                    }
                }
                _ => {}
            }
        }
        again.sort_unstable();
        again.dedup();
        again
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
#[path = "place_tests.rs"]
mod tests;
