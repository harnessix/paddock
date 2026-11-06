//! The chrome of a guest window, pure, with tests on Linux: the title bar
//! height and its buttons from the Accessibility frames, the corner radius
//! from the alpha of the window's image, when the agent measures the
//! radius again, and the parts of a full screen window that the host
//! shows already. `ax.rs` reads the frames, `capture.rs` the image.

use crate::macvm::proto::{Frame, Level, Window};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The tallest title bar the agent reports, in points: a unified toolbar
/// with large items is about 80.
pub const BAR_MAX: f64 = 120.0;
/// The most buttons the agent reports per window.
pub const CONTROLS_MAX: usize = 32;
/// A measured radius below this is no radius: an image without the window
/// (no Screen Recording permission shows the desktop, fully opaque).
pub const RADIUS_MIN: f64 = 1.5;
/// The corner patch that the radius fit reads, in pixels.
const PATCH: usize = 96;

/// The Accessibility frames of a window's title bar, in guest points.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarParts {
    /// `AXTitleUIElement`.
    pub title: Option<Frame>,
    /// The close, minimize, zoom and full screen buttons.
    pub buttons: Vec<Frame>,
    /// The `AXToolbar` child of the window.
    pub toolbar: Option<Frame>,
    /// The items of the toolbar, without the static texts.
    pub items: Vec<Frame>,
}

/// The title bar height of `window` from its parts: the bottom of a
/// toolbar at the top, else twice the middle of the window buttons or of
/// the title (the title bar centers them), the largest of these. None
/// without any part: the host takes its default.
pub fn title_bar(window: Frame, parts: &BarParts) -> Option<f64> {
    let top = window.y;
    let mut best: Option<f64> = None;
    let mut take = |h: f64| {
        if h.is_finite() && h > 0.0 {
            best = Some(best.map_or(h, |b| b.max(h)));
        }
    };
    if let Some(t) = parts.toolbar {
        // A toolbar at the top of the window, not one lower in it.
        if t.y >= top - 1.0 && t.y <= top + 60.0 {
            take(t.y + t.h - top);
        }
    }
    for b in &parts.buttons {
        take(2.0 * (b.y + b.h / 2.0 - top));
    }
    if let Some(t) = parts.title {
        take(2.0 * (t.y + t.h / 2.0 - top));
    }
    best.map(|h| h.min(BAR_MAX).min(window.h).max(0.0))
}

/// The buttons in the title bar band of `window`, relative to its top
/// left, on half points, at most [`CONTROLS_MAX`].
pub fn controls(window: Frame, parts: &BarParts, bar: f64) -> Vec<Frame> {
    let half = |v: f64| (v * 2.0).round() / 2.0;
    parts
        .buttons
        .iter()
        .chain(parts.items.iter())
        .filter(|f| f.w > 0.0 && f.h > 0.0)
        .map(|f| Frame::new(f.x - window.x, f.y - window.y, f.w, f.h))
        .filter(|f| f.y < bar && f.y + f.h > 0.0 && f.x < window.w && f.x + f.w > 0.0)
        .map(|f| Frame::new(half(f.x), half(f.y), half(f.w), half(f.h)))
        .take(CONTROLS_MAX)
        .collect()
}

/// An alpha map: `w` by `h` values from 0 (clear) to 1 (opaque), row by
/// row from the top.
#[derive(Debug, Clone, Copy)]
pub struct Alpha<'a> {
    pub w: usize,
    pub h: usize,
    pub a: &'a [f32],
}

/// A corner of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Alpha<'_> {
    /// The alpha at `(x, y)` counted from `corner`, inwards.
    fn at(&self, corner: Corner, x: usize, y: usize) -> f64 {
        let (cx, cy) = match corner {
            Corner::TopLeft => (x, y),
            Corner::TopRight => (self.w - 1 - x, y),
            Corner::BottomLeft => (x, self.h - 1 - y),
            Corner::BottomRight => (self.w - 1 - x, self.h - 1 - y),
        };
        self.a.get(cy * self.w + cx).map_or(0.0, |v| *v as f64)
    }
}

/// The clear width of each row of a corner, in pixels: the clear part
/// of each pixel up to the first opaque one, so an anti-aliased edge
/// counts its share. A shadow around the window is taken out first: the
/// corner pixel itself is outside the window, so its alpha is the
/// shadow's, and every alpha is scaled from it to 1. None when a row
/// has no window pixel across the patch: no window in the image.
fn clear_widths(alpha: &Alpha, corner: Corner, n: usize) -> Option<Vec<f64>> {
    let shadow = alpha.at(corner, 0, 0).min(0.9);
    let solid =
        |x: usize, y: usize| ((alpha.at(corner, x, y) - shadow) / (1.0 - shadow)).clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(n);
    for y in 0..n {
        (0..n).find(|x| solid(*x, y) >= 0.5)?;
        let mut c = 0.0;
        for x in 0..n {
            let a = solid(x, y);
            if a >= 0.99 {
                break;
            }
            c += 1.0 - a;
        }
        out.push(c);
    }
    Some(out)
}

/// The clear width of row `y` of a corner of radius `r`, in pixels: the
/// mean over eight lines across the row.
fn model(r: f64, y: usize) -> f64 {
    let mut sum = 0.0;
    for i in 0..8 {
        let yc = y as f64 + (i as f64 + 0.5) / 8.0;
        if yc < r {
            let d = r - yc;
            sum += r - (r * r - d * d).max(0.0).sqrt();
        }
    }
    sum / 8.0
}

