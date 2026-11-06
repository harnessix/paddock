//! The host-led drag: the hit test of a press, the slop, the resize
//! arithmetic, the rules a led frame passes, and the throttle of
//! `window.move` with the follow of the guest's reports.

use super::*;

fn bar() -> Chrome {
    Chrome {
        title_bar: 28.0,
        // The three window buttons of a 400 x 300 window.
        controls: vec![Frame::new(8.0, 6.0, 54.0, 16.0)],
        movable: true,
        resizable: true,
        radius: None,
    }
}

fn press_at(x: f64, y: f64, clicks: isize) -> Press {
    press(Level::Normal, &bar(), 400.0, 300.0, x, y, clicks)
}

#[test]
fn a_press_in_the_title_bar_away_from_the_buttons_drags() {
    assert_eq!(press_at(200.0, 14.0, 1), Press::Drag);
    assert_eq!(press_at(200.0, 28.0, 1), Press::Drag);
    // Below the title bar: the content gets it.
    assert_eq!(press_at(200.0, 29.0, 1), Press::Pass);
    assert_eq!(press_at(200.0, 150.0, 1), Press::Pass);
    // On the window buttons: the guest gets it.
    assert_eq!(press_at(20.0, 14.0, 1), Press::Pass);
}

#[test]
fn a_double_click_on_the_title_bar_goes_to_the_guest_for_its_zoom() {
    assert_eq!(press_at(200.0, 14.0, 2), Press::Pass);
    assert_eq!(press_at(2.0, 150.0, 2), Press::Pass);
}

#[test]
fn a_press_on_an_edge_resizes_and_a_corner_moves_two_edges() {
    assert_eq!(
        press_at(398.0, 150.0, 1),
        Press::Resize(Edges {
            right: true,
            ..Edges::default()
        })
    );
    assert_eq!(
        press_at(2.0, 298.0, 1),
        Press::Resize(Edges {
            left: true,
            bottom: true,
            ..Edges::default()
        })
    );
    assert_eq!(
        press_at(200.0, 1.0, 1),
        Press::Resize(Edges {
            top: true,
            ..Edges::default()
        })
    );
}

#[test]
fn a_window_that_cannot_move_or_resize_passes_every_press() {
    let fixed = Chrome {
        movable: false,
        resizable: false,
        ..bar()
    };
    for (x, y) in [(200.0, 14.0), (398.0, 150.0), (2.0, 2.0)] {
        assert_eq!(
            press(Level::Normal, &fixed, 400.0, 300.0, x, y, 1),
            Press::Pass
        );
    }
}

#[test]
fn only_normal_windows_lead_and_a_press_outside_passes() {
    for level in [Level::Sheet, Level::Menu, Level::Popover, Level::Floating] {
        assert_eq!(
            press(level, &bar(), 400.0, 300.0, 200.0, 14.0, 1),
            Press::Pass
        );
    }
    assert_eq!(press_at(-1.0, 14.0, 1), Press::Pass);
    assert_eq!(press_at(401.0, 14.0, 1), Press::Pass);
}

#[test]
fn a_toolbar_window_drags_below_the_buttons_row_but_not_on_an_item() {
    let tool = Chrome {
        title_bar: 52.0,
        controls: vec![
            Frame::new(20.0, 18.0, 54.0, 16.0),
            Frame::new(300.0, 12.0, 28.0, 28.0),
        ],
        ..bar()
    };
    let at = |x, y| press(Level::Normal, &tool, 400.0, 300.0, x, y, 1);
    assert_eq!(at(200.0, 45.0), Press::Drag);
    assert_eq!(at(310.0, 20.0), Press::Pass);
    assert_eq!(at(200.0, 60.0), Press::Pass);
}

#[test]
fn the_slop_is_three_points_in_any_direction() {
    assert!(!past_slop((10.0, 10.0), (12.0, 10.0)));
    assert!(!past_slop((10.0, 10.0), (12.0, 12.0)));
    assert!(past_slop((10.0, 10.0), (13.0, 10.0)));
    assert!(past_slop((10.0, 10.0), (10.0, 7.0)));
}

