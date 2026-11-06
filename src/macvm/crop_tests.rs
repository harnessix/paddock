//! The crop arithmetic on one screen, two screens side by side with the
//! main on the right, one screen above another, and a frame that crosses
//! two screens.

use super::*;
use crate::macvm::session;

fn f(x: f64, y: f64, w: f64, h: f64) -> Frame {
    Frame::new(x, y, w, h)
}

/// A 1440 by 900 point main screen at 2x (2880 by 1800 pixels).
fn one() -> ScreenSet {
    ScreenSet::one(1440.0, 900.0)
}

/// A 1920 by 1080 screen left of the main 1440 by 900 screen, tops
/// aligned: the left screen is at x -1920 and reaches y 900 as well.
fn side_by_side() -> ScreenSet {
    ScreenSet {
        screens: vec![
            Rect::new(-1920.0, -180.0, 1920.0, 1080.0),
            Rect::new(0.0, 0.0, 1440.0, 900.0),
        ],
        main: 1,
        scales: vec![1.0, 2.0],
    }
}

/// A 1920 by 1080 screen above the main 1440 by 900 screen.
fn above() -> ScreenSet {
    ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(-240.0, 900.0, 1920.0, 1080.0),
        ],
        main: 0,
        scales: vec![2.0, 2.0],
    }
}

#[test]
fn host_frame_cases() {
    // One screen: y flips against the screen height.
    let s = one();
    assert_eq!(
        host_frame(f(100.0, 50.0, 400.0, 300.0), &s),
        Rect::new(100.0, 550.0, 400.0, 300.0)
    );
    // The top left guest point is the top left of the screen.
    assert_eq!(
        host_frame(f(0.0, 0.0, 10.0, 10.0), &s),
        Rect::new(0.0, 890.0, 10.0, 10.0)
    );
    // Two screens side by side, the main on the right: guest x 0 is the
    // left edge of the left screen, guest y 0 is the top of both.
    let s = side_by_side();
    assert_eq!(s.union(), Rect::new(-1920.0, -180.0, 3360.0, 1080.0));
    assert_eq!(
        host_frame(f(0.0, 0.0, 100.0, 100.0), &s),
        Rect::new(-1920.0, 800.0, 100.0, 100.0)
    );
    // A window on the main screen at its top left.
    assert_eq!(
        host_frame(f(1920.0, 0.0, 100.0, 100.0), &s),
        Rect::new(0.0, 800.0, 100.0, 100.0)
    );
    // One screen above the other: guest y 0 is the top of the upper one.
    let s = above();
    assert_eq!(s.union(), Rect::new(-240.0, 0.0, 1920.0, 1980.0));
    assert_eq!(
        host_frame(f(240.0, 1080.0, 100.0, 100.0), &s),
        Rect::new(0.0, 800.0, 100.0, 100.0)
    );
    // A frame that crosses the two screens keeps its size and sits over
    // the seam.
    let s = side_by_side();
    let r = host_frame(f(1820.0, 100.0, 200.0, 100.0), &s);
    assert_eq!(r, Rect::new(-100.0, 700.0, 200.0, 100.0));
}

#[test]
fn view_origin_cases() {
    let d = Display {
        w: 1440.0,
        h: 900.0,
        scale: 2.0,
    };
    // A window at the top left: the view's top left is the window's.
    assert_eq!(view_origin(f(0.0, 0.0, 400.0, 300.0), d), (0.0, -600.0));
    // Lower and to the right: the view moves up and left by the same.
    assert_eq!(
        view_origin(f(100.0, 50.0, 400.0, 300.0), d),
        (-100.0, -550.0)
    );
    // A window at the bottom edge: the view's bottom is the window's.
    assert_eq!(view_origin(f(0.0, 600.0, 400.0, 300.0), d), (0.0, 0.0));
    // The union of two screens as the display.
    let d = side_by_side().display(2.0);
    assert_eq!((d.w, d.h), (3360.0, 1080.0));
    assert_eq!(
        view_origin(f(1920.0, 0.0, 100.0, 100.0), d),
        (-1920.0, -980.0)
    );
}

