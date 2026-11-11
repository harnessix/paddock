//! What the host paints over the guest picture of a window, pure, with
//! tests on Linux. `windows_cover.rs` paints it on AppKit.
//!
//! Every rectangle here is in points from the top left of the window's
//! content, y down.
//!
//! 1. The undrawn part of a resize ([`undrawn`]). While the host window
//!    is larger than the guest window's last frame, the view would show
//!    the guest desktop right of it and below it. The host covers that
//!    part with the window's own colors, sampled from its edges at the
//!    press ([`Samples`]), until the guest reports the new size.
//! 2. The covered part of an unfocused window ([`covers`]). The guest
//!    window stays where it is when the person moves the host window, so
//!    two guest windows can overlap in the guest while their host windows
//!    do not. The guest raises the focused window, so it is always right.
//!    An unfocused window shows the other window's pixels where that one
//!    lies above it in the guest and the two have other offsets. There
//!    the host shows the last picture of the window from before it lost
//!    the top, or its edge color where that picture had another window in
//!    it ([`pieces`]). Moving guest windows apart instead would need a
//!    `window.move` for a host move, and each one shows the guest desktop
//!    for a moment, which is the flash this design removes.
//! 3. A click on a covered part would reach the other window in the
//!    guest. The host holds it ([`Held`]), raises the window in the guest,
//!    and sends the click when the cover is gone or after [`HOLD_MAX`].

use super::place::Offset;
use super::proto::Frame;
use std::time::{Duration, Instant};

/// Two sizes closer than this are the same size, in points.
const EPS: f64 = 0.5;

/// What fills a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    /// The color of the title bar, sampled near its right end.
    Title,
    /// The color of the right edge below the title bar.
    Side,
    /// The color of the bottom edge.
    Bottom,
    /// The window's last picture from before another window covered it.
    Picture,
}

/// The parts of a host window of content size `host` that the guest
/// window of size `guest` has not drawn yet: right of it and below it,
/// and its own rounded corners of radius `radius` where they are no
/// longer the host's corners. `title_bar` splits the right strip into
/// the title bar color and the side color.
pub fn undrawn(
    host: (f64, f64),
    guest: (f64, f64),
    title_bar: f64,
    radius: f64,
) -> Vec<(Frame, Paint)> {
    let (hw, hh) = host;
    let (gw, gh) = guest;
    let wider = hw > gw + EPS;
    let taller = hh > gh + EPS;
    let mut out = Vec::new();
    if wider {
        let bottom = hh.min(gh);
        let bar = title_bar.clamp(0.0, bottom);
        if bar > 0.0 {
            out.push((Frame::new(gw, 0.0, hw - gw, bar), Paint::Title));
        }
        if bottom > bar {
            out.push((Frame::new(gw, bar, hw - gw, bottom - bar), Paint::Side));
        }
    }
    if taller {
        out.push((Frame::new(0.0, gh, hw, hh - gh), Paint::Bottom));
    }
    let r = radius.max(0.0).min(gw / 2.0).min(gh / 2.0);
    if r > 0.0 && gw <= hw + EPS && gh <= hh + EPS {
        if wider {
            out.push((Frame::new(gw - r, 0.0, r, r), Paint::Title));
        }
        if wider || taller {
            let paint = if taller { Paint::Bottom } else { Paint::Side };
            out.push((Frame::new(gw - r, gh - r, r, r), paint));
        }
        if taller {
            out.push((Frame::new(0.0, gh - r, r, r), Paint::Bottom));
        }
    }
    out
}

/// One window of the session as the cover sees it, front first in the
/// guest's order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub id: u32,
    /// The guest frame.
    pub frame: Frame,
    pub offset: Offset,
    /// Shown in the guest: not minimized, not in full screen.
    pub shown: bool,
}

/// The part of `a` inside `b`, in the coordinates of `a`'s top left.
fn local_overlap(a: Frame, b: Frame) -> Option<Frame> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let w = (a.x + a.w).min(b.x + b.w) - x;
    let h = (a.y + a.h).min(b.y + b.h) - y;
    (w > EPS && h > EPS).then(|| Frame::new(x - a.x, y - a.y, w, h))
}

