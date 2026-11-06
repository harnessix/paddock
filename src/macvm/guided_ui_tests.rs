use super::*;

/// The guest display of the guided step: 1240 by 1000 points.
const ASPECT: f64 = 1240.0 / 1000.0;
/// The title bar of a titled window on macOS 14 and 15.
const TITLE: f64 = 28.0;

/// The whole window frame, title bar included, in screen coordinates.
fn frame(l: &WindowLayout) -> Rect {
    Rect::new(l.content.x, l.content.y, l.content.w, l.content.h + TITLE)
}

fn aspect_of(r: &Rect) -> f64 {
    r.w / r.h
}

/// The claims that hold on every screen.
fn check(visible: Rect) -> WindowLayout {
    let l = window_layout(visible, ASPECT, TITLE);
    assert!(
        visible.contains(&frame(&l)),
        "{:?} not inside {visible:?}",
        frame(&l)
    );
    // The panel sits at the right edge and the view left of it.
    assert_eq!(l.panel.x + l.panel.w, l.content.w);
    assert_eq!(l.panel.h, l.content.h);
    assert!(l.view.x >= 0.0 && l.view.x + l.view.w <= l.panel.x + 0.5);
    assert!(l.view.y >= 0.0 && l.view.y + l.view.h <= l.content.h + 0.5);
    // The view keeps the guest aspect within a point of rounding.
    assert!(
        (aspect_of(&l.view) - ASPECT).abs() < 0.01,
        "{}",
        aspect_of(&l.view)
    );
    // The smallest size is never above the first frame.
    assert!(l.min_w <= l.content.w && l.min_h <= l.content.h);
    // Centered in the visible frame, within a point.
    let f = frame(&l);
    let left = f.x - visible.x;
    let right = visible.x + visible.w - (f.x + f.w);
    assert!((left - right).abs() <= 1.0, "{left} {right}");
    let below = f.y - visible.y;
    let above = visible.y + visible.h - (f.y + f.h);
    assert!((below - above).abs() <= 1.0, "{below} {above}");
    l
}

#[test]
fn a_13_inch_laptop_with_the_dock_at_the_bottom_fits_the_window() {
    // A 13-inch laptop at 1440 by 900 points: the menu bar takes 25
    // points at the top, the Dock 70 at the bottom.
    let visible = Rect::new(0.0, 70.0, 1440.0, 900.0 - 25.0 - 70.0);
    let l = check(visible);
    let f = frame(&l);
    assert!(f.y >= 70.0, "under the Dock: {f:?}");
    assert!(f.y + f.h <= 875.0, "under the menu bar: {f:?}");
    // The height decides: the whole room is used.
    assert_eq!(l.content.h, (805.0 - 2.0 * SCREEN_MARGIN - TITLE).floor());
    assert!(l.view.w >= 900.0, "{:?}", l.view);
}

#[test]
fn a_small_1280_by_720_screen_still_fits_with_the_panel() {
    let visible = Rect::new(0.0, 0.0, 1280.0, 720.0 - 25.0);
    let l = check(visible);
    assert_eq!(l.panel.w, PANEL_W);
    assert!(l.view.h >= MIN_VIEW_H, "{:?}", l.view);
}

#[test]
fn a_big_screen_gets_the_guest_display_at_its_own_size() {
    let visible = Rect::new(0.0, 0.0, 3008.0, 1692.0 - 25.0);
    let l = check(visible);
    assert_eq!(l.view.w, MAX_VIEW_W);
    assert_eq!(l.view.h, 1000.0);
    assert_eq!(l.content.w, MAX_VIEW_W + PANEL_W);
}

#[test]
fn a_second_screen_left_of_the_main_one_keeps_its_origin() {
    // visibleFrame has a negative x on a screen left of the main one, and
    // a Dock on the left moves x to the right.
    check(Rect::new(-1920.0, 0.0, 1920.0, 1055.0));
    check(Rect::new(80.0, 0.0, 1360.0, 875.0));
}

