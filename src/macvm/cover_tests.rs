//! The undrawn part of a resize, the covered parts of an unfocused
//! window, the held click, and the edge colors.

use super::*;

fn f(x: f64, y: f64, w: f64, h: f64) -> Frame {
    Frame::new(x, y, w, h)
}

fn area(parts: &[(Frame, Paint)]) -> f64 {
    parts.iter().map(|(r, _)| r.w * r.h).sum()
}

#[test]
fn a_wider_host_window_covers_the_strip_right_of_the_guest_window() {
    let parts = undrawn((500.0, 300.0), (400.0, 300.0), 28.0, 0.0);
    assert_eq!(
        parts,
        vec![
            (f(400.0, 0.0, 100.0, 28.0), Paint::Title),
            (f(400.0, 28.0, 100.0, 272.0), Paint::Side),
        ]
    );
}

#[test]
fn a_taller_host_window_covers_the_strip_below_and_both_corners() {
    let parts = undrawn((400.0, 360.0), (400.0, 300.0), 28.0, 10.0);
    assert!(parts.contains(&(f(0.0, 300.0, 400.0, 60.0), Paint::Bottom)));
    // The guest's own bottom corners show the guest desktop now: covered.
    assert!(parts.contains(&(f(390.0, 290.0, 10.0, 10.0), Paint::Bottom)));
    assert!(parts.contains(&(f(0.0, 290.0, 10.0, 10.0), Paint::Bottom)));
    // The top corners are still the host's own: not covered.
    assert!(!parts.iter().any(|(r, _)| r.y == 0.0));
}

#[test]
fn a_corner_drag_covers_both_strips_and_the_three_inner_corners() {
    let parts = undrawn((500.0, 360.0), (400.0, 300.0), 28.0, 12.0);
    assert!(parts.contains(&(f(400.0, 0.0, 100.0, 28.0), Paint::Title)));
    assert!(parts.contains(&(f(400.0, 28.0, 100.0, 272.0), Paint::Side)));
    assert!(parts.contains(&(f(0.0, 300.0, 500.0, 60.0), Paint::Bottom)));
    assert!(parts.contains(&(f(388.0, 0.0, 12.0, 12.0), Paint::Title)));
    assert!(parts.contains(&(f(388.0, 288.0, 12.0, 12.0), Paint::Bottom)));
    assert!(parts.contains(&(f(0.0, 288.0, 12.0, 12.0), Paint::Bottom)));
    // Every point outside the guest window is covered once.
    let strips: f64 = 100.0 * 300.0 + 500.0 * 60.0;
    assert_eq!(area(&parts[..3]), strips);
}

#[test]
fn a_host_window_that_is_not_larger_covers_nothing() {
    assert!(undrawn((400.0, 300.0), (400.0, 300.0), 28.0, 10.0).is_empty());
    assert!(undrawn((300.0, 200.0), (400.0, 300.0), 28.0, 10.0).is_empty());
    // Half a point is the same size.
    assert!(undrawn((400.4, 300.4), (400.0, 300.0), 28.0, 10.0).is_empty());
    // Wider but shorter: the right strip ends at the host's bottom, and
    // the guest's bottom corner is outside the host window.
    let parts = undrawn((500.0, 200.0), (400.0, 300.0), 28.0, 10.0);
    assert_eq!(
        parts,
        vec![
            (f(400.0, 0.0, 100.0, 28.0), Paint::Title),
            (f(400.0, 28.0, 100.0, 172.0), Paint::Side),
        ]
    );
}

fn layer(id: u32, frame: Frame, dx: f64) -> Layer {
    Layer {
        id,
        frame,
        offset: Offset { dx, dy: 0.0 },
        shown: true,
    }
}

#[test]
fn a_window_above_with_another_offset_covers_the_overlap() {
    // Window 1 is focused and on top in the guest. The person dragged the
    // host window of 2 away, so 2 has another offset.
    let layers = [
        layer(1, f(100.0, 100.0, 400.0, 300.0), 0.0),
        layer(2, f(300.0, 200.0, 400.0, 300.0), 500.0),
    ];
    let c = covers(&layers);
    assert_eq!(c[0], (1, vec![]));
    assert_eq!(c[1], (2, vec![f(0.0, 0.0, 200.0, 200.0)]));
    // The same offset: the host window of 1 lies above 2 on the host too.
    let same = [layers[0], layer(2, layers[1].frame, 0.0)];
    assert_eq!(covers(&same)[1], (2, vec![]));
    // Every window above, whatever its offset, for the picture's taint.
    assert_eq!(overlaps(&same, 2, false), vec![f(0.0, 0.0, 200.0, 200.0)]);
}

#[test]
fn hidden_windows_and_windows_apart_cover_nothing() {
    let mut top = layer(1, f(100.0, 100.0, 400.0, 300.0), 0.0);
    top.shown = false;
    let layers = [top, layer(2, f(300.0, 200.0, 400.0, 300.0), 500.0)];
    assert_eq!(covers(&layers), vec![(2, vec![])]);
    let apart = [
        layer(1, f(0.0, 0.0, 100.0, 100.0), 0.0),
        layer(2, f(100.0, 0.0, 100.0, 100.0), 300.0),
    ];
    assert_eq!(covers(&apart)[1], (2, vec![]));
    assert!(overlaps(&apart, 9, true).is_empty());
}

