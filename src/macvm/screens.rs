//! A change of the host screen set during a session:
//! `NSApplicationDidChangeScreenParametersNotification`. The guest display
//! keeps its size until the next start, so the map between guest points and
//! host points stays the one of the old screen set. A window that is no
//! longer on a screen (less than a quarter of it shows) moves onto the main
//! screen of the new set, and the guest gets its new frame. Pure, with
//! tests on Linux; `run_windows.rs` calls it.

use super::crop::{self, Display, Rect, ScreenSet};
use super::proto::{Frame, Level};

/// One window of the session as the fold knows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub id: u32,
    pub frame: Frame,
    pub level: Level,
    /// A sheet, popover or candidate list moves with its parent.
    pub child: bool,
    pub minimized: bool,
}

/// What a change of the screen set means for the session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rescue {
    /// The windows that move, with their new guest frames.
    pub moves: Vec<(u32, Frame)>,
    /// The badge `display changed, restart for full size`: the union of
    /// the screens or their scale is not the one of the guest display.
    pub badge: bool,
}

/// The windows of `shown` that are off the screens of `new`, moved onto
/// its main screen. `old` is the screen set of the start, which maps
/// guest points to host points for the whole session.
pub fn rescue(shown: &[Shown], old: &ScreenSet, new: &ScreenSet, display: Display) -> Rescue {
    if new.screens.is_empty() {
        // No screen at all (the lid closed with no other screen): wait
        // for the next change.
        return Rescue::default();
    }
    let badge = !old.union().near(&new.union()) || old.main_scale() != new.main_scale();
    let target = inside(new.main(), old.union()).unwrap_or_else(|| old.main());
    let mut moves = Vec::new();
    for w in shown {
        let movable = matches!(w.level, Level::Normal | Level::Floating);
        if w.child || w.minimized || !movable || w.frame.w <= 0.0 || w.frame.h <= 0.0 {
            continue;
        }
        let host = crop::host_frame(w.frame, old);
        if new.visible_area(host) >= host.area() / 4.0 {
            continue;
        }
        let placed = place(host, target);
        let frame = crop::clamp_into(crop::guest_frame(placed, old), display);
        moves.push((w.id, frame));
    }
    Rescue { moves, badge }
}

/// The part of `a` inside `b`, when they share any.
fn inside(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let w = (a.x + a.w).min(b.x + b.w) - x;
    let h = (a.y + a.h).min(b.y + b.h) - y;
    (w > 0.0 && h > 0.0).then(|| Rect::new(x, y, w, h))
}

/// A window of the size of `r` in the middle of `screen`; a window larger
/// than the screen sits at its top left corner, so its title bar shows.
fn place(r: Rect, screen: Rect) -> Rect {
    let x = screen.x + ((screen.w - r.w) / 2.0).max(0.0);
    let top = screen.y + screen.h;
    let y = if r.h >= screen.h {
        top - r.h
    } else {
        screen.y + (screen.h - r.h) / 2.0
    };
    Rect::new(x, y, r.w, r.h)
}

#[cfg(test)]
#[path = "screens_tests.rs"]
mod tests;
