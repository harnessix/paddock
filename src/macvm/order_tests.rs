//! The fold over the window events: each change, a child before its
//! parent, the rebuild after a new hello, the host focus without a
//! duplicate, the Space change and the refusals.

use super::*;
use crate::macvm::crop::ScreenSet;

fn main() -> Rect {
    Rect::new(0.0, 0.0, 1440.0, 900.0)
}

fn win(id: u32, level: Level, frame: Frame) -> Window {
    Window {
        id,
        pid: 500,
        app: "com.example.app".into(),
        title: format!("window {id}"),
        frame,
        level,
        parent: None,
        minimized: false,
        focused: false,
        resizable: true,
        movable: true,
        ..Window::default()
    }
}

fn normal(id: u32) -> Window {
    win(id, Level::Normal, Frame::new(100.0, 100.0, 400.0, 300.0))
}

fn event(order: &[u32], changed: Vec<Window>, removed: &[u32]) -> WindowsEvent {
    WindowsEvent {
        order: order.to_vec(),
        changed,
        removed: removed.to_vec(),
    }
}

/// Two normal windows, 1 in front and focused.
fn two() -> Windows {
    let mut w = Windows::new(main());
    let mut a = normal(1);
    a.focused = true;
    let b = normal(2);
    let changes = w.apply(&event(&[1, 2], vec![a, b], &[]));
    assert_eq!(changes.len(), 4, "{changes:?}");
    assert_eq!(w.ids(), &[1, 2]);
    assert_eq!(w.guest_focus(), Some(1));
    w
}

#[test]
fn apply_each_change() {
    let mut w = Windows::new(main());
    // Open: a new window, then its order.
    let a = normal(1);
    assert_eq!(
        w.apply(&event(&[1], vec![a.clone()], &[])),
        vec![Change::Open(a.clone()), Change::Reorder(vec![1])]
    );
    assert_eq!(w.get(1).unwrap().frame, a.frame);
    assert_eq!(w.len(), 1);
    // The same window again: nothing.
    assert_eq!(w.apply(&event(&[1], vec![a.clone()], &[])), vec![]);
    // Move.
    let mut moved = a.clone();
    moved.frame = Frame::new(200.0, 100.0, 400.0, 300.0);
    assert_eq!(
        w.apply(&event(&[1], vec![moved.clone()], &[])),
        vec![Change::Move(1, moved.frame)]
    );
    // A move under half a point is no move.
    let mut tiny = moved.clone();
    tiny.frame.x += 0.25;
    assert_eq!(w.apply(&event(&[1], vec![tiny], &[])), vec![]);
    // Retitle.
    let mut titled = moved.clone();
    titled.title = "Untitled 2".into();
    assert_eq!(
        w.apply(&event(&[1], vec![titled.clone()], &[])),
        vec![Change::Retitle(1, "Untitled 2".into())]
    );
    assert_eq!(w.titles(), vec![(1, "Untitled 2".to_string())]);
    // Minimize and restore.
    let mut mini = titled.clone();
    mini.minimized = true;
    assert_eq!(
        w.apply(&event(&[1], vec![mini.clone()], &[])),
        vec![Change::Minimize(1)]
    );
    assert_eq!(
        w.apply(&event(&[1], vec![titled.clone()], &[])),
        vec![Change::Restore(1)]
    );
    // Focus from the flag.
    let mut focused = titled.clone();
    focused.focused = true;
    assert_eq!(
        w.apply(&event(&[1], vec![focused.clone()], &[])),
        vec![Change::Focus(1)]
    );
    assert_eq!(w.guest_focus(), Some(1));
    // Reorder: a second window behind, then in front.
    let b = normal(2);
    assert_eq!(
        w.apply(&event(&[1, 2], vec![b.clone()], &[])),
        vec![Change::Open(b.clone()), Change::Reorder(vec![1, 2])]
    );
    assert_eq!(
        w.apply(&event(&[2, 1], vec![], &[])),
        vec![Change::Reorder(vec![2, 1])]
    );
    // An order that lacks a known id keeps that id at the end.
    assert_eq!(
        w.apply(&event(&[1], vec![], &[])),
        vec![Change::Reorder(vec![1, 2])]
    );
    // Close: the removed id, and an unknown id is nothing.
    assert_eq!(
        w.apply(&event(&[1], vec![], &[2, 99])),
        vec![Change::Close(2)]
    );
    assert_eq!(w.ids(), &[1]);
    // A level change is a close and an open under the same id.
    let mut floating = focused.clone();
    floating.level = Level::Floating;
    assert_eq!(
        w.apply(&event(&[1], vec![floating.clone()], &[])),
        vec![Change::Close(1), Change::Open(floating.clone())]
    );
    assert_eq!(w.get(1).unwrap().level, Level::Floating);
    // The last window goes: empty.
    assert_eq!(w.apply(&event(&[], vec![], &[1])), vec![Change::Close(1)]);
    assert!(w.is_empty());
    assert_eq!(w.apply(&event(&[], vec![], &[])), vec![]);
}