/// The parts of window `id` that a window above it in the guest lies on,
/// in the window's coordinates. With `other_offset`, only the windows
/// whose offset is not the window's own: where the offsets match, the
/// host window of the other one is above it on the host too.
pub fn overlaps(layers: &[Layer], id: u32, other_offset: bool) -> Vec<Frame> {
    let Some(i) = layers.iter().position(|l| l.id == id) else {
        return Vec::new();
    };
    let me = layers[i];
    if !me.shown {
        return Vec::new();
    }
    layers[..i]
        .iter()
        .filter(|l| l.shown && !(other_offset && l.offset.near(me.offset)))
        .filter_map(|l| local_overlap(me.frame, l.frame))
        .collect()
}

/// The covered parts of every window of `layers`: the ones that another
/// window covers in the guest while the host shows the two apart. An
/// entry for each shown window, empty when nothing covers it.
pub fn covers(layers: &[Layer]) -> Vec<(u32, Vec<Frame>)> {
    layers
        .iter()
        .filter(|l| l.shown)
        .map(|l| (l.id, overlaps(layers, l.id, true)))
        .collect()
}

/// The shown windows below `id` in the guest that it lies on: their
/// pictures go stale when the guest raises `id`.
pub fn lies_on(layers: &[Layer], id: u32) -> Vec<u32> {
    let Some(i) = layers.iter().position(|l| l.id == id) else {
        return Vec::new();
    };
    let me = layers[i];
    if !me.shown {
        return Vec::new();
    }
    layers[i + 1..]
        .iter()
        .filter(|l| l.shown && local_overlap(l.frame, me.frame).is_some())
        .map(|l| l.id)
        .collect()
}

/// `a` without `b`: up to four rectangles.
pub fn subtract(a: Frame, b: Frame) -> Vec<Frame> {
    let Some(o) = local_overlap(a, b).map(|o| Frame::new(o.x + a.x, o.y + a.y, o.w, o.h)) else {
        return vec![a];
    };
    let mut out = Vec::new();
    let (ax2, ay2, ox2, oy2) = (a.x + a.w, a.y + a.h, o.x + o.w, o.y + o.h);
    if o.y - a.y > EPS {
        out.push(Frame::new(a.x, a.y, a.w, o.y - a.y));
    }
    if ay2 - oy2 > EPS {
        out.push(Frame::new(a.x, oy2, a.w, ay2 - oy2));
    }
    if o.x - a.x > EPS {
        out.push(Frame::new(a.x, o.y, o.x - a.x, o.h));
    }
    if ax2 - ox2 > EPS {
        out.push(Frame::new(ox2, o.y, ax2 - ox2, o.h));
    }
    out
}

/// What paints each covered rectangle: the window's picture where it was
/// the window's own when the host took it, the side color where another
/// window lay on it then (`tainted`), and the side color everywhere
/// without a picture.
pub fn pieces(cover: &[Frame], picture: bool, tainted: &[Frame]) -> Vec<(Frame, Paint)> {
    let mut out = Vec::new();
    for c in cover {
        if !picture {
            out.push((*c, Paint::Side));
            continue;
        }
        let mut clean = vec![*c];
        for t in tainted {
            clean = clean.into_iter().flat_map(|r| subtract(r, *t)).collect();
        }
        for t in tainted {
            if let Some(o) = local_overlap(*c, *t) {
                out.push((Frame::new(o.x + c.x, o.y + c.y, o.w, o.h), Paint::Side));
            }
        }
        out.extend(clean.into_iter().map(|r| (r, Paint::Picture)));
    }
    out
}

/// True when point `(x, y)` is in one of `rects`.
pub fn hit(rects: &[Frame], x: f64, y: f64) -> bool {
    rects
        .iter()
        .any(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
}

/// The longest time a click on a covered part waits for the raise.
pub const HOLD_MAX: Duration = Duration::from_millis(400);

/// A press on a covered part of a window, held until the guest raised the
/// window: the press, its release when it came, and the point in the
/// window's coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Held<E> {
    pub down: E,
    pub up: Option<E>,
    pub at: (f64, f64),
    pub since: Instant,
}

