//! The guest layout in the session of `paddock-vm app`, macOS only: the
//! top-level guest windows go apart in the guest (`apart.rs`), so every
//! session window shows its guest window live. A layout move changes the
//! offset of the session window (`place.rs`), never its host frame. Also
//! the `window host-click ID X Y`, `window apart hold` and `window apart
//! go` lines of the control socket in a bench session, for the Mac checks
//! `stacked` and `click`.

use super::apart::{self, Room, Seen};
use super::order::Change;
use super::place::Offset;
use super::proto::{Frame, Level, Request};
use super::run::{Session, Why};
use super::text;
use std::time::Instant;

impl Session<'_> {
    /// Where the layout puts windows: the whole guest display, and the
    /// width of the host screens for the widest window.
    fn apart_room(&self) -> Room {
        let guest = self.shell.windows.guest_display();
        Room {
            w: guest.w,
            h: guest.h,
            top: apart::MENU_BAR,
            screens_w: self.shell.windows.display().w,
        }
    }

    /// The windows of the layout: the top-level `normal` and `floating`
    /// windows that show, neither minimized nor in full screen.
    fn apart_windows(&self) -> Vec<Seen> {
        let o = &self.shell.order;
        o.ids()
            .iter()
            .filter_map(|id| o.get(*id).map(|e| (*id, e)))
            .filter(|(id, e)| {
                e.parent.is_none()
                    && !self.shell.places.is_child(*id)
                    && matches!(e.level, Level::Normal | Level::Floating)
                    && !e.minimized
                    && !e.full_screen
            })
            .map(|(id, e)| Seen {
                id,
                frame: e.frame,
                movable: e.chrome.movable,
                key: e.focused,
            })
            .collect()
    }

    /// Each pass of the session loop: the layout moves that are due go
    /// to the guest.
    pub(super) fn apart_tick(&mut self) {
        // One window alone takes part too: a zoom wider than the screens.
        if self.agent.is_none() || self.shell.order.is_empty() {
            return;
        }
        let windows = self.apart_windows();
        let busy = !self.shell.lead.is_empty();
        let room = self.apart_room();
        let moves = self.shell.apart.tick(&windows, room, Instant::now(), busy);
        for (id, frame) in moves {
            self.events.record(
                "window",
                "moved apart in the guest",
                &[("id", id as f64), ("x", frame.x), ("y", frame.y)],
            );
            if !self.send(Request::WindowMove { id, frame }, Why::Layout(id)) {
                self.shell.apart.unsent(id);
            }
        }
    }

    /// The guest frames of a windows event that answer a layout move: the
    /// offset takes the move, so the host window stays where it is. The
    /// rest of the session then moves only the view.
    pub(super) fn apart_filter(&mut self, changes: &[Change]) {
        let now = Instant::now();
        for c in changes {
            if let Change::Move(id, f) = c {
                if self.shell.apart.layout_move(*id, *f, now) {
                    self.apart_keep_host(*id, *f);
                }
            }
        }
    }

    /// The offset of `id` for its new guest frame `guest` at the host
    /// frame of now.
    fn apart_keep_host(&mut self, id: u32, guest: Frame) {
        let Some(host) = self.shell.windows.host_guest_frame(id) else {
            return;
        };
        self.shell.places.set(id, Offset::between(host, guest));
    }

    /// The reply, the error or the timeout of a layout move. A reply with
    /// the frame moves the view at once, before the windows event.
    pub(super) fn apart_answered(&mut self, id: u32, frame: Option<Frame>) {
        if !self.shell.apart.answered(id, frame, Instant::now()) {
            return;
        }
        let Some(f) = frame else {
            return;
        };
        self.apart_keep_host(id, f);
        self.shell.windows.follow_view(id, f);
        self.update_covers();
    }

    /// The `window host-click ID X Y` line of a bench session: a host
    /// click at point X Y of the session window (points from its top
    /// left, y down). The answer names the guest point under the click.
    /// And `window apart hold` and `window apart go`, which hold the
    /// layout and let it go on. None for another line.
    pub(super) fn apart_control(&mut self, words: &[&str]) -> Option<Result<String, String>> {
        let line = words.join(" ");
        let held = match words {
            ["apart", "hold"] => Some(true),
            ["apart", "go"] => Some(false),
            ["host-click", ..] => None,
            _ => return None,
        };
        if !self.plan.bench {
            return Some(Err(text::control_request_unknown(&line)));
        }
        if let Some(on) = held {
            self.shell.apart.hold(on);
            return Some(Ok(if on { "held" } else { "going" }.into()));
        }
        let ["host-click", id, x, y] = words else {
            return Some(Err(text::control_request_unknown(&line)));
        };
        let num = |s: &str| s.parse::<f64>().ok().filter(|v| v.is_finite());
        let (Ok(id), Some(x), Some(y)) = (id.parse::<u32>(), num(x), num(y)) else {
            return Some(Err(text::control_request_unknown(&line)));
        };
        let Some((gx, gy)) = self.shell.windows.host_click(id, x, y) else {
            return Some(Err(text::no_window(id)));
        };
        let v = serde_json::json!({ "guest": [gx, gy] });
        Some(Ok(v.to_string()))
    }
}
