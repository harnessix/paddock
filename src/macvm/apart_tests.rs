use super::*;

/// A 13-inch Mac screen of 1512x982 points with as much room again to the
/// right.
fn room() -> Room {
    Room {
        w: 3024.0,
        h: 982.0,
        top: MENU_BAR,
        screens_w: 1512.0,
    }
}

fn item(id: u32, frame: Frame) -> Item {
    Item {
        id,
        frame,
        movable: true,
        fresh: false,
        key: false,
        recent: 0,
    }
}

fn after(items: &[Item], moves: &[(u32, Frame)]) -> Vec<Frame> {
    items
        .iter()
        .map(|i| {
            moves
                .iter()
                .find(|(id, _)| *id == i.id)
                .map_or(i.frame, |(_, f)| *f)
        })
        .collect()
}

fn disjoint(frames: &[Frame]) -> bool {
    for (i, a) in frames.iter().enumerate() {
        for b in &frames[i + 1..] {
            if overlap(*a, *b) {
                return false;
            }
        }
    }
    true
}

fn inside(f: Frame, r: Room) -> bool {
    f.x >= 0.0 && f.y >= r.top - EPS && f.x + f.w <= r.w + EPS && f.y + f.h <= r.h + EPS
}

#[test]
fn two_sample_windows_at_one_frame_go_apart() {
    // The case measured on a Mac: both windows at 329,178 1390x760.
    let f = Frame::new(329.0, 178.0, 1390.0, 760.0);
    let mut a = item(122, f);
    a.recent = 1;
    let mut b = item(125, f);
    b.fresh = true;
    b.key = true;
    b.recent = 2;
    let items = [a, b];
    let moves = plan(&items, room());
    let frames = after(&items, &moves);
    assert!(disjoint(&frames), "{frames:?}");
    assert!(frames.iter().all(|f| inside(*f, room())), "{frames:?}");
    // The sizes stay. Next to each other they do not fit while the old
    // window keeps its place, so both go, the key window first.
    assert_eq!((frames[1].w, frames[1].h), (1390.0, 760.0));
    assert_eq!(frames[1], Frame::new(0.0, MENU_BAR, 1390.0, 760.0));
    assert_eq!(frames[0].x, 1390.0 + GAP);
}

#[test]
fn three_stacked_windows_go_apart_with_the_gap() {
    let f = Frame::new(200.0, 100.0, 600.0, 400.0);
    let items: Vec<Item> = (1..=3)
        .map(|id| Item {
            fresh: true,
            recent: id as u64,
            key: id == 3,
            ..item(id, f)
        })
        .collect();
    let moves = plan(&items, room());
    let frames = after(&items, &moves);
    assert!(disjoint(&frames), "{frames:?}");
    // The key window stays where it is.
    assert_eq!(frames[2], f);
    // The others keep the gap from it.
    for g in &frames[..2] {
        assert!(!overlap(*g, f));
    }
}

#[test]
fn a_window_that_overlaps_nothing_stays() {
    let items = [
        Item {
            fresh: true,
            ..item(1, Frame::new(0.0, 30.0, 400.0, 300.0))
        },
        item(2, Frame::new(500.0, 30.0, 400.0, 300.0)),
    ];
    assert!(plan(&items, room()).is_empty());
}

#[test]
fn the_fresh_window_moves_and_the_old_one_stays() {
    let old = Frame::new(100.0, 100.0, 500.0, 400.0);
    let new = Frame::new(300.0, 200.0, 500.0, 400.0);
    let items = [
        Item {
            key: true,
            recent: 5,
            ..item(1, old)
        },
        Item {
            fresh: true,
            ..item(2, new)
        },
    ];
    let moves = plan(&items, room());
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].0, 2);
    assert!(!overlap(moves[0].1, old));
}

#[test]
fn a_window_that_cannot_move_stays_and_the_others_go_around() {
    let fixed = Frame::new(100.0, 100.0, 500.0, 400.0);
    let items = [
        Item {
            movable: false,
            fresh: true,
            ..item(1, fixed)
        },
        Item {
            fresh: true,
            ..item(2, fixed)
        },
    ];
    let moves = plan(&items, room());
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0].0, 2);
    assert!(!overlap(moves[0].1, fixed));
}