#[test]
fn a_tiny_screen_shrinks_the_view_and_never_leaves_the_screen() {
    let visible = Rect::new(0.0, 0.0, 1024.0, 500.0);
    let l = window_layout(visible, ASPECT, TITLE);
    assert!(visible.contains(&frame(&l)), "{:?}", frame(&l));
    assert!(l.min_w <= l.content.w && l.min_h <= l.content.h);
}

#[test]
fn a_bad_aspect_falls_back_and_stays_on_screen() {
    let visible = Rect::new(0.0, 0.0, 1440.0, 805.0);
    for aspect in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let l = window_layout(visible, aspect, TITLE);
        assert!(visible.contains(&frame(&l)), "{aspect}");
    }
}

#[test]
fn a_resize_keeps_the_aspect_and_fills_the_room_left_of_the_panel() {
    // Wider than the aspect: bars left and right, the full height.
    let v = view_frame(2000.0, 800.0, ASPECT);
    assert_eq!(v.h, 800.0);
    assert!((aspect_of(&v) - ASPECT).abs() < 0.01);
    assert!(v.x > 0.0 && v.x + v.w <= 2000.0 - PANEL_W);
    // Taller than the aspect: bars above and below, the full width.
    let v = view_frame(PANEL_W + 600.0, 1200.0, ASPECT);
    assert_eq!(v.w, 600.0);
    assert_eq!(v.x, 0.0);
    assert!(v.y > 0.0);
    assert!((aspect_of(&v) - ASPECT).abs() < 0.01);
    // A content narrower than the panel gives a one-point view, not a
    // negative one.
    let v = view_frame(100.0, 100.0, ASPECT);
    assert!(v.w >= 0.0 && v.h >= 0.0);
}

#[test]
fn stack_puts_rows_under_each_other_and_skips_hidden_ones() {
    let rows = [
        Row { h: 20.0, gap: 8.0 },
        Row { h: 0.0, gap: 8.0 },
        Row { h: 50.0, gap: 4.0 },
    ];
    let (tops, total) = stack(&rows, 16.0);
    assert_eq!(tops, vec![16.0, 44.0, 44.0]);
    assert_eq!(total, 16.0 + 20.0 + 8.0 + 50.0 + 16.0);
    assert_eq!(stack(&[], 10.0), (vec![], 20.0));
}

#[test]
fn key_strokes_map_us_keys_with_shift() {
    let k = key_of('b').unwrap();
    assert_eq!((k.code, k.shift, k.ch, k.plain), (0x0B, false, 'b', 'b'));
    let k = key_of('B').unwrap();
    assert_eq!((k.code, k.shift, k.plain), (0x0B, true, 'b'));
    let k = key_of('"').unwrap();
    assert_eq!((k.code, k.shift, k.plain), (0x27, true, '\''));
    assert_eq!(key_of('/').unwrap().code, 0x2C);
    assert_eq!(key_of(' ').unwrap().code, 0x31);
    assert_eq!(key_of('.').unwrap().code, 0x2F);
    assert_eq!(key_of('\n').unwrap().code, 0x24);
    assert_eq!(key_of('é'), None);
    assert_eq!(key_of('€'), None);
    assert_eq!(key_of('\u{7}'), None);
    assert_eq!(key_strokes("ab€c"), Err('€'));
    assert_eq!(key_strokes("").unwrap(), vec![]);
}

#[test]
fn every_printable_ascii_character_has_one_key_and_no_two_share_a_stroke() {
    let mut seen = std::collections::BTreeSet::new();
    for c in (0x20u8..0x7f).map(char::from) {
        let k = key_of(c).unwrap_or_else(|| panic!("no key for {c:?}"));
        assert_eq!(k.ch, c);
        assert!(seen.insert((k.code, k.shift)), "{c:?} shares a stroke");
        assert_ne!(k.code, SHIFT_KEY);
    }
    assert_eq!(seen.len(), 95);
}