#[test]
fn child_before_parent() {
    let mut w = Windows::new(main());
    let parent = normal(10);
    let mut sheet = win(11, Level::Sheet, Frame::new(150.0, 120.0, 300.0, 100.0));
    sheet.parent = Some(10);
    let mut popover = win(12, Level::Popover, Frame::new(160.0, 130.0, 100.0, 50.0));
    popover.parent = Some(11);
    // The children come first in the event, the parent last.
    let changes = w.apply(&event(
        &[12, 11, 10],
        vec![popover.clone(), sheet.clone(), parent.clone()],
        &[],
    ));
    assert_eq!(
        changes,
        vec![
            Change::Open(parent),
            Change::Open(sheet.clone()),
            Change::Open(popover),
            Change::Reorder(vec![12, 11, 10]),
        ]
    );
    // A child of a parent the host already has opens at once.
    let mut tip = win(13, Level::Tooltip, Frame::new(0.0, 0.0, 50.0, 20.0));
    tip.parent = Some(10);
    let changes = w.apply(&event(&[13, 12, 11, 10], vec![tip.clone()], &[]));
    assert_eq!(changes[0], Change::Open(tip));
    // A child whose parent names itself does not wait forever.
    let mut odd = win(14, Level::Popover, Frame::new(0.0, 0.0, 50.0, 20.0));
    odd.parent = Some(14);
    let changes = w.apply(&event(&[14, 13, 12, 11, 10], vec![odd.clone()], &[]));
    assert_eq!(changes[0], Change::Open(odd));
    // Two windows that name each other as parent: both open.
    let mut x = win(20, Level::Popover, Frame::new(0.0, 0.0, 50.0, 20.0));
    x.parent = Some(21);
    let mut y = win(21, Level::Popover, Frame::new(0.0, 0.0, 50.0, 20.0));
    y.parent = Some(20);
    let changes = w.apply(&event(&[20, 21], vec![x, y], &[]));
    assert_eq!(
        changes
            .iter()
            .filter(|c| matches!(c, Change::Open(_)))
            .count(),
        2
    );
}

#[test]
fn rebuild_keeps_matching_windows() {
    let mut w = two();
    let mut a = normal(1);
    a.focused = true;
    let b = normal(2);
    // A list equal to the map: no change.
    assert_eq!(
        w.rebuild(&event(&[1, 2], vec![a.clone(), b.clone()], &[])),
        vec![]
    );
    // One window gone: one close, nothing else.
    assert_eq!(
        w.rebuild(&event(&[1], vec![a.clone()], &[])),
        vec![Change::Close(2)]
    );
    assert_eq!(w.ids(), &[1]);
    // A moved window: a move, not a close and an open.
    let mut moved = a.clone();
    moved.frame = Frame::new(300.0, 100.0, 400.0, 300.0);
    assert_eq!(
        w.rebuild(&event(&[1], vec![moved.clone()], &[])),
        vec![Change::Move(1, moved.frame)]
    );
    // A new window in the list opens.
    let c = normal(3);
    assert_eq!(
        w.rebuild(&event(&[3, 1], vec![c.clone(), moved.clone()], &[])),
        vec![Change::Open(c), Change::Reorder(vec![3, 1])]
    );
    // An empty list after a restart closes every window.
    let changes = w.rebuild(&event(&[], vec![], &[]));
    assert_eq!(changes, vec![Change::Close(1), Change::Close(3)]);
    assert!(w.is_empty());
}

