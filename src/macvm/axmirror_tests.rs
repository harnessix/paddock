//! The mirror model: the diff, the stable ids, the stale press, the
//! cap, the hit test and the record.

use super::*;

fn node(role: &str, title: &str, path: &[usize], frame: Frame) -> Node {
    Node {
        role: role.into(),
        title: title.into(),
        value: None,
        frame,
        path: path.to_vec(),
        enabled: true,
        focused: false,
        children: Vec::new(),
    }
}

/// A toolbar with two buttons and a text area with one field.
fn tree() -> Vec<Node> {
    vec![
        Node {
            children: vec![
                node(
                    "AXButton",
                    "Bold",
                    &[0, 0],
                    Frame::new(10.0, 5.0, 20.0, 20.0),
                ),
                node(
                    "AXButton",
                    "Italic",
                    &[0, 1],
                    Frame::new(40.0, 5.0, 20.0, 20.0),
                ),
            ],
            ..node("AXToolbar", "", &[0], Frame::new(0.0, 0.0, 400.0, 30.0))
        },
        Node {
            children: vec![Node {
                value: Some("hello".into()),
                focused: true,
                ..node(
                    "AXTextArea",
                    "",
                    &[1, 0],
                    Frame::new(0.0, 30.0, 400.0, 270.0),
                )
            }],
            ..node(
                "AXScrollArea",
                "",
                &[1],
                Frame::new(0.0, 30.0, 400.0, 270.0),
            )
        },
    ]
}

#[test]
fn diff_stable_ids_and_stale_press() {
    let mut m = Mirror::default();
    assert_eq!(m.press(1), Err(Dropped::NoTree));
    // The first tree: one rebuild, every node has an id.
    let changes = m.tree(7, tree());
    assert_eq!(changes, vec![Change::Rebuild(vec![])]);
    assert_eq!(Node::count(m.nodes()), 5);
    assert_eq!(m.record().elements, 5);
    let toolbar = m.id_of(&[0]).unwrap();
    let bold = m.id_of(&[0, 0]).unwrap();
    let italic = m.id_of(&[0, 1]).unwrap();
    let field = m.id_of(&[1, 0]).unwrap();
    assert_ne!(bold, italic);
    // A value change is an update: the ids stay.
    let mut t = tree();
    t[1].children[0].value = Some("hello world".into());
    t[0].children[0].value = Some("1".into());
    let changes = m.tree(7, t);
    assert_eq!(
        changes,
        vec![Change::Update(vec![0, 0]), Change::Update(vec![1, 0])]
    );
    assert_eq!(m.id_of(&[0, 0]), Some(bold));
    assert_eq!(m.id_of(&[1, 0]), Some(field));
    assert_eq!(m.node_of(bold).unwrap().value.as_deref(), Some("1"));
    // A press on a live element names the window, the path and the role.
    let (window, n) = m.press(bold).unwrap();
    assert_eq!(window, 7);
    assert_eq!(n.path, vec![0, 0]);
    assert_eq!(n.role, "AXButton");
    // A third button: the toolbar list is rebuilt with new ids, the text
    // area keeps its id.
    let mut t = tree();
    t[0].children.push(node(
        "AXButton",
        "Underline",
        &[0, 2],
        Frame::new(70.0, 5.0, 20.0, 20.0),
    ));
    let changes = m.tree(7, t);
    assert_eq!(
        changes,
        vec![Change::Rebuild(vec![0]), Change::Update(vec![1, 0])]
    );
    assert_eq!(m.id_of(&[1, 0]), Some(field));
    assert_eq!(m.id_of(&[0]), Some(toolbar));
    assert_ne!(m.id_of(&[0, 0]), Some(bold));
    assert_eq!(m.record().elements, 6);
    // The stale press: the old id of Bold is dropped, not sent.
    assert_eq!(m.press(bold).map(|_| ()), Err(Dropped::Stale));
    assert_eq!(m.set(bold).map(|_| ()), Err(Dropped::Stale));
    let new_bold = m.id_of(&[0, 0]).unwrap();
    assert!(m.press(new_bold).is_ok());
    // Disabled and away.
    let mut t = tree();
    t[0].children
        .push(node("AXButton", "Underline", &[0, 2], Frame::default()));
    t[0].children[0].enabled = false;
    m.tree(7, t);
    assert_eq!(m.press(new_bold).map(|_| ()), Err(Dropped::Disabled));
    let italic = m.id_of(&[0, 1]).unwrap();
    m.agent_gone();
    assert!(m.away());
    assert_eq!(m.press(italic).map(|_| ()), Err(Dropped::Away));
    assert_eq!(m.timeout(), Vec::new());
    m.agent_back();
    assert!(m.press(italic).is_ok());
    // Another window: a full rebuild, every id is new.
    let changes = m.tree(8, tree());
    assert_eq!(changes, vec![Change::Rebuild(vec![])]);
    assert_eq!(m.window(), Some(8));
    assert_ne!(m.id_of(&[0, 1]), Some(italic));
    // A role change at a position rebuilds that list.
    let mut t = tree();
    t[1].children[0].role = "AXTextField".into();
    assert_eq!(m.tree(8, t), vec![Change::Rebuild(vec![1])]);
    // The close of another window keeps the tree; of this one removes it.
    assert!(!m.window_closed(7));
    assert_eq!(m.window(), Some(8));
    assert!(m.window_closed(8));
    assert_eq!(m.window(), None);
    assert!(m.nodes().is_empty());
    assert_eq!(m.press(1).map(|_| ()), Err(Dropped::NoTree));
    assert!(!m.clear());
}

