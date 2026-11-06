//! The host-led drag and resize of the session windows on AppKit, the
//! view offset, the chrome and the full screen, macOS only. `lead.rs`
//! decides; this file runs it on the `SessionWindow`s of `windows.rs`.
//!
//! A press in the title bar waits for a move of [`lead::DRAG_SLOP`]
//! points: then `performWindowDragWithEvent:` moves the host window in
//! the window server, and every `windowDidMove:` while the drag runs is
//! a [`HostEvent::LeadMoved`]. A release without a move replays the press
//! and the release to the guest. An edge press resizes the host window
//! with each drag event. The view keeps its offset from the guest
//! window's last known frame ([`place_view`]), so the content moves with
//! the host frame until the guest reports its new frame.

use super::crop::{self, Rect};
use super::lead::{self, Chrome, Edges, Press};
use super::proto::Frame;
use super::windows::{ns_rect, HostEvent, HostWindows, SessionWindow};
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, Message};
use objc2_app_kit::{NSApplication, NSEvent, NSEventType, NSWindowStyleMask};
use objc2_foundation::NSPoint;

/// The mouse gesture of one window.
#[derive(Debug, Default)]
pub enum Gesture {
    #[default]
    Idle,
    /// A title bar press, held back until the mouse moves or lets go.
    Pending {
        down: Retained<NSEvent>,
        at: (f64, f64),
    },
    /// The window server drags the host window.
    Dragging,
    /// The host resizes the window: the edges, the frame at the press and
    /// the mouse at the press, in host points.
    Resizing {
        edges: Edges,
        start: Rect,
        at: (f64, f64),
    },
}

/// What `sendEvent:` does with an event after [`mouse`].
pub enum Mouse {
    /// On to the view, as before.
    Pass,
    /// The host used it.
    Swallow,
    /// Send this held press first, then the event.
    Replay(Retained<NSEvent>),
}

/// The mouse in host screen points.
fn mouse_at() -> (f64, f64) {
    let p = NSEvent::mouseLocation();
    (p.x, p.y)
}

fn rect_of(w: &SessionWindow) -> Rect {
    let f = w.frame();
    Rect::new(f.origin.x, f.origin.y, f.size.width, f.size.height)
}

/// True while the left mouse button is down.
fn left_down() -> bool {
    NSEvent::pressedMouseButtons() & 1 != 0
}

/// One mouse event of a session window: the start of a drag or a
/// resize, its steps, its end.
pub fn mouse(w: &SessionWindow, event: &NSEvent) -> Mouse {
    let iv = w.ivars();
    match event.r#type() {
        NSEventType::LeftMouseDown => {
            if iv.full_screen.get() {
                return Mouse::Pass;
            }
            let f = w.frame();
            let p = event.locationInWindow();
            let chrome: Chrome = iv.chrome.borrow().clone();
            let press = lead::press(
                iv.level,
                &chrome,
                f.size.width,
                f.size.height,
                p.x,
                f.size.height - p.y,
                event.clickCount(),
            );
            let gesture = match press {
                Press::Pass => return Mouse::Pass,
                Press::Drag => Gesture::Pending {
                    down: event.retain(),
                    at: mouse_at(),
                },
                Press::Resize(edges) => {
                    iv.led.set(true);
                    Gesture::Resizing {
                        edges,
                        start: rect_of(w),
                        at: mouse_at(),
                    }
                }
            };
            *iv.gesture.borrow_mut() = gesture;
            // The press does not reach the guest, so the window takes the
            // key itself, as a native window does on a title bar press.
            if !w.isKeyWindow() && w.canBecomeKeyWindow() {
                NSApplication::sharedApplication(MainThreadMarker::from(w)).activate();
                w.makeKeyAndOrderFront(None);
            }
            Mouse::Swallow
        }
        NSEventType::LeftMouseDragged => {
            let state = std::mem::take(&mut *iv.gesture.borrow_mut());
            match state {
                Gesture::Idle => Mouse::Pass,
                Gesture::Pending { down, at } => {
                    if !lead::past_slop(at, mouse_at()) {
                        *iv.gesture.borrow_mut() = Gesture::Pending { down, at };
                        return Mouse::Swallow;
                    }
                    *iv.gesture.borrow_mut() = Gesture::Dragging;
                    iv.led.set(true);
                    w.performWindowDragWithEvent(&down);
                    // The call may run the whole drag before it returns.
                    if !left_down() {
                        end(w);
                    }
                    Mouse::Swallow
                }
                Gesture::Dragging => {
                    *iv.gesture.borrow_mut() = Gesture::Dragging;
                    Mouse::Swallow
                }
                Gesture::Resizing { edges, start, at } => {
                    let now = mouse_at();
                    let r = lead::resized(start, edges, now.0 - at.0, now.1 - at.1);
                    *iv.gesture.borrow_mut() = Gesture::Resizing { edges, start, at };
                    // `windowDidResize:` reports the frame.
                    w.setFrame_display(ns_rect(r), true);
                    Mouse::Swallow
                }
            }
        }
        NSEventType::LeftMouseUp => {
            let state = std::mem::take(&mut *iv.gesture.borrow_mut());
            match state {
                Gesture::Idle => Mouse::Pass,
                Gesture::Pending { down, .. } => Mouse::Replay(down),
                Gesture::Dragging | Gesture::Resizing { .. } => {
                    end(w);
                    Mouse::Swallow
                }
            }
        }
        _ => Mouse::Pass,
    }
}