#[test]
fn host_focus_no_duplicate() {
    let mut w = two();
    // The guest focus is already on 1: a click on 1 sends nothing.
    assert_eq!(w.on_host_focus(1), vec![]);
    // Cmd-Tab to 2: one request, and a second become-key before the
    // guest answers sends nothing more.
    assert_eq!(w.on_host_focus(2), vec![Request::WindowFocus { id: 2 }]);
    assert_eq!(w.on_host_focus(2), vec![]);
    // The guest answers: the host window 2 is key already, so the
    // change is still Focus (the host checks isKeyWindow), and the
    // guest focus moved.
    assert_eq!(w.on_guest_focus(Some(2)), Some(Change::Focus(2)));
    assert_eq!(w.guest_focus(), Some(2));
    assert!(w.get(2).unwrap().focused);
    assert!(!w.get(1).unwrap().focused);
    // The same focus again: nothing.
    assert_eq!(w.on_guest_focus(Some(2)), None);
    // Now a click on 2 sends nothing, one on 1 sends one.
    assert_eq!(w.on_host_focus(2), vec![]);
    assert_eq!(w.on_host_focus(1), vec![Request::WindowFocus { id: 1 }]);
    // The guest focus goes to another app: no host change, the next
    // click on 1 asks again.
    assert_eq!(w.on_guest_focus(None), None);
    assert_eq!(w.on_host_focus(1), vec![Request::WindowFocus { id: 1 }]);
    // An unknown window sends nothing.
    assert_eq!(w.on_host_focus(99), vec![]);
    assert_eq!(w.on_guest_focus(Some(99)), None);
}

#[test]
fn host_minimize_goes_to_the_guest_once() {
    let mut w = two();
    assert_eq!(
        w.on_host_minimize(1, true),
        vec![Request::WindowMinimize { id: 1 }]
    );
    // The guest reports it minimized: no host change, no second request.
    let mut a = normal(1);
    a.minimized = true;
    assert_eq!(w.apply(&event(&[1, 2], vec![a], &[])), vec![]);
    assert_eq!(w.on_host_minimize(1, true), vec![]);
    assert_eq!(
        w.on_host_minimize(1, false),
        vec![Request::WindowRaise { id: 1 }]
    );
    assert_eq!(w.on_host_minimize(99, false), vec![]);
}

#[test]
fn space_change_raises_active() {
    let mut w = two();
    let c = normal(3);
    let mut sheet = win(4, Level::Sheet, Frame::new(0.0, 0.0, 100.0, 50.0));
    sheet.parent = Some(3);
    w.apply(&event(&[1, 2, 3, 4], vec![c, sheet], &[]));
    // Windows 1 and 3 are on the active Space, 2 is on another one: the
    // active top-level windows are raised back to front, 3 then 1, so
    // 1 ends on top in the guest as on the host. The sheet follows its
    // parent and is not raised itself.
    assert_eq!(
        w.on_space_change(&[1, 3, 4]),
        vec![
            Request::WindowRaise { id: 3 },
            Request::WindowRaise { id: 1 }
        ]
    );
    assert!(w.get(1).unwrap().space_active);
    assert!(!w.get(2).unwrap().space_active);
    assert!(w.get(3).unwrap().space_active);
    // Only 2 active: one raise.
    assert_eq!(
        w.on_space_change(&[2]),
        vec![Request::WindowRaise { id: 2 }]
    );
    // Nothing active, or unknown ids: nothing.
    assert_eq!(w.on_space_change(&[]), vec![]);
    assert_eq!(w.on_space_change(&[77]), vec![]);
}

