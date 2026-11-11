//! The chrome of the windows in the main loop: the title bar and the full
//! screen flag in the `windows` event, the radius measured once and sent
//! in a later event, and no measure for a full screen window.

use super::*;

fn window_event(
    host: &mut Host,
    pred: impl Fn(&crate::macvm::proto::Window) -> bool,
) -> crate::macvm::proto::Window {
    match host.until(|e| matches!(e, Event::Windows(w) if w.changed.iter().any(&pred))) {
        Event::Windows(w) => w.changed.into_iter().find(|x| pred(x)).unwrap(),
        _ => unreachable!(),
    }
}

#[test]
fn the_radius_is_measured_once_and_reported_with_the_title_bar() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    {
        let mut s = st.lock().unwrap();
        s.title_bars.insert(1, 52.0);
        s.radii.insert(1, 16.0);
        s.radii.insert(2, 12.0);
        // Window 2 in full screen, away from window 1: it covers no part.
        s.full_screen.insert(2);
        s.windows[1].bounds = Frame::new(900.0, 0.0, 100.0, 100.0);
    }
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let req = host.send(Request::AppLaunch {
        path: "/System/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    // The first report: the title bar and the full screen flag at once,
    // the radius not yet (the window opens with an animation).
    let first = window_event(&mut host, |w| w.id == 1);
    assert_eq!(first.title_bar, Some(52.0));
    assert_eq!(first.radius, None);
    // A later event carries the measured radius.
    let measured = window_event(&mut host, |w| w.id == 1 && w.radius.is_some());
    assert_eq!(measured.radius, Some(16.0));
    // Some polls later: still one measure of window 1, none of the full
    // screen window 2.
    for _ in 0..3 {
        host.until(|e| matches!(e, Event::Health(_)));
    }
    {
        let s = st.lock().unwrap();
        assert_eq!(s.radius_asks.iter().filter(|id| **id == 1).count(), 1);
        assert!(!s.radius_asks.contains(&2), "{:?}", s.radius_asks);
    }
    let req = host.send(Request::AgentRestart);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(agent.join().unwrap(), 0);
}

#[test]
fn a_full_screen_window_is_reported_and_its_drawn_parts_are_not() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    {
        let mut s = st.lock().unwrap();
        // Window 1 goes full screen; window 2 is its toolbar window, over
        // its top, unknown to Accessibility (the fake knows no element).
        s.full_screen.insert(1);
        s.windows[0].bounds = Frame::new(0.0, 0.0, 1440.0, 900.0);
        s.windows[1].bounds = Frame::new(0.0, 0.0, 1440.0, 52.0);
    }
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let req = host.send(Request::AppLaunch {
        path: "/System/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    let Event::Windows(w) = host.until(|e| matches!(e, Event::Windows(w) if w.order.contains(&1)))
    else {
        unreachable!()
    };
    assert_eq!(w.order, vec![1]);
    assert!(w.changed[0].full_screen);
    let req = host.send(Request::AgentRestart);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(agent.join().unwrap(), 0);
}
