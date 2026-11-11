//! The host-led drag and resize in the session of `paddock-vm app`, macOS
//! only, and the covers of overlapping windows.
//!
//! A drag moves the host window only: the window's offset (`place.rs`)
//! takes the move, and the guest gets no `window.move`. A resize sends
//! the new size through the rules of `lead.rs` (one move in flight, the
//! newest pending), and the reply's frame moves the view at once. Each
//! resize writes its timings into the event log (`trace.rs`). Also the
//! full screen transitions, and the `window host-move`, `window
//! host-resize`, `window host-release` and `window frames` lines of the
//! control socket, for the Mac checks. `run_windows.rs` holds the rest
//! of the windows.

use super::cover::{self, Layer};
use super::crop;
use super::lead::{self, Follow};
use super::order::Change;
use super::place::Offset;
use super::proto::{Frame, Level, Request};
use super::run::{Session, Why};
use super::text;
use super::trace::Trace;
use super::windows::LeadStep;
use std::time::Instant;

impl Session<'_> {
    /// One step of a host-led drag or resize.
    pub(super) fn lead_moved(&mut self, step: LeadStep) {
        let host = crop::guest_frame(step.host, self.shell.windows.screens());
        if step.resize {
            self.lead_resize(step.id, host, step.held, step.at);
        } else {
            self.lead_drag(step.id, host, step.held);
        }
    }

    /// The host frame `host` (in guest points) after the rules, when the
    /// user let go: a frame that the rules refuse goes to the ruled one.
    fn lead_rule(&mut self, id: u32, host: Frame) -> Frame {
        let Some(e) = self.shell.order.get(id) else {
            return host;
        };
        let child = e.parent.is_some() || self.shell.places.is_child(id);
        let ruled = lead::rule(
            e.level,
            child,
            host,
            self.shell.windows.screens(),
            self.shell.windows.display(),
        );
        if !ruled.near(&host, crop::EPS) {
            self.shell.windows.set_host(id, ruled);
            self.events.record(
                "window",
                "the host drag ended where the window cannot be: moved back",
                &[("id", id as f64)],
            );
        }
        ruled
    }

    /// A drag step: only the host window moved. Its offset takes the
    /// move, its children go with it, and the guest hears nothing.
    fn lead_drag(&mut self, id: u32, host: Frame, held: bool) {
        let Some(guest) = self.shell.windows.guest_of(id) else {
            return;
        };
        if self.shell.places.is_child(id) {
            // A child keeps the offset of its parent: back to its place.
            if !held {
                let back = self
                    .shell
                    .places
                    .host(id, guest, self.shell.windows.display());
                self.shell.windows.set_host(id, back);
            }
            return;
        }
        let host = if held { host } else { self.lead_rule(id, host) };
        let offset = Offset::between(host, guest);
        self.set_offset(id, offset);
        if !held {
            self.events.record(
                "window",
                "the host drag ended, the guest window stayed",
                &[("id", id as f64), ("dx", offset.dx), ("dy", offset.dy)],
            );
        }
    }

    /// Set the offset of `id`; its children go to their new places and
    /// the covers follow.
    fn set_offset(&mut self, id: u32, offset: Offset) {
        if self.shell.places.set(id, offset) {
            let kids = self.shell.places.anchored(self.shell.places.root(id));
            self.shell.windows.reposition(&kids, &self.shell.places);
            self.update_covers();
        }
    }

    /// A resize step to host frame `host` (in guest points). `at` is the
    /// time of the AppKit event.
    fn lead_resize(&mut self, id: u32, host: Frame, held: bool, at: Instant) {
        let now = Instant::now();
        let Some(e) = self.shell.order.get(id) else {
            return;
        };
        let level = e.level;
        let base = self
            .shell
            .lead
            .target(id)
            .or_else(|| self.shell.windows.guest_of(id))
            .unwrap_or(e.frame);
        self.shell
            .traces
            .entry(id)
            .or_insert_with(|| Trace::new(now))
            .step(now.saturating_duration_since(at));
        let host = if held { host } else { self.lead_rule(id, host) };
        let target = lead::resize_target(
            level,
            base,
            host.w,
            host.h,
            self.shell.windows.screens().main(),
            self.shell.windows.display(),
        );
        self.set_offset(id, Offset::between(host, target));
        let sent = self.shell.lead.host_resized(id, target, held, now);
        if !held {
            if let Some(d) = self.shell.lead.released(id, now) {
                // The guest has this size already.
                let frame = self.shell.windows.guest_of(id).unwrap_or(target);
                self.lead_follow(id, frame, Follow::Reached(d));
                return;
            }
        }
        self.trace_behind(id);
        if let Some(f) = sent {
            self.lead_send(id, f);
        }
    }

    /// `window.move` of a resize. Without an agent the move counts as
    /// answered, so the lead ends and the host settles at the guest's
    /// frame.
    fn lead_send(&mut self, id: u32, frame: Frame) {
        let mut next = Some(frame);
        while let Some(f) = next {
            if self.send(Request::WindowMove { id, frame: f }, Why::Lead(id)) {
                if let Some(t) = self.shell.traces.get_mut(&id) {
                    t.sent(Instant::now());
                }
                return;
            }
            next = self.shell.lead.answered(id, Instant::now());
        }
        if !self.shell.lead.leading(id) {
            self.trace_end(id);
        }
    }

    /// The reply, the error or the timeout of a resize's `window.move`,
    /// with the frame and the agent's time of a reply that has them.
    pub(super) fn lead_answered(&mut self, id: u32, moved: Option<(Frame, f64)>) {
        let now = Instant::now();
        if let Some(t) = self.shell.traces.get_mut(&id) {
            t.replied(now, moved.map(|(_, ms)| ms));
        }
        let next = self.shell.lead.answered(id, now);
        if let Some((frame, _)) = moved {
            let follow = self.shell.lead.reply_frame(id, frame, now);
            self.lead_follow(id, frame, follow);
        }
        if let Some(f) = next {
            self.lead_send(id, f);
        }
    }

    /// What a guest frame of a led window does, from a reply or a window
    /// event. False for [`Follow::Guest`]: the caller moves the host
    /// window as for any guest move.
    fn lead_follow(&mut self, id: u32, frame: Frame, follow: Follow) -> bool {
        let painted = Instant::now();
        match follow {
            Follow::Guest => return false,
            Follow::Stale => return true,
            Follow::View => {
                self.shell.windows.follow_view(id, frame);
                if let Some(host) = self.shell.windows.host_guest_frame(id) {
                    self.set_offset(id, Offset::between(host, frame));
                }
            }
            Follow::Reached(_) | Follow::Settle => {
                self.lead_settle(id, frame);
                if let Follow::Reached(d) = follow {
                    self.events.record(
                        "window",
                        "the guest followed the host resize",
                        &[("id", id as f64), ("ms", d.as_secs_f64() * 1000.0)],
                    );
                }
            }
        }
        if let Some(t) = self.shell.traces.get_mut(&id) {
            t.painted(painted.elapsed());
        }
        self.trace_behind(id);
        if !self.shell.lead.leading(id) {
            self.trace_end(id);
        }
        true
    }

    /// The lead of `id` ended with the guest at `frame`: the host window
    /// keeps its top left corner and takes the guest's size.
    fn lead_settle(&mut self, id: u32, frame: Frame) {
        let Some(host) = self.shell.windows.host_guest_frame(id) else {
            return;
        };
        let settled = Frame::new(host.x, host.y, frame.w, frame.h);
        if !settled.near(&host, crop::EPS) {
            self.shell.windows.set_host(id, settled);
        }
        self.shell.windows.follow_view(id, frame);
        self.set_offset(id, Offset::between(settled, frame));
    }

    /// Whether the cover of `id` shows now, for its trace.
    fn trace_behind(&mut self, id: u32) {
        let behind = self
            .shell
            .windows
            .cover_of(id)
            .is_some_and(|(undrawn, _, _)| undrawn > 0);
        if let Some(t) = self.shell.traces.get_mut(&id) {
            t.behind(behind, Instant::now());
        }
    }

    /// The resize of `id` is over: its timings into the event log.
    fn trace_end(&mut self, id: u32) {
        let Some(mut t) = self.shell.traces.remove(&id) else {
            return;
        };
        let mut fields = vec![("id", id as f64)];
        fields.extend(t.fields(self.shell.lead.dropped(id), Instant::now()));
        self.events
            .record("window", "the host resize ended", &fields);
    }

    /// The changes of a guest event, with the frames of led windows
    /// turned into view moves: the host frame stays where the user put
    /// it.
    pub(super) fn lead_filter(&mut self, changes: Vec<Change>) -> Vec<Change> {
        if self.shell.lead.is_empty() {
            return changes;
        }
        let now = Instant::now();
        let mut out = Vec::with_capacity(changes.len());
        for c in changes {
            match c {
                Change::Move(id, f) => {
                    let follow = self.shell.lead.guest_frame(id, f, now);
                    if !self.lead_follow(id, f, follow) {
                        out.push(c);
                    }
                }
                Change::Close(id) | Change::FullScreen(id, _) => {
                    self.shell.lead.remove(id);
                    self.shell.traces.remove(&id);
                    out.push(c);
                }
                other => out.push(other),
            }
        }
        out
    }

    /// Each pass of the session loop: a window server drag that ended
    /// without a release event, the held clicks that waited long enough,
    /// and the leads that the guest did not follow (its minimum size, a
    /// refusal): those host windows settle at the guest's frame.
    pub(super) fn lead_tick(&mut self) {
        self.shell.windows.drags_ended();
        self.shell.windows.held_tick();
        if self.shell.lead.is_empty() {
            return;
        }
        for id in self.shell.lead.expired(Instant::now()) {
            let Some(frame) = self.shell.windows.guest_of(id) else {
                continue;
            };
            self.lead_settle(id, frame);
            self.trace_end(id);
            self.events.record(
                "window",
                "the guest kept the window at its own frame: the host follows",
                &[("id", id as f64)],
            );
        }
    }

    /// The layers of the guest's order for the covers: front first, each
    /// with its guest frame and offset.
    fn layers(&self) -> Vec<Layer> {
        let o = &self.shell.order;
        o.ids()
            .iter()
            .filter_map(|id| o.get(*id).map(|e| (*id, e)))
            .map(|(id, e)| Layer {
                id,
                frame: self.shell.windows.guest_of(id).unwrap_or(e.frame),
                offset: self.shell.places.offset(id),
                shown: !e.minimized && !e.full_screen && e.level != Level::Hidden,
            })
            .collect()
    }

    /// The covered parts of every window, after any change of the guest
    /// order, a guest frame or an offset.
    pub(super) fn update_covers(&mut self) {
        let layers = self.layers();
        for (id, rects) in cover::covers(&layers) {
            self.shell.windows.set_covers(
                id,
                rects,
                cover::overlaps(&layers, id, false),
                cover::lies_on(&layers, id),
            );
        }
    }

    /// The host window of `id` became key, and the guest raises its window
    /// next: keep the picture of each window that it will lie on in the
    /// guest, while that window still shows its own content.
    pub(super) fn pictures_before_raise(&mut self, id: u32) {
        let now = self.layers();
        let Some(i) = now.iter().position(|l| l.id == id) else {
            return;
        };
        let mut after = now.clone();
        let front = after.remove(i);
        after.insert(0, front);
        for l in &now {
            if l.id == id || !l.shown {
                continue;
            }
            let newly = cover::overlaps(&after, l.id, false);
            if newly.is_empty() || newly == cover::overlaps(&now, l.id, false) {
                continue;
            }
            self.shell.windows.take_picture(l.id);
        }
    }

    /// A press on a covered part of `id` waits: the guest raises the
    /// window, then the press goes on.
    pub(super) fn held_click(&mut self, id: u32) {
        self.events.record(
            "window",
            "a click on a covered part waits for the raise",
            &[("id", id as f64)],
        );
        self.send(Request::WindowRaise { id }, Why::Other);
    }

    /// A full screen transition of the host window of `id` ended. On the
    /// way back the host window takes the guest's frame at its offset,
    /// which AppKit replaced with the frame before the full screen.
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
            self.shell
                .windows
                .apply(&[Change::Move(id, frame)], &self.shell.places);
        } else if !entered && guest_full {
            // The host left the full screen on its own (Mission Control):
            // the guest window is still in full screen, so the host goes
            // back to it.
            self.shell
                .windows
                .apply(&[Change::FullScreen(id, true)], &self.shell.places);
        }
        self.update_covers();
    }

    /// The `window host-move ID X Y`, `window host-resize ID W H`,
    /// `window host-release ID` and `window frames ID` lines of the
    /// control socket, or None for another line.
    pub(super) fn lead_control(&mut self, words: &[&str]) -> Option<Result<String, String>> {
        let line = words.join(" ");
        let bad = || Err(text::control_request_unknown(&line));
        let num = |s: &str| s.parse::<f64>().ok().filter(|v| v.is_finite());
        match words {
            ["host-move", id, x, y] => {
                let (Ok(id), Some(x), Some(y)) = (id.parse::<u32>(), num(x), num(y)) else {
                    return Some(bad());
                };
                let Some(host) = self.shell.windows.host_guest_frame(id) else {
                    return Some(Err(text::no_window(id)));
                };
                let frame = Frame::new(x, y, host.w, host.h);
                // As a drag would: the host window first, then the step.
                self.shell.windows.set_host(id, frame);
                self.lead_drag(id, frame, false);
                Some(Ok("moved".into()))
            }
            ["host-resize", id, w, h] => {
                let (Ok(id), Some(w), Some(h)) = (id.parse::<u32>(), num(w), num(h)) else {
                    return Some(bad());
                };
                let Some(host) = self.shell.windows.host_guest_frame(id) else {
                    return Some(Err(text::no_window(id)));
                };
                // As an edge drag would: the top left corner stays.
                let frame = Frame::new(host.x, host.y, w.max(lead::MIN_W), h.max(lead::MIN_H));
                self.shell.windows.set_host(id, frame);
                self.lead_resize(id, frame, true, Instant::now());
                Some(Ok("resized".into()))
            }
            ["host-release", id] => {
                let Ok(id) = id.parse::<u32>() else {
                    return Some(bad());
                };
                let Some(host) = self.shell.windows.host_guest_frame(id) else {
                    return Some(Err(text::no_window(id)));
                };
                self.lead_resize(id, host, false, Instant::now());
                Some(Ok("released".into()))
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
                let (undrawn, covered, side) =
                    self.shell.windows.cover_of(id).unwrap_or((0, 0, None));
                let offset = self.shell.places.offset(id);
                let f = |r: Frame| serde_json::json!([r.x, r.y, r.w, r.h]);
                let v = serde_json::json!({
                    "host": f(host),
                    "guest": f(e.frame),
                    "view": self.shell.windows.guest_of(id).map(f),
                    "offset": [offset.dx, offset.dy],
                    "moves_sent": self.shell.moves_sent.get(&id).copied().unwrap_or(0),
                    "leading": self.shell.lead.leading(id),
                    "undrawn": undrawn,
                    "covered": covered,
                    "edge": side,
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
