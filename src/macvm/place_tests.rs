//! The offset of each host window: its map from the guest frame, the
//! rules on the host frame, the anchors of child windows, and the fold of
//! the guest's changes.

use super::*;
use crate::macvm::crop::Rect;
use crate::macvm::order::{Change, Windows};
use crate::macvm::proto::{Window, WindowsEvent};

fn f(x: f64, y: f64, w: f64, h: f64) -> Frame {
    Frame::new(x, y, w, h)
}

fn display() -> Display {
    Display {
        w: 1440.0,
        h: 900.0,
        scale: 2.0,
    }
}

fn one() -> ScreenSet {
    ScreenSet::one(1440.0, 900.0)
}

fn top(id: u32, pid: u32, frame: Frame) -> Candidate {
    Candidate {
        id,
        pid,
        frame,
        minimized: false,
        focused: false,
    }
}

#[test]
fn the_offset_maps_the_guest_frame_to_the_host_frame_and_back() {
    let guest = f(100.0, 80.0, 400.0, 300.0);
    let host = f(340.0, 20.0, 400.0, 300.0);
    let o = Offset::between(host, guest);
    assert_eq!(
        o,
        Offset {
            dx: 240.0,
            dy: -60.0
        }
    );
    assert_eq!(o.apply(guest), host);
    // A guest move by a delta moves the host by the same delta.
    assert_eq!(
        o.apply(f(110.0, 90.0, 400.0, 300.0)),
        f(350.0, 30.0, 400.0, 300.0)
    );
    assert!(o.near(Offset {
        dx: 240.2,
        dy: -60.2
    }));
    assert!(!o.near(Offset {
        dx: 241.0,
        dy: -60.0
    }));
    assert_eq!(Offset::ZERO.apply(guest), guest);
}

#[test]
fn the_rules_move_the_host_frame_and_only_the_offset_changes() {
    let guest = f(100.0, 100.0, 400.0, 300.0);
    // Inside the display: unchanged.
    let o = Offset { dx: 50.0, dy: 20.0 };
    assert_eq!(ruled(guest, o, &one(), display()), o);
    // Beyond the right edge: the host frame goes back inside.
    let far = Offset {
        dx: 1200.0,
        dy: 0.0,
    };
    let r = ruled(guest, far, &one(), display());
    assert_eq!(r.apply(guest), f(1040.0, 100.0, 400.0, 300.0));
    // In the gap between two screens: onto the nearest screen.
    let two = ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(1600.0, 0.0, 1440.0, 900.0),
        ],
        main: 0,
        scales: Vec::new(),
    };
    let wide = Display {
        w: 3040.0,
        h: 900.0,
        scale: 2.0,
    };
    let small = f(0.0, 100.0, 140.0, 100.0);
    let into_gap = Offset {
        dx: 1450.0,
        dy: 0.0,
    };
    let r = ruled(small, into_gap, &two, wide);
    assert_eq!(r.apply(small), f(1300.0, 100.0, 140.0, 100.0));
    // The guest frame never changes: the rule is on the host frame only.
    assert_eq!(small, f(0.0, 100.0, 140.0, 100.0));
}

#[test]
fn a_child_takes_the_offset_of_its_anchor_and_follows_it() {
    let mut p = Places::default();
    p.open(1, None, Offset { dx: 300.0, dy: 0.0 });
    p.open(2, Some(1), Offset::ZERO);
    p.open(3, Some(2), Offset::ZERO);
    assert_eq!(p.offset(2), Offset { dx: 300.0, dy: 0.0 });
    assert_eq!(p.offset(3), Offset { dx: 300.0, dy: 0.0 });
    assert_eq!(p.root(3), 1);
    assert!(p.is_child(2) && !p.is_child(1));
    // The person drags window 1: its children go with it.
    assert!(p.set(
        1,
        Offset {
            dx: 500.0,
            dy: 40.0
        }
    ));
    assert_eq!(
        p.offset(3),
        Offset {
            dx: 500.0,
            dy: 40.0
        }
    );
    assert!(!p.set(
        1,
        Offset {
            dx: 500.0,
            dy: 40.0
        }
    ));
    // A set on a child sets its root.
    assert!(p.set(3, Offset { dx: 10.0, dy: 0.0 }));
    assert_eq!(p.offset(1), Offset { dx: 10.0, dy: 0.0 });
    let mut kids = p.anchored(1);
    kids.sort();
    assert_eq!(kids, vec![2, 3]);
}

#[test]
fn a_child_of_a_closed_window_keeps_its_place() {
    let mut p = Places::default();
    p.open(
        1,
        None,
        Offset {
            dx: 200.0,
            dy: 10.0,
        },
    );
    p.open(2, Some(1), Offset::ZERO);
    p.close(1);
    assert!(!p.is_child(2));
    assert_eq!(
        p.offset(2),
        Offset {
            dx: 200.0,
            dy: 10.0
        }
    );
    // An anchor that the map does not know, or the window itself, is no
    // anchor.
    p.open(5, Some(99), Offset::ZERO);
    p.open(6, Some(6), Offset::ZERO);
    assert!(!p.is_child(5) && !p.is_child(6));
    assert_eq!(p.offset(42), Offset::ZERO);
}