#[test]
fn a_resize_keeps_the_opposite_edge_and_the_minimum_size() {
    let start = Rect::new(100.0, 100.0, 400.0, 300.0);
    let right = Edges {
        right: true,
        ..Edges::default()
    };
    assert_eq!(
        resized(start, right, 50.0, 9.0),
        Rect::new(100.0, 100.0, 450.0, 300.0)
    );
    // The left edge: the right edge stays at 500.
    let left = Edges {
        left: true,
        ..Edges::default()
    };
    let r = resized(start, left, 30.0, 0.0);
    assert_eq!(r, Rect::new(130.0, 100.0, 370.0, 300.0));
    assert_eq!(r.x + r.w, 500.0);
    // The bottom edge (y up): the top edge stays at 400.
    let bottom = Edges {
        bottom: true,
        ..Edges::default()
    };
    let r = resized(start, bottom, 0.0, -40.0);
    assert_eq!(r, Rect::new(100.0, 60.0, 400.0, 340.0));
    assert_eq!(r.y + r.h, 400.0);
    // Never below the minimum, and the opposite edge still stays.
    let r = resized(start, left, 1000.0, 0.0);
    assert_eq!(r.w, MIN_W);
    assert_eq!(r.x + r.w, 500.0);
    let top = Edges {
        top: true,
        ..Edges::default()
    };
    assert_eq!(resized(start, top, 0.0, -1000.0).h, MIN_H);
}

fn two_screens() -> ScreenSet {
    // Two 1000 x 800 screens side by side with a 200 point gap: the
    // guest display covers the gap too.
    ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            Rect::new(1200.0, 0.0, 1000.0, 800.0),
        ],
        main: 0,
        scales: Vec::new(),
    }
}

#[test]
fn a_drag_into_the_screen_gap_lands_on_the_nearest_screen() {
    let screens = two_screens();
    let display = screens.display(2.0);
    // The window sits in the gap: less than a quarter on a screen.
    let f = Frame::new(1010.0, 100.0, 180.0, 100.0);
    let ruled = rule(Level::Normal, false, f, &screens, display);
    assert!(!ruled.near(&f, 0.5), "{ruled:?}");
    let host = crop::host_frame(ruled, &screens);
    assert!(screens.visible_area(host) >= host.area() / 4.0);
    // A child window follows its parent, gap or not.
    assert_eq!(rule(Level::Normal, true, f, &screens, display), f);
    // A frame on a screen stays.
    let fine = Frame::new(100.0, 100.0, 400.0, 300.0);
    assert_eq!(rule(Level::Normal, false, fine, &screens, display), fine);
}

#[test]
fn a_resize_never_makes_a_window_larger_than_its_level_allows() {
    let screens = ScreenSet::one(1440.0, 900.0);
    let display = screens.display(2.0);
    let big = Frame::new(0.0, 0.0, 2000.0, 1200.0);
    let floating = rule(Level::Floating, false, big, &screens, display);
    assert!(crop::size_allowed(
        Level::Floating,
        floating,
        screens.main()
    ));
    let popover = rule(Level::Popover, true, big, &screens, display);
    assert!(crop::size_allowed(Level::Popover, popover, screens.main()));
    // Off the display: back inside.
    let off = Frame::new(-300.0, 2000.0, 400.0, 300.0);
    let r = rule(Level::Normal, false, off, &screens, display);
    assert!(r.x >= 0.0 && r.y + r.h <= display.h + 0.5, "{r:?}");
}

fn f(x: f64) -> Frame {
    Frame::new(x, 100.0, 400.0, 300.0)
}