#[test]
fn the_install_command_types_whole() {
    let cmd = crate::macvm::seed::command();
    let s = key_strokes(&cmd).unwrap();
    assert_eq!(s.len(), cmd.chars().count());
    let back: String = s.iter().map(|k| k.ch).collect();
    assert_eq!(back, cmd);
    // The quotes and the capitals of the path need shift.
    assert!(s.iter().any(|k| k.shift && k.ch == '"'));
}

#[test]
fn key_actions_hold_shift_only_around_a_shifted_key() {
    let s = key_strokes("aB").unwrap();
    assert_eq!(
        key_actions(&s),
        vec![
            KeyAction::Down(s[0]),
            KeyAction::Up(s[0]),
            KeyAction::ShiftDown,
            KeyAction::Down(s[1]),
            KeyAction::Up(s[1]),
            KeyAction::ShiftUp,
        ]
    );
    // The password of the setup needs no shift at all.
    let pw = crate::macvm::golden::make_password().unwrap();
    let a = key_actions(&key_strokes(&pw).unwrap());
    assert_eq!(a.len(), 2 * pw.len());
    assert!(!a.contains(&KeyAction::ShiftDown));
    assert!(key_actions(&[]).is_empty());
}

fn actions_of(text: &str) -> Vec<KeyAction> {
    key_actions(&key_strokes(text).unwrap())
}

#[test]
fn play_keys_sends_every_action_with_a_pause_after_each() {
    let actions = actions_of("aB\n");
    let mut sent = Vec::new();
    let mut pauses = 0;
    let r = play_keys(
        &actions,
        || true,
        |a| {
            sent.push(a);
            true
        },
        || pauses += 1,
    );
    assert_eq!(r, Played::All);
    assert_eq!(sent, actions);
    assert_eq!(pauses, actions.len());
    // The Return key ends the text.
    assert_eq!(sent.last(), Some(&KeyAction::Up(key_of('\n').unwrap())));
}

#[test]
fn play_keys_stops_between_keys_and_never_holds_one_down() {
    let actions = actions_of("aBc");
    // `go` says yes for the first two keys, then no.
    let mut asks = 0;
    let mut sent = Vec::new();
    let r = play_keys(
        &actions,
        || {
            asks += 1;
            asks <= 2
        },
        |a| {
            sent.push(a);
            true
        },
        || {},
    );
    assert_eq!(r, Played::Stopped);
    assert_eq!(asks, 3, "asked before a, before the shift of B, before c");
    // a down and up, then shift, B down and up, shift up: nothing held.
    assert_eq!(sent.len(), 6);
    assert_eq!(sent.last(), Some(&KeyAction::ShiftUp));
    let downs = sent
        .iter()
        .filter(|a| matches!(a, KeyAction::Down(_)))
        .count();
    let ups = sent
        .iter()
        .filter(|a| matches!(a, KeyAction::Up(_)))
        .count();
    assert_eq!(downs, ups);
}

#[test]
fn play_keys_sends_nothing_when_go_says_no_at_once() {
    let mut sent = 0;
    let r = play_keys(
        &actions_of("abc"),
        || false,
        |_| {
            sent += 1;
            true
        },
        || {},
    );
    assert_eq!(r, Played::Stopped);
    assert_eq!(sent, 0);
}

#[test]
fn play_keys_ends_at_a_failed_send() {
    let mut sent = 0;
    let r = play_keys(
        &actions_of("abc"),
        || true,
        |_| {
            sent += 1;
            false
        },
        || {},
    );
    assert_eq!(r, Played::Failed);
    assert_eq!(sent, 1);
}

#[test]
fn code_down_names_the_key_and_its_direction() {
    let a = key_of('a').unwrap();
    assert_eq!(KeyAction::Down(a).code_down(), (0x00, true));
    assert_eq!(KeyAction::Up(a).code_down(), (0x00, false));
    assert_eq!(KeyAction::ShiftDown.code_down(), (SHIFT_KEY, true));
    assert_eq!(KeyAction::ShiftUp.code_down(), (SHIFT_KEY, false));
}