#[test]
fn a_cycle_of_anchors_ends() {
    let mut p = Places::default();
    p.open(1, None, Offset::ZERO);
    p.open(2, Some(1), Offset::ZERO);
    // A report that makes 1 a child of 2: the walk stops after DEPTH.
    p.map.get_mut(&1).unwrap().anchor = Some(2);
    let _ = p.root(1);
    let _ = p.offset(2);
}

#[test]
fn a_child_host_frame_stays_inside_the_display() {
    let mut p = Places::default();
    p.open(1, None, Offset { dx: 900.0, dy: 0.0 });
    p.open(2, Some(1), Offset::ZERO);
    // A menu at the right of the parent in the guest: shifted by the
    // offset it would hang off the screen.
    let menu = f(400.0, 200.0, 200.0, 300.0);
    assert_eq!(p.host(2, menu, display()), f(1240.0, 200.0, 200.0, 300.0));
    // The top-level window itself is placed as its offset says.
    let w = f(100.0, 100.0, 400.0, 300.0);
    assert_eq!(p.host(1, w, display()), f(1000.0, 100.0, 400.0, 300.0));
}

#[test]
fn a_menu_without_a_parent_anchors_to_the_window_under_its_corner() {
    let tops = [
        top(10, 500, f(600.0, 100.0, 400.0, 300.0)),
        top(11, 500, f(100.0, 100.0, 400.0, 300.0)),
        top(12, 700, f(150.0, 150.0, 200.0, 200.0)),
    ];
    // The guest names the parent: that one.
    assert_eq!(
        anchor_of(
            20,
            500,
            Level::Popover,
            Some(12),
            f(0.0, 0.0, 9.0, 9.0),
            &tops
        ),
        Some(12)
    );
    // A context menu at a click inside window 11.
    assert_eq!(
        anchor_of(
            20,
            500,
            Level::Menu,
            None,
            f(200.0, 200.0, 150.0, 200.0),
            &tops
        ),
        Some(11)
    );
    // A window of another process under the corner counts after the own
    // process.
    assert_eq!(
        anchor_of(
            20,
            700,
            Level::Tooltip,
            None,
            f(200.0, 200.0, 50.0, 20.0),
            &tops
        ),
        Some(12)
    );
    assert_eq!(
        anchor_of(
            20,
            800,
            Level::Tooltip,
            None,
            f(700.0, 200.0, 50.0, 20.0),
            &tops
        ),
        Some(10)
    );
    // Under no window: the key window, else the front one.
    let mut keyed = tops;
    keyed[1].focused = true;
    assert_eq!(
        anchor_of(
            20,
            500,
            Level::Sheet,
            None,
            f(1300.0, 800.0, 50.0, 20.0),
            &keyed
        ),
        Some(11)
    );
    assert_eq!(
        anchor_of(
            20,
            500,
            Level::Menu,
            None,
            f(1300.0, 800.0, 50.0, 20.0),
            &tops
        ),
        Some(10)
    );
    // A normal or floating window without a parent is a top-level one.
    for level in [Level::Normal, Level::Floating] {
        assert_eq!(
            anchor_of(20, 500, level, None, f(200.0, 200.0, 50.0, 20.0), &tops),
            None
        );
    }
    // A minimized window holds nothing.
    let mut min = tops;
    min[1].minimized = true;
    assert_eq!(
        anchor_of(
            20,
            500,
            Level::Menu,
            None,
            f(200.0, 200.0, 150.0, 200.0),
            &min
        ),
        Some(12)
    );
    assert_eq!(
        anchor_of(20, 500, Level::Menu, None, f(0.0, 0.0, 1.0, 1.0), &[]),
        None
    );
}

fn win(id: u32, level: Level, frame: Frame) -> Window {
    Window {
        id,
        pid: 500,
        app: "com.example.app".into(),
        title: format!("window {id}"),
        frame,
        level,
        movable: true,
        resizable: true,
        ..Window::default()
    }
}

fn event(order: &[u32], changed: Vec<Window>, removed: &[u32]) -> WindowsEvent {
    WindowsEvent {
        order: order.to_vec(),
        changed,
        removed: removed.to_vec(),
    }
}

/// One guest event through the fold and the places, as the session runs
/// them.
fn step(o: &mut Windows, p: &mut Places, ev: &WindowsEvent) -> (Vec<Change>, Vec<u32>) {
    let changes = o.apply(ev);
    let again = p.follow(&changes, o, &one(), display());
    (changes, again)
}

#[test]
fn a_new_top_level_window_opens_where_the_guest_put_it() {
    let mut o = Windows::with_screens(one());
    let mut p = Places::default();
    let a = win(1, Level::Normal, f(100.0, 100.0, 400.0, 300.0));
    step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    assert_eq!(p.offset(1), Offset::ZERO);
    assert_eq!(p.host(1, a.frame, display()), a.frame);
}