#[test]
fn round_trip() {
    for s in [one(), side_by_side(), above()] {
        for g in [
            f(0.0, 0.0, 10.0, 10.0),
            f(100.5, 50.25, 400.0, 300.0),
            f(1820.0, 100.0, 200.0, 100.0),
            f(240.0, 1080.0, 100.0, 100.0),
        ] {
            let back = guest_frame(host_frame(g, &s), &s);
            assert!(back.near(&g, 1e-9), "{g:?} came back as {back:?} on {s:?}");
        }
    }
}

#[test]
fn clamp_cases() {
    let d = Display {
        w: 1440.0,
        h: 900.0,
        scale: 2.0,
    };
    // Beyond the right edge: moved left to the edge.
    assert_eq!(
        clamp_into(f(1300.0, 100.0, 400.0, 300.0), d),
        f(1040.0, 100.0, 400.0, 300.0)
    );
    // Beyond the bottom edge: moved up to the edge.
    assert_eq!(
        clamp_into(f(100.0, 800.0, 400.0, 300.0), d),
        f(100.0, 600.0, 400.0, 300.0)
    );
    // Both, and a negative origin.
    assert_eq!(
        clamp_into(f(-50.0, -20.0, 400.0, 300.0), d),
        f(0.0, 0.0, 400.0, 300.0)
    );
    // Larger than the display: the size stays, the frame sits at 0,0.
    assert_eq!(
        clamp_into(f(100.0, 100.0, 2000.0, 1000.0), d),
        f(0.0, 0.0, 2000.0, 1000.0)
    );
    // Inside: unchanged.
    assert_eq!(
        clamp_into(f(100.0, 100.0, 400.0, 300.0), d),
        f(100.0, 100.0, 400.0, 300.0)
    );
}

#[test]
fn size_allowed_per_level() {
    let main = one().main();
    // A menu of half the screen is refused, a small one passes.
    assert!(!size_allowed(Level::Menu, f(0.0, 0.0, 720.0, 900.0), main));
    assert!(size_allowed(Level::Menu, f(0.0, 0.0, 300.0, 400.0), main));
    // Exactly a quarter passes.
    assert!(size_allowed(
        Level::Popover,
        f(0.0, 0.0, 720.0, 450.0),
        main
    ));
    assert!(!size_allowed(
        Level::Tooltip,
        f(0.0, 0.0, 721.0, 450.0),
        main
    ));
    // A floating window larger than the main screen is refused, one of
    // the screen's size passes.
    assert!(!size_allowed(
        Level::Floating,
        f(0.0, 0.0, 1441.0, 900.0),
        main
    ));
    assert!(!size_allowed(
        Level::Sheet,
        f(0.0, 0.0, 1440.0, 901.0),
        main
    ));
    assert!(size_allowed(
        Level::Floating,
        f(0.0, 0.0, 1440.0, 900.0),
        main
    ));
    // A normal window of any size passes.
    assert!(size_allowed(
        Level::Normal,
        f(0.0, 0.0, 5000.0, 5000.0),
        main
    ));
    for level in Level::ALL {
        assert!(
            size_allowed(level, f(0.0, 0.0, 100.0, 50.0), main),
            "{level:?}"
        );
    }
}

