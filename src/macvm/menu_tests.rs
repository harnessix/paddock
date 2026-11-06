//! The diff, the shortcuts, the cap and the fallback of the menu bar
//! replica.

use super::*;

fn item(title: &str, path: &[usize]) -> MenuItem {
    MenuItem {
        title: title.into(),
        path: path.to_vec(),
        enabled: true,
        ..MenuItem::default()
    }
}

fn with_shortcut(mut i: MenuItem, text: &str) -> MenuItem {
    i.shortcut = Shortcut::parse(text);
    i
}

fn menu(title: &str, index: usize, children: Vec<MenuItem>) -> MenuItem {
    MenuItem {
        title: title.into(),
        path: vec![index],
        enabled: true,
        checked: false,
        shortcut: None,
        children,
    }
}

/// `File` with `New`, a separator and `Close`, and `Edit` with `Undo`
/// and a `Find` submenu of two items. The paths are Accessibility
/// indices: the bar starts at 2, after the Apple and the app menu.
fn tree() -> Vec<MenuItem> {
    vec![
        menu(
            "File",
            2,
            vec![
                with_shortcut(item("New", &[2, 0]), "Cmd-N"),
                item("", &[2, 1]),
                with_shortcut(item("Close", &[2, 2]), "Cmd-W"),
            ],
        ),
        menu(
            "Edit",
            3,
            vec![
                with_shortcut(item("Undo", &[3, 0]), "Cmd-Z"),
                MenuItem {
                    children: vec![
                        with_shortcut(item("Find…", &[3, 1, 0]), "Cmd-F"),
                        with_shortcut(item("Find Next", &[3, 1, 1]), "Cmd-G"),
                    ],
                    ..item("Find", &[3, 1])
                },
            ],
        ),
    ]
}

#[test]
fn diff_cases() {
    let old = tree();
    // The same tree: nothing.
    assert!(diff(&old, &tree()).is_empty());

    // An added item in File: that menu is rebuilt, Edit is not.
    let mut new = tree();
    new[0]
        .children
        .push(with_shortcut(item("Save", &[2, 3]), "Cmd-S"));
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![0])]);

    // A removed item in the Find submenu: only the submenu is rebuilt.
    let mut new = tree();
    new[1].children[1].children.pop();
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![1, 1])]);

    // A retitled item: a rebuild of its menu.
    let mut new = tree();
    new[0].children[2].title = "Close Window".into();
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![0])]);

    // A disabled item and a checked one: state changes only, with the
    // positions, not the Accessibility paths.
    let mut new = tree();
    new[0].children[2].enabled = false;
    new[1].children[1].children[0].checked = true;
    assert_eq!(
        diff(&old, &new),
        vec![
            Change::State {
                position: vec![0, 2],
                enabled: false,
                checked: false,
            },
            Change::State {
                position: vec![1, 1, 0],
                enabled: true,
                checked: true,
            },
        ]
    );

    // A new top menu: the bar.
    let mut new = tree();
    new.push(menu("View", 4, vec![item("Zoom", &[4, 0])]));
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![])]);

    // A retitled top menu: the bar.
    let mut new = tree();
    new[1].title = "Bearbeiten".into();
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![])]);

    // A changed shortcut: the menu.
    let mut new = tree();
    new[0].children[0].shortcut = Shortcut::parse("Cmd-Shift-N");
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![0])]);

    // A changed Accessibility path at the same position: the menu, so
    // the press goes to the right element.
    let mut new = tree();
    new[0].children[0].path = vec![2, 5];
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![0])]);

    // An item that became a submenu: the menu.
    let mut new = tree();
    new[0].children[0].children = vec![item("Chat", &[2, 0, 0])];
    assert_eq!(diff(&old, &new), vec![Change::Rebuild(vec![0])]);

    // From nothing to a tree: the bar.
    assert_eq!(diff(&[], &old), vec![Change::Rebuild(vec![])]);
    for c in diff(&old, &new) {
        assert!(!c.kind().is_empty());
    }
}