#[test]
fn one_move_in_flight_and_only_the_latest_pending() {
    let t = Instant::now();
    let mut l = Leads::default();
    assert_eq!(l.host_moved(1, f(10.0), true, t), Some(f(10.0)));
    // In flight: the next frames wait, and only the newest is kept.
    assert_eq!(l.host_moved(1, f(20.0), true, t), None);
    assert_eq!(l.host_moved(1, f(30.0), true, t), None);
    assert_eq!(l.target(1), Some(f(30.0)));
    // The reply sends the newest pending frame, not 20.
    assert_eq!(l.answered(1, t), Some(f(30.0)));
    // Its reply has nothing pending.
    assert_eq!(l.answered(1, t), None);
    // Nothing in flight: the next move goes at once.
    assert_eq!(l.host_moved(1, f(40.0), true, t), Some(f(40.0)));
    // A second window has its own lead.
    assert_eq!(l.host_moved(2, f(5.0), false, t), Some(f(5.0)));
    assert_eq!(l.len(), 2);
}

#[test]
fn a_move_to_the_same_frame_while_in_flight_sends_nothing_more() {
    let t = Instant::now();
    let mut l = Leads::default();
    l.host_moved(1, f(10.0), true, t);
    assert_eq!(l.host_moved(1, f(10.2), true, t), None);
    assert_eq!(l.answered(1, t), None);
}

#[test]
fn guest_reports_move_only_the_view_until_the_guest_reaches_the_host() {
    let t = Instant::now();
    let mut l = Leads::default();
    l.host_moved(1, f(10.0), true, t);
    l.host_moved(1, f(50.0), true, t);
    // An intermediate frame while the user holds the window.
    assert_eq!(l.guest_frame(1, f(10.0), t), Follow::View);
    l.answered(1, t);
    l.released(1);
    // 50 is in flight now.
    assert_eq!(l.guest_frame(1, f(10.0), t), Follow::View);
    let later = t + Duration::from_millis(30);
    l.answered(1, later);
    let at = later + Duration::from_millis(5);
    assert_eq!(
        l.guest_frame(1, f(51.0), at),
        Follow::Reached(at.duration_since(t))
    );
    assert!(!l.leading(1));
    // No lead: the host follows the guest as before.
    assert_eq!(l.guest_frame(1, f(80.0), at), Follow::Window);
}

#[test]
fn a_stale_report_after_the_reply_waits_for_the_grace_then_the_guest_wins() {
    let t = Instant::now();
    let mut l = Leads::default();
    l.host_moved(1, f(50.0), false, t);
    l.answered(1, t);
    // The guest list lags one poll: still the view only.
    assert_eq!(
        l.guest_frame(1, f(10.0), t + Duration::from_millis(100)),
        Follow::View
    );
    // After the grace the guest's frame is its answer (a minimum size, a
    // refusal): the host window follows it.
    assert_eq!(l.guest_frame(1, f(10.0), t + GRACE), Follow::Window);
    assert!(!l.leading(1));
}

#[test]
fn an_idle_lead_without_a_matching_report_expires_after_the_grace() {
    let t = Instant::now();
    let mut l = Leads::default();
    l.host_moved(1, f(50.0), false, t);
    l.host_moved(2, f(50.0), true, t);
    l.host_moved(3, f(50.0), false, t);
    // No reply yet: nothing expires.
    assert!(l.expired(t + GRACE * 10).is_empty());
    l.answered(1, t);
    l.answered(2, t);
    assert!(l.expired(t + GRACE / 2).is_empty());
    // 2 is still held: it stays. 1 goes.
    assert_eq!(l.expired(t + GRACE), vec![1]);
    assert!(l.leading(2) && l.leading(3) && !l.leading(1));
    // A failed or timed out move counts as answered.
    assert_eq!(l.answered(3, t), None);
    l.released(2);
    assert_eq!(l.expired(t + GRACE), vec![2, 3]);
    assert!(l.is_empty());
}

#[test]
fn remove_and_clear_end_the_leads() {
    let t = Instant::now();
    let mut l = Leads::default();
    l.host_moved(1, f(1.0), true, t);
    l.host_moved(2, f(1.0), true, t);
    l.remove(1);
    assert!(!l.leading(1));
    assert_eq!(l.answered(1, t), None);
    l.clear();
    assert!(l.is_empty());
}
