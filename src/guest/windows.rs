//! The window model of the guest agent, pure: which windows the agent
//! follows, the level of each, the snapshot and the diff that becomes a
//! `windows` event.

use crate::macvm::proto::{Frame, Level, Window, WindowsEvent};
pub use crate::mark::{tree, ProcTable};
use std::collections::BTreeSet;

/// A window of the window server, before the agent classifies it. The
/// list needs no permission.
#[derive(Debug, Clone, PartialEq)]
pub struct RawWindow {
    pub id: u32,
    pub pid: u32,
    /// `kCGWindowOwnerName`.
    pub owner: String,
    /// The bundle id of the owner, when it is an app.
    pub bundle: Option<String>,
    /// `kCGWindowLayer`.
    pub layer: i64,
    pub bounds: Frame,
    pub alpha: f64,
}

/// What Accessibility says about one window. Everything is optional:
/// without the permission the agent still reports the geometry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxInfo {
    pub title: String,
    /// The subrole, or the role when there is no subrole: `AXSheet`,
    /// `AXFloatingWindow`, `AXPopover`.
    pub hint: Option<String>,
    pub minimized: bool,
    pub focused: bool,
    pub resizable: bool,
    pub movable: bool,
    /// The title bar height with the toolbar, in points (`chrome.rs`).
    pub title_bar: Option<f64>,
    /// The buttons in the title bar, relative to the window's top left.
    pub controls: Vec<Frame>,
    /// `AXFullScreen`.
    pub full_screen: bool,
}

/// The app name of a guest prompt window.
pub const SYSTEM_APP: &str = "system";

/// The processes whose windows are guest prompts: a TCC dialog, a crash
/// report, a Gatekeeper prompt, a disk dialog (`dialogs.rs`). Their windows show on the host as
/// `floating` windows of the app `system`, so the user can answer them.
pub const SYSTEM_PROMPTS: &[&str] = &[
    "UserNotificationCenter",
    "coreservicesd",
    "CoreServicesUIAgent",
    "SecurityAgent",
    "DiskArbitrationAgent",
];

/// The bundle id prefix of the input methods of macOS.
pub const INPUT_METHOD_PREFIX: &str = "com.apple.inputmethod.";

/// Third party input methods. A profile can extend the list.
pub const THIRD_PARTY_INPUT_METHODS: &[&str] = &[
    "com.sogou.inputmethod.sogou",
    "com.baidu.inputmethod.BaiduIM",
    "com.tencent.inputmethod.QQInput",
    "im.rime.inputmethod.Squirrel",
    "com.googlecode.rimeime",
    "com.google.inputmethod.Japanese",
    "jp.sourceforge.inputmethod.aquaskk",
    "com.valloric.inputmethod.openvanilla",
];

/// True for the bundle id of an input method.
pub fn is_input_method(bundle: &str, extra: &[String]) -> bool {
    bundle.starts_with(INPUT_METHOD_PREFIX)
        || THIRD_PARTY_INPUT_METHODS
            .iter()
            .any(|p| bundle == *p || bundle.starts_with(&format!("{p}.")))
        || extra.iter().any(|p| bundle == p)
}

/// What the agent follows right now.
#[derive(Debug, Clone, Default)]
pub struct Follow {
    /// The pid of the app that paddock launched.
    pub root: Option<u32>,
    pub procs: ProcTable,
    /// True while the app is the front application: the input method
    /// windows belong to it then.
    pub app_focused: bool,
    /// Third party input method bundle ids from the profile.
    pub extra_input_methods: Vec<String>,
}

impl Follow {
    /// The pids of the app: its root and every descendant. Empty without
    /// a root.
    pub fn app_pids(&self) -> BTreeSet<u32> {
        match self.root {
            Some(root) => tree(&self.procs, root),
            None => BTreeSet::new(),
        }
    }

    /// True while the app process itself is in the process table.
    pub fn app_alive(&self) -> bool {
        self.root.is_some_and(|r| self.procs.contains_key(&r))
    }
}

/// Why the agent reports a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A window of the app or one of its helpers.
    App,
    /// A guest prompt.
    System,
    /// A candidate window of an input method, while the app is focused.
    InputMethod,
}

/// The kind of a window, or None for a window the agent does not report.
pub fn classify(w: &RawWindow, app_pids: &BTreeSet<u32>, follow: &Follow) -> Option<Kind> {
    if app_pids.contains(&w.pid) {
        return Some(Kind::App);
    }
    if SYSTEM_PROMPTS.contains(&w.owner.as_str()) {
        return Some(Kind::System);
    }
    if follow.app_focused
        && follow.root.is_some()
        && w.bundle
            .as_deref()
            .is_some_and(|b| is_input_method(b, &follow.extra_input_methods))
    {
        return Some(Kind::InputMethod);
    }
    None
}

/// The pids whose windows the agent reports and observes: the app pids,
/// the system prompt processes with a window, and the input method
/// processes while the app is focused.
pub fn followed(raw: &[RawWindow], follow: &Follow) -> BTreeSet<u32> {
    let app = follow.app_pids();
    let mut out = app.clone();
    for w in raw {
        if classify(w, &app, follow).is_some() {
            out.insert(w.pid);
        }
    }
    out
}

