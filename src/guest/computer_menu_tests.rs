//! The menu checks of the guest server in app scope with a fake screen:
//! a menu that opens late after the press of a drag, a menu that opens
//! while the pointer moves, and right or ctrl clicks with a count.

use super::*;
use crate::guest::computer_fake::{self as fake, raw};
use crate::macvm::computer::{Button, Mod};

/// An open menu of the app, as the window server lists it.
fn menu_window() -> RawWindow {
    let mut w = raw(77, 500, "Notes", Frame::new(120.0, 24.0, 220.0, 300.0));
    w.layer = MENU_LAYER;
    w
}

fn left_drag() -> Input {
    Input::Drag {
        from: Point { x: 200.0, y: 200.0 },
        to: Point { x: 260.0, y: 320.0 },
        button: Button::Left,
        mods: vec![],
    }
}

/// The index of the escape key down in `posted`.
fn escape_at(posted: &[Low]) -> Option<usize> {
    posted.iter().position(|l| {
        matches!(
            l,
            Low::Key {
                code: 53,
                down: true,
                ..
            }
        )
    })
}

#[test]
fn a_menu_that_opens_late_after_the_press_of_a_drag_is_seen() {
    let (server, st, _) = fake::server(fake::scene());
    // Read 1 is the check of the points. The menu shows from read 3: the
    // second look after the press, past the first look.
    st.lock().unwrap().menu_at_look = Some((3, menu_window()));
    let e = server.input(Target::APP, &left_drag()).unwrap_err();
    assert_eq!(e.code, Code::Covered);
    assert!(e.text.contains("after 1 of 2 steps"), "{}", e.text);
    let posted = st.lock().unwrap().posted.clone();
    assert!(
        !posted.iter().any(|l| matches!(l, Low::Drag { .. })),
        "the pointer never moved with the button down: {posted:?}"
    );
    let esc = escape_at(&posted).expect("escape went");
    let up = posted
        .iter()
        .position(|l| matches!(l, Low::Up { .. }))
        .expect("the button went up");
    assert!(esc < up, "{posted:?}");
    assert!(
        matches!(posted[up], Low::Up { at, .. } if at == Point { x: 200.0, y: 200.0 }),
        "{posted:?}"
    );
}

#[test]
fn a_menu_that_opens_while_the_pointer_moves_closes_before_the_let_go() {
    let (server, st, _) = fake::server(fake::scene());
    // Move, down, then the first move with the button down opens it.
    st.lock().unwrap().menu_after = Some((3, menu_window()));
    let e = server.input(Target::APP, &left_drag()).unwrap_err();
    assert_eq!(e.code, Code::Covered);
    let posted = st.lock().unwrap().posted.clone();
    let esc = escape_at(&posted).expect("escape went");
    let up = posted
        .iter()
        .position(|l| matches!(l, Low::Up { .. }))
        .expect("the button went up");
    assert!(esc < up, "{posted:?}");
    // The menu closed before the let-go: escape down and up, then the
    // button up as the last event, so no item of the menu is picked.
    assert_eq!(up, posted.len() - 1, "{posted:?}");
    assert!(
        matches!(
            posted[up - 1],
            Low::Key {
                code: 53,
                down: false,
                ..
            }
        ),
        "{posted:?}"
    );
}

#[test]
fn a_drag_with_no_menu_goes_after_the_looks() {
    let (server, st, _) = fake::server(fake::scene());
    assert!(server.input(Target::APP, &left_drag()).is_ok());
    let s = st.lock().unwrap();
    assert!(
        matches!(s.posted.last(), Some(Low::Up { at, .. }) if *at == Point { x: 260.0, y: 320.0 }),
        "{:?}",
        s.posted
    );
    // A look before the moves and a look before the let-go, each until
    // the bound passes.
    assert!(s.slept >= MENU_WAIT * 2, "{:?}", s.slept);
    assert!(escape_at(&s.posted).is_none());
}

#[test]
fn a_right_or_ctrl_multi_click_is_refused_for_the_app_windows() {
    let (server, st, _) = fake::server(fake::scene());
    let click = |button, count, mods: Vec<Mod>| Input::Click {
        at: Point { x: 200.0, y: 200.0 },
        button,
        count,
        mods,
    };
    for action in [
        click(Button::Right, 2, vec![]),
        click(Button::Right, 3, vec![]),
        click(Button::Left, 2, vec![Mod::Ctrl]),
    ] {
        let e = server.input(Target::APP, &action).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{action:?}");
        assert!(e.text.contains("context menu"), "{}", e.text);
        assert!(st.lock().unwrap().posted.is_empty(), "{action:?}");
    }
    // One right click goes, a left double click goes, and in desktop
    // scope a right double click goes.
    for (target, action) in [
        (Target::APP, click(Button::Right, 1, vec![])),
        (Target::APP, click(Button::Left, 2, vec![])),
        (Target::DESKTOP, click(Button::Right, 2, vec![])),
    ] {
        assert!(server.input(target, &action).is_ok(), "{action:?}");
    }
}