/// The Spaces case: two windows, one on the active Space.
/// The raise names that one only, and nothing changes for the other.
#[test]
fn spaces_raise_active_only() {
    let mut w = two();
    assert_eq!(
        w.on_space_change(&[2]),
        vec![Request::WindowRaise { id: 2 }]
    );
    assert!(!w.get(1).unwrap().space_active);
    assert!(w.get(2).unwrap().space_active);
    // The host's order is untouched: the raise is for the guest.
    assert_eq!(w.ids(), &[1, 2]);
    assert_eq!(
        w.on_space_change(&[1]),
        vec![Request::WindowRaise { id: 1 }]
    );
}

/// A window in the gap between two screens opens on the nearest screen
/// and the guest hears about it; a move into the gap the same; a child
/// window follows its parent and is never corrected.
#[test]
fn gap_window_is_corrected() {
    let screens = ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(1600.0, -90.0, 1920.0, 1080.0),
        ],
        main: 0,
        scales: vec![2.0, 1.0],
    };
    let mut w = Windows::with_screens(screens);
    let mut a = win(1, Level::Normal, Frame::new(1445.0, 100.0, 140.0, 100.0));
    let changes = w.apply(&event(&[1], vec![a.clone()], &[]));
    let mut shown = a.clone();
    shown.frame = Frame::new(1300.0, 100.0, 140.0, 100.0);
    assert_eq!(
        changes,
        vec![
            Change::Open(shown),
            Change::Correct(1, Frame::new(1300.0, 100.0, 140.0, 100.0)),
            Change::Reorder(vec![1]),
        ]
    );
    assert_eq!(
        w.get(1).unwrap().frame,
        Frame::new(1300.0, 100.0, 140.0, 100.0)
    );
    // The guest followed the correction: no further change.
    a.frame = Frame::new(1300.0, 100.0, 140.0, 100.0);
    assert_eq!(w.apply(&event(&[1], vec![a.clone()], &[])), vec![]);
    // A move into the gap: the host moves it to the edge and corrects.
    a.frame = Frame::new(1470.0, 100.0, 120.0, 100.0);
    assert_eq!(
        w.apply(&event(&[1], vec![a.clone()], &[])),
        vec![
            Change::Move(1, Frame::new(1600.0, 100.0, 120.0, 100.0)),
            Change::Correct(1, Frame::new(1600.0, 100.0, 120.0, 100.0)),
        ]
    );
    // A popover in the gap under the window stays where the guest put it.
    let mut pop = win(2, Level::Popover, Frame::new(1450.0, 150.0, 100.0, 40.0));
    pop.parent = Some(1);
    assert_eq!(
        w.apply(&event(&[2, 1], vec![pop.clone()], &[])),
        vec![Change::Open(pop), Change::Reorder(vec![2, 1])]
    );
    assert_eq!(
        Change::Correct(1, Frame::new(1.0, 2.0, 3.0, 4.0)).kind(),
        "correct 1 to 1,2 3x4"
    );
}

