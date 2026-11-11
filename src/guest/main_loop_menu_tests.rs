//! The menu bar in the main loop: the event after a focus change and a
//! menu notification, the request, the cap, the press and the timeout.

use super::*;
use crate::macvm::menu::{wire_size, MenuItem, Shortcut, CAP};

fn entry(title: &str, path: &[usize], shortcut: Option<&str>) -> MenuItem {
    MenuItem {
        title: title.into(),
        path: path.to_vec(),
        shortcut: shortcut.and_then(Shortcut::parse),
        enabled: true,
        checked: false,
        children: Vec::new(),
    }
}

fn bar() -> Vec<MenuItem> {
    vec![MenuItem {
        children: vec![
            entry("New", &[2, 0], Some("Cmd-N")),
            entry("", &[2, 1], None),
            entry("Close", &[2, 2], Some("Cmd-W")),
        ],
        ..entry("File", &[2], None)
    }]
}

/// A bar larger than the cap.
fn huge() -> Vec<MenuItem> {
    (0..8)
        .map(|m| MenuItem {
            children: (0..12)
                .map(|i| MenuItem {
                    children: (0..6)
                        .map(|s| entry(&format!("Entry {m} {i} {s}"), &[m + 2, i, s], None))
                        .collect(),
                    ..entry(&format!("Item {m} {i}"), &[m + 2, i], None)
                })
                .collect(),
            ..entry(&format!("Menu {m}"), &[m + 2], None)
        })
        .collect()
}

fn menu_event(host: &mut Host) -> (Vec<MenuItem>, bool) {
    match host.until(|e| matches!(e, Event::Menu { .. })) {
        Event::Menu { app, items, cut } => {
            assert_eq!(app, APP);
            (items, cut)
        }
        _ => unreachable!(),
    }
}

/// A poll ran after the caller's change (the poll is every 100 ms, the
/// `health` after it every 2 s) and no `menu` event came with it.
fn no_menu_event(host: &mut Host) {
    let before = host.seen.len();
    std::thread::sleep(Duration::from_millis(300));
    host.until(|e| matches!(e, Event::Health(_)));
    assert!(
        !host.seen[before..]
            .iter()
            .any(|e| matches!(e, Event::Menu { .. })),
        "{:?}",
        &host.seen[before..]
    );
}

#[test]
fn menu_events_requests_and_presses() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    st.lock().unwrap().menu = bar();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    // A press before the launch: no app.
    let req = host.send(Request::MenuPress { path: vec![2, 0] });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchApp,
            ..
        }
    ));
    let req = host.send(Request::AppLaunch {
        path: "/System/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    // The focus change after the launch sends the tree on its own.
    let (items, cut) = menu_event(&mut host);
    assert_eq!(items, bar());
    assert!(!cut);
    assert_eq!(st.lock().unwrap().menu_reads, 1);
    // A notification with the same tree: read, not sent again.
    st.lock().unwrap().menu_dirty = true;
    no_menu_event(&mut host);
    assert_eq!(st.lock().unwrap().menu_reads, 2);
    // A notification with a changed tree: sent.
    {
        let mut st = st.lock().unwrap();
        st.menu[0].children[2].enabled = false;
        st.menu_dirty = true;
    }
    let (items, _) = menu_event(&mut host);
    assert!(!items[0].children[2].enabled);
    // A request: the event, then the reply, even with no change.
    let req = host.send(Request::Menu { app: APP.into() });
    let (items, _) = menu_event(&mut host);
    assert!(!items[0].children[2].enabled);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    // A press: the path reaches the platform, and the tree is read again
    // after it because a press can change a state.
    let reads = st.lock().unwrap().menu_reads;
    let req = host.send(Request::MenuPress { path: vec![2, 0] });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    no_menu_event(&mut host);
    assert_eq!(st.lock().unwrap().presses, vec![vec![2, 0]]);
    assert!(st.lock().unwrap().menu_reads > reads);
    let req = host.send(Request::MenuPress { path: vec![] });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::BadRequest,
            ..
        }
    ));
    st.lock().unwrap().press_result = Some((ErrorCode::Timeout, "AX timed out".into()));
    let req = host.send(Request::MenuPress { path: vec![2, 2] });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::Timeout,
            ..
        }
    ));
    // A hung app: the request fails with Timeout and no event goes out;
    // a notification is one log line and no event.
    st.lock().unwrap().menu_result = Some((ErrorCode::Timeout, "AX timed out".into()));
    let req = host.send(Request::Menu { app: APP.into() });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::Timeout,
            ..
        }
    ));
    st.lock().unwrap().menu_dirty = true;
    no_menu_event(&mut host);
    // A tree above the cap goes out cut, under the cap.
    {
        let mut st = st.lock().unwrap();
        st.menu_result = None;
        st.menu = huge();
        assert!(wire_size(&st.menu) > CAP);
        st.menu_dirty = true;
    }
    let (items, cut) = menu_event(&mut host);
    assert!(cut);
    assert!(wire_size(&items) <= CAP);
    assert_eq!(items.len(), 8);
    // The app exits: no more reads, and a request says no app.
    {
        let mut st = st.lock().unwrap();
        st.procs.remove(&100);
        st.windows.retain(|w| w.pid != 100);
    }
    host.until(|e| matches!(e, Event::AppExited { .. }));
    let reads = st.lock().unwrap().menu_reads;
    st.lock().unwrap().menu_dirty = true;
    no_menu_event(&mut host);
    assert_eq!(st.lock().unwrap().menu_reads, reads);
    let req = host.send(Request::Menu { app: APP.into() });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchApp,
            ..
        }
    ));
    drop(host);
    assert!(!agent.is_finished());
}