#[test]
fn subtract_leaves_the_parts_outside() {
    let a = f(0.0, 0.0, 100.0, 100.0);
    assert_eq!(subtract(a, f(200.0, 0.0, 10.0, 10.0)), vec![a]);
    assert!(subtract(a, f(-10.0, -10.0, 200.0, 200.0)).is_empty());
    let parts = subtract(a, f(25.0, 25.0, 50.0, 50.0));
    assert_eq!(parts.len(), 4);
    let left: f64 = parts.iter().map(|r| r.w * r.h).sum();
    assert_eq!(left, 100.0 * 100.0 - 50.0 * 50.0);
    assert_eq!(
        subtract(a, f(50.0, 0.0, 50.0, 100.0)),
        vec![f(0.0, 0.0, 50.0, 100.0)]
    );
}

#[test]
fn the_picture_paints_only_where_it_was_the_windows_own() {
    let cover = [f(0.0, 0.0, 200.0, 200.0)];
    // No picture: the side color.
    assert_eq!(pieces(&cover, false, &[]), vec![(cover[0], Paint::Side)]);
    // A clean picture: the picture.
    assert_eq!(pieces(&cover, true, &[]), vec![(cover[0], Paint::Picture)]);
    // A third window lay on the top left 50 x 50 when the host took the
    // picture: the side color there, the picture around it.
    let p = pieces(&cover, true, &[f(0.0, 0.0, 50.0, 50.0)]);
    assert!(p.contains(&(f(0.0, 0.0, 50.0, 50.0), Paint::Side)));
    let picture: f64 = p
        .iter()
        .filter(|(_, k)| *k == Paint::Picture)
        .map(|(r, _)| r.w * r.h)
        .sum();
    assert_eq!(picture, 200.0 * 200.0 - 50.0 * 50.0);
}

#[test]
fn a_held_click_goes_when_the_cover_is_gone_or_after_the_wait() {
    let t = Instant::now();
    let covered = [f(0.0, 0.0, 200.0, 200.0)];
    let mut h = Held::new("down", (50.0, 50.0), t);
    assert!(!h.ready(&covered, t));
    h.up = Some("up");
    assert!(!h.ready(&covered, t + HOLD_MAX / 2));
    assert!(h.ready(&[], t + Duration::from_millis(30)));
    assert!(h.ready(&covered, t + HOLD_MAX));
    assert!(Held::new((), (250.0, 50.0), t).ready(&covered, t));
    assert!(hit(&covered, 0.0, 199.9) && !hit(&covered, 200.0, 10.0));
}

#[test]
fn the_samples_sit_inside_the_edges_and_away_from_the_corners() {
    let s = Samples::of(400.0, 300.0, 28.0, 10.0);
    assert_eq!(s.title.len(), POINTS);
    assert!(s
        .title
        .iter()
        .all(|&(x, y)| (300.0..=388.0).contains(&x) && y == 14.0));
    assert!(s
        .side
        .iter()
        .all(|&(x, y)| x == 398.0 && (40.0..=288.0).contains(&y)));
    assert!(s
        .bottom
        .iter()
        .all(|&(x, y)| y == 298.0 && (12.0..=388.0).contains(&x)));
    // No title bar: no title samples. A tiny window: none at all.
    assert!(Samples::of(400.0, 300.0, 0.0, 10.0).title.is_empty());
    assert_eq!(Samples::of(5.0, 5.0, 28.0, 10.0), Samples::default());
}

#[test]
fn the_edge_color_is_the_median_and_missing_edges_borrow() {
    let white = [1.0, 1.0, 1.0, 1.0];
    let black = [0.0, 0.0, 0.0, 1.0];
    // Two text pixels in nine do not change the color.
    let mut v = vec![white; 7];
    v.extend([black, black]);
    assert_eq!(median(&v), Some(white));
    assert_eq!(median(&[]), None);
    let gray = [0.5, 0.5, 0.5, 1.0];
    let e = Edges::from(None, Some(gray), None).unwrap();
    assert_eq!((e.title, e.side, e.bottom), (gray, gray, gray));
    let e = Edges::from(Some(white), None, Some(black)).unwrap();
    assert_eq!(e.side, black);
    assert_eq!(e.of(Paint::Title), white);
    assert_eq!(e.of(Paint::Picture), black);
    assert!(Edges::from(None, None, None).is_none());
}

#[test]
fn a_window_lies_on_the_shown_windows_below_it_that_it_overlaps() {
    let mut hidden = layer(4, f(0.0, 0.0, 1000.0, 1000.0), 0.0);
    hidden.shown = false;
    let layers = [
        layer(1, f(0.0, 0.0, 100.0, 100.0), 0.0),
        layer(2, f(50.0, 50.0, 100.0, 100.0), 300.0),
        layer(3, f(500.0, 500.0, 100.0, 100.0), 0.0),
        hidden,
        layer(5, f(90.0, 40.0, 40.0, 40.0), 0.0),
    ];
    // Window 2 lies on 5; 1 is above it, 3 is apart, 4 is hidden.
    assert_eq!(lies_on(&layers, 2), vec![5]);
    assert_eq!(lies_on(&layers, 1), vec![2, 5]);
    assert!(lies_on(&layers, 3).is_empty());
    assert!(lies_on(&layers, 4).is_empty());
    assert!(lies_on(&layers, 9).is_empty());
}
