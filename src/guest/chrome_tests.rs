//! The title bar from the Accessibility frames, the radius fit on made
//! alpha maps (with anti-aliasing, a shadow, no window), the measure
//! cache, and the full screen parts.

use super::*;

const WIN: Frame = Frame::new(200.0, 100.0, 600.0, 400.0);

#[test]
fn a_plain_title_bar_is_twice_the_middle_of_the_buttons() {
    // The close button of a plain window: 14 points high, its middle 14
    // points below the top.
    let parts = BarParts {
        buttons: vec![Frame::new(209.0, 107.0, 14.0, 14.0)],
        title: Some(Frame::new(400.0, 106.0, 100.0, 16.0)),
        ..BarParts::default()
    };
    assert_eq!(title_bar(WIN, &parts), Some(28.0));
}

#[test]
fn a_toolbar_window_reports_the_bottom_of_its_toolbar() {
    let parts = BarParts {
        buttons: vec![Frame::new(220.0, 119.0, 14.0, 14.0)],
        toolbar: Some(Frame::new(200.0, 100.0, 600.0, 52.0)),
        ..BarParts::default()
    };
    assert_eq!(title_bar(WIN, &parts), Some(52.0));
    // A toolbar low in the window (not a title bar): the buttons decide.
    let low = BarParts {
        toolbar: Some(Frame::new(200.0, 400.0, 600.0, 40.0)),
        ..parts
    };
    assert_eq!(title_bar(WIN, &low), Some(52.0));
}

#[test]
fn no_part_gives_no_title_bar_and_a_huge_one_is_capped() {
    assert_eq!(title_bar(WIN, &BarParts::default()), None);
    let huge = BarParts {
        toolbar: Some(Frame::new(200.0, 100.0, 600.0, 300.0)),
        ..BarParts::default()
    };
    assert_eq!(title_bar(WIN, &huge), Some(BAR_MAX));
    let small = Frame::new(200.0, 100.0, 300.0, 20.0);
    let parts = BarParts {
        buttons: vec![Frame::new(209.0, 107.0, 14.0, 14.0)],
        ..BarParts::default()
    };
    assert_eq!(title_bar(small, &parts), Some(20.0));
}

#[test]
fn the_controls_are_relative_to_the_window_and_in_the_bar_only() {
    let parts = BarParts {
        buttons: vec![Frame::new(209.0, 107.0, 14.0, 14.0)],
        items: vec![
            Frame::new(700.25, 110.0, 28.0, 28.0),
            // Below the bar: not a title bar button.
            Frame::new(300.0, 300.0, 28.0, 28.0),
            // No size.
            Frame::new(300.0, 110.0, 0.0, 28.0),
        ],
        ..BarParts::default()
    };
    assert_eq!(
        controls(WIN, &parts, 52.0),
        vec![
            Frame::new(9.0, 7.0, 14.0, 14.0),
            Frame::new(500.5, 10.0, 28.0, 28.0),
        ]
    );
    let many = BarParts {
        items: (0..100)
            .map(|i| Frame::new(200.0 + i as f64, 100.0, 4.0, 4.0))
            .collect(),
        ..BarParts::default()
    };
    assert_eq!(controls(WIN, &many, 28.0).len(), CONTROLS_MAX);
}

/// An image of `w` by `h` pixels: an opaque window with corners of radius
/// `r` pixels, anti-aliased by 8 x 8 samples per pixel, over `around`
/// (0 for clear, a shadow below one half).
fn image(w: usize, h: usize, r: f64, around: f32) -> Vec<f32> {
    let mut a = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut inside = 0;
            for sy in 0..8 {
                for sx in 0..8 {
                    let px = x as f64 + (sx as f64 + 0.5) / 8.0;
                    let py = y as f64 + (sy as f64 + 0.5) / 8.0;
                    let cx = px.clamp(r, w as f64 - r);
                    let cy = py.clamp(r, h as f64 - r);
                    let (dx, dy) = (px - cx, py - cy);
                    if dx * dx + dy * dy <= r * r {
                        inside += 1;
                    }
                }
            }
            let cover = inside as f32 / 64.0;
            a[y * w + x] = cover + (1.0 - cover) * around;
        }
    }
    a
}

#[test]
fn the_radius_of_a_drawn_corner_comes_back_within_a_quarter_point() {
    for (r_pt, scale) in [
        (10.0, 2.0),
        (12.0, 2.0),
        (16.0, 2.0),
        (26.0, 2.0),
        (12.0, 1.0),
    ] {
        let px = r_pt * scale;
        let a = image(200, 160, px, 0.0);
        let alpha = Alpha {
            w: 200,
            h: 160,
            a: &a,
        };
        let got = radius_points(&alpha, scale).unwrap();
        assert!((got - r_pt).abs() <= 0.25, "{r_pt} at {scale}x: {got}");
    }
}

#[test]
fn a_faint_shadow_around_the_window_does_not_change_the_radius() {
    let a = image(200, 160, 32.0, 0.3);
    let alpha = Alpha {
        w: 200,
        h: 160,
        a: &a,
    };
    let got = radius_points(&alpha, 2.0).unwrap();
    assert!((got - 16.0).abs() <= 0.5, "{got}");
}

