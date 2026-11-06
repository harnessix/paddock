//! The input source state machine: the eight numbered cases, the
//! plain layout table and the generic rule.

use super::*;
use crate::guest::windows::THIRD_PARTY_INPUT_METHODS;

const PINYIN: &str = "com.apple.inputmethod.SCIM.ITABC";
const GERMAN: &str = "com.apple.keylayout.German";
const SOGOU: &str = "com.sogou.inputmethod.sogou.pinyin";

fn us() -> Source {
    Source::layout(US, "U.S.", "en")
}

fn abc() -> Source {
    Source::layout(ABC, "ABC", "en")
}

fn german() -> Source {
    Source::layout(GERMAN, "German", "de")
}

fn pinyin() -> Source {
    Source::method(PINYIN, "Pinyin - Simplified", "zh-Hans")
}

fn host(sources: Vec<Source>, current: &str) -> HostState {
    HostState {
        sources,
        current: current.into(),
        reduce_motion: false,
        increase_contrast: true,
    }
}

fn set(sources: &[&str], current: &str) -> Action {
    Action::GuestSet {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        current: current.into(),
        reduce_motion: false,
        increase_contrast: true,
    }
}

#[test]
fn sync_cases() {
    // 1. US on both sides: one GuestSet, no HostSelect, nothing twice,
    //    and a resign restores nothing.
    let mut s = InputSync::new();
    let h = host(vec![us()], US);
    assert_eq!(s.on_key(&h), vec![set(&[US], US)]);
    assert_eq!(s.on_key(&h), vec![]);
    assert_eq!(s.saved(), None);
    assert_eq!(s.on_resign(), vec![]);

    // 2. Pinyin on the host: the host goes to ABC, the guest to Pinyin,
    //    the badge names it, and the record gets the saved source.
    let mut s = InputSync::new();
    let h = host(vec![pinyin(), abc()], PINYIN);
    assert_eq!(
        s.on_key(&h),
        vec![
            Action::HostSelect(ABC.into()),
            set(&[PINYIN, ABC], PINYIN),
            Action::Badge(Some("input: Pinyin - Simplified".into())),
        ]
    );
    assert_eq!(s.saved(), Some(PINYIN));
    assert_eq!(s.guest(), Some(PINYIN));
    // 8. The echo of the shim's own switch changes nothing.
    assert_eq!(s.on_host_change(&host(vec![pinyin(), abc()], ABC)), vec![]);

    // 3. Resign: the host gets Pinyin back and the badge goes.
    assert_eq!(
        s.on_resign(),
        vec![Action::Restore(PINYIN.into()), Action::Badge(None)]
    );
    assert_eq!(s.saved(), None);
    // A host change while not key is the user's business.
    assert_eq!(s.on_host_change(&host(vec![pinyin(), abc()], ABC)), vec![]);

    // 4. A host change while key: the user picks German from the menu
    //    bar. The guest follows, no restore is owed any more.
    let mut s = InputSync::new();
    let h = host(vec![pinyin(), abc(), german()], PINYIN);
    s.on_key(&h);
    assert_eq!(
        s.on_host_change(&host(vec![pinyin(), abc(), german()], GERMAN)),
        vec![set(&[PINYIN, ABC, GERMAN], GERMAN), Action::Badge(None)]
    );
    assert_eq!(s.saved(), None);
    assert_eq!(s.on_resign(), vec![]);
    // The user picks Pinyin from the menu bar while key: the host goes
    // back to ABC and the guest composes.
    s.on_key(&host(vec![pinyin(), abc(), german()], GERMAN));
    assert_eq!(
        s.on_host_change(&host(vec![pinyin(), abc(), german()], PINYIN)),
        vec![
            Action::HostSelect(ABC.into()),
            set(&[PINYIN, ABC, GERMAN], PINYIN),
            Action::Badge(Some("input: Pinyin - Simplified".into())),
        ]
    );
    assert_eq!(s.saved(), Some(PINYIN));

    // 5. The guest lacks an id: the badge names it. The current one
    //    first when it is missing.
    let mut s = InputSync::new();
    let sogou = Source::method(SOGOU, "Sogou Pinyin", "zh-Hans");
    s.on_key(&host(vec![sogou, abc()], SOGOU));
    assert_eq!(
        s.on_guest_reply(&[SOGOU.into()]),
        vec![Action::Badge(Some(
            "keyboard layout Sogou Pinyin not in the guest".into()
        ))]
    );
    assert_eq!(s.on_guest_reply(&[SOGOU.into()]), vec![]);
    assert_eq!(s.on_guest_reply(&[]), vec![]);
    // A failed input.set: the badge says so.
    assert_eq!(
        s.on_guest_error(),
        vec![Action::Badge(Some("keyboard layout not set".into()))]
    );

    // 6. Exit while key restores the host.
    let mut s = InputSync::new();
    s.on_key(&host(vec![pinyin(), abc()], PINYIN));
    assert_eq!(
        s.on_exit(),
        vec![Action::Restore(PINYIN.into()), Action::Badge(None)]
    );
    assert_eq!(s.on_exit(), vec![]);

    // 7. The Globe key while key: the guest goes to the next source, the
    //    host stays. Not key: nothing.
    let mut s = InputSync::new();
    let h = host(vec![us(), german()], US);
    s.on_key(&h);
    assert_eq!(
        s.on_globe(&h),
        vec![
            set(&[US, GERMAN], GERMAN),
            Action::Badge(Some("input: German".into()))
        ]
    );
    assert_eq!(s.guest(), Some(GERMAN));
    assert_eq!(
        s.on_globe(&h),
        vec![set(&[US, GERMAN], US), Action::Badge(None)]
    );
    s.on_resign();
    assert_eq!(s.on_globe(&h), vec![]);
    // Globe onto Pinyin while the host shows ABC: no second HostSelect.
    let mut s = InputSync::new();
    let h = host(vec![pinyin(), abc()], PINYIN);
    s.on_key(&h);
    assert_eq!(
        s.on_globe(&h),
        vec![set(&[PINYIN, ABC], ABC), Action::Badge(None)]
    );
    assert_eq!(
        s.on_globe(&h),
        vec![
            set(&[PINYIN, ABC], PINYIN),
            Action::Badge(Some("input: Pinyin - Simplified".into()))
        ]
    );
}