#[test]
fn refuse_hidden_level_sends_move_back() {
    let mut w = two();
    let before = w.get(1).unwrap().frame;
    // Window 1 goes full screen: the host window closes, and the change
    // carries the last frame to send back.
    let mut full = normal(1);
    full.level = Level::Hidden;
    full.frame = Frame::new(0.0, 0.0, 1440.0, 900.0);
    assert_eq!(
        w.apply(&event(&[1, 2], vec![full.clone()], &[])),
        vec![
            Change::Close(1),
            Change::Refuse {
                id: 1,
                reason: Reason::FullScreen,
                back: Some(before),
            },
            Change::Reorder(vec![2]),
        ]
    );
    assert!(w.get(1).is_none());
    // A window the host never showed goes full screen: no frame to go
    // back to, and no host window.
    let mut new_full = normal(5);
    new_full.level = Level::Hidden;
    assert_eq!(
        w.apply(&event(&[5, 2], vec![new_full], &[])),
        vec![Change::Refuse {
            id: 5,
            reason: Reason::FullScreen,
            back: None,
        }]
    );
    // Back at its frame: it opens again.
    let changes = w.apply(&event(&[1, 2], vec![normal(1)], &[]));
    assert_eq!(changes[0], Change::Open(normal(1)));
}

#[test]
fn refuse_oversize_keeps_the_last_frame() {
    let mut w = Windows::new(main());
    // A new floating window larger than the main screen: no host window,
    // and the frame to send back is the largest allowed one.
    let big = win(7, Level::Floating, Frame::new(10.0, 10.0, 2000.0, 1000.0));
    assert_eq!(
        w.apply(&event(&[7], vec![big.clone()], &[])),
        vec![Change::Refuse {
            id: 7,
            reason: Reason::TooLarge(Level::Floating),
            back: Some(Frame::new(10.0, 10.0, 1440.0, 900.0)),
        }]
    );
    assert!(w.is_empty());
    // A known menu that grows past a quarter of the screen: the host
    // window stays at its old frame, the title still follows.
    let menu = win(8, Level::Menu, Frame::new(50.0, 50.0, 200.0, 300.0));
    let changes = w.apply(&event(&[8], vec![menu.clone()], &[]));
    assert_eq!(changes[0], Change::Open(menu.clone()));
    let mut grown = menu.clone();
    grown.frame = Frame::new(50.0, 50.0, 1000.0, 800.0);
    grown.title = "big".into();
    assert_eq!(
        w.apply(&event(&[8], vec![grown], &[])),
        vec![
            Change::Retitle(8, "big".into()),
            Change::Refuse {
                id: 8,
                reason: Reason::TooLarge(Level::Menu),
                back: Some(menu.frame),
            },
        ]
    );
    assert_eq!(w.get(8).unwrap().frame, menu.frame);
    // A normal window of any size passes.
    let huge = win(9, Level::Normal, Frame::new(0.0, 0.0, 5000.0, 4000.0));
    assert_eq!(
        w.apply(&event(&[9, 8], vec![huge.clone()], &[])),
        vec![Change::Open(huge), Change::Reorder(vec![9, 8])]
    );
}

#[test]
fn change_kind_has_the_id_and_no_title() {
    let w = normal(3);
    assert_eq!(Change::Open(w).kind(), "open 3 Normal");
    assert_eq!(Change::Close(3).kind(), "close 3");
    assert_eq!(
        Change::Move(3, Frame::new(1.0, 2.0, 3.0, 4.0)).kind(),
        "move 3 to 1,2 3x4"
    );
    assert_eq!(Change::Retitle(3, "secret".into()).kind(), "retitle 3");
    assert_eq!(Change::Reorder(vec![3, 1]).kind(), "reorder [3, 1]");
    assert_eq!(Change::Minimize(3).kind(), "minimize 3");
    assert_eq!(Change::Restore(3).kind(), "restore 3");
    assert_eq!(Change::Focus(3).kind(), "focus 3");
    let r = Change::Refuse {
        id: 3,
        reason: Reason::TooLarge(Level::Menu),
        back: None,
    };
    assert_eq!(r.kind(), "refuse 3 TooLarge(Menu)");
}

fn full_screen(mut w: Window, on: bool) -> Window {
    w.full_screen = on;
    if on {
        w.frame = Frame::new(0.0, 0.0, 1440.0, 900.0);
    }
    w
}