/// `kCGFloatingWindowLevel`.
const FLOATING: i64 = 3;
/// `kCGModalPanelWindowLevel`.
const MODAL_PANEL: i64 = 8;
/// `kCGUtilityWindowLevel`.
const UTILITY: i64 = 19;
/// `kCGDockWindowLevel`.
const DOCK: i64 = 20;
/// `kCGMainMenuWindowLevel`.
const MAIN_MENU: i64 = 24;
/// `kCGStatusWindowLevel`.
const STATUS: i64 = 25;
/// `kCGPopUpMenuWindowLevel`.
const POPUP_MENU: i64 = 101;
/// `kCGHelpWindowLevel` and `kCGOverlayWindowLevel`.
const HELP: i64 = 102;
/// `kCGDraggingWindowLevel`: from here up nothing is a window of the app.
const DRAGGING: i64 = 500;

/// The level of a window from its window server layer and its
/// Accessibility subrole or role.
pub fn level(layer: i64, hint: Option<&str>) -> Level {
    match hint {
        Some("AXSheet") => return Level::Sheet,
        Some("AXFloatingWindow") => return Level::Floating,
        Some("AXPopover") => return Level::Popover,
        _ => {}
    }
    match layer {
        i64::MIN..=-1 => Level::Hidden,
        0 => Level::Normal,
        FLOATING | MODAL_PANEL | UTILITY => Level::Floating,
        DOCK | MAIN_MENU | STATUS => Level::Menu,
        POPUP_MENU => match hint {
            Some("AXMenu") | None => Level::Menu,
            Some(_) => Level::Popover,
        },
        HELP => Level::Tooltip,
        DRAGGING.. => Level::Hidden,
        _ => Level::Floating,
    }
}

/// The windows the agent reports, front first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub windows: Vec<Window>,
}

impl Snapshot {
    pub fn ids(&self) -> Vec<u32> {
        self.windows.iter().map(|w| w.id).collect()
    }

    pub fn get(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// The focused window of the app, when there is one.
    pub fn key(&self) -> Option<u32> {
        self.windows.iter().find(|w| w.focused).map(|w| w.id)
    }

    /// A new snapshot from the window server list, front first. `ax`
    /// gives the Accessibility facts of each reported window. `key` is
    /// the key window of the app before this snapshot, the parent of the
    /// input method windows.
    pub fn build(
        raw: &[RawWindow],
        follow: &Follow,
        key: Option<u32>,
        mut ax: impl FnMut(&RawWindow) -> AxInfo,
    ) -> Snapshot {
        let app = follow.app_pids();
        let mut windows = Vec::new();
        // The windows that Accessibility does not know: the parts of a
        // full screen window that it draws itself.
        let mut no_element = Vec::new();
        for w in raw {
            let Some(kind) = classify(w, &app, follow) else {
                continue;
            };
            let info = ax(w);
            let (app_name, level, parent) = match kind {
                Kind::App => (
                    w.bundle.clone().unwrap_or_else(|| w.owner.clone()),
                    level(w.layer, info.hint.as_deref()),
                    None,
                ),
                Kind::System => (SYSTEM_APP.to_string(), Level::Floating, None),
                Kind::InputMethod => (
                    w.bundle.clone().unwrap_or_else(|| w.owner.clone()),
                    Level::Popover,
                    key,
                ),
            };
            windows.push(Window {
                id: w.id,
                pid: w.pid,
                app: app_name,
                title: info.title,
                frame: w.bounds,
                level,
                parent,
                minimized: info.minimized,
                focused: info.focused,
                resizable: info.resizable,
                movable: info.movable,
                title_bar: info.title_bar,
                controls: info.controls,
                radius: None,
                full_screen: info.full_screen,
            });
            if info.hint.is_none() {
                no_element.push(w.id);
            }
        }
        super::chrome::drop_drawn_parts(&mut windows, &no_element);
        Snapshot { windows }
    }
}

/// The event that takes the host from `old` to `new`, or None when
/// nothing differs. `changed` holds a window that is new or whose any
/// field differs. `order` is every id of `new`, front first.
pub fn diff(old: &Snapshot, new: &Snapshot) -> Option<WindowsEvent> {
    let changed: Vec<Window> = new
        .windows
        .iter()
        .filter(|w| old.get(w.id) != Some(w))
        .cloned()
        .collect();
    let removed: Vec<u32> = old
        .windows
        .iter()
        .map(|w| w.id)
        .filter(|id| new.get(*id).is_none())
        .collect();
    let order = new.ids();
    if changed.is_empty() && removed.is_empty() && order == old.ids() {
        return None;
    }
    Some(WindowsEvent {
        order,
        changed,
        removed,
    })
}

/// The full list, as the first event after a `hello`.
pub fn full(snap: &Snapshot) -> WindowsEvent {
    WindowsEvent {
        order: snap.ids(),
        changed: snap.windows.clone(),
        removed: Vec::new(),
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