#[test]
fn without_room_the_least_recent_window_is_covered_never_the_key_one() {
    // Three windows of 1000x900 fit twice in the room, not three times.
    let f = Frame::new(0.0, 30.0, 1000.0, 900.0);
    let key = Item {
        key: true,
        recent: 9,
        ..item(1, f)
    };
    let second = Item {
        recent: 8,
        ..item(2, Frame::new(1100.0, 30.0, 1000.0, 900.0))
    };
    let new = Item {
        fresh: true,
        recent: 1,
        ..item(3, Frame::new(50.0, 40.0, 1000.0, 900.0))
    };
    let items = [key, second, new];
    let moves = plan(&items, room());
    let frames = after(&items, &moves);
    // Nothing covers the key window, and the second window is clear of
    // the key window too: only the least recent one may be covered.
    assert!(!overlap(frames[2], frames[0]), "{frames:?}");
    assert!(!overlap(frames[1], frames[0]), "{frames:?}");
    assert!(frames.iter().all(|f| inside(*f, room())), "{frames:?}");
    assert_eq!(frames[0].w, f.w);
}

#[test]
fn a_window_wider_than_the_screens_gets_their_width() {
    // The guest's zoom fills the whole guest display.
    let zoomed = Frame::new(0.0, 25.0, 3024.0, 957.0);
    let items = [Item {
        fresh: true,
        ..item(1, zoomed)
    }];
    let moves = plan(&items, room());
    assert_eq!(moves, vec![(1, Frame::new(0.0, 25.0, 1512.0, 957.0))]);
    assert_eq!(
        fit(Frame::new(5.0, 30.0, 400.0, 300.0), room()),
        Frame::new(5.0, 30.0, 400.0, 300.0)
    );
}

#[test]
fn free_place_is_the_nearest_inside_the_room() {
    let others = [Frame::new(0.0, 25.0, 700.0, 500.0)];
    let f = free_place(Frame::new(10.0, 30.0, 600.0, 400.0), &others, room(), GAP).unwrap();
    assert!(!overlap(
        f,
        Frame::new(-GAP, 25.0 - GAP, 700.0 + 2.0 * GAP, 500.0 + 2.0 * GAP)
    ));
    assert!(inside(f, room()));
    // Below the other window: nearer than right of it.
    assert_eq!(f, Frame::new(0.0, 549.0, 600.0, 400.0));
    // A window as large as the room has no free place next to another one.
    let big = Frame::new(0.0, 25.0, 3024.0, 957.0);
    assert_eq!(free_place(big, &others, room(), 0.0), None);
}

fn seen(id: u32, frame: Frame, key: bool) -> Seen {
    Seen {
        id,
        frame,
        movable: true,
        key,
    }
}

#[test]
fn tick_waits_until_the_frames_settle() {
    let t0 = Instant::now();
    let f = Frame::new(200.0, 100.0, 600.0, 400.0);
    let mut a = Apart::default();
    let ws = [seen(1, f, false), seen(2, f, true)];
    assert!(a.tick(&ws, room(), t0, false).is_empty());
    assert!(a.tick(&ws, room(), t0 + SETTLE / 2, false).is_empty());
    // A lead (a host drag or resize) holds the layout.
    assert!(a.tick(&ws, room(), t0 + SETTLE, true).is_empty());
    let moves = a.tick(&ws, room(), t0 + SETTLE, false);
    assert_eq!(moves.len(), 1, "{moves:?}");
    // The key window stays, the other one goes.
    assert_eq!(moves[0].0, 1);
    assert!(a.waits(1));
    // Nothing more while the move waits.
    assert!(a.tick(&ws, room(), t0 + SETTLE * 2, false).is_empty());
}

#[test]
fn the_answer_to_a_layout_move_keeps_the_host_window() {
    let t0 = Instant::now();
    let f = Frame::new(200.0, 100.0, 600.0, 400.0);
    let mut a = Apart::default();
    let ws = [seen(1, f, false), seen(2, f, true)];
    a.tick(&ws, room(), t0, false);
    let moves = a.tick(&ws, room(), t0 + SETTLE, false);
    let (id, target) = moves[0];
    let t1 = t0 + SETTLE + Duration::from_millis(30);
    // An app's own move of another window is no layout move.
    assert!(!a.layout_move(2, Frame::new(0.0, 30.0, 600.0, 400.0), t1));
    // The guest's frame of the moved window is.
    assert!(a.layout_move(id, target, t1));
    assert!(!a.waits(id));
    // And then the window is not fresh: no second layout.
    let ws = [seen(1, target, false), seen(2, f, true)];
    assert!(a.tick(&ws, room(), t1 + SETTLE, false).is_empty());
    // A later move of the app is the app's own again.
    assert!(!a.layout_move(id, Frame::new(0.0, 30.0, 600.0, 400.0), t1 + SETTLE));
}

