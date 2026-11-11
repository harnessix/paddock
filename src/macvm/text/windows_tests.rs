//! One test per function of text/windows.rs, named as the function.

use super::super::windows as t;
use super::super::BADGE_MAX;
use crate::macvm::proto::Level;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn menu_about() {
    assert_eq!(t::menu_about("Sample Notes"), "About Sample Notes");
}

#[test]
fn menu_hide() {
    assert_eq!(t::menu_hide("Sample Notes"), "Hide Sample Notes");
}

#[test]
fn menu_window() {
    assert_eq!(t::menu_window(), "Window");
}

#[test]
fn menu_received() {
    assert_eq!(t::menu_received(42), "menu of 42 items");
}

#[test]
fn menu_cut() {
    has(&t::menu_cut(20 * 1024), &["20 KiB", "submenu is left out"]);
}

#[test]
fn menu_timeout_kept() {
    has(&t::menu_timeout_kept(), &["no answer", "stays"]);
}

#[test]
fn menu_pressed() {
    assert_eq!(t::menu_pressed("File > New"), "menu press File > New");
}

#[test]
fn menu_press_dropped() {
    assert_eq!(
        t::menu_press_dropped("the item is disabled", &[2, 1]),
        "menu press dropped: the item is disabled ([2, 1])"
    );
}

#[test]
fn menu_drop_reason() {
    use crate::macvm::menu::Dropped;
    let all = [
        Dropped::NoTree,
        Dropped::NoSuchItem,
        Dropped::Disabled,
        Dropped::Submenu,
        Dropped::Away,
    ];
    for d in all {
        assert!(!t::menu_drop_reason(d).is_empty());
    }
    has(t::menu_drop_reason(Dropped::Away), &["away"]);
    has(t::menu_drop_reason(Dropped::NoSuchItem), &["no such item"]);
}

#[test]
fn menu_first_pass() {
    has(&t::menu_first_pass(), &["paddock menu"]);
}

#[test]
fn no_content_view() {
    has(&t::no_content_view(), &["content view"]);
}

#[test]
fn no_agent_for_request() {
    has(&t::no_agent_for_request(), &["guest agent"]);
}

#[test]
fn control_request_timeout() {
    has(&t::control_request_timeout(), &["in time"]);
}

#[test]
fn bench_event_invalid() {
    has(&t::bench_event_invalid(&"bad"), &["windows event", "bad"]);
}

#[test]
fn control_request_unknown() {
    assert_eq!(
        t::control_request_unknown("window fly"),
        "unknown control request: window fly"
    );
    assert!(t::control_request_unknown(&"x".repeat(200)).len() < 100);
}

#[test]
fn stream_not_built() {
    has(&t::stream_not_built(), &["viewport", "not built"]);
}

#[test]
fn size_refused() {
    for level in Level::ALL {
        let b = t::size_refused(level);
        assert!(b.chars().count() <= BADGE_MAX, "{b}");
        has(&b, &["too large", "moved back"]);
    }
    assert_eq!(
        t::size_refused(Level::Menu),
        "menu window too large, moved back"
    );
}

#[test]
fn level_word() {
    assert_eq!(t::level_word(Level::Floating), "floating");
    assert_eq!(t::level_word(Level::Hidden), "full screen");
    let words: Vec<&str> = Level::ALL.iter().map(|l| t::level_word(*l)).collect();
    assert_eq!(words.len(), 7);
    for w in &words {
        assert_eq!(words.iter().filter(|x| x == &w).count(), 1, "{w}");
    }
}

#[test]
fn open_again() {
    assert_eq!(t::open_again("Sample Notes"), "Open Sample Notes again");
}

#[test]
fn restart_agent() {
    assert_eq!(t::restart_agent(), "Restart guest agent");
}

#[test]
fn restart_guest() {
    assert_eq!(t::restart_guest(), "Restart guest");
}

#[test]
fn firewall_rules() {
    assert_eq!(t::firewall_rules(), "Firewall rules…");
}

#[test]
fn quit() {
    assert_eq!(t::quit(), "Quit");
}

#[test]
fn ax_tree_received() {
    assert_eq!(
        t::ax_tree_received(7, 42),
        "ax tree of window 7 with 42 nodes"
    );
}

#[test]
fn ax_tree_cut() {
    assert_eq!(
        t::ax_tree_cut(64 * 1024),
        "the Accessibility tree of the window is larger than 64 KiB: a subtree is left out"
    );
}

#[test]
fn ax_tree_timeout_kept() {
    assert_eq!(
        t::ax_tree_timeout_kept(),
        "the ax.tree request got no answer, the last tree stays"
    );
}

#[test]
fn ax_tree_malformed() {
    assert_eq!(
        t::ax_tree_malformed(7),
        "the Accessibility tree of window 7 has wrong paths and is dropped"
    );
}

#[test]
fn ax_watch_started() {
    assert_eq!(
        t::ax_watch_started(7),
        "an Accessibility client reads window 7: the mirror is on"
    );
}

#[test]
fn ax_tree_gone() {
    assert_eq!(
        t::ax_tree_gone(),
        "the Accessibility tree is gone with its window"
    );
}

#[test]
fn ax_pressed() {
    assert_eq!(t::ax_pressed("AXButton Bold"), "ax press AXButton Bold");
}

#[test]
fn ax_set_sent() {
    assert_eq!(t::ax_set_sent("AXTextField"), "ax set AXTextField");
}

#[test]
fn ax_action_dropped() {
    assert_eq!(
        t::ax_action_dropped("press", "the element is disabled"),
        "ax press dropped: the element is disabled"
    );
}

#[test]
fn ax_drop_reason() {
    use crate::macvm::axmirror::Dropped;
    assert_eq!(
        t::ax_drop_reason(Dropped::Stale),
        "the element is from an earlier tree"
    );
    assert_eq!(t::ax_drop_reason(Dropped::Away), "the guest agent is away");
    assert_eq!(
        t::ax_drop_reason(Dropped::Disabled),
        "the element is disabled"
    );
    assert_eq!(
        t::ax_drop_reason(Dropped::NoTree),
        "no tree, nothing to press in the guest"
    );
}
