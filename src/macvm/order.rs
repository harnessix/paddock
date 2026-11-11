//! The order model of the host windows: a map from guest window id to what
//! the host shows of it, and a pure fold of the guest's `windows` and
//! `focus` events into [`Change`]s for the AppKit side, and of the host's
//! own focus, minimize and Space changes into [`Request`]s for the guest.
//! Pure, with tests on Linux. `windows.rs` turns the changes into
//! `NSWindow` calls.
//!
//! The host is the source of truth for placement, the guest for existence.
//! The fold never invents a window: a window opens on the guest's word and
//! closes on it. Two things it refuses (item 20 of the checklist): a window
//! on a level outside the app levels (`level: hidden`, below the desktop or
//! at the drag and screen saver levels) and a window above `normal` that is larger
//! than its level allows. A refusal is a [`Change::Refuse`] with the frame
//! to send back in `window.move`. The fold keeps the guest's own frames:
//! where each host window goes (its offset, the screen gap rule) is the
//! work of `place.rs`, which never moves a guest window.
//!
//! A `normal` window in the guest's full screen (`full_screen`, from
//! `AXFullScreen`) is no refusal: the host window goes to the host's full
//! screen ([`Change::FullScreen`]) and shows the whole guest display. The
//! title bar, buttons and corner radius that the agent reports reach the
//! host window as [`Change::Chrome`].

use super::crop::{self, Rect, ScreenSet, EPS};
pub use super::lead::Chrome;
use super::proto::{Frame, Level, Request, Window, WindowsEvent, TITLE_BAR};
use std::collections::{BTreeMap, BTreeSet};

/// What the host shows of one guest window.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The process of the window, for the anchor of a child (`place.rs`).
    pub pid: u32,
    pub level: Level,
    pub parent: Option<u32>,
    /// The last frame the host followed, in guest points.
    pub frame: Frame,
    pub title: String,
    pub minimized: bool,
    /// The guest's key window.
    pub focused: bool,
    /// The host window is on the active Space.
    pub space_active: bool,
    /// The window is in the guest's full screen, and so the host window
    /// in the host's.
    pub full_screen: bool,
    pub chrome: Chrome,
}

/// The chrome of a guest window as the host uses it: the title bar of
/// the report or [`TITLE_BAR`].
pub fn chrome_of(w: &Window) -> Chrome {
    Chrome {
        title_bar: w
            .title_bar
            .unwrap_or(TITLE_BAR)
            .clamp(0.0, w.frame.h.max(0.0)),
        controls: w.controls.clone(),
        movable: w.movable,
        resizable: w.resizable,
        radius: w.radius,
    }
}

/// True when the host takes the window's full screen: a top-level
/// `normal` window only.
pub fn full_screen_of(w: &Window) -> bool {
    w.full_screen && w.level == Level::Normal && w.parent.is_none()
}

/// Why the host does not follow a window event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The guest window went to a level outside the app levels (`level:
    /// hidden`). An `AXFullScreen` window is no refusal.
    FullScreen,
    /// The window is larger than its level allows (item 20).
    TooLarge(Level),
}

/// One step for the AppKit side, in the order to run them.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Open(Window),
    Close(u32),
    Move(u32, Frame),
    Retitle(u32, String),
    /// Every window of the session, front first.
    Reorder(Vec<u32>),
    Minimize(u32),
    Restore(u32),
    /// The guest's key window changed: make this host window key when
    /// it is not.
    Focus(u32),
    /// The guest put a window where the host does not follow. `back` is
    /// the frame to send in `window.move`: the last frame the host
    /// followed, or the largest allowed frame of a new window. None when
    /// there is no frame to go back to (a full screen window the host
    /// never showed).
    Refuse {
        id: u32,
        reason: Reason,
        back: Option<Frame>,
    },
    /// The window entered (true) or left the guest's full screen: the
    /// host window does the same. Before the `Move` of the same event.
    FullScreen(u32, bool),
    /// The title bar, the buttons, the radius or the movable and
    /// resizable flags of the window changed.
    Chrome(u32, Chrome),
}

/// The windows of the session and the fold over the events.
#[derive(Debug, Clone, PartialEq)]
pub struct Windows {
    entries: BTreeMap<u32, Entry>,
    /// Every id, front first.
    order: Vec<u32>,
    /// The guest's key window, from its last `focus` event or flag.
    guest_focus: Option<u32>,
    /// The window the host asked the guest to focus, until the guest
    /// answers with a `focus` event, so a second click sends nothing.
    host_asked: Option<u32>,
    /// The host screens: the main one for the size rule.
    screens: ScreenSet,
}

impl Windows {
    /// An empty map. `main` is the main screen in host points, and the
    /// only screen.
    pub fn new(main: Rect) -> Self {
        Windows::with_screens(ScreenSet {
            screens: vec![main],
            main: 0,
            scales: Vec::new(),
        })
    }