#[test]
fn frames_titles_and_focus_are_updates() {
    let old = tree();
    let mut new = tree();
    new[0].children[1].title = "Italics".into();
    new[0].children[1].frame.x += 1.0;
    new[1].children[0].focused = false;
    new[0].children[0].focused = true;
    assert_eq!(
        diff(&old, &new),
        vec![
            Change::Update(vec![0, 0]),
            Change::Update(vec![0, 1]),
            Change::Update(vec![1, 0]),
        ]
    );
    // A frame within half a point is the same frame.
    let mut same = tree();
    same[0].frame.w += 0.25;
    assert_eq!(diff(&old, &same), Vec::new());
    // Children appearing under a leaf rebuild that list.
    let mut deeper = tree();
    deeper[1].children[0].children = vec![node("AXStaticText", "", &[1, 0, 0], Frame::default())];
    assert_eq!(diff(&old, &deeper), vec![Change::Rebuild(vec![1, 0])]);
    assert_eq!(Change::Rebuild(vec![]).kind(), "rebuild the tree");
    assert_eq!(Change::Rebuild(vec![1]).kind(), "rebuild the list at [1]");
    assert_eq!(Change::Update(vec![1, 0]).kind(), "update [1, 0]");
}

#[test]
fn well_formed_checks_paths_and_depth() {
    assert!(well_formed(&tree()));
    assert!(well_formed(&[]));
    let mut wrong = tree();
    wrong[0].children[1].path = vec![0, 5];
    assert!(!well_formed(&wrong));
    // Five levels: one too many.
    let mut deep = node("AXGroup", "", &[0], Frame::default());
    let mut cursor = &mut deep;
    for level in 1..DEPTH + 1 {
        let path: Vec<usize> = std::iter::repeat_n(0, level + 1).collect();
        cursor.children = vec![node("AXGroup", "", &path, Frame::default())];
        cursor = &mut cursor.children[0];
    }
    assert!(!well_formed(std::slice::from_ref(&deep)));
    deep.children[0].children[0].children[0].children.clear();
    assert!(well_formed(std::slice::from_ref(&deep)));
}