#[test]
fn an_opaque_image_or_an_empty_one_gives_no_radius() {
    // No Screen Recording: the image shows the desktop, opaque.
    let a = vec![1f32; 100 * 100];
    let alpha = Alpha {
        w: 100,
        h: 100,
        a: &a,
    };
    assert_eq!(radius_points(&alpha, 2.0), None);
    // Not drawn yet: clear.
    let a = vec![0f32; 100 * 100];
    let alpha = Alpha {
        w: 100,
        h: 100,
        a: &a,
    };
    assert_eq!(radius_points(&alpha, 2.0), None);
    // Too small, a short buffer, no scale.
    let a = vec![1f32; 9];
    assert_eq!(radius_points(&Alpha { w: 3, h: 3, a: &a }, 2.0), None);
    assert_eq!(
        radius_points(
            &Alpha {
                w: 10,
                h: 10,
                a: &a
            },
            2.0
        ),
        None
    );
    let a = image(100, 100, 20.0, 0.0);
    assert_eq!(
        radius_points(
            &Alpha {
                w: 100,
                h: 100,
                a: &a
            },
            0.0
        ),
        None
    );
}

#[test]
fn one_odd_corner_does_not_move_the_median() {
    // A window whose top left corner is square (a custom title bar) and
    // the other three round.
    let mut a = image(200, 160, 24.0, 0.0);
    for y in 0..40 {
        for x in 0..40 {
            a[y * 200 + x] = 1.0;
        }
    }
    let alpha = Alpha {
        w: 200,
        h: 160,
        a: &a,
    };
    let got = radius_points(&alpha, 2.0).unwrap();
    assert!((got - 12.0).abs() <= 0.25, "{got}");
    assert_eq!(corner_radius_px(&alpha, Corner::TopLeft), Some(0.0));
}

fn window(id: u32) -> Window {
    Window {
        id,
        pid: 500,
        frame: WIN,
        level: Level::Normal,
        title_bar: Some(28.0),
        ..Window::default()
    }
}

#[test]
fn a_window_is_measured_once_after_its_open_animation() {
    let t = Instant::now();
    let mut r = Radii::default();
    let mut calls = 0;
    // Too early: no measure.
    assert_eq!(
        r.radius(&window(1), t, || {
            calls += 1;
            Some(12.0)
        }),
        None
    );
    assert_eq!(calls, 0);
    let later = t + FIRST_WAIT;
    assert_eq!(
        r.radius(&window(1), later, || {
            calls += 1;
            Some(12.0)
        }),
        Some(12.0)
    );
    // Known: never measured again.
    for i in 1..10 {
        let at = later + RETRY * i;
        assert_eq!(
            r.radius(&window(1), at, || {
                calls += 1;
                Some(99.0)
            }),
            Some(12.0)
        );
    }
    assert_eq!(calls, 1);
}

#[test]
fn a_failed_measure_is_tried_a_few_times_then_left_to_the_fallback() {
    let t = Instant::now();
    let mut r = Radii::default();
    let mut calls = 0;
    let mut at = t + FIRST_WAIT;
    for _ in 0..20 {
        assert_eq!(
            r.radius(&window(1), at, || {
                calls += 1;
                None
            }),
            None
        );
        at += RETRY / 2;
    }
    assert_eq!(calls, TRIES as usize);
}

#[test]
fn a_new_style_measures_again_and_full_screen_never_measures() {
    let t = Instant::now();
    let mut r = Radii::default();
    let w = window(1);
    assert_eq!(r.radius(&w, t, || Some(12.0)), None);
    assert_eq!(r.radius(&w, t + FIRST_WAIT, || Some(12.0)), Some(12.0));
    // A toolbar came: another radius on macOS 26.
    let mut tool = w.clone();
    tool.title_bar = Some(52.0);
    let at = t + FIRST_WAIT * 2;
    assert_eq!(r.radius(&tool, at, || Some(26.0)), None);
    assert_eq!(r.radius(&tool, at + FIRST_WAIT, || Some(26.0)), Some(26.0));
    let mut full = tool.clone();
    full.full_screen = true;
    assert_eq!(
        r.radius(&full, at + FIRST_WAIT * 4, || panic!("no capture")),
        None
    );
    r.retain(&[]);
    assert!(r.is_empty());
}

#[test]
fn the_parts_over_a_full_screen_window_go_and_other_windows_stay() {
    let mut full = window(1);
    full.full_screen = true;
    full.frame = Frame::new(0.0, 0.0, 1440.0, 900.0);
    // The toolbar window over it: no element, same app, inside.
    let mut bar = window(2);
    bar.frame = Frame::new(0.0, 0.0, 1440.0, 52.0);
    // A sheet of the app: Accessibility knows it.
    let mut sheet = window(3);
    sheet.frame = Frame::new(400.0, 52.0, 600.0, 300.0);
    // A window of another app (a guest prompt) over it.
    let mut other = window(4);
    other.pid = 600;
    other.frame = Frame::new(0.0, 0.0, 400.0, 52.0);
    let mut list = vec![bar.clone(), full.clone(), sheet.clone(), other.clone()];
    drop_drawn_parts(&mut list, &[2, 4]);
    let ids: Vec<u32> = list.iter().map(|w| w.id).collect();
    assert_eq!(ids, vec![1, 3, 4]);
    // Without a full screen window nothing goes.
    let mut list = vec![bar, sheet];
    drop_drawn_parts(&mut list, &[2]);
    assert_eq!(list.len(), 2);
}
