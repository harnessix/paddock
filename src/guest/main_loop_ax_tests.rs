//! The Accessibility mirror in the main loop: the request, the event
//! after a change, the unchanged tree not sent again, the cap, the press
//! and the set with their role, the failures, and the end of the watch
//! when the window goes.

use super::*;
use crate::macvm::axmirror::{wire_size, Node, CAP};
use crate::macvm::proto::Frame;

fn node(role: &str, title: &str, path: &[usize]) -> Node {
    Node {
        role: role.into(),
        title: title.into(),
        value: None,
        frame: Frame::new(0.0, 0.0, 10.0, 10.0),
        path: path.to_vec(),
        enabled: true,
        focused: false,
        children: Vec::new(),
    }
}

fn small() -> Vec<Node> {
    vec![Node {
        children: vec![node("AXButton", "Bold", &[0, 0])],
        ..node("AXToolbar", "", &[0])
    }]
}

/// A tree larger than the cap: 4 groups of 40 rows of 12 cells.
fn huge() -> Vec<Node> {
    (0..4)
        .map(|g| Node {
            children: (0..40)
                .map(|r| Node {
                    children: (0..12)
                        .map(|c| node("AXCell", &format!("cell {g} {r} {c}"), &[g, r, c]))
                        .collect(),
                    ..node("AXRow", "", &[g, r])
                })
                .collect(),
            ..node("AXGroup", &format!("group {g}"), &[g])
        })
        .collect()
}

fn tree_event(host: &mut Host) -> (u32, Vec<Node>, bool) {
    match host.until(|e| matches!(e, Event::AxTree { .. })) {
        Event::AxTree { window, nodes, cut } => (window, nodes, cut),
        _ => unreachable!(),
    }
}

/// A poll ran after the caller's change and no `ax.tree` event came.
fn no_tree_event(host: &mut Host) {
    let before = host.seen.len();
    std::thread::sleep(Duration::from_millis(400));
    host.until(|e| matches!(e, Event::Health(_)));
    assert!(
        !host.seen[before..]
            .iter()
            .any(|e| matches!(e, Event::AxTree { .. })),
        "{:?}",
        &host.seen[before..]
    );
}

#[test]
fn tree_requests_events_press_and_set() {
    let (a, b) = UnixStream::pair().unwrap();
    let (fake, st) = Fake::new();
    with_app(&st);
    st.lock().unwrap().ax_tree = small();
    let agent = start(fake, vec![a]);
    let mut host = Host::new(b);
    host.handshake();
    let req = host.send(Request::AppLaunch {
        path: "/System/Applications/TextEdit.app".into(),
        args: vec![],
        env: Default::default(),
    });
    assert!(matches!(host.reply(req), Outcome::Ok { .. }));
    host.until(|e| matches!(e, Event::Windows(w) if w.order.contains(&1)));
    // An unknown window: no tree, no watch.
    let req = host.send(Request::AxTree { window: 99 });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchWindow,
            ..
        }
    ));
    assert_eq!(st.lock().unwrap().ax_tree_reads, 0);
    // No watch yet: a notification reads nothing.
    st.lock().unwrap().ax_tree_dirty = true;
    no_tree_event(&mut host);
    assert_eq!(st.lock().unwrap().ax_tree_reads, 0);
    // The request: the event, then the reply.
    let req = host.send(Request::AxTree { window: 1 });
    let (window, nodes, cut) = tree_event(&mut host);
    assert_eq!(window, 1);
    assert_eq!(nodes, small());
    assert!(!cut);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(st.lock().unwrap().ax_tree_reads, 1);
    // A notification with the same tree: read, not sent again.
    st.lock().unwrap().ax_tree_dirty = true;
    no_tree_event(&mut host);
    assert_eq!(st.lock().unwrap().ax_tree_reads, 2);
    // A notification with a changed tree: sent.
    {
        let mut st = st.lock().unwrap();
        st.ax_tree[0].children[0].value = Some("1".into());
        st.ax_tree_dirty = true;
    }
    let (_, nodes, _) = tree_event(&mut host);
    assert_eq!(nodes[0].children[0].value.as_deref(), Some("1"));
    // A press: the window, the path and the role reach the platform, and
    // the tree is read again after it.
    let reads = st.lock().unwrap().ax_tree_reads;
    let req = host.send(Request::AxPress {
        window: 1,
        path: vec![0, 0],
        role: "AXButton".into(),
    });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    no_tree_event(&mut host);
    assert_eq!(
        st.lock().unwrap().ax_presses,
        vec![(1, vec![0, 0], "AXButton".to_string())]
    );
    assert!(st.lock().unwrap().ax_tree_reads > reads);
    // A set.
    let req = host.send(Request::AxSet {
        window: 1,
        path: vec![0, 0],
        role: "AXButton".into(),
        value: "0".into(),
    });
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    assert_eq!(
        st.lock().unwrap().ax_sets,
        vec![(1, vec![0, 0], "AXButton".to_string(), "0".to_string())]
    );
    // A press on an unknown window, and one the platform refuses.
    let req = host.send(Request::AxPress {
        window: 99,
        path: vec![0],
        role: "AXButton".into(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchWindow,
            ..
        }
    ));
    st.lock().unwrap().ax_action_result = Some((ErrorCode::BadRequest, "another role".into()));
    let req = host.send(Request::AxPress {
        window: 1,
        path: vec![0, 0],
        role: "AXButton".into(),
    });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::BadRequest,
            ..
        }
    ));
    // A hung app: the request fails with Timeout and no event goes out;
    // a notification is one log line and no event.
    st.lock().unwrap().ax_tree_result = Some((ErrorCode::Timeout, "AX timed out".into()));
    let req = host.send(Request::AxTree { window: 1 });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::Timeout,
            ..
        }
    ));
    st.lock().unwrap().ax_tree_dirty = true;
    no_tree_event(&mut host);
    // A tree above the cap goes out cut, under the cap.
    {
        let mut st = st.lock().unwrap();
        st.ax_tree_result = None;
        st.ax_tree = huge();
        assert!(wire_size(&st.ax_tree) > CAP);
        st.ax_tree_dirty = true;
    }
    let (_, nodes, cut) = tree_event(&mut host);
    assert!(cut);
    assert!(wire_size(&nodes) <= CAP);
    assert_eq!(nodes.len(), 4);
    // The watched window goes: no more reads, and a request says so.
    {
        let mut st = st.lock().unwrap();
        st.windows.retain(|w| w.id != 1);
    }
    host.until(|e| matches!(e, Event::Windows(w) if w.removed.contains(&1)));
    let reads = st.lock().unwrap().ax_tree_reads;
    st.lock().unwrap().ax_tree_dirty = true;
    no_tree_event(&mut host);
    assert_eq!(st.lock().unwrap().ax_tree_reads, reads);
    let req = host.send(Request::AxTree { window: 1 });
    assert!(matches!(
        host.reply(req),
        Outcome::Error {
            code: ErrorCode::NoSuchWindow,
            ..
        }
    ));
    // Another window of the app can be watched.
    let req = host.send(Request::AxTree { window: 2 });
    let (window, _, _) = tree_event(&mut host);
    assert_eq!(window, 2);
    assert_eq!(host.reply(req), Outcome::Ok { data: None });
    drop(host);
    assert!(!agent.is_finished());
}
