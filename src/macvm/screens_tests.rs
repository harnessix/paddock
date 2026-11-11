//! The screen change fold of `screens.rs`.

use super::*;

fn two() -> ScreenSet {
    ScreenSet {
        screens: vec![
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Rect::new(1440.0, 0.0, 1920.0, 1080.0),
        ],
        main: 0,
        scales: vec![2.0, 2.0],
    }
}

fn main_only() -> ScreenSet {
    ScreenSet {
        screens: vec![Rect::new(0.0, 0.0, 1440.0, 900.0)],
        main: 0,
        scales: vec![2.0],
    }
}

fn shown(id: u32, host: Rect, old: &ScreenSet) -> Shown {
    Shown {
        id,
        frame: crop::guest_frame(host, old),
        level: Level::Normal,
        child: false,
        minimized: false,
    }
}

#[test]
fn a_window_on_the_removed_screen_comes_to_the_main_screen() {
    let old = two();
    let new = main_only();
    let display = old.display(2.0);
    let on_second = shown(7, Rect::new(2000.0, 100.0, 800.0, 600.0), &old);
    let on_main = shown(8, Rect::new(100.0, 100.0, 400.0, 300.0), &old);
    let r = rescue(&[on_second, on_main], &old, &new, display);
    assert!(r.badge, "the union shrank: restart for full size");
    assert_eq!(r.moves.len(), 1, "{r:?}");
    let (id, frame) = r.moves[0];
    assert_eq!(id, 7);
    // The new frame is wholly on the main screen, in the map of the start.
    let host = crop::host_frame(frame, &old);
    assert_eq!(new.visible_area(host), host.area());
    assert_eq!((frame.w, frame.h), (800.0, 600.0));
}

#[test]
fn children_minimized_and_menu_windows_stay() {
    let old = two();
    let new = main_only();
    let host = Rect::new(2000.0, 100.0, 300.0, 200.0);
    let mut sheet = shown(1, host, &old);
    sheet.child = true;
    let mut mini = shown(2, host, &old);
    mini.minimized = true;
    let mut menu = shown(3, host, &old);
    menu.level = Level::Menu;
    let r = rescue(&[sheet, mini, menu], &old, &new, old.display(2.0));
    assert!(r.moves.is_empty(), "{r:?}");
}

#[test]
fn the_same_screens_change_nothing() {
    let old = two();
    let w = shown(7, Rect::new(2000.0, 100.0, 800.0, 600.0), &old);
    assert_eq!(
        rescue(&[w], &old, &old.clone(), old.display(2.0)),
        Rescue::default()
    );
    // No screen at all: wait for the next change.
    let none = ScreenSet {
        screens: vec![],
        main: 0,
        scales: vec![],
    };
    assert_eq!(
        rescue(&[w], &old, &none, old.display(2.0)),
        Rescue::default()
    );
}

#[test]
fn a_new_scale_alone_shows_the_badge() {
    let old = main_only();
    let mut new = main_only();
    new.scales = vec![1.0];
    let r = rescue(&[], &old, &new, old.display(2.0));
    assert!(r.badge);
    assert!(r.moves.is_empty());
}

#[test]
fn a_window_larger_than_the_main_screen_covers_it() {
    let old = two();
    let new = main_only();
    let display = old.display(2.0);
    let big = shown(9, Rect::new(1500.0, 0.0, 1800.0, 1000.0), &old);
    let r = rescue(&[big], &old, &new, display);
    let (_, frame) = r.moves[0];
    // The guest keeps its windows inside its display, so the frame is
    // inside it, at the left edge of the main screen, and covers the
    // whole main screen.
    assert_eq!(crop::clamp_into(frame, display), frame);
    let host = crop::host_frame(frame, &old);
    assert_eq!(host.x, 0.0);
    assert_eq!(new.visible_area(host), 1440.0 * 900.0);
}
