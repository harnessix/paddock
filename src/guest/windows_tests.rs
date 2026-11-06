use super::*;

const APP: &str = "com.openai.chat";

fn raw(id: u32, pid: u32, owner: &str, bundle: Option<&str>, layer: i64) -> RawWindow {
    RawWindow {
        id,
        pid,
        owner: owner.into(),
        bundle: bundle.map(String::from),
        layer,
        bounds: Frame::new(10.0 * id as f64, 20.0, 800.0, 600.0),
        alpha: 1.0,
    }
}

/// launchd 1 > app 100 > helper 101 > grandchild 102. Unrelated 200.
/// Input method 300 (Pinyin). Finder 400. TCC prompt 500.
fn procs() -> ProcTable {
    [
        (1, 0),
        (100, 1),
        (101, 100),
        (102, 101),
        (200, 1),
        (300, 1),
        (400, 1),
        (500, 1),
    ]
    .into()
}

fn follow(focused: bool) -> Follow {
    Follow {
        root: Some(100),
        procs: procs(),
        app_focused: focused,
        extra_input_methods: vec!["com.example.ime".into()],
    }
}

fn list() -> Vec<RawWindow> {
    vec![
        raw(1, 100, "ChatGPT", Some(APP), 0),
        raw(2, 102, "ChatGPT Helper", Some(APP), 0),
        raw(3, 200, "Safari", Some("com.apple.Safari"), 0),
        raw(4, 300, "SCIM", Some("com.apple.inputmethod.SCIM"), 101),
        raw(5, 400, "Finder", Some("com.apple.finder"), 0),
        raw(6, 500, "UserNotificationCenter", None, 8),
    ]
}

fn ax(w: &RawWindow) -> AxInfo {
    AxInfo {
        title: format!("Window {}", w.id),
        hint: None,
        minimized: false,
        focused: w.id == 1,
        resizable: true,
        movable: true,
        ..AxInfo::default()
    }
}

#[test]
fn followed_set_cases() {
    let f = follow(true);
    assert_eq!(f.app_pids(), [100, 101, 102].into());
    assert!(f.app_alive());
    // The helper, the grandchild, the input method while focused and the
    // prompt process are followed. Safari and the Finder are not.
    let set = followed(&list(), &f);
    assert_eq!(set, [100, 101, 102, 300, 500].into());
    // Not focused: the input method goes.
    let set = followed(&list(), &follow(false));
    assert_eq!(set, [100, 101, 102, 500].into());
    // No root: only the prompts.
    let none = Follow {
        root: None,
        ..follow(true)
    };
    assert_eq!(followed(&list(), &none), [500].into());
    assert!(!none.app_alive());
    // A root that exited is not alive.
    let mut gone = follow(true);
    gone.procs.remove(&100);
    assert!(!gone.app_alive());
    // The profile's own input method counts.
    assert!(is_input_method("com.example.ime", &f.extra_input_methods));
    assert!(is_input_method("com.sogou.inputmethod.sogou.pinyin", &[]));
    assert!(!is_input_method("com.apple.Safari", &[]));
}

#[test]
fn input_method_windows_are_popovers_of_key_window() {
    let snap = Snapshot::build(&list(), &follow(true), Some(1), ax);
    let ime = snap.get(4).expect("the input method window");
    assert_eq!(ime.level, Level::Popover);
    assert_eq!(ime.parent, Some(1));
    assert_eq!(ime.app, "com.apple.inputmethod.SCIM");
    // The prompt is a floating window of `system`.
    let prompt = snap.get(6).unwrap();
    assert_eq!(
        (prompt.app.as_str(), prompt.level),
        (SYSTEM_APP, Level::Floating)
    );
    assert_eq!(prompt.parent, None);
    // Safari and the Finder are not in the list; the order is front first.
    assert_eq!(snap.ids(), vec![1, 2, 4, 6]);
    assert_eq!(snap.key(), Some(1));
    // Without focus the input method window is gone.
    let snap = Snapshot::build(&list(), &follow(false), Some(1), ax);
    assert_eq!(snap.ids(), vec![1, 2, 6]);
}