#[test]
fn cap_cuts_deepest_subtrees_first() {
    // 6 groups of 10 rows of 10 cells of 3 texts: far above the cap.
    let mut big: Vec<Node> = (0..6)
        .map(|g| Node {
            children: (0..10)
                .map(|r| Node {
                    children: (0..10)
                        .map(|c| Node {
                            children: (0..3)
                                .map(|t| {
                                    node(
                                        "AXStaticText",
                                        &format!("text {g} {r} {c} {t}"),
                                        &[g, r, c, t],
                                        Frame::default(),
                                    )
                                })
                                .collect(),
                            ..node("AXCell", "", &[g, r, c], Frame::default())
                        })
                        .collect(),
                    ..node("AXRow", "", &[g, r], Frame::default())
                })
                .collect(),
            ..node("AXGroup", &format!("group {g}"), &[g], Frame::default())
        })
        .collect();
    assert!(well_formed(&big));
    assert!(wire_size(&big) > CAP);
    assert!(cap(&mut big, CAP));
    assert!(wire_size(&big) <= CAP);
    assert!(well_formed(&big));
    // The top level is whole, the first groups keep more than the last.
    assert_eq!(big.len(), 6);
    assert!(Node::count(&big[0].children) >= Node::count(&big[5].children));
    assert!(big[0].children[0].children[0].title.is_empty());
    // A small tree is not cut.
    let mut small = tree();
    assert!(!cap(&mut small, CAP));
    assert_eq!(small, tree());
    // A limit no tree fits: everything goes, and the loop ends.
    let mut tiny = tree();
    assert!(cap(&mut tiny, 1));
    assert!(tiny.is_empty());
}

#[test]
fn hit_test_and_focus_find_the_deepest_node() {
    let t = tree();
    let hit = Node::hit(&t, Point { x: 15.0, y: 10.0 }).unwrap();
    assert_eq!(hit.title, "Bold");
    let hit = Node::hit(&t, Point { x: 200.0, y: 100.0 }).unwrap();
    assert_eq!(hit.role, "AXTextArea");
    // The toolbar itself between the buttons.
    let hit = Node::hit(&t, Point { x: 35.0, y: 10.0 }).unwrap();
    assert_eq!(hit.role, "AXToolbar");
    assert!(Node::hit(&t, Point { x: 500.0, y: 10.0 }).is_none());
    assert_eq!(Node::focused(&t).unwrap().role, "AXTextArea");
    let mut none = tree();
    none[1].children[0].focused = false;
    assert!(Node::focused(&none).is_none());
    assert_eq!(t[0].children[0].name(), "AXButton Bold");
    assert_eq!(t[0].name(), "AXToolbar");
    assert_eq!(Node::at(&t, &[0, 1]).unwrap().title, "Italic");
    assert!(Node::at(&t, &[]).is_none());
    assert!(Node::at(&t, &[2]).is_none());
}

#[test]
fn host_rect_follows_the_window() {
    // A window of 400 by 300 at (100, 500) on the host; a 20 by 20
    // element 10 from the left and 5 from the top of the window.
    let win = Rect::new(100.0, 500.0, 400.0, 300.0);
    let r = host_rect(win, Frame::new(10.0, 5.0, 20.0, 20.0));
    assert_eq!(r, Rect::new(110.0, 775.0, 20.0, 20.0));
    // The window dragged 50 right: the element goes with it.
    let moved = Rect::new(150.0, 500.0, 400.0, 300.0);
    assert_eq!(host_rect(moved, Frame::new(10.0, 5.0, 20.0, 20.0)).x, 160.0);
    // A screen point inside that element maps back onto it.
    let p = window_point(win, 115.0, 790.0);
    assert_eq!((p.x, p.y), (15.0, 10.0));
    assert_eq!(Node::hit(&tree(), p).unwrap().title, "Bold");
}

#[test]
fn record_and_wire_form() {
    let mut m = Mirror::default();
    assert_eq!(
        m.record(),
        AxRecord {
            window: None,
            elements: 0,
            watched: false,
            away: false,
        }
    );
    m.wanted();
    m.tree(3, tree());
    let r = m.record();
    assert_eq!(r.window, Some(3));
    assert_eq!(r.elements, 5);
    assert!(r.watched);
    let json = serde_json::to_string(&tree()[0].children[0]).unwrap();
    assert_eq!(
        json,
        r#"{"role":"AXButton","title":"Bold","frame":{"x":10.0,"y":5.0,"w":20.0,"h":20.0},"path":[0,0],"enabled":true}"#
    );
    let back: Node = serde_json::from_str(&json).unwrap();
    assert_eq!(back, tree()[0].children[0]);
}