impl<E> Held<E> {
    pub fn new(down: E, at: (f64, f64), now: Instant) -> Self {
        Held {
            down,
            up: None,
            at,
            since: now,
        }
    }

    /// True when the press can go to the guest: nothing covers its point
    /// any more, or it waited [`HOLD_MAX`].
    pub fn ready(&self, covered: &[Frame], now: Instant) -> bool {
        !hit(covered, self.at.0, self.at.1) || now.saturating_duration_since(self.since) >= HOLD_MAX
    }
}

/// The sample points of the three edge colors of a window of size `w` by
/// `h`, in points from its top left: the title bar left of its right
/// corner, the right edge below the title bar, the bottom edge between
/// its corners. Two points in from each edge, so the window's own border
/// line is not in them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Samples {
    pub title: Vec<(f64, f64)>,
    pub side: Vec<(f64, f64)>,
    pub bottom: Vec<(f64, f64)>,
}

/// How many points each edge gives.
const POINTS: usize = 9;
/// How far in from an edge the samples are, in points.
const INSET: f64 = 2.0;

impl Samples {
    pub fn of(w: f64, h: f64, title_bar: f64, radius: f64) -> Samples {
        if w < 4.0 * INSET || h < 4.0 * INSET {
            return Samples::default();
        }
        let r = radius.max(0.0) + INSET;
        let along = |from: f64, to: f64| -> Vec<f64> {
            if to <= from {
                return vec![(from + to) / 2.0];
            }
            (0..POINTS)
                .map(|i| from + (to - from) * i as f64 / (POINTS - 1) as f64)
                .collect()
        };
        let bar = title_bar.clamp(0.0, h);
        let title = if bar >= 2.0 * INSET {
            // The right end of the title bar, away from the buttons.
            along((w * 0.75).max(r), w - r)
                .into_iter()
                .map(|x| (x, bar / 2.0))
                .collect()
        } else {
            Vec::new()
        };
        let side = along(bar + r, h - r)
            .into_iter()
            .map(|y| (w - INSET, y))
            .collect();
        let bottom = along(r, w - r)
            .into_iter()
            .map(|x| (x, h - INSET))
            .collect();
        Samples {
            title,
            side,
            bottom,
        }
    }
}

/// An RGBA color, each part 0 to 1.
pub type Rgba = [f64; 4];

/// The color of one edge: the median of each part over the samples, so a
/// text line or an icon in a few samples does not change it. None without
/// samples.
pub fn median(colors: &[Rgba]) -> Option<Rgba> {
    if colors.is_empty() {
        return None;
    }
    let mut out = [0.0; 4];
    for (i, part) in out.iter_mut().enumerate() {
        let mut v: Vec<f64> = colors.iter().map(|c| c[i]).collect();
        v.sort_by(f64::total_cmp);
        *part = v[v.len() / 2];
    }
    Some(out)
}

/// The three edge colors of a window. A missing color takes the side
/// color, and the side color takes the bottom one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges {
    pub title: Rgba,
    pub side: Rgba,
    pub bottom: Rgba,
}

impl Edges {
    /// From the medians of the three sample sets. None when no edge gave a
    /// color.
    pub fn from(title: Option<Rgba>, side: Option<Rgba>, bottom: Option<Rgba>) -> Option<Edges> {
        let side = side.or(bottom).or(title)?;
        Some(Edges {
            title: title.unwrap_or(side),
            side,
            bottom: bottom.unwrap_or(side),
        })
    }

    pub fn of(&self, paint: Paint) -> Rgba {
        match paint {
            Paint::Title => self.title,
            Paint::Bottom => self.bottom,
            Paint::Side | Paint::Picture => self.side,
        }
    }
}

#[cfg(test)]
#[path = "cover_tests.rs"]
mod tests;
