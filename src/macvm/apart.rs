//! The guest layout of a session's windows: no top-level guest window
//! covers another one, so every session window shows its guest window
//! live. Pure, with tests on Linux. `run_apart.rs` runs it in the session.
//!
//! A session window shows its rectangle of the one guest display. When
//! two guest windows overlap there (an app that opens its windows at one
//! frame, a window that grew), the one below shows only its last picture.
//! So the host moves guest windows apart with `window.move`, the move of
//! Accessibility in the guest:
//!
//! - A window that opened, moved or changed its size and then kept its
//!   frame for [`SETTLE`] is fresh. The windows that did not change keep
//!   their places; a fresh window that overlaps one of them goes to the
//!   free place nearest to where it is ([`plan`]), [`GAP`] points from
//!   the others when there is room for that.
//! - Only top-level windows take part. A menu, popover, sheet, tooltip or
//!   input method candidate list stays with its parent, a window in full
//!   screen or minimized is left out, and a window that the guest does not
//!   let move stays where it is (the others go around it).
//! - The guest display has room to the right of the host screens
//!   (`crop::room_display`). A window larger than the host screens gets
//!   their size ([`fit`]): the guest's zoom fills the whole guest display.
//! - When a window does not fit anywhere, it may cover the windows that
//!   had the focus least recently. It never covers the key window, which
//!   the guest keeps in front.
//! - A move of this layout changes the offset of the session window
//!   (`place.rs`), not its host frame ([`Apart::layout_move`]). An app's
//!   own move still moves the host window by the same amount.
//! - Each window gets at most [`TRIES`] moves per change of its own, so a
//!   guest that keeps a window where it is ends the layout.

use super::crop::EPS;
use super::proto::Frame;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How long a changed frame has to stay before the layout moves it: an
/// app that opens a window and then places it is done by then.
pub const SETTLE: Duration = Duration::from_millis(150);

/// How long a layout move waits for the guest's frame. After it the next
/// layout may run.
pub const PENDING: Duration = Duration::from_millis(1500);

/// The space between two windows that the layout puts apart, in points:
/// the shadow of a guest window stays off its neighbor when there is room.
pub const GAP: f64 = 24.0;

/// The top of the area where the layout puts a window, in guest points:
/// below the guest's menu bar.
pub const MENU_BAR: f64 = 25.0;

/// The layout moves of one window per change of its own.
pub const TRIES: u8 = 2;

/// The most windows that the layout places. More go as they are.
pub const MOST: usize = 64;

/// One top-level guest window for [`plan`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub id: u32,
    /// The guest frame.
    pub frame: Frame,
    /// The layout may move the window. Else it stays, and the others go
    /// around it.
    pub movable: bool,
    /// The window opened or changed since the last layout.
    pub fresh: bool,
    /// The guest's key window.
    pub key: bool,
    /// When the window last had the focus: larger is more recent.
    pub recent: u64,
}

/// Where the layout puts windows, in guest points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// The width of the guest display, with its room right of the host
    /// screens.
    pub w: f64,
    pub h: f64,
    /// The top of the area: below the guest's menu bar.
    pub top: f64,
    /// The width of the host screens: the widest that a window gets.
    pub screens_w: f64,
}

/// The frame of a window larger than the host screens: their width, and
/// the height of the room, at the same top left corner.
pub fn fit(frame: Frame, room: Room) -> Frame {
    let w = frame.w.min(room.screens_w.max(1.0));
    let h = frame.h.min((room.h - room.top).max(1.0));
    Frame::new(frame.x, frame.y, w, h)
}

/// True when the two frames share an area wider and higher than [`EPS`].
pub fn overlap(a: Frame, b: Frame) -> bool {
    a.x < b.x + b.w - EPS && b.x < a.x + a.w - EPS && a.y < b.y + b.h - EPS && b.y < a.y + a.h - EPS
}

/// `f` grown by `gap` on every side.
fn grown(f: Frame, gap: f64) -> Frame {
    Frame::new(f.x - gap, f.y - gap, f.w + 2.0 * gap, f.h + 2.0 * gap)
}

/// The free place for a window of frame `frame` nearest to its top left
/// corner: inside the room, below the menu bar, and `gap` points from
/// every frame of `others`. None when there is no such place.
pub fn free_place(frame: Frame, others: &[Frame], room: Room, gap: f64) -> Option<Frame> {
    free_near(frame, (frame.x, frame.y), others, room, gap)
}

