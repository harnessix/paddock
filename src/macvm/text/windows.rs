//! The texts of the host windows of a session: the badges of a refused
//! window, the app menu, the menu bar replica, the Accessibility mirror,
//! the frame source, and the answers of the control socket lines that the
//! Mac checks drive.

use super::super::axmirror::Dropped as AxDropped;
use super::super::menu::Dropped;
use super::super::proto::Level;
use super::fit;

// ---- Badges (at most BADGE_MAX characters) ------------------------------

/// A window above `normal` that is larger than its level allows (item
/// 20 of the checklist): the host moved it back.
pub fn size_refused(level: Level) -> String {
    format!("{} window too large, moved back", level_word(level))
}

/// The word of a window level, for a badge.
pub fn level_word(level: Level) -> &'static str {
    match level {
        Level::Normal => "normal",
        Level::Floating => "floating",
        Level::Menu => "menu",
        Level::Popover => "popover",
        Level::Tooltip => "tooltip",
        Level::Sheet => "sheet",
        Level::Hidden => "full screen",
    }
}

// ---- Dock menu ----------------------------------------------------------

pub fn open_again(app: &str) -> String {
    format!("Open {app} again")
}

pub fn restart_agent() -> String {
    "Restart guest agent".into()
}

pub fn restart_guest() -> String {
    "Restart guest".into()
}

pub fn firewall_rules() -> String {
    "Firewall rules…".into()
}

pub fn quit() -> String {
    "Quit".into()
}

// ---- The app menu and the Window menu ------------------------------------

pub fn menu_about(app: &str) -> String {
    format!("About {app}")
}

pub fn menu_hide(app: &str) -> String {
    format!("Hide {app}")
}

pub fn menu_window() -> String {
    "Window".into()
}

// ---- The menu bar replica, the event log lines --------------------------

/// A `menu` event arrived with this many items.
pub fn menu_received(items: usize) -> String {
    format!("menu of {items} items")
}

/// The guest cut whole submenus to fit the cap.
pub fn menu_cut(limit: usize) -> String {
    format!(
        "the menu bar of the app is larger than {} KiB: a submenu is left out",
        limit / 1024
    )
}

/// The `menu` request got no reply or an error: the host menu stays.
pub fn menu_timeout_kept() -> String {
    "the menu request got no answer, the last menu bar stays".into()
}

/// The user chose this item of the replica.
pub fn menu_pressed(titles: &str) -> String {
    format!("menu press {titles}")
}

/// A press that was not sent, with the reason and the path.
pub fn menu_press_dropped(reason: &str, path: &[usize]) -> String {
    format!("menu press dropped: {reason} ({path:?})")
}

/// The reason word of a dropped press.
pub fn menu_drop_reason(d: Dropped) -> &'static str {
    match d {
        Dropped::NoTree => "the host shows the paddock menu, nothing to press in the guest",
        Dropped::NoSuchItem => "the last menu bar has no such item",
        Dropped::Disabled => "the item is disabled",
        Dropped::Submenu => "the item is a submenu",
        Dropped::Away => "the guest agent is away",
    }
}

/// The app exited: the host shows the paddock menu again.
pub fn menu_first_pass() -> String {
    "the menu bar is the paddock menu again".into()
}

// ---- The Accessibility mirror, the event log lines ------------------------

/// An `ax.tree` event arrived for this window with this many nodes.
pub fn ax_tree_received(window: u32, nodes: usize) -> String {
    format!("ax tree of window {window} with {nodes} nodes")
}

/// The guest cut whole subtrees to fit the cap.
pub fn ax_tree_cut(limit: usize) -> String {
    format!(
        "the Accessibility tree of the window is larger than {} KiB: a subtree is left out",
        limit / 1024
    )
}

/// The `ax.tree` request got no reply or an error: the last tree stays.
pub fn ax_tree_timeout_kept() -> String {
    "the ax.tree request got no answer, the last tree stays".into()
}

/// The guest sent a tree whose paths are not its positions: dropped.
pub fn ax_tree_malformed(window: u32) -> String {
    format!("the Accessibility tree of window {window} has wrong paths and is dropped")
}

/// An Accessibility client asked a session window for its children.
pub fn ax_watch_started(window: u32) -> String {
    format!("an Accessibility client reads window {window}: the mirror is on")
}

/// The window of the tree closed or the app exited: no tree.
pub fn ax_tree_gone() -> String {
    "the Accessibility tree is gone with its window".into()
}

/// An Accessibility client pressed this element of the mirror.
pub fn ax_pressed(name: &str) -> String {
    format!("ax press {name}")
}

/// An Accessibility client set the value of this element of the mirror.
pub fn ax_set_sent(name: &str) -> String {
    format!("ax set {name}")
}

/// A press or a set that was not sent, with the reason.
pub fn ax_action_dropped(what: &str, reason: &str) -> String {
    format!("ax {what} dropped: {reason}")
}

/// The reason word of a dropped press or set.
pub fn ax_drop_reason(d: AxDropped) -> &'static str {
    match d {
        AxDropped::NoTree => "no tree, nothing to press in the guest",
        AxDropped::Stale => "the element is from an earlier tree",
        AxDropped::Disabled => "the element is disabled",
        AxDropped::Away => "the guest agent is away",
    }
}

/// A host window without a content view, which AppKit never gives.
pub fn no_content_view() -> String {
    "the host window has no content view".into()
}

/// A control socket request that needs the guest agent while none is
/// connected.
pub fn no_agent_for_request() -> String {
    "no guest agent is connected".into()
}

/// The session loop did not answer a control socket line in time.
pub fn control_request_timeout() -> String {
    "the session did not answer in time".into()
}

/// The JSON of a `bench windows` line is not a windows event.
pub fn bench_event_invalid(e: &dyn std::fmt::Display) -> String {
    format!("the bench line is not a windows event: {e}")
}

/// A line on the control socket that the shim does not know.
pub fn control_request_unknown(line: &str) -> String {
    format!("unknown control request: {}", fit(line, 60))
}

/// The stream frame source is a stub: sessions use the viewport windows.
pub fn stream_not_built() -> String {
    "the stream frame source is not built: the viewport windows show the guest".into()
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