/// The radius of one corner in pixels: the circle whose clear widths fit
/// the measured ones best, in steps of a twentieth of a pixel. None
/// without a window in the image.
pub fn corner_radius_px(alpha: &Alpha, corner: Corner) -> Option<f64> {
    let n = alpha.w.min(alpha.h).min(PATCH);
    if n < 4 {
        return None;
    }
    let widths = clear_widths(alpha, corner, n)?;
    let mut best = (f64::MAX, 0.0);
    let steps = (n as f64 * 20.0) as usize;
    for i in 0..=steps {
        let r = i as f64 / 20.0;
        let err: f64 = widths
            .iter()
            .enumerate()
            .map(|(y, c)| (c - model(r, y)).powi(2))
            .sum();
        if err < best.0 {
            best = (err, r);
        }
    }
    Some(best.1)
}

/// The corner radius of a window image in points: the median of the four
/// corners, `scale` pixels per point. None when no corner has a window,
/// or the radius is below [`RADIUS_MIN`].
pub fn radius_points(alpha: &Alpha, scale: f64) -> Option<f64> {
    if scale <= 0.0 || alpha.a.len() < alpha.w * alpha.h {
        return None;
    }
    let mut r: Vec<f64> = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ]
    .iter()
    .filter_map(|c| corner_radius_px(alpha, *c))
    .map(|px| px / scale)
    .collect();
    if r.is_empty() {
        return None;
    }
    r.sort_by(f64::total_cmp);
    let mid = r.len() / 2;
    let median = if r.len().is_multiple_of(2) {
        (r[mid - 1] + r[mid]) / 2.0
    } else {
        r[mid]
    };
    let median = (median * 4.0).round() / 4.0;
    (median >= RADIUS_MIN).then_some(median)
}

/// The first measure of a new window waits this long: it opens with an
/// animation.
pub const FIRST_WAIT: Duration = Duration::from_millis(300);
/// A failed measure is tried again after this time.
pub const RETRY: Duration = Duration::from_millis(500);
/// The tries of one window style.
pub const TRIES: u8 = 4;

/// What changes the corner radius of a window: its level, its title bar
/// (a toolbar comes or goes), its full screen.
#[derive(Debug, Clone, PartialEq)]
struct Style {
    level: Level,
    bar: Option<i64>,
    full_screen: bool,
}

fn style(w: &Window) -> Style {
    Style {
        level: w.level,
        bar: w.title_bar.map(|b| (b * 2.0).round() as i64),
        full_screen: w.full_screen,
    }
}

#[derive(Debug, Clone)]
struct Measure {
    style: Style,
    radius: Option<f64>,
    tries: u8,
    next: Instant,
}

/// The measured radius of each window: once per window and style, a few
/// tries when the image has no window yet.
#[derive(Debug, Clone, Default)]
pub struct Radii {
    map: BTreeMap<u32, Measure>,
}

impl Radii {
    /// The radius of `w` for its report. `measure` captures the window and
    /// runs only when a measure is due. A window in full screen has square
    /// corners and needs no measure.
    pub fn radius(
        &mut self,
        w: &Window,
        now: Instant,
        measure: impl FnOnce() -> Option<f64>,
    ) -> Option<f64> {
        if w.full_screen {
            return None;
        }
        let s = style(w);
        let m = self.map.entry(w.id).or_insert_with(|| Measure {
            style: s.clone(),
            radius: None,
            tries: 0,
            next: now + FIRST_WAIT,
        });
        if m.style != s {
            // A new style: measure again, after the change settles.
            *m = Measure {
                style: s,
                radius: None,
                tries: 0,
                next: now + FIRST_WAIT,
            };
        }
        if m.radius.is_none() && m.tries < TRIES && now >= m.next {
            m.tries += 1;
            m.radius = measure();
            m.next = now + RETRY;
        }
        m.radius
    }

    /// Forget the windows that are gone.
    pub fn retain(&mut self, ids: &[u32]) {
        self.map.retain(|id, _| ids.contains(id));
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The radius of every reported window, measured when [`Radii`] says it
/// is due, and the cache of the windows that went forgotten.
pub fn measure(
    radii: &mut Radii,
    windows: &mut [Window],
    raw: &[super::windows::RawWindow],
    p: &mut impl super::platform::Platform,
    now: Instant,
) {
    for w in windows.iter_mut() {
        if let Some(r) = raw.iter().find(|r| r.id == w.id) {
            w.radius = radii.radius(w, now, || p.window_radius(r));
        }
    }
    let ids: Vec<u32> = windows.iter().map(|w| w.id).collect();
    radii.retain(&ids);
}

/// Drop the windows that a full screen window of the same app covers and
/// that Accessibility does not know (`no_element`): the toolbar and title
/// bar that macOS shows over a full screen window are windows of their
/// own. The host shows the whole guest display for a full screen window,
/// so these parts are on it already, and a host window for them would
/// draw a second frame over the top of the window.
pub fn drop_drawn_parts(windows: &mut Vec<Window>, no_element: &[u32]) {
    let full: Vec<(u32, u32, Frame)> = windows
        .iter()
        .filter(|w| w.full_screen)
        .map(|w| (w.id, w.pid, w.frame))
        .collect();
    if full.is_empty() {
        return;
    }
    let inside = |a: &Frame, b: &Frame| {
        a.x >= b.x - 1.0
            && a.y >= b.y - 1.0
            && a.x + a.w <= b.x + b.w + 1.0
            && a.y + a.h <= b.y + b.h + 1.0
    };
    windows.retain(|w| {
        w.full_screen
            || !no_element.contains(&w.id)
            || !full
                .iter()
                .any(|(id, pid, f)| *id != w.id && *pid == w.pid && inside(&w.frame, f))
    });
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