#[test]
fn fit_to_limit_shrinks_to_the_allowed_size() {
    let main = one().main();
    let big = f(10.0, 20.0, 2000.0, 1000.0);
    let fit = fit_to_limit(Level::Floating, big, main);
    assert_eq!(fit, f(10.0, 20.0, 1440.0, 900.0));
    assert!(size_allowed(Level::Floating, fit, main));
    let fit = fit_to_limit(Level::Menu, f(0.0, 0.0, 1440.0, 900.0), main);
    assert!(size_allowed(Level::Menu, fit, main), "{fit:?}");
    assert!(fit.w >= 700.0 && fit.h >= 440.0, "{fit:?}");
    let small = f(0.0, 0.0, 100.0, 100.0);
    assert_eq!(fit_to_limit(Level::Menu, small, main), small);
    assert_eq!(fit_to_limit(Level::Normal, big, main), big);
}

#[test]
fn corner_radius_per_macos() {
    assert_eq!(corner_radius(11), 10.0);
    assert_eq!(corner_radius(15), 10.0);
    assert_eq!(corner_radius(26), 12.0);
    assert_eq!(corner_radius(10), 6.0);
}

#[test]
fn display_from_pixels_divides_by_the_scale() {
    let d = Display::from_pixels(session::Display {
        w: 2880,
        h: 1800,
        scale: 2,
    });
    assert_eq!((d.w, d.h, d.scale), (1440.0, 900.0, 2.0));
    let d = Display::from_pixels(session::Display {
        w: 1920,
        h: 1080,
        scale: 0,
    });
    assert_eq!((d.w, d.h, d.scale), (1920.0, 1080.0, 1.0));
}

#[test]
fn rect_near_and_main_of_an_empty_set() {
    assert!(Rect::new(0.0, 0.0, 1.0, 1.0).near(&Rect::new(0.4, 0.0, 1.0, 1.0)));
    assert!(!Rect::new(0.0, 0.0, 1.0, 1.0).near(&Rect::new(0.6, 0.0, 1.0, 1.0)));
    let empty = ScreenSet {
        screens: vec![],
        main: 0,
        scales: vec![],
    };
    assert_eq!(empty.main(), Rect::default());
    assert_eq!(empty.union(), Rect::default());
    assert_eq!(empty.nearest(Rect::new(0.0, 0.0, 1.0, 1.0)), None);
    assert_eq!(side_by_side().main(), Rect::new(0.0, 0.0, 1440.0, 900.0));
}

/// A 1440 by 900 main screen at 2x and a 1920 by 1080 screen at 1x to its
/// right, 160 points apart, tops at 900 and 990.
fn with_gap() -> ScreenSet {
    ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(1600.0, -90.0, 1920.0, 1080.0),
        ],
        main: 0,
        scales: vec![2.0, 1.0],
    }
}

#[test]
fn gap_clamps_to_nearest() {
    let set = with_gap();
    // The union spans the gap: the guest can put a window there.
    assert_eq!(set.union(), Rect::new(0.0, -90.0, 3520.0, 1080.0));
    // A window in the gap, nearer to the main screen: onto its right edge.
    let in_gap = f(1445.0, 100.0, 140.0, 100.0);
    assert_eq!(set.visible_area(host_frame(in_gap, &set)), 0.0);
    assert_eq!(
        gap_clamp_guest(in_gap, &set),
        f(1300.0, 100.0, 140.0, 100.0)
    );
    // Nearer to the second screen: onto its left edge.
    let right = f(1470.0, 100.0, 120.0, 100.0);
    assert_eq!(gap_clamp_guest(right, &set), f(1600.0, 100.0, 120.0, 100.0));
    // A window that is on a screen stays, also one that is mostly on it.
    let on_main = f(100.0, 200.0, 400.0, 300.0);
    assert_eq!(gap_clamp_guest(on_main, &set), on_main);
    let mostly = f(1200.0, 200.0, 300.0, 300.0);
    assert_eq!(gap_clamp_guest(mostly, &set), mostly);
    // Less than a quarter visible: moved onto the nearest screen by its
    // center, here the main one, to its right edge.
    let barely = f(1400.0, 200.0, 180.0, 300.0);
    assert_eq!(
        gap_clamp_guest(barely, &set),
        f(1260.0, 200.0, 180.0, 300.0)
    );
    // A frame wider than its nearest screen sits at the screen's left edge.
    assert_eq!(
        gap_clamp(
            Rect::new(1500.0, 0.0, 2000.0, 50.0),
            &ScreenSet::one(1440.0, 900.0)
        ),
        Rect::new(0.0, 0.0, 2000.0, 50.0)
    );
    // Below the second screen (the union is taller than the main screen):
    // the center is under the main screen's bottom, so it moves up.
    let below = f(600.0, 1000.0, 200.0, 60.0);
    assert_eq!(gap_clamp_guest(below, &set), f(600.0, 930.0, 200.0, 60.0));
    // An empty frame is left alone.
    assert_eq!(
        gap_clamp(Rect::new(1500.0, 0.0, 0.0, 0.0), &set),
        Rect::new(1500.0, 0.0, 0.0, 0.0)
    );
}