/// [`free_place`] nearest to the point `near` instead.
fn free_near(
    frame: Frame,
    near: (f64, f64),
    others: &[Frame],
    room: Room,
    gap: f64,
) -> Option<Frame> {
    let (w, h) = (frame.w, frame.h);
    let mut xs = vec![frame.x, 0.0, room.w - w];
    let mut ys = vec![frame.y, room.top, room.h - h];
    for o in others {
        xs.extend([o.x + o.w + gap, o.x - w - gap, o.x]);
        ys.extend([o.y + o.h + gap, o.y - h - gap, o.y]);
    }
    let inside = |x: f64, y: f64| {
        x >= -EPS && y >= room.top - EPS && x + w <= room.w + EPS && y + h <= room.h + EPS
    };
    let mut best: Option<(f64, Frame)> = None;
    for &y in &ys {
        for &x in &xs {
            if !inside(x, y) {
                continue;
            }
            let f = Frame::new(x, y, w, h);
            if others.iter().any(|o| overlap(f, grown(*o, gap))) {
                continue;
            }
            let d = (x - near.0).hypot(y - near.1);
            // The nearest; at the same distance the higher, then the left.
            let better = match best {
                None => true,
                Some((bd, bf)) => d < bd - EPS || (d < bd + EPS && (y, x) < (bf.y, bf.x)),
            };
            if better {
                best = Some((d, f));
            }
        }
    }
    best.map(|(_, f)| f)
}

/// The moves that put the windows apart: `(id, frame)` for each window
/// that goes to a new frame. The windows that did not change come first
/// and keep their places, the key window first among each, then the
/// others by how recently they had the focus; a fresh window goes to the
/// free place nearest to it. When that leaves a window without a free
/// place, the windows are packed again from the top left corner of the
/// room instead, the key window first.
pub fn plan(items: &[Item], room: Room) -> Vec<(u32, Frame)> {
    let mut order: Vec<&Item> = items.iter().take(MOST).collect();
    order.sort_by_key(|i| (i.fresh, !i.key, std::cmp::Reverse(i.recent), i.id));
    if let Some(moves) = pass(&order, room, false) {
        return moves;
    }
    order.sort_by_key(|i| (!i.key, std::cmp::Reverse(i.recent), i.id));
    pass(&order, room, true).unwrap_or_default()
}

/// One pass of [`plan`] over the windows in `order`. Without `packed` a
/// window that has its own place keeps it, else every movable window
/// goes to the free place nearest to the top left corner of the room.
/// None when a pass without `packed` leaves a window without a free
/// place; a packed pass then lets the windows that had the focus least
/// recently be covered.
fn pass(order: &[&Item], room: Room, packed: bool) -> Option<Vec<(u32, Frame)>> {
    // The frames that a later window keeps clear of, with the key flag and
    // the recency of their window.
    let mut placed: Vec<(Frame, bool, u64)> = Vec::new();
    for it in order.iter().filter(|i| !i.movable) {
        placed.push((it.frame, it.key, it.recent));
    }
    let mut moves = Vec::new();
    for it in order.iter().filter(|i| i.movable) {
        let frame = fit(it.frame, room);
        let clear = !placed.iter().any(|(p, _, _)| overlap(frame, *p));
        let to = if clear && !packed {
            frame
        } else {
            let all: Vec<Frame> = placed.iter().map(|p| p.0).collect();
            let near = if packed {
                (0.0, room.top)
            } else {
                (frame.x, frame.y)
            };
            let free = free_near(frame, near, &all, room, GAP)
                .or_else(|| free_near(frame, near, &all, room, 0.0));
            match free {
                Some(f) => f,
                None if !packed => return None,
                None => cover_least_recent(frame, &placed, room).unwrap_or(frame),
            }
        };
        if !to.near(&it.frame, EPS) {
            moves.push((it.id, to));
        }
        placed.push((to, it.key, it.recent));
    }
    Some(moves)
}

/// A place for `frame` clear of all but the windows of `placed` that had
/// the focus least recently, never of the key window.
fn cover_least_recent(frame: Frame, placed: &[(Frame, bool, u64)], room: Room) -> Option<Frame> {
    // Most recent first, the key window before all.
    let mut ranked: Vec<&(Frame, bool, u64)> = placed.iter().collect();
    ranked.sort_by_key(|p| (!p.1, std::cmp::Reverse(p.2)));
    for keep in (1..ranked.len()).rev() {
        let kept: Vec<Frame> = ranked[..keep].iter().map(|p| p.0).collect();
        if let Some(f) = free_place(frame, &kept, room, 0.0) {
            return Some(f);
        }
    }
    None
}

/// A layout move that the guest has not answered yet.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pending {
    target: Frame,
    sent: Instant,
    /// The move gave the window another size ([`fit`]): the host window
    /// follows it as it follows an app's own move.
    fit: bool,
}

/// One window for [`Apart::tick`]: an [`Item`] without the fresh flag and
/// the recency, which the state keeps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seen {
    pub id: u32,
    pub frame: Frame,
    pub movable: bool,
    pub key: bool,
}