    /// An empty map over the host screens.
    pub fn with_screens(screens: ScreenSet) -> Self {
        Windows {
            entries: BTreeMap::new(),
            order: Vec::new(),
            guest_focus: None,
            host_asked: None,
            screens,
        }
    }

    fn main(&self) -> Rect {
        self.screens.main()
    }

    pub fn get(&self, id: u32) -> Option<&Entry> {
        self.entries.get(&id)
    }

    /// Every id, front first.
    pub fn ids(&self) -> &[u32] {
        &self.order
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn guest_focus(&self) -> Option<u32> {
        self.guest_focus
    }

    /// The id and title of every window, front first.
    pub fn titles(&self) -> Vec<(u32, String)> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id).map(|e| (*id, e.title.clone())))
            .collect()
    }

    /// One `windows` event of the guest: the removed windows close, the
    /// changed ones open, move, retitle, minimize or restore, a parent
    /// opens before its child, then the focus and the order.
    pub fn apply(&mut self, ev: &WindowsEvent) -> Vec<Change> {
        let mut out = Vec::new();
        for id in &ev.removed {
            if self.entries.remove(id).is_some() {
                out.push(Change::Close(*id));
            }
        }
        self.order.retain(|id| self.entries.contains_key(id));
        for w in self.parents_first(&ev.changed) {
            self.change(&w, &mut out);
        }
        if let Some(w) = ev
            .changed
            .iter()
            .find(|w| w.focused && self.entries.contains_key(&w.id))
        {
            if let Some(c) = self.on_guest_focus(Some(w.id)) {
                out.push(c);
            }
        }
        let mut order: Vec<u32> = ev
            .order
            .iter()
            .copied()
            .filter(|id| self.entries.contains_key(id))
            .collect();
        for id in self.order.iter().chain(self.entries.keys()) {
            if !order.contains(id) && self.entries.contains_key(id) {
                order.push(*id);
            }
        }
        if order != self.order {
            self.order = order.clone();
            out.push(Change::Reorder(order));
        }
        out
    }

    /// The full list after a new `hello` (an agent restart): the entries
    /// whose id is in the list stay and get the moves and retitles that
    /// differ, the rest close, the new ones open. A host window that
    /// survives keeps its `NSWindow`, so nothing flickers.
    pub fn rebuild(&mut self, full: &WindowsEvent) -> Vec<Change> {
        let present: BTreeSet<u32> = full.changed.iter().map(|w| w.id).collect();
        let removed: Vec<u32> = self
            .entries
            .keys()
            .filter(|id| !present.contains(id))
            .copied()
            .collect();
        self.host_asked = None;
        let ev = WindowsEvent {
            order: full.order.clone(),
            changed: full.changed.clone(),
            removed,
        };
        self.apply(&ev)
    }

    /// The guest's `focus` event: the host window becomes key when it is
    /// not. None when the focus already matches.
    pub fn on_guest_focus(&mut self, window: Option<u32>) -> Option<Change> {
        self.host_asked = None;
        if self.guest_focus == window {
            return None;
        }
        self.guest_focus = window;
        for (id, e) in &mut self.entries {
            e.focused = Some(*id) == window;
        }
        window
            .filter(|id| self.entries.contains_key(id))
            .map(Change::Focus)
    }

    /// A host window became key (a click, Cmd-Tab, the Dock, the Window
    /// menu): `window.focus` to the guest, only when the guest's key
    /// window is another one and the host did not ask already. A click
    /// reaches the guest through the view and focuses the window there,
    /// so the guest's `focus` event comes first and this sends nothing.
    pub fn on_host_focus(&mut self, id: u32) -> Vec<Request> {
        if !self.entries.contains_key(&id)
            || self.guest_focus == Some(id)
            || self.host_asked == Some(id)
        {
            return Vec::new();
        }
        self.host_asked = Some(id);
        vec![Request::WindowFocus { id }]
    }

    /// The user minimized or restored a host window: the same in the
    /// guest, unless the guest did it first and the host followed.
    pub fn on_host_minimize(&mut self, id: u32, minimized: bool) -> Vec<Request> {
        let Some(e) = self.entries.get_mut(&id) else {
            return Vec::new();
        };
        if e.minimized == minimized {
            return Vec::new();
        }
        e.minimized = minimized;
        if minimized {
            vec![Request::WindowMinimize { id }]
        } else {
            vec![Request::WindowRaise { id }]
        }
    }

    /// The active Space changed: the top-level windows on it are raised in
    /// the guest, back to front, so the guest's order among them equals the
    /// host's and the visible windows are on top of the ones on other
    /// Spaces. Nothing for the others.
    pub fn on_space_change(&mut self, active: &[u32]) -> Vec<Request> {
        for (id, e) in &mut self.entries {
            e.space_active = active.contains(id);
        }
        self.order
            .iter()
            .rev()
            .filter(|id| active.contains(id))
            .filter(|id| self.entries.get(id).is_some_and(|e| e.parent.is_none()))
            .map(|id| Request::WindowRaise { id: *id })
            .collect()
    }

    /// The changed windows with every parent before its child, when
    /// both are in the same event and the parent is new.
    fn parents_first(&self, changed: &[Window]) -> Vec<Window> {
        let ids: BTreeSet<u32> = changed.iter().map(|w| w.id).collect();
        let mut done: BTreeSet<u32> = BTreeSet::new();
        let mut out = Vec::new();
        let mut pending: Vec<&Window> = changed.iter().collect();
        while !pending.is_empty() {
            let before = pending.len();
            pending.retain(|w| {
                let waits = w.parent.is_some_and(|p| {
                    p != w.id
                        && ids.contains(&p)
                        && !done.contains(&p)
                        && !self.entries.contains_key(&p)
                });
                if !waits {
                    done.insert(w.id);
                    out.push((*w).clone());
                }
                waits
            });
            if pending.len() == before {
                // A cycle of parents: take them as they come.
                out.extend(pending.drain(..).cloned());
            }
        }
        out
    }

    /// One changed window: the refusals, then open or the differences.
    fn change(&mut self, w: &Window, out: &mut Vec<Change>) {
        if w.level == Level::Hidden {
            let back = self.entries.remove(&w.id).map(|e| e.frame);
            if back.is_some() {
                out.push(Change::Close(w.id));
            }
            out.push(Change::Refuse {
                id: w.id,
                reason: Reason::FullScreen,
                back,
            });
            return;
        }
        if !crop::size_allowed(w.level, w.frame, self.main()) {
            // The host window stays at its last frame and keeps the rest.
            let back = match self.entries.get_mut(&w.id) {
                Some(e) => {
                    if e.title != w.title {
                        e.title = w.title.clone();
                        out.push(Change::Retitle(w.id, w.title.clone()));
                    }
                    Some(e.frame)
                }
                None => Some(crop::fit_to_limit(w.level, w.frame, self.main())),
            };
            out.push(Change::Refuse {
                id: w.id,
                reason: Reason::TooLarge(w.level),
                back,
            });
            return;
        }
        let full_screen = full_screen_of(w);
        let chrome = chrome_of(w);
        let frame = w.frame;
        let shown = w.clone();
        let entry = Entry {
            pid: w.pid,
            level: w.level,
            parent: w.parent,
            frame,
            title: w.title.clone(),
            minimized: w.minimized,
            focused: w.focused,
            space_active: true,
            full_screen,
            chrome: chrome.clone(),
        };
        match self.entries.get_mut(&w.id) {
            None => {
                self.entries.insert(w.id, entry);
                out.push(Change::Open(shown));
            }
            Some(e) if e.level != w.level || e.parent != w.parent => {
                // Another kind of window under the same id: a new host
                // window, because the level and the parent are set once.
                *e = entry;
                out.push(Change::Close(w.id));
                out.push(Change::Open(shown));
            }
            Some(e) => {
                if e.full_screen != full_screen {
                    e.full_screen = full_screen;
                    out.push(Change::FullScreen(w.id, full_screen));
                }
                if e.chrome != chrome {
                    e.chrome = chrome.clone();
                    out.push(Change::Chrome(w.id, chrome));
                }
                if !e.frame.near(&frame, EPS) {
                    e.frame = frame;
                    out.push(Change::Move(w.id, frame));
                }
                if e.title != w.title {
                    e.title = w.title.clone();
                    out.push(Change::Retitle(w.id, w.title.clone()));
                }
                if e.minimized != w.minimized {
                    e.minimized = w.minimized;
                    out.push(if w.minimized {
                        Change::Minimize(w.id)
                    } else {
                        Change::Restore(w.id)
                    });
                }
                e.focused = w.focused;
            }
        }
    }
}

impl Change {
    /// A short text for the logs: the kind and the id, no title.
    pub fn kind(&self) -> String {
        match self {
            Change::Open(w) => format!("open {} {:?}", w.id, w.level), // not user text
            Change::Close(id) => format!("close {id}"),                // not user text
            Change::Move(id, f) => format!("move {id} to {},{} {}x{}", f.x, f.y, f.w, f.h), // not user text
            Change::Retitle(id, _) => format!("retitle {id}"), // not user text
            Change::Reorder(ids) => format!("reorder {ids:?}"), // not user text
            Change::Minimize(id) => format!("minimize {id}"),  // not user text
            Change::Restore(id) => format!("restore {id}"),    // not user text
            Change::Focus(id) => format!("focus {id}"),        // not user text
            Change::Refuse { id, reason, .. } => format!("refuse {id} {reason:?}"), // not user text
            Change::FullScreen(id, on) => format!("full screen {id} {on}"), // not user text
            Change::Chrome(id, c) => {
                let (bar, radius) = (c.title_bar, c.radius);
                format!("chrome {id} bar {bar} radius {radius:?}") // not user text
            }
        }
    }
}

#[cfg(test)]
#[path = "order_tests.rs"]
mod tests;
