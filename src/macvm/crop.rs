//! The crop arithmetic of the viewport windows: where a guest window is
//! on the host screens, and where the full-display view sits inside the
//! host window so that only that guest window shows. Pure, with tests on
//! Linux.
//!
//! Three coordinate systems, all in points:
//!
//! 1. Guest points: the window server of the guest, origin at the top
//!    left of the guest display, y down. The agent reports frames this
//!    way ([`Frame`]).
//! 2. Host screen points: AppKit's screen space, origin at the bottom
//!    left of the main screen, y up. `NSScreen.frame` and
//!    `NSWindow.frame` are in it ([`Rect`]).
//! 3. The view origin: the content view of a host window, origin at the
//!    bottom left of the window, y up. The `VZVirtualMachineView` is as
//!    large as the guest display and sits at a negative origin there, so
//!    the window shows one guest window's rectangle.
//!
//! The guest display is the union of the host screens in points, and its
//! top left corner is the top left corner of that union. So guest point
//! `(0, 0)` is the top of the tallest screen and the left edge of the
//! leftmost screen, whatever the main screen's place.

use super::proto::{Frame, Level};

/// Two frames that differ by less than this are the same frame.
pub const EPS: f64 = 0.5;

/// A rectangle in host screen points (AppKit, origin bottom left of the
/// main screen).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    pub fn area(&self) -> f64 {
        self.w * self.h
    }

    /// True when every edge is within [`EPS`] of the other.
    pub fn near(&self, o: &Rect) -> bool {
        (self.x - o.x).abs() < EPS
            && (self.y - o.y).abs() < EPS
            && (self.w - o.w).abs() < EPS
            && (self.h - o.h).abs() < EPS
    }
}

/// The host screens in points, with the main screen (the one with the
/// menu bar, at origin `(0, 0)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenSet {
    pub screens: Vec<Rect>,
    /// The index of the main screen in `screens`.
    pub main: usize,
    /// The backing scale of each screen (2 on a Retina screen), in the
    /// order of `screens`. Empty when the scales are not known.
    pub scales: Vec<f64>,
}

impl ScreenSet {
    /// One screen of `w` by `h` points, the main one, scale unknown.
    pub fn one(w: f64, h: f64) -> Self {
        ScreenSet {
            screens: vec![Rect::new(0.0, 0.0, w, h)],
            main: 0,
            scales: Vec::new(),
        }
    }

    /// The scale of the main screen, when the scales are known.
    pub fn main_scale(&self) -> Option<f64> {
        self.scales
            .get(self.main)
            .or_else(|| self.scales.first())
            .copied()
    }

    /// True when two screens have two scales: the guest
    /// display gets the main scale, and a window on the other screen is
    /// drawn soft. The start lines say it.
    pub fn mixed_scale(&self) -> bool {
        self.scales
            .iter()
            .any(|s| self.scales.first().is_some_and(|f| (s - f).abs() > 0.01))
    }

    /// The display of a session in pixels: the union of the screens at
    /// the main scale, within the sizes that `session::Display` takes.
    /// None without a scale.
    pub fn session_display(&self) -> Option<super::session::Display> {
        let scale = self.main_scale()?.round().clamp(1.0, 4.0);
        let u = self.union();
        let px = |points: f64, min: u32| ((points * scale).round() as u32).clamp(min, 16384);
        Some(super::session::Display {
            w: px(u.w, 640),
            h: px(u.h, 480),
            scale: scale as u32,
        })
    }