#[test]
fn a_reply_moves_the_view_and_a_late_one_does_not() {
    let t0 = Instant::now();
    let f = Frame::new(200.0, 100.0, 600.0, 400.0);
    let mut a = Apart::default();
    let ws = [seen(1, f, false), seen(2, f, true)];
    a.tick(&ws, room(), t0, false);
    let (id, target) = a.tick(&ws, room(), t0 + SETTLE, false)[0];
    assert!(a.answered(id, Some(target), t0 + SETTLE));
    // The windows event after the reply is no layout move any more, and
    // the session finds the host frame unchanged by then.
    assert!(!a.layout_move(id, target, t0 + SETTLE));
    // A move after the wait counts as the app's own.
    let mut b = Apart::default();
    b.tick(&ws, room(), t0, false);
    let (id, target) = b.tick(&ws, room(), t0 + SETTLE, false)[0];
    assert!(!b.layout_move(id, target, t0 + SETTLE + PENDING * 2));
    // An error ends the wait.
    let mut c = Apart::default();
    c.tick(&ws, room(), t0, false);
    let (id, _) = c.tick(&ws, room(), t0 + SETTLE, false)[0];
    assert!(!c.answered(id, None, t0 + SETTLE));
    assert!(!c.waits(id));
}

#[test]
fn a_guest_that_keeps_the_window_ends_the_layout() {
    let t0 = Instant::now();
    let f = Frame::new(200.0, 100.0, 600.0, 400.0);
    let mut a = Apart::default();
    let ws = [seen(1, f, false), seen(2, f, true)];
    a.tick(&ws, room(), t0, false);
    let mut t = t0 + SETTLE;
    let mut sent = 0;
    for _ in 0..6 {
        let moves = a.tick(&ws, room(), t, false);
        for (id, _) in &moves {
            // The guest refuses: the window stays at f.
            assert!(!a.answered(*id, None, t));
            sent += 1;
        }
        t += SETTLE;
    }
    assert!(sent <= TRIES as usize, "{sent} moves");
}

#[test]
fn a_fit_move_lets_the_host_window_follow() {
    let t0 = Instant::now();
    let zoomed = Frame::new(0.0, 25.0, 3024.0, 957.0);
    let mut a = Apart::default();
    let ws = [seen(1, zoomed, true)];
    a.tick(&ws, room(), t0, false);
    let moves = a.tick(&ws, room(), t0 + SETTLE, false);
    assert_eq!(moves, vec![(1, Frame::new(0.0, 25.0, 1512.0, 957.0))]);
    // The host window follows the new size as for an app's move.
    assert!(!a.layout_move(1, moves[0].1, t0 + SETTLE));
    assert!(!a.waits(1));
}

#[test]
fn the_focus_order_comes_from_the_key_window() {
    let t0 = Instant::now();
    let f = Frame::new(0.0, 30.0, 1000.0, 900.0);
    let g = Frame::new(1100.0, 30.0, 1000.0, 900.0);
    let mut a = Apart::default();
    // Window 2 was key, then window 1.
    a.tick(&[seen(1, f, false), seen(2, g, true)], room(), t0, false);
    a.tick(&[seen(1, f, true), seen(2, g, false)], room(), t0, false);
    assert!(a.recent[&1] > a.recent[&2]);
    a.clear();
    assert!(a.recent.is_empty());
}

#[test]
fn a_held_layout_waits_for_go() {
    let t0 = Instant::now();
    let f = Frame::new(200.0, 100.0, 600.0, 400.0);
    let mut a = Apart::default();
    let ws = [seen(1, f, false), seen(2, f, true)];
    a.hold(true);
    a.tick(&ws, room(), t0, false);
    assert!(a.tick(&ws, room(), t0 + SETTLE * 4, false).is_empty());
    a.hold(false);
    assert_eq!(a.tick(&ws, room(), t0 + SETTLE * 4, false).len(), 1);
}
