//! The host-led drag and resize in the session of `paddock-vm app`, macOS
//! only: each host move of a led window passes the rules of `lead.rs`,
//! goes to the guest as `window.move` (one in flight, the latest pending),
//! and the guest's reports move only the view until the guest is where
//! the host is. Also the full screen transitions and the `window
//! host-move` and `window frames` lines of the control socket, for the
//! Mac checks. `run_windows.rs` holds the rest of the windows.

use super::crop::{self, Rect};
use super::lead::{self, Follow};
use super::order::Change;
use super::proto::{Frame, Request};
use super::run::{Session, Why};
use super::text;
use std::time::Instant;

impl Session<'_> {
    /// The host window of `id` moved or resized in a host-led gesture.
    pub(super) fn lead_moved(&mut self, id: u32, host: Rect, held: bool) {
        let guest = crop::guest_frame(host, self.shell.windows.screens());
        self.lead_step(id, guest, held);
    }

    /// One step of a lead to guest frame `guest`. When the user lets go at
    /// a place that the rules refuse, the host window goes to the ruled
    /// frame.
    fn lead_step(&mut self, id: u32, guest: Frame, held: bool) {
        let Some(e) = self.shell.order.get(id) else {
            return;
        };
        let child = e.parent.is_some() || self.shell.windows.is_child(id);
        let ruled = lead::rule(
            e.level,
            child,
            guest,
            self.shell.windows.screens(),
            self.shell.windows.display(),
        );
        if !held && !ruled.near(&guest, crop::EPS) {
            self.shell.windows.set_host(id, ruled);
            self.events.record(
                "window",
                "the host drag ended where the window cannot be: moved back",
                &[("id", id as f64)],
            );
        }
        let sent = self.shell.lead.host_moved(id, ruled, held, Instant::now());
        if !held {
            self.shell.lead.released(id);
        }
        if let Some(f) = sent {
            self.lead_send(id, f);
        }
    }

    /// `window.move` of a lead. Without an agent the move counts as
    /// answered, so the lead ends and the host follows the guest again.
    fn lead_send(&mut self, id: u32, frame: Frame) {
        let mut next = Some(frame);
        while let Some(f) = next {
            if self.send(Request::WindowMove { id, frame: f }, Why::Lead(id)) {
                return;
            }
            next = self.shell.lead.answered(id, Instant::now());
        }
    }

    /// The reply, the error or the timeout of a lead's `window.move`.
    pub(super) fn lead_answered(&mut self, id: u32) {
        if let Some(f) = self.shell.lead.answered(id, Instant::now()) {
            self.lead_send(id, f);
        }
    }

    /// The changes of a guest event, with the moves of led windows turned
    /// into view moves: the host frame stays where the user put it.
    pub(super) fn lead_filter(&mut self, changes: Vec<Change>) -> Vec<Change> {
        if self.shell.lead.is_empty() {
            return changes;
        }
        let now = Instant::now();
        let mut out = Vec::with_capacity(changes.len());
        for c in changes {
            match c {
                Change::Move(id, f) => match self.shell.lead.guest_frame(id, f, now) {
                    Follow::Window => out.push(c),
                    Follow::View => self.shell.windows.follow_view(id, f),
                    Follow::Reached(d) => {
                        self.events.record(
                            "window",
                            "the guest followed the host move",
                            &[("id", id as f64), ("ms", d.as_secs_f64() * 1000.0)],
                        );
                        // At most two points apart: the host takes the
                        // guest's frame exactly.
                        out.push(c);
                    }
                },
                Change::Close(id) | Change::FullScreen(id, _) => {
                    self.shell.lead.remove(id);
                    out.push(c);
                }
                other => out.push(other),
            }
        }
        out
    }

    /// Each pass of the session loop: a window server drag that ended
    /// without a release event, and the leads that the guest did not
    /// follow (its minimum size, a refusal): those host windows go back
    /// to the guest's frame.
    pub(super) fn lead_tick(&mut self) {
        self.shell.windows.drags_ended();
        if self.shell.lead.is_empty() {
            return;
        }
        for id in self.shell.lead.expired(Instant::now()) {
            let Some(frame) = self.shell.order.get(id).map(|e| e.frame) else {
                continue;
            };
            self.shell.windows.apply(&[Change::Move(id, frame)]);
            self.events.record(
                "window",
                "the guest kept the window at its own frame: the host follows",
                &[("id", id as f64)],
            );
        }
    }

    /// A full screen transition of the host window of `id` ended. On the
    /// way back the host window takes the guest's frame, which AppKit
    /// replaced with the frame before the full screen.
    pub(super) fn full_screen_done(&mut self, id: u32, entered: bool) {
        let Some(e) = self.shell.order.get(id) else {
            return;
        };
        let (frame, guest_full) = (e.frame, e.full_screen);
        self.events.record(
            "window",
            if entered {
                "the host window entered full screen"
            } else {
                "the host window left full screen"
            },
            &[("id", id as f64)],
        );
        if !entered && !guest_full {
            self.shell.windows.apply(&[Change::Move(id, frame)]);
        } else if !entered && guest_full {
            // The host left the full screen on its own (Mission Control):
            // the guest window is still in full screen, so the host goes
            // back to it.
            self.shell.windows.apply(&[Change::FullScreen(id, true)]);
        }
    }

    /// The `window host-move ID X Y` and `window frames ID` lines of the
    /// control socket, or None for another line.
    pub(super) fn lead_control(&mut self, words: &[&str]) -> Option<Result<String, String>> {
        let line = words.join(" ");
        let bad = || Err(text::control_request_unknown(&line));
        match words {
            ["host-move", id, x, y] => {
                let (Ok(id), Ok(x), Ok(y)) =
                    (id.parse::<u32>(), x.parse::<f64>(), y.parse::<f64>())
                else {
                    return Some(bad());
                };
                let Some(e) = self.shell.order.get(id) else {
                    return Some(Err(text::no_window(id)));
                };
                let frame = Frame::new(x, y, e.frame.w, e.frame.h);
                // As a drag would: the host window first, then the lead.
                self.shell.windows.set_host(id, frame);
                self.lead_step(id, frame, false);
                Some(Ok("moved".into()))
            }
            ["frames", id] => {
                let Ok(id) = id.parse::<u32>() else {
                    return Some(bad());
                };
                let (Some(e), Some(host)) = (
                    self.shell.order.get(id),
                    self.shell.windows.host_guest_frame(id),
                ) else {
                    return Some(Err(text::no_window(id)));
                };
                let (radius, host_full) = self.shell.windows.clip_of(id).unwrap_or((0.0, false));
                let f = |r: Frame| serde_json::json!([r.x, r.y, r.w, r.h]);
                let v = serde_json::json!({
                    "host": f(host),
                    "guest": f(e.frame),
                    "leading": self.shell.lead.leading(id),
                    "radius": radius,
                    "measured": e.chrome.radius,
                    "title_bar": e.chrome.title_bar,
                    "full_screen": host_full,
                    "guest_full_screen": e.full_screen,
                });
                Some(Ok(v.to_string()))
            }
            _ => None,
        }
    }
}