#[test]
fn hello_sends_the_set_without_a_switch() {
    let mut s = InputSync::new();
    let h = host(vec![pinyin(), abc()], PINYIN);
    assert_eq!(s.on_hello(&h), vec![set(&[PINYIN, ABC], PINYIN)]);
    assert_eq!(s.saved(), None);
    // Key after the hello: the switch and the badge, the set not again.
    assert_eq!(
        s.on_key(&h),
        vec![
            Action::HostSelect(ABC.into()),
            Action::Badge(Some("input: Pinyin - Simplified".into())),
        ]
    );
    // An agent restart while key: the new agent gets the set again, with
    // the saved source, not the plain one the host shows.
    assert_eq!(
        s.on_hello(&host(vec![pinyin(), abc()], ABC)),
        vec![set(&[PINYIN, ABC], PINYIN)]
    );
    // A key window again while the host is still on the plain layout
    // (the resign did not run yet): no second switch, the guest keeps
    // the saved source.
    assert_eq!(s.on_key(&host(vec![pinyin(), abc()], ABC)), vec![]);
    assert_eq!(s.saved(), Some(PINYIN));
}

#[test]
fn plain_layout_table_complete() {
    for id in NAMED_METHODS {
        assert_eq!(table_layout(id), Some(ABC), "{id}");
    }
    for bundle in THIRD_PARTY_INPUT_METHODS {
        assert!(table_layout(bundle).is_some(), "{bundle}");
        assert!(
            table_layout(&format!("{bundle}.pinyin")).is_some(),
            "{bundle}"
        );
    }
    // A table key is a whole component: `SCIMX` is not `SCIM`.
    assert_eq!(table_layout("com.apple.inputmethod.SCIMX.Y"), None);
    // The generic rule: ABC for an unknown input method without a host.
    let unknown = Source::method("org.example.inputmethod.Thing", "Thing", "xx");
    assert_eq!(plain_layout(&unknown, &[]), ABC);
    // With a host: the enabled layout of the method's language, as
    // TISCopyInputSourceForLanguage gives.
    let hindi = Source::method("org.example.inputmethod.Hindi", "Hindi", "hi-IN");
    let devanagari = Source::layout("com.apple.keylayout.Devanagari", "Devanagari", "hi");
    assert_eq!(
        plain_layout(&hindi, &[german(), devanagari.clone(), pinyin()]),
        devanagari.id
    );
    // No layout of the language: ABC, then US, then the first layout.
    assert_eq!(plain_layout(&unknown, &[german(), abc()]), ABC);
    assert_eq!(plain_layout(&unknown, &[german(), us()]), US);
    assert_eq!(plain_layout(&unknown, &[german()]), GERMAN);
    // A table entry that the host has not enabled falls back the same
    // way: Pinyin with only US enabled gives US.
    assert_eq!(plain_layout(&pinyin(), &[pinyin(), us()]), US);
    assert_eq!(plain_layout(&pinyin(), &[pinyin(), abc(), us()]), ABC);
}