#[test]
fn shortcut_cases() {
    let n = Shortcut::parse("Cmd-N").unwrap();
    assert_eq!(n.key, "N");
    assert_eq!(n.mods, Mods::from_ax(0));
    assert_eq!(n.key_equivalent(), "n");
    assert_eq!(n.flags(), FLAG_COMMAND);
    assert_eq!(n.text(), "Cmd-N");
    assert_eq!(n.to_string(), "Cmd-N");

    let shift_n = Shortcut::parse("Cmd-Shift-N").unwrap();
    assert_eq!(shift_n.key_equivalent(), "n");
    assert_eq!(shift_n.flags(), FLAG_COMMAND | FLAG_SHIFT);
    assert_eq!(shift_n.text(), "Cmd-Shift-N");
    // The modifier words in any order give the same shortcut.
    assert_eq!(Shortcut::parse("Shift-Cmd-N").unwrap(), shift_n);

    let comma = Shortcut::parse("Cmd-,").unwrap();
    assert_eq!(comma.key, ",");
    assert_eq!(comma.key_equivalent(), ",");
    assert_eq!(comma.flags(), FLAG_COMMAND);
    assert_eq!(comma.text(), "Cmd-,");
    // Command and the minus key.
    let minus = Shortcut::parse("Cmd--").unwrap();
    assert_eq!(minus.key, "-");
    assert_eq!(minus.text(), "Cmd--");

    // A function key alone: the AppKit character and the Function flag.
    let f5 = Shortcut::parse("F5").unwrap();
    assert_eq!(f5.key_equivalent(), "\u{F708}");
    assert_eq!(f5.flags(), FLAG_FUNCTION);
    assert_eq!(f5.text(), "F5");
    let f11 = Shortcut::parse("Cmd-Opt-F11").unwrap();
    assert_eq!(f11.key_equivalent(), "\u{F70E}");
    assert_eq!(f11.flags(), FLAG_COMMAND | FLAG_OPTION | FLAG_FUNCTION);
    let f19 = Shortcut::parse("F19").unwrap();
    assert_eq!(f19.key_equivalent(), "\u{F716}");
    assert!(Shortcut::parse("F20").is_none());
    assert!(Shortcut::parse("F0").is_none());

    // Named keys: the arrows carry the Function flag, Return does not.
    let left = Shortcut::parse("Cmd-Left").unwrap();
    assert_eq!(left.key_equivalent(), "\u{F702}");
    assert_eq!(left.flags(), FLAG_COMMAND | FLAG_FUNCTION);
    let ret = Shortcut::parse("Cmd-Return").unwrap();
    assert_eq!(ret.key_equivalent(), "\r");
    assert_eq!(ret.flags(), FLAG_COMMAND);
    let del = Shortcut::parse("Cmd-Delete").unwrap();
    assert_eq!(del.key_equivalent(), "\u{8}");
    let ctrl = Shortcut::parse("Ctrl-Opt-Space").unwrap();
    assert_eq!(ctrl.key_equivalent(), " ");
    assert_eq!(ctrl.flags(), FLAG_CONTROL | FLAG_OPTION);
    assert_eq!(ctrl.text(), "Ctrl-Opt-Space");

    // Not shortcuts.
    assert!(Shortcut::parse("").is_none());
    assert!(Shortcut::parse("Cmd-").is_none());
    assert!(Shortcut::parse("Cmd-Nope").is_none());

    // From the Accessibility attributes: the command character with the
    // Carbon mask (0 Command, 1 Shift, 2 Option, 4 Control, 8 no
    // Command), or the glyph of a function key.
    assert_eq!(Shortcut::from_ax("n", 0, None), Some(n.clone()));
    assert_eq!(Shortcut::from_ax("N", 1, None), Some(shift_n));
    assert_eq!(Shortcut::from_ax(",", 0, Some(0)), Some(comma));
    assert_eq!(Shortcut::from_ax("", 8, Some(115)), Some(f5));
    assert_eq!(Shortcut::from_ax("", 2, Some(121)), Some(f11));
    assert_eq!(Shortcut::from_ax("", 8, Some(146)), Some(f19));
    assert_eq!(Shortcut::from_ax("", 0, Some(100)), Some(left));
    assert_eq!(
        Shortcut::from_ax("", 14, Some(9)),
        Shortcut::parse("Ctrl-Opt-Space")
    );
    // The control characters an app may put into the command character.
    assert_eq!(Shortcut::from_ax("\r", 0, None), Some(ret));
    assert_eq!(Shortcut::from_ax("\u{8}", 0, None), Some(del));
    assert_eq!(
        Shortcut::from_ax("\u{F708}", 8, None),
        Shortcut::parse("F5")
    );
    // No character and no glyph: no shortcut. An unknown glyph: none.
    assert_eq!(Shortcut::from_ax("", 0, None), None);
    assert_eq!(Shortcut::from_ax("", 0, Some(999)), None);

    // The wire form is short.
    let json = serde_json::to_string(&n).unwrap();
    assert_eq!(json, r#"{"key":"N","mods":{"command":true}}"#);
    assert_eq!(serde_json::from_str::<Shortcut>(&json).unwrap(), n);
}

/// A bar of `menus` top menus, each with `items` items that each carry a
/// submenu of `sub` items.
fn big(menus: usize, items: usize, sub: usize) -> Vec<MenuItem> {
    (0..menus)
        .map(|m| {
            let children = (0..items)
                .map(|i| MenuItem {
                    children: (0..sub)
                        .map(|s| item(&format!("Entry {m} {i} {s}"), &[m + 2, i, s]))
                        .collect(),
                    ..item(&format!("Item {m} {i}"), &[m + 2, i])
                })
                .collect();
            menu(&format!("Menu {m}"), m + 2, children)
        })
        .collect()
}

/// Every submenu of `cut` keeps all of its original items or none.
fn whole_submenus(original: &[MenuItem], cut: &[MenuItem]) {
    assert_eq!(original.len(), cut.len());
    for (o, c) in original.iter().zip(cut) {
        assert_eq!(o.title, c.title);
        assert_eq!(o.path, c.path);
        if c.children.is_empty() {
            assert!(c.enabled == o.enabled || !o.children.is_empty());
            if !o.children.is_empty() {
                assert!(!c.enabled, "a cut submenu is disabled: {}", c.title);
            }
        } else {
            assert_eq!(o.children.len(), c.children.len(), "{} is half", c.title);
            whole_submenus(&o.children, &c.children);
        }
    }
}

#[test]
fn cap_cuts_submenu() {
    // Under the cap: nothing changes.
    let mut small = tree();
    assert!(!cap(&mut small, CAP));
    assert_eq!(small, tree());

    // Above the cap: whole submenus go, the deepest and last first,
    // until the tree fits, and the rest is untouched.
    let original = big(6, 10, 8);
    assert!(wire_size(&original) > CAP);
    let mut items = original.clone();
    assert!(cap(&mut items, CAP));
    assert!(wire_size(&items) <= CAP, "{}", wire_size(&items));
    whole_submenus(&original, &items);
    // The first menu kept every submenu, the last one lost some.
    assert!(items[0].children.iter().all(|i| !i.children.is_empty()));
    assert!(items[5].children.iter().any(|i| i.children.is_empty()));
    assert!(MenuItem::count(&items) < MenuItem::count(&original));

    // A small cap on a tree of submenus cuts the submenus before any top
    // menu, and the top menus only when that is not enough.
    let original = big(3, 4, 4);
    let mut items = original.clone();
    let only_subs = wire_size(&big(3, 4, 0)) + 300;
    assert!(cap(&mut items, only_subs));
    whole_submenus(&original, &items);
    assert!(items.iter().all(|m| !m.children.is_empty()));
    let mut items = original.clone();
    assert!(cap(&mut items, 200));
    whole_submenus(&original, &items);
    assert!(items.iter().any(|m| m.children.is_empty() && !m.enabled));
    assert!(wire_size(&items) <= 200);
}

#[test]
fn fallback_cases() {
    let mut f = Fallback::default();
    // No tree: the first pass menu, nothing to press.
    assert_eq!(f.shown(), Shown::FirstPass);
    assert!(f.items().is_empty());
    assert_eq!(f.press(&[2, 0]), Err(Dropped::NoTree));
    assert!(f.timeout().is_empty());
    assert!(f.app_exited().is_empty());

    // The first tree rebuilds the bar.
    assert_eq!(f.tree(tree()), vec![Change::Rebuild(vec![])]);
    assert_eq!(f.shown(), Shown::Tree);
    assert_eq!(f.items(), &tree()[..]);
    assert_eq!(f.press(&[2, 0]).unwrap().title, "New");

    // A timeout of the request keeps the last tree.
    assert!(f.timeout().is_empty());
    assert_eq!(f.shown(), Shown::Tree);
    assert_eq!(f.items(), &tree()[..]);

    // The agent goes: the tree stays, a press is dropped as away. A new
    // hello: the tree stays until the next event, presses go again.
    assert!(f.agent_gone().is_empty());
    assert!(f.away());
    assert_eq!(f.shown(), Shown::Tree);
    assert_eq!(f.press(&[2, 0]), Err(Dropped::Away));
    assert!(f.agent_back().is_empty());
    assert!(!f.away());
    assert_eq!(f.items(), &tree()[..]);
    assert!(f.press(&[2, 0]).is_ok());

    // The next event after a restart: only what changed.
    let mut new = tree();
    new[0].children[2].enabled = false;
    assert_eq!(
        f.tree(new.clone()),
        vec![Change::State {
            position: vec![0, 2],
            enabled: false,
            checked: false,
        }]
    );
    assert_eq!(f.press(&[2, 2]), Err(Dropped::Disabled));
    assert_eq!(f.press(&[3, 1]), Err(Dropped::Submenu));
    assert_eq!(f.press(&[3, 1, 1]).unwrap().title, "Find Next");

    // A tree that arrives while away ends the away state.
    f.agent_gone();
    assert!(f.tree(tree()).len() == 1);
    assert!(!f.away());

    // The record names the state.
    let r = f.record();
    assert_eq!((r.shown.as_str(), r.items, r.away), ("tree", 9, false));
    // The app exited: the first pass menu, once.
    assert_eq!(f.app_exited(), vec![Change::Rebuild(vec![])]);
    assert_eq!(f.record().shown, "first-pass");
    assert_eq!(f.record().items, 0);
    assert_eq!(f.shown(), Shown::FirstPass);
    assert!(f.app_exited().is_empty());
    assert_eq!(f.press(&[2, 0]), Err(Dropped::NoTree));
}

#[test]
fn press_unknown_path_dropped() {
    let mut f = Fallback::default();
    f.tree(tree());
    // A path the tree lacks, a stale one and an empty one.
    assert_eq!(f.press(&[2, 9]), Err(Dropped::NoSuchItem));
    assert_eq!(f.press(&[7]), Err(Dropped::NoSuchItem));
    assert_eq!(f.press(&[2, 0, 0]), Err(Dropped::NoSuchItem));
    assert_eq!(f.press(&[]), Err(Dropped::NoSuchItem));
    // A path to a top menu is a submenu, not a press.
    assert_eq!(f.press(&[2]), Err(Dropped::Submenu));
    // The titles of a path for the log: the known part.
    assert_eq!(
        MenuItem::titles(f.items(), &[3, 1, 1]),
        "Edit > Find > Find Next"
    );
    assert_eq!(MenuItem::titles(f.items(), &[2, 9]), "File");
    assert_eq!(MenuItem::titles(f.items(), &[9]), "");
    // The position form, for the host menus.
    assert_eq!(MenuItem::at(f.items(), &[1, 1, 0]).unwrap().title, "Find…");
    assert!(MenuItem::at(f.items(), &[1, 1, 7]).is_none());
    assert!(MenuItem::at(f.items(), &[]).is_none());
    assert!(f.items()[0].children[1].is_separator());
    assert!(!f.items()[0].is_separator());
    assert_eq!(MenuItem::count(f.items()), 9);
}

#[test]
fn wire_form_is_compact() {
    let json = serde_json::to_string(&tree()).unwrap();
    // No `checked`, `shortcut` or `children` keys where they are empty.
    assert!(!json.contains("\"checked\":false"));
    assert!(!json.contains("\"shortcut\":null"));
    assert!(!json.contains("\"children\":[]"));
    let back: Vec<MenuItem> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, tree());
    // About 200 items fit the cap.
    assert!(wire_size(&big(5, 20, 1)) < CAP);
}