#[test]
fn a_guest_move_moves_the_host_window_by_the_guest_delta() {
    let mut o = Windows::with_screens(one());
    let mut p = Places::default();
    let mut a = win(1, Level::Normal, f(100.0, 100.0, 400.0, 300.0));
    step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    // The person dragged the host window 300 points to the right.
    p.set(1, Offset { dx: 300.0, dy: 0.0 });
    // The app moves its own window 20 points down.
    a.frame = f(100.0, 120.0, 400.0, 300.0);
    let (changes, again) = step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    assert!(changes.contains(&Change::Move(1, a.frame)));
    assert!(again.is_empty());
    assert_eq!(p.host(1, a.frame, display()), f(400.0, 120.0, 400.0, 300.0));
}

#[test]
fn a_guest_zoom_keeps_the_host_window_on_the_screen() {
    let mut o = Windows::with_screens(one());
    let mut p = Places::default();
    let mut a = win(1, Level::Normal, f(100.0, 100.0, 400.0, 300.0));
    step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    p.open(2, Some(1), Offset::ZERO);
    p.set(1, Offset { dx: 500.0, dy: 0.0 });
    // The guest zooms the window to its display: shifted by the offset it
    // would be off the screen, so the rule takes the offset back.
    a.frame = f(0.0, 25.0, 1440.0, 875.0);
    let (_, again) = step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    assert_eq!(p.host(1, a.frame, display()), a.frame);
    assert_eq!(p.offset(1), Offset::ZERO);
    // The child of the window goes to its new place too.
    assert_eq!(again, vec![2]);
}

#[test]
fn a_new_window_in_the_screen_gap_goes_onto_a_screen_on_the_host_only() {
    let two = ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(1600.0, 0.0, 1440.0, 900.0),
        ],
        main: 0,
        scales: Vec::new(),
    };
    let wide = Display {
        w: 3040.0,
        h: 900.0,
        scale: 2.0,
    };
    let mut o = Windows::with_screens(two.clone());
    let mut p = Places::default();
    let a = win(1, Level::Normal, f(1445.0, 100.0, 140.0, 100.0));
    let changes = o.apply(&event(&[1], vec![a.clone()], &[]));
    // The fold keeps the guest's frame and asks the guest for nothing.
    assert_eq!(o.get(1).unwrap().frame, a.frame);
    assert!(!changes.iter().any(|c| matches!(c, Change::Refuse { .. })));
    p.follow(&changes, &o, &two, wide);
    assert_eq!(p.host(1, a.frame, wide), f(1300.0, 100.0, 140.0, 100.0));
}

#[test]
fn a_context_menu_opens_at_the_offset_of_the_window_it_belongs_to() {
    let mut o = Windows::with_screens(one());
    let mut p = Places::default();
    let a = win(1, Level::Normal, f(100.0, 100.0, 400.0, 300.0));
    let b = win(2, Level::Normal, f(700.0, 100.0, 400.0, 300.0));
    step(&mut o, &mut p, &event(&[1, 2], vec![a, b], &[]));
    p.set(
        1,
        Offset {
            dx: 200.0,
            dy: 50.0,
        },
    );
    // A right click at 250,250 in window 1: the menu has no parent.
    let menu = win(3, Level::Menu, f(250.0, 250.0, 150.0, 200.0));
    step(&mut o, &mut p, &event(&[3, 1, 2], vec![menu.clone()], &[]));
    assert!(p.is_child(3));
    assert_eq!(
        p.host(3, menu.frame, display()),
        f(450.0, 300.0, 150.0, 200.0)
    );
    // An input method candidate list names its parent (window 2).
    let mut ime = win(4, Level::Popover, f(720.0, 150.0, 200.0, 80.0));
    ime.parent = Some(2);
    step(
        &mut o,
        &mut p,
        &event(&[4, 3, 1, 2], vec![ime.clone()], &[]),
    );
    assert_eq!(p.host(4, ime.frame, display()), ime.frame);
    // The menu closes, then its window: nothing is left.
    step(&mut o, &mut p, &event(&[4, 1, 2], vec![], &[3]));
    step(&mut o, &mut p, &event(&[4, 2], vec![], &[1]));
    assert!(!p.contains(3) && !p.contains(1));
    assert_eq!(p.len(), 2);
}

#[test]
fn a_full_screen_window_keeps_its_offset_for_the_way_back() {
    let mut o = Windows::with_screens(one());
    let mut p = Places::default();
    let mut a = win(1, Level::Normal, f(100.0, 100.0, 400.0, 300.0));
    step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    p.set(1, Offset { dx: 300.0, dy: 0.0 });
    a.full_screen = true;
    a.frame = f(0.0, 0.0, 1440.0, 900.0);
    step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    assert_eq!(p.offset(1), Offset { dx: 300.0, dy: 0.0 });
    a.full_screen = false;
    a.frame = f(100.0, 100.0, 400.0, 300.0);
    step(&mut o, &mut p, &event(&[1], vec![a.clone()], &[]));
    assert_eq!(p.host(1, a.frame, display()), f(400.0, 100.0, 400.0, 300.0));
}