#[test]
fn names_and_kinds() {
    let mut s = InputSync::new();
    assert_eq!(s.name("com.apple.keylayout.US"), "US");
    s.on_key(&host(vec![pinyin()], PINYIN));
    assert_eq!(s.name(PINYIN), "Pinyin - Simplified");
    assert_eq!(
        Action::HostSelect(ABC.into()).kind(),
        format!("host select {ABC}")
    );
    assert_eq!(set(&[US], US).kind(), format!("input.set {US} of 1"));
    assert_eq!(Action::Badge(None).kind(), "badge cleared");
    assert_eq!(Action::Badge(Some("x".into())).kind(), "badge");
    assert_eq!(
        Action::Restore(US.into()).kind(),
        format!("host restore {US}")
    );
}

#[test]
fn globe_key_fires_on_a_lone_press() {
    let mut g = GlobeKey::default();
    let down = KeyEvent::Flags {
        code: FN_KEY_CODE,
        function: true,
    };
    let up = KeyEvent::Flags {
        code: FN_KEY_CODE,
        function: false,
    };
    // Press and release: swallowed, then fired.
    assert_eq!(g.step(down), Globe::Swallow);
    assert_eq!(g.step(up), Globe::Fire);
    // A release without a press (the press went to another window).
    assert_eq!(g.step(up), Globe::Swallow);
    // Fn with a key: the key passes, nothing fires.
    assert_eq!(g.step(down), Globe::Swallow);
    assert_eq!(g.step(KeyEvent::Key), Globe::Pass);
    assert_eq!(g.step(KeyEvent::Key), Globe::Pass);
    assert_eq!(g.step(up), Globe::Swallow);
    // Other modifiers and keys pass.
    assert_eq!(
        g.step(KeyEvent::Flags {
            code: 56,
            function: false
        }),
        Globe::Pass
    );
    assert_eq!(g.step(KeyEvent::Key), Globe::Pass);
    assert_eq!(g, GlobeKey::default());
}

#[test]
fn the_guest_source_name_while_it_differs() {
    let mut s = InputSync::new();
    assert_eq!(s.guest_differs(), None);
    // US on both sides: no name for the badge of the marker.
    s.on_key(&host(vec![us()], US));
    assert_eq!(s.guest_differs(), None);
    // Pinyin: the host shows ABC, the guest composes in Pinyin.
    let mut s = InputSync::new();
    s.on_key(&host(vec![pinyin(), abc()], PINYIN));
    assert_eq!(s.guest_differs().as_deref(), Some("Pinyin - Simplified"));
    // Resign: the host shows Pinyin again, as the guest.
    s.on_resign();
    assert_eq!(s.guest_differs(), None);
}