/// The state of the layout in a session.
#[derive(Debug, Clone, Default)]
pub struct Apart {
    /// The last frame of each window and when it came.
    seen: BTreeMap<u32, (Frame, Instant)>,
    /// The windows that changed since the last layout.
    fresh: BTreeSet<u32>,
    /// The focus clock: the last value of `recent`.
    clock: u64,
    recent: BTreeMap<u32, u64>,
    pending: BTreeMap<u32, Pending>,
    /// The layout moves of each window since its last own change.
    tries: BTreeMap<u32, u8>,
    /// `window apart hold` of a bench session: no layout until `window
    /// apart go`, so a Mac check can stack windows first.
    held: bool,
}

impl Apart {
    /// A guest frame of `id` from a windows event: true when it is the
    /// answer to a layout move, so the host window stays where it is. A
    /// fit move is no such answer: the host window follows it.
    pub fn layout_move(&mut self, id: u32, frame: Frame, now: Instant) -> bool {
        let Some(p) = self.pending.get(&id).copied() else {
            return false;
        };
        if now.saturating_duration_since(p.sent) > PENDING {
            self.pending.remove(&id);
            return false;
        }
        // The layout never changes the size of a window, unless it fits it.
        let same_size = (frame.w - p.target.w).abs() < EPS && (frame.h - p.target.h).abs() < EPS;
        if !same_size {
            return false;
        }
        self.pending.remove(&id);
        self.seen.insert(id, (frame, now));
        !p.fit
    }

    /// The guest's reply to the layout move of `id`: the frame it read
    /// back, or None for an error or a timeout. True when the host window
    /// stays and only the view follows `frame`.
    pub fn answered(&mut self, id: u32, frame: Option<Frame>, now: Instant) -> bool {
        match frame {
            Some(f) => self.layout_move(id, f, now),
            None => {
                self.pending.remove(&id);
                false
            }
        }
    }

    /// One pass of the session loop with the top-level windows of now:
    /// the moves to send, after the frames settled, with no lead (`busy`)
    /// and no layout move in flight.
    pub fn tick(
        &mut self,
        windows: &[Seen],
        room: Room,
        now: Instant,
        busy: bool,
    ) -> Vec<(u32, Frame)> {
        self.pending
            .retain(|_, p| now.saturating_duration_since(p.sent) <= PENDING);
        let ids: BTreeSet<u32> = windows.iter().map(|w| w.id).collect();
        self.seen.retain(|id, _| ids.contains(id));
        self.fresh.retain(|id| ids.contains(id));
        self.recent.retain(|id, _| ids.contains(id));
        self.tries.retain(|id, _| ids.contains(id));
        for w in windows {
            let same = self
                .seen
                .get(&w.id)
                .is_some_and(|(f, _)| f.near(&w.frame, EPS));
            if !same {
                self.seen.insert(w.id, (w.frame, now));
                self.fresh.insert(w.id);
                self.tries.remove(&w.id);
            }
            if w.key && self.recent.get(&w.id) != Some(&self.clock) {
                self.clock += 1;
                self.recent.insert(w.id, self.clock);
            }
        }
        if busy || self.held || !self.pending.is_empty() || self.fresh.is_empty() {
            return Vec::new();
        }
        let settled = self.fresh.iter().all(|id| {
            self.seen
                .get(id)
                .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= SETTLE)
        });
        if !settled {
            return Vec::new();
        }
        let items: Vec<Item> = windows
            .iter()
            .map(|w| Item {
                id: w.id,
                frame: w.frame,
                movable: w.movable && self.tries.get(&w.id).copied().unwrap_or(0) < TRIES,
                fresh: self.fresh.contains(&w.id),
                key: w.key,
                recent: self.recent.get(&w.id).copied().unwrap_or(0),
            })
            .collect();
        self.fresh.clear();
        let moves = plan(&items, room);
        for (id, target) in &moves {
            *self.tries.entry(*id).or_default() += 1;
            let old = windows.iter().find(|w| w.id == *id).map(|w| w.frame);
            let fit =
                old.is_some_and(|o| (o.w - target.w).abs() >= EPS || (o.h - target.h).abs() >= EPS);
            self.pending.insert(
                *id,
                Pending {
                    target: *target,
                    sent: now,
                    fit,
                },
            );
        }
        moves
    }

    /// Hold the layout, or let it go on (`window apart hold` and `window
    /// apart go` of a bench session).
    pub fn hold(&mut self, on: bool) {
        self.held = on;
    }

    /// The move of `id` could not go out: no layout move waits for it.
    pub fn unsent(&mut self, id: u32) {
        self.pending.remove(&id);
    }

    /// A layout move of `id` waits for the guest.
    pub fn waits(&self, id: u32) -> bool {
        self.pending.contains_key(&id)
    }

    /// The windows went (a quit, a guest restart, a new hello).
    pub fn clear(&mut self) {
        *self = Apart::default();
    }
}

#[cfg(test)]
#[path = "apart_tests.rs"]
mod tests;