    /// The index of the screen nearest to the center of `r`: the one that
    /// holds the center, else the one at the shortest distance.
    pub fn nearest(&self, r: Rect) -> Option<usize> {
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let distance = |s: &Rect| {
            let dx = (s.x - cx).max(cx - s.x - s.w).max(0.0);
            let dy = (s.y - cy).max(cy - s.y - s.h).max(0.0);
            (dx * dx + dy * dy).sqrt()
        };
        self.screens
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| distance(a).total_cmp(&distance(b)))
            .map(|(i, _)| i)
    }

    /// The area of `r` that is on a screen.
    pub fn visible_area(&self, r: Rect) -> f64 {
        self.screens.iter().map(|s| intersection(*s, r)).sum()
    }

    pub fn main(&self) -> Rect {
        self.screens
            .get(self.main)
            .or_else(|| self.screens.first())
            .copied()
            .unwrap_or_default()
    }

    /// The bounding box of every screen.
    pub fn union(&self) -> Rect {
        let Some(first) = self.screens.first() else {
            return Rect::default();
        };
        let mut min_x = first.x;
        let mut min_y = first.y;
        let mut max_x = first.x + first.w;
        let mut max_y = first.y + first.h;
        for s in &self.screens[1..] {
            min_x = min_x.min(s.x);
            min_y = min_y.min(s.y);
            max_x = max_x.max(s.x + s.w);
            max_y = max_y.max(s.y + s.h);
        }
        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }

    /// The guest display that covers this set at `scale`.
    pub fn display(&self, scale: f64) -> Display {
        let u = self.union();
        Display {
            w: u.w,
            h: u.h,
            scale,
        }
    }
}

/// The guest display in points and its scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

impl Display {
    /// From the session's display, which is in pixels.
    pub fn from_pixels(d: super::session::Display) -> Self {
        let scale = d.scale.max(1) as f64;
        Display {
            w: d.w as f64 / scale,
            h: d.h as f64 / scale,
            scale,
        }
    }
}

/// The host window frame of a guest window: system 1 to system 2.
pub fn host_frame(guest: Frame, screens: &ScreenSet) -> Rect {
    let u = screens.union();
    Rect::new(
        u.x + guest.x,
        u.y + u.h - guest.y - guest.h,
        guest.w,
        guest.h,
    )
}

/// The guest frame of a host window frame: system 2 to system 1.
pub fn guest_frame(host: Rect, screens: &ScreenSet) -> Frame {
    let u = screens.union();
    Frame::new(host.x - u.x, u.y + u.h - host.y - host.h, host.w, host.h)
}

/// Where the full-display view sits inside the host window of `guest`
/// (system 3), so that the window shows exactly that guest window:
/// `(-x, -(display.h - y - h))`.
pub fn view_origin(guest: Frame, display: Display) -> (f64, f64) {
    (-guest.x, -(display.h - guest.y - guest.h))
}

/// [`view_origin`] for a host window of height `host_h` that shows the
/// guest window of frame `guest` at its top left corner: while the host
/// leads a drag or a resize, the content keeps its place in the host
/// window until the guest reports the new frame. With `host_h ==
/// guest.h` it is [`view_origin`].
pub fn view_origin_at(guest: Frame, host_h: f64, display: Display) -> (f64, f64) {
    (-guest.x, -(display.h - guest.y - host_h))
}

/// Where the full-display view sits in a host window in full screen whose
/// content is `content_h` points high: the guest display at origin zero,
/// its top left at the top left of the screen.
pub fn full_screen_origin(display: Display, content_h: f64) -> (f64, f64) {
    (0.0, content_h - display.h)
}

/// The radius of the host clip of a window: the one that the agent
/// measured on the window, when it is a sane number, else the radius of
/// the guest's macOS version ([`corner_radius`]). A window in full screen
/// has square corners.
pub fn clip_radius(measured: Option<f64>, macos_major: u32, full_screen: bool) -> f64 {
    if full_screen {
        return 0.0;
    }
    match measured {
        Some(r) if r.is_finite() && (0.0..=RADIUS_MAX).contains(&r) => r,
        _ => corner_radius(macos_major),
    }
}

/// The largest corner radius that the host takes from the agent, in
/// points.
pub const RADIUS_MAX: f64 = 40.0;