#[test]
fn level_mapping_covers_the_seven_levels() {
    assert_eq!(level(0, None), Level::Normal);
    assert_eq!(level(0, Some("AXStandardWindow")), Level::Normal);
    assert_eq!(level(3, None), Level::Floating);
    assert_eq!(level(0, Some("AXFloatingWindow")), Level::Floating);
    assert_eq!(level(8, Some("AXDialog")), Level::Floating);
    assert_eq!(level(24, None), Level::Menu);
    assert_eq!(level(101, None), Level::Menu);
    assert_eq!(level(101, Some("AXMenu")), Level::Menu);
    assert_eq!(level(101, Some("AXUnknown")), Level::Popover);
    assert_eq!(level(101, Some("AXPopover")), Level::Popover);
    assert_eq!(level(102, None), Level::Tooltip);
    assert_eq!(level(0, Some("AXSheet")), Level::Sheet);
    assert_eq!(level(3, Some("AXSheet")), Level::Sheet);
    assert_eq!(level(-20, None), Level::Hidden);
    assert_eq!(level(1000, None), Level::Hidden);
    let seen: std::collections::BTreeSet<String> = [
        level(0, None),
        level(3, None),
        level(24, None),
        level(101, Some("AXPopover")),
        level(102, None),
        level(0, Some("AXSheet")),
        level(-1, None),
    ]
    .iter()
    .map(|l| format!("{l:?}"))
    .collect();
    assert_eq!(seen.len(), Level::ALL.len());
}

#[test]
fn diff_cases() {
    let base = Snapshot::build(&list(), &follow(false), None, ax);
    // Unchanged: None.
    assert_eq!(diff(&base, &base), None);
    // A new window.
    let mut l = list();
    l.push(raw(7, 101, "ChatGPT Helper", Some(APP), 0));
    let new = Snapshot::build(&l, &follow(false), None, ax);
    let ev = diff(&base, &new).unwrap();
    assert_eq!(ev.order, vec![1, 2, 6, 7]);
    assert_eq!(ev.changed.iter().map(|w| w.id).collect::<Vec<_>>(), vec![7]);
    assert!(ev.removed.is_empty());
    // A moved window.
    let mut l = list();
    l[0].bounds = Frame::new(100.0, 100.0, 800.0, 600.0);
    let moved = Snapshot::build(&l, &follow(false), None, ax);
    let ev = diff(&base, &moved).unwrap();
    assert_eq!(ev.changed.len(), 1);
    assert_eq!(ev.changed[0].frame, Frame::new(100.0, 100.0, 800.0, 600.0));
    assert!(ev.removed.is_empty());
    // A removed window.
    let mut l = list();
    l.remove(1);
    let fewer = Snapshot::build(&l, &follow(false), None, ax);
    let ev = diff(&base, &fewer).unwrap();
    assert_eq!(ev.removed, vec![2]);
    assert!(ev.changed.is_empty());
    assert_eq!(ev.order, vec![1, 6]);
    // A reorder with no other change: order only.
    let mut l = list();
    l.swap(0, 1);
    let reordered = Snapshot::build(&l, &follow(false), None, ax);
    let ev = diff(&base, &reordered).unwrap();
    assert_eq!(ev.order, vec![2, 1, 6]);
    assert!(ev.changed.is_empty() && ev.removed.is_empty());
    // A title change.
    let retitled = Snapshot::build(&list(), &follow(false), None, |w| {
        let mut i = ax(w);
        if w.id == 2 {
            i.title = "Settings".into();
        }
        i
    });
    let ev = diff(&base, &retitled).unwrap();
    assert_eq!(ev.changed.len(), 1);
    assert_eq!(ev.changed[0].title, "Settings");
    // An input method window that appears with the focus: a popover of
    // the key window.
    let with_ime = Snapshot::build(&list(), &follow(true), Some(1), ax);
    let ev = diff(&base, &with_ime).unwrap();
    assert_eq!(ev.changed.len(), 1);
    assert_eq!(ev.changed[0].id, 4);
    assert_eq!(ev.changed[0].level, Level::Popover);
    assert_eq!(ev.changed[0].parent, Some(1));
    // The full list after a hello carries every window.
    let f = full(&base);
    assert_eq!(f.order, base.ids());
    assert_eq!(f.changed.len(), 3);
}

#[test]
fn frames_compare_with_a_tolerance() {
    let a = Frame::new(0.0, 0.0, 10.0, 10.0);
    assert!(a.near(&Frame::new(1.0, -1.0, 11.0, 9.5), 1.0));
    assert!(!a.near(&Frame::new(2.5, 0.0, 10.0, 10.0), 1.0));
}