#[test]
fn mixed_scale_and_session_display() {
    let set = with_gap();
    assert!(set.mixed_scale());
    assert_eq!(set.main_scale(), Some(2.0));
    // The union in pixels at the main scale.
    assert_eq!(
        set.session_display(),
        Some(session::Display {
            w: 7040,
            h: 2160,
            scale: 2
        })
    );
    assert!(!above().mixed_scale());
    assert!(!one().mixed_scale());
    assert_eq!(one().session_display(), None);
    // The sizes stay within what the display argument takes.
    let tiny = ScreenSet {
        screens: vec![Rect::new(0.0, 0.0, 100.0, 50.0)],
        main: 0,
        scales: vec![1.0],
    };
    assert_eq!(
        tiny.session_display(),
        Some(session::Display {
            w: 640,
            h: 480,
            scale: 1
        })
    );
}

#[test]
fn the_view_keeps_the_top_left_of_the_guest_window_in_a_taller_host_window() {
    let d = Display {
        w: 1440.0,
        h: 900.0,
        scale: 2.0,
    };
    let g = Frame::new(100.0, 50.0, 400.0, 300.0);
    // The same height: the plain origin.
    assert_eq!(view_origin_at(g, 300.0, d), view_origin(g, d));
    // A host window 40 points taller (a resize of the bottom edge): the
    // guest point (100, 50) stays at the window's top left, y up.
    let (x, y) = view_origin_at(g, 340.0, d);
    assert_eq!(x, -100.0);
    // The view point of guest (100, 50) is (100, 900 - 50) in the view;
    // in the window it is at y = view y + origin y = the window height.
    assert_eq!(900.0 - 50.0 + y, 340.0);
}

#[test]
fn full_screen_shows_the_whole_display_from_the_top_left() {
    let d = Display {
        w: 1440.0,
        h: 900.0,
        scale: 2.0,
    };
    assert_eq!(full_screen_origin(d, 900.0), (0.0, 0.0));
    // A content view higher than the display: the top rows line up.
    assert_eq!(full_screen_origin(d, 920.0), (0.0, 20.0));
}

#[test]
fn the_clip_takes_the_measured_radius_and_falls_back_to_the_version() {
    assert_eq!(clip_radius(Some(16.0), 26, false), 16.0);
    assert_eq!(clip_radius(Some(0.0), 26, false), 0.0);
    assert_eq!(clip_radius(None, 26, false), corner_radius(26));
    assert_eq!(clip_radius(None, 15, false), corner_radius(15));
    // A report out of range or not a number: the fallback.
    assert_eq!(clip_radius(Some(-1.0), 26, false), corner_radius(26));
    assert_eq!(clip_radius(Some(400.0), 26, false), corner_radius(26));
    assert_eq!(clip_radius(Some(f64::NAN), 26, false), corner_radius(26));
    // Full screen: square corners, measured or not.
    assert_eq!(clip_radius(Some(16.0), 26, true), 0.0);
    assert_eq!(clip_radius(None, 15, true), 0.0);
}