/// A frame moved inside the display when it is beyond an edge. The size
/// stays, also when it is larger than the display: then the frame sits
/// at the top left edge, and the user can drag it.
pub fn clamp_into(guest: Frame, display: Display) -> Frame {
    let x = guest.x.min(display.w - guest.w).max(0.0);
    let y = guest.y.min(display.h - guest.h).max(0.0);
    Frame::new(x, y, guest.w, guest.h)
}

/// The area that two rectangles share.
pub fn intersection(a: Rect, b: Rect) -> f64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0.0 || h <= 0.0 {
        0.0
    } else {
        w * h
    }
}

/// A host frame moved onto the nearest screen when less than a quarter of
/// it is on any screen: a window in the gap between two screens. The guest
/// clamps its windows to its display, which is the union of the screens
/// with the gap, so only the host knows. A frame larger than the screen
/// sits at the screen's top left edge, as [`clamp_into`] does. Any other
/// frame comes back unchanged.
pub fn gap_clamp(r: Rect, screens: &ScreenSet) -> Rect {
    if r.w <= 0.0 || r.h <= 0.0 || screens.visible_area(r) >= r.area() / 4.0 {
        return r;
    }
    let Some(s) = screens.nearest(r).and_then(|i| screens.screens.get(i)) else {
        return r;
    };
    let x = if r.w > s.w {
        s.x
    } else {
        r.x.clamp(s.x, s.x + s.w - r.w)
    };
    let y = if r.h > s.h {
        s.y + s.h - r.h
    } else {
        r.y.clamp(s.y, s.y + s.h - r.h)
    };
    Rect::new(x, y, r.w, r.h)
}

/// [`gap_clamp`] for a guest frame: the guest frame it should have, the
/// same one when it is on a screen.
pub fn gap_clamp_guest(guest: Frame, screens: &ScreenSet) -> Frame {
    let host = host_frame(guest, screens);
    let moved = gap_clamp(host, screens);
    if moved.near(&host) {
        guest
    } else {
        guest_frame(moved, screens)
    }
}

/// The corner radius of a guest window, in points, by the guest's macOS
/// major version, when the agent measured none ([`clip_radius`]): macOS 11 to 25 have 10 points, macOS 26 (the glass
/// design) 12, which the Mac check `corners` confirms. Before macOS 11
/// the corners were about 6 points.
pub fn corner_radius(macos_major: u32) -> f64 {
    match macos_major {
        0..=10 => 6.0,
        11..=25 => 10.0,
        _ => 12.0,
    }
}

/// Item 20 of the security checklist: a guest cannot make a lookalike of
/// a host dialog. `menu`, `popover` and `tooltip` windows get at most a
/// quarter of the main screen's area, `floating` and `sheet` windows at
/// most the main screen's size. A `normal` window has any size.
pub fn size_allowed(level: Level, frame: Frame, main: Rect) -> bool {
    match level {
        Level::Normal | Level::Hidden => true,
        Level::Floating | Level::Sheet => frame.w <= main.w + EPS && frame.h <= main.h + EPS,
        Level::Menu | Level::Popover | Level::Tooltip => {
            frame.w * frame.h <= main.area() / 4.0 + EPS
        }
    }
}

/// The largest frame of `level` that [`size_allowed`] passes, at the
/// place of `frame`: the answer to a window that was never allowed.
pub fn fit_to_limit(level: Level, frame: Frame, main: Rect) -> Frame {
    if size_allowed(level, frame, main) {
        return frame;
    }
    let (w, h) = match level {
        Level::Normal | Level::Hidden => (frame.w, frame.h),
        Level::Floating | Level::Sheet => (frame.w.min(main.w), frame.h.min(main.h)),
        Level::Menu | Level::Popover | Level::Tooltip => {
            // Shrink both sides by the same factor to a quarter of the area.
            let f = (main.area() / 4.0 / (frame.w * frame.h)).sqrt();
            ((frame.w * f).floor(), (frame.h * f).floor())
        }
    };
    Frame::new(frame.x, frame.y, w, h)
}

#[cfg(test)]
#[path = "crop_tests.rs"]
mod tests;