#[test]
fn a_window_enters_full_screen_before_its_move_and_leaves_it_before_the_move_back() {
    let mut m = Windows::new(main());
    m.apply(&event(&[1], vec![normal(1)], &[]));
    let on = m.apply(&event(&[1], vec![full_screen(normal(1), true)], &[]));
    assert_eq!(
        on,
        vec![
            Change::FullScreen(1, true),
            Change::Move(1, Frame::new(0.0, 0.0, 1440.0, 900.0)),
        ]
    );
    assert!(m.get(1).unwrap().full_screen);
    // The same report again: nothing.
    assert!(m
        .apply(&event(&[1], vec![full_screen(normal(1), true)], &[]))
        .is_empty());
    let off = m.apply(&event(&[1], vec![normal(1)], &[]));
    assert_eq!(
        off,
        vec![
            Change::FullScreen(1, false),
            Change::Move(1, Frame::new(100.0, 100.0, 400.0, 300.0)),
        ]
    );
    assert!(!m.get(1).unwrap().full_screen);
}

#[test]
fn a_window_that_opens_in_full_screen_opens_with_the_flag_and_no_refusal() {
    let mut m = Windows::new(main());
    let out = m.apply(&event(&[1], vec![full_screen(normal(1), true)], &[]));
    assert!(matches!(&out[0], Change::Open(w) if w.full_screen));
    assert!(!out.iter().any(|c| matches!(c, Change::Refuse { .. })));
    assert!(m.get(1).unwrap().full_screen);
}

#[test]
fn only_a_top_level_normal_window_takes_the_full_screen() {
    let mut m = Windows::new(main());
    m.apply(&event(&[1], vec![normal(1)], &[]));
    let mut sheet = win(2, Level::Sheet, Frame::new(120.0, 120.0, 300.0, 200.0));
    sheet.parent = Some(1);
    sheet.full_screen = true;
    let out = m.apply(&event(&[2, 1], vec![sheet], &[]));
    assert!(!out.iter().any(|c| matches!(c, Change::FullScreen(..))));
    assert!(!m.get(2).unwrap().full_screen);
}

#[test]
fn a_full_screen_window_in_the_screen_gap_is_not_corrected() {
    // The display covers two screens and the gap: a full screen window
    // covers the display, gap included.
    let screens = ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            Rect::new(1200.0, 0.0, 1000.0, 800.0),
        ],
        main: 0,
        scales: Vec::new(),
    };
    let mut m = Windows::with_screens(screens);
    let mut w = normal(1);
    w.full_screen = true;
    w.frame = Frame::new(1010.0, 0.0, 180.0, 800.0);
    let out = m.apply(&event(&[1], vec![w], &[]));
    assert!(!out.iter().any(|c| matches!(c, Change::Correct(..))));
}

#[test]
fn a_new_title_bar_or_radius_is_a_chrome_change() {
    let mut m = Windows::new(main());
    m.apply(&event(&[1], vec![normal(1)], &[]));
    assert_eq!(m.get(1).unwrap().chrome.title_bar, TITLE_BAR);
    let mut w = normal(1);
    w.title_bar = Some(52.0);
    w.radius = Some(16.0);
    w.controls = vec![Frame::new(20.0, 18.0, 54.0, 16.0)];
    let out = m.apply(&event(&[1], vec![w.clone()], &[]));
    assert_eq!(out, vec![Change::Chrome(1, chrome_of(&w))]);
    let c = &m.get(1).unwrap().chrome;
    assert_eq!((c.title_bar, c.radius), (52.0, Some(16.0)));
    // Unchanged: nothing.
    assert!(m.apply(&event(&[1], vec![w], &[])).is_empty());
}

#[test]
fn the_title_bar_is_never_taller_than_the_window() {
    let mut w = normal(1);
    w.frame.h = 20.0;
    w.title_bar = Some(52.0);
    assert_eq!(chrome_of(&w).title_bar, 20.0);
    w.title_bar = Some(-3.0);
    assert_eq!(chrome_of(&w).title_bar, 0.0);
}