/// The user let go: the last frame, not held, then no lead on AppKit.
fn end(w: &SessionWindow) {
    let iv = w.ivars();
    *iv.gesture.borrow_mut() = Gesture::Idle;
    iv.led.set(false);
    (w.sink())(HostEvent::LeadMoved(w.id(), rect_of(w), false));
}

/// `windowDidMove:` and `windowDidResize:`: the view keeps its place,
/// and a host-led change goes to the session.
pub fn moved(w: &SessionWindow) {
    place_view(w);
    if w.ivars().led.get() {
        (w.sink())(HostEvent::LeadMoved(w.id(), rect_of(w), true));
    }
}

/// The full screen transition ended.
pub fn full_screen_done(w: &SessionWindow, entered: bool) {
    place_view(w);
    (w.sink())(HostEvent::FullScreenDone(w.id(), entered));
}

/// The view at its offset: the whole guest display from the top left in
/// full screen, else the guest window's last frame at the top left of
/// the host window, whatever its height now.
pub fn place_view(w: &SessionWindow) {
    let Some(view) = w.view() else {
        return;
    };
    let iv = w.ivars();
    let h = w
        .contentView()
        .map(|c| c.frame().size.height)
        .unwrap_or_else(|| w.frame().size.height);
    let (x, y) = if iv.full_screen.get() {
        crop::full_screen_origin(iv.display, h)
    } else {
        crop::view_origin_at(iv.guest.get(), h, iv.display)
    };
    view.setFrameOrigin(NSPoint::new(x, y));
}

impl HostWindows {
    /// The guest moved the window of `id`: the host window and the view
    /// follow. In full screen only the view's frame is kept for the way
    /// back.
    pub(super) fn move_to(&self, id: u32, frame: Frame) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        w.ivars().guest.set(frame);
        if w.ivars().full_screen.get() || w.styleMask().contains(NSWindowStyleMask::FullScreen) {
            return;
        }
        w.setFrame_display(ns_rect(crop::host_frame(frame, self.screens())), true);
        place_view(w);
    }

    /// The guest reported a frame of a window that the host leads: only
    /// the view follows, the host frame stays where the user put it.
    pub fn follow_view(&self, id: u32, frame: Frame) {
        if let Some(w) = self.windows.get(&id) {
            w.ivars().guest.set(frame);
            place_view(w);
        }
    }

    /// Put the host window of `id` at guest frame `frame` and keep the
    /// view's content: the end of a drag that broke a rule, and the
    /// `window host-move` line of the control socket.
    pub fn set_host(&self, id: u32, frame: Frame) {
        if let Some(w) = self.windows.get(&id) {
            w.setFrame_display(ns_rect(crop::host_frame(frame, self.screens())), true);
            place_view(w);
        }
    }

    /// The host frame of `id` in guest points.
    pub fn host_guest_frame(&self, id: u32) -> Option<Frame> {
        let w = self.windows.get(&id)?;
        Some(crop::guest_frame(rect_of(w), self.screens()))
    }

    /// The corner radius of the clip of `id` now, and whether the host
    /// window is in full screen.
    pub fn clip_of(&self, id: u32) -> Option<(f64, bool)> {
        let w = self.windows.get(&id)?;
        let radius = w
            .contentView()
            .and_then(|c| c.layer())
            .map(|l| l.cornerRadius())?;
        Some((
            radius,
            w.styleMask().contains(NSWindowStyleMask::FullScreen),
        ))
    }

    /// The level and parent of `id` for [`lead::rule`].
    pub fn is_child(&self, id: u32) -> bool {
        self.windows
            .get(&id)
            .is_some_and(|w| w.parentWindow().is_some())
    }

    /// The radius of the clip of `w`: the measured one, else the one of
    /// the guest's macOS, square in full screen.
    pub(super) fn clip(&self, w: &SessionWindow) {
        let iv = w.ivars();
        let r = crop::clip_radius(iv.chrome.borrow().radius, self.macos, iv.full_screen.get());
        if let Some(layer) = w.contentView().and_then(|c| c.layer()) {
            layer.setCornerRadius(r);
        }
    }

    /// The title bar, buttons and radius of the guest window of `id`.
    pub(super) fn set_chrome(&self, id: u32, chrome: &Chrome) {
        if let Some(w) = self.windows.get(&id) {
            *w.ivars().chrome.borrow_mut() = chrome.clone();
            self.clip(w);
        }
    }

    /// The guest window of `id` entered or left its full screen: the host
    /// window does the same. The way back puts the host window at the
    /// guest frame when the transition ends (`HostEvent::FullScreenDone`).
    pub(super) fn full_screen(&self, id: u32, on: bool) {
        let Some(w) = self.windows.get(&id) else {
            return;
        };
        w.ivars().full_screen.set(on);
        self.clip(w);
        if w.styleMask().contains(NSWindowStyleMask::FullScreen) != on {
            w.toggleFullScreen(None);
        }
        place_view(w);
    }

    /// The windows whose window server drag ended without a release event
    /// (`performWindowDragWithEvent:` returned at once): the session asks
    /// each tick.
    pub fn drags_ended(&self) {
        if left_down() {
            return;
        }
        for w in self.windows.values() {
            if matches!(*w.ivars().gesture.borrow(), Gesture::Dragging) {
                end(w);
            }
        }
    }
}
