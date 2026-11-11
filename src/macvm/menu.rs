//! The menu bar replica of an app session, the pure part: the tree of
//! [`MenuItem`]s that the guest agent reads over Accessibility and sends in
//! a `menu` event, the shortcut of an item and its `NSEvent` modifier
//! flags, the diff between two trees so the host rebuilds only the submenus
//! that changed, the 20 KiB cap that cuts whole submenus, and the
//! [`Fallback`] state of the host: no tree yet (the first pass menu), the
//! last tree, the agent away. The host never shows half a tree: a timeout,
//! a lost agent and an agent restart keep the last tree until the next
//! `menu` event, and the app's exit brings the first pass menu back. Tests
//! run on every system.
//!
//! `dock.rs` builds the `NSMenu`s from the changes, `src/guest/menu.rs`
//! reads and presses in the guest.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The cap of a `menu` event on the wire: the guest cuts whole submenus
/// until the JSON of the items fits.
pub const CAP: usize = 20 * 1024;
/// How deep the guest reads: the items of each menu, and the items of
/// their submenus.
pub const DEPTH: usize = 2;

/// One entry of the menu bar replica.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MenuItem {
    /// Empty for a separator.
    pub title: String,
    /// The indices of the item in the Accessibility tree of the guest:
    /// the menu bar item, then the item in each menu. `menu.press` names
    /// it, so a path the guest never sent cannot press anything.
    pub path: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub checked: bool,
    /// The items of a submenu. Empty for a plain item, and for a submenu
    /// that the cap cut (then `enabled` is false).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MenuItem>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl MenuItem {
    /// A separator: no title and no submenu.
    pub fn is_separator(&self) -> bool {
        self.title.is_empty() && self.children.is_empty()
    }

    /// The items of a tree, the separators and the submenu entries
    /// included.
    pub fn count(items: &[MenuItem]) -> usize {
        items.iter().map(|i| 1 + MenuItem::count(&i.children)).sum()
    }

    /// The item of a tree by its Accessibility path.
    pub fn find<'a>(items: &'a [MenuItem], path: &[usize]) -> Option<&'a MenuItem> {
        if path.is_empty() {
            return None;
        }
        let mut list = items;
        for depth in 1..=path.len() {
            let item = list.iter().find(|i| i.path == path[..depth])?;
            if depth == path.len() {
                return Some(item);
            }
            list = &item.children;
        }
        None
    }

    /// The item of a tree by its position path: the index in each list.
    pub fn at<'a>(items: &'a [MenuItem], position: &[usize]) -> Option<&'a MenuItem> {
        let (first, rest) = position.split_first()?;
        let item = items.get(*first)?;
        if rest.is_empty() {
            Some(item)
        } else {
            MenuItem::at(&item.children, rest)
        }
    }

    /// The titles along a path, `File > New`, for a log line.
    pub fn titles(items: &[MenuItem], path: &[usize]) -> String {
        let mut names = Vec::new();
        let mut list = items;
        for depth in 1..=path.len() {
            let Some(item) = list.iter().find(|i| i.path == path[..depth]) else {
                break;
            };
            names.push(item.title.as_str());
            list = &item.children;
        }
        names.join(" > ")
    }
}

// ---- Shortcuts ------------------------------------------------------------

/// The modifier keys of a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Mods {
    #[serde(default, skip_serializing_if = "is_false")]
    pub command: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub option: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub control: bool,
}

/// `NSEventModifierFlags` of AppKit, the bits the key equivalent mask
/// takes.
pub const FLAG_SHIFT: u64 = 1 << 17;
pub const FLAG_CONTROL: u64 = 1 << 18;
pub const FLAG_OPTION: u64 = 1 << 19;
pub const FLAG_COMMAND: u64 = 1 << 20;
pub const FLAG_FUNCTION: u64 = 1 << 23;

impl Mods {
    /// The `NSEventModifierFlags` bits of these modifiers.
    pub fn flags(self) -> u64 {
        let mut f = 0;
        if self.command {
            f |= FLAG_COMMAND;
        }
        if self.shift {
            f |= FLAG_SHIFT;
        }
        if self.option {
            f |= FLAG_OPTION;
        }
        if self.control {
            f |= FLAG_CONTROL;
        }
        f
    }

    /// The Carbon menu modifier mask that `AXMenuItemCmdModifiers`
    /// carries: bit 0 Shift, bit 1 Option, bit 2 Control, bit 3 no
    /// Command. Zero is Command alone.
    pub fn from_ax(mask: i64) -> Mods {
        Mods {
            command: mask & 8 == 0,
            shift: mask & 1 != 0,
            option: mask & 2 != 0,
            control: mask & 4 != 0,
        }
    }
}

/// The shortcut of an item: the key as text (`N`, `,`, `F5`, `Left`,
/// `Delete`) and the modifiers. `Cmd-Shift-N` is its text form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortcut {
    pub key: String,
    #[serde(default)]
    pub mods: Mods,
}

/// The named keys, with their key equivalent character for `NSMenuItem`
/// (the function key characters of AppKit) and the Carbon menu glyph that
/// `AXMenuItemCmdGlyph` reports.
const NAMED_KEYS: &[(&str, char, i64)] = &[
    ("Up", '\u{F700}', 104),
    ("Down", '\u{F701}', 106),
    ("Left", '\u{F702}', 100),
    ("Right", '\u{F703}', 101),
    ("Home", '\u{F729}', 102),
    ("End", '\u{F72B}', 105),
    ("PageUp", '\u{F72C}', 98),
    ("PageDown", '\u{F72D}', 107),
    ("ForwardDelete", '\u{F728}', 10),
    ("Help", '\u{F746}', 103),
    ("Clear", '\u{F739}', 28),
    ("Delete", '\u{8}', 23),
    ("Escape", '\u{1B}', 27),
    ("Return", '\r', 11),
    ("Enter", '\u{3}', 4),
    ("Tab", '\t', 2),
    ("Space", ' ', 9),
];

/// The glyph of F1 and of F16: F1 to F15 are 111 to 125, F16 to F19 are
/// 143 to 146.
const GLYPH_F1: i64 = 111;
const GLYPH_F16: i64 = 143;
/// The key equivalent character of F1; the others follow it.
const CHAR_F1: u32 = 0xF704;

impl Shortcut {
    /// The shortcut of an Accessibility menu item from its command
    /// character, its modifier mask and its glyph. None when the item has
    /// no shortcut.
    pub fn from_ax(cmd_char: &str, mask: i64, glyph: Option<i64>) -> Option<Shortcut> {
        let mods = Mods::from_ax(mask);
        let key = match cmd_char.chars().next() {
            Some(c) if !c.is_control() && !c.is_whitespace() && !is_function_char(c) => {
                c.to_uppercase().to_string()
            }
            Some(c) if is_function_char(c) => {
                let n = c as u32 - CHAR_F1 + 1;
                format!("F{n}") // not user text
            }
            Some(c) => NAMED_KEYS
                .iter()
                .find(|(_, ch, _)| *ch == c)
                .map(|(name, _, _)| name.to_string())?,
            None => match glyph {
                Some(g) if (GLYPH_F1..GLYPH_F1 + 15).contains(&g) => {
                    format!("F{}", g - GLYPH_F1 + 1) // not user text
                }
                Some(g) if (GLYPH_F16..GLYPH_F16 + 4).contains(&g) => {
                    format!("F{}", g - GLYPH_F16 + 16) // not user text
                }
                Some(g) => NAMED_KEYS
                    .iter()
                    .find(|(_, _, glyph)| *glyph == g)
                    .map(|(name, _, _)| name.to_string())?,
                None => return None,
            },
        };
        Some(Shortcut { key, mods })
    }

    /// The text form, `Cmd-Shift-N`, `Cmd-,`, `F5`.
    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.mods.command {
            s.push_str("Cmd-");
        }
        if self.mods.control {
            s.push_str("Ctrl-");
        }
        if self.mods.option {
            s.push_str("Opt-");
        }
        if self.mods.shift {
            s.push_str("Shift-");
        }
        s.push_str(&self.key);
        s
    }

    /// The text form back into a shortcut: the modifier words in any
    /// order, then the key. `Cmd--` is Command and the minus key.
    pub fn parse(text: &str) -> Option<Shortcut> {
        let mut rest = text;
        let mut mods = Mods::default();
        loop {
            if let Some(r) = rest.strip_prefix("Cmd-").filter(|r| !r.is_empty()) {
                mods.command = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Shift-").filter(|r| !r.is_empty()) {
                mods.shift = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Opt-").filter(|r| !r.is_empty()) {
                mods.option = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("Ctrl-").filter(|r| !r.is_empty()) {
                mods.control = true;
                rest = r;
            } else {
                break;
            }
        }
        if rest.is_empty() {
            return None;
        }
        let known = rest.chars().count() == 1
            || NAMED_KEYS.iter().any(|(n, _, _)| *n == rest)
            || function_number(rest).is_some();
        known.then(|| Shortcut {
            key: rest.to_string(),
            mods,
        })
    }

    /// The key equivalent of the `NSMenuItem`: a lower case letter, the
    /// symbol itself, or the function key character of a named key.
    pub fn key_equivalent(&self) -> String {
        if let Some(n) = function_number(&self.key) {
            return char::from_u32(CHAR_F1 + n - 1)
                .map(String::from)
                .unwrap_or_default();
        }
        if let Some((_, c, _)) = NAMED_KEYS.iter().find(|(n, _, _)| *n == self.key) {
            return c.to_string();
        }
        self.key.to_lowercase()
    }

    /// The key equivalent modifier mask of the `NSMenuItem`: the
    /// modifiers, and the Function flag for a function key or a named
    /// key, as the nibs of AppKit carry it.
    pub fn flags(&self) -> u64 {
        let named = function_number(&self.key).is_some()
            || NAMED_KEYS
                .iter()
                .any(|(n, c, _)| *n == self.key && *c >= '\u{F700}');
        self.mods.flags() | if named { FLAG_FUNCTION } else { 0 }
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// The number of a function key name, `F5` gives 5. F1 to F19.
fn function_number(key: &str) -> Option<u32> {
    let n: u32 = key.strip_prefix('F')?.parse().ok()?;
    (1..=19).contains(&n).then_some(n)
}

/// A function key character of AppKit, F1 to F19.
fn is_function_char(c: char) -> bool {
    (CHAR_F1..CHAR_F1 + 19).contains(&(c as u32))
}

// ---- The diff -------------------------------------------------------------

/// What the host changes in its `NSMenu`s between two trees. The paths
/// are positions: the index in each list, the first one among the app's
/// menus after the paddock app menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The items of the menu at this position differ in count, titles,
    /// shortcuts, paths or submenus: build the list again. The empty
    /// position is the menu bar itself.
    Rebuild(Vec<usize>),
    /// The item at this position changed only its state.
    State {
        position: Vec<usize>,
        enabled: bool,
        checked: bool,
    },
}

impl Change {
    /// A short text for the log.
    pub fn kind(&self) -> String {
        match self {
            Change::Rebuild(p) if p.is_empty() => "rebuild the menu bar".into(),
            Change::Rebuild(p) => format!("rebuild the menu at {p:?}"), // not user text
            Change::State {
                position,
                enabled,
                checked,
            } => format!(
                // not user text
                "item {position:?} {}{}",
                if *enabled { "enabled" } else { "disabled" },
                if *checked { ", checked" } else { "" }
            ),
        }
    }
}

/// The changes from the tree `old` to the tree `new`.
pub fn diff(old: &[MenuItem], new: &[MenuItem]) -> Vec<Change> {
    let mut out = Vec::new();
    diff_list(old, new, &[], &mut out);
    out
}

/// Two items at the same position stand for the same entry when their
/// title, shortcut, path and the fact of a submenu agree.
fn same_entry(a: &MenuItem, b: &MenuItem) -> bool {
    a.title == b.title
        && a.shortcut == b.shortcut
        && a.path == b.path
        && a.children.is_empty() == b.children.is_empty()
}

fn diff_list(old: &[MenuItem], new: &[MenuItem], position: &[usize], out: &mut Vec<Change>) {
    let aligned = old.len() == new.len() && old.iter().zip(new).all(|(a, b)| same_entry(a, b));
    if !aligned {
        out.push(Change::Rebuild(position.to_vec()));
        return;
    }
    for (i, (a, b)) in old.iter().zip(new).enumerate() {
        let mut here = position.to_vec();
        here.push(i);
        if a.enabled != b.enabled || a.checked != b.checked {
            out.push(Change::State {
                position: here.clone(),
                enabled: b.enabled,
                checked: b.checked,
            });
        }
        if !a.children.is_empty() {
            diff_list(&a.children, &b.children, &here, out);
        }
    }
}

// ---- The cap --------------------------------------------------------------

/// The bytes of the items on the wire.
pub fn wire_size(items: &[MenuItem]) -> usize {
    serde_json::to_vec(items).map(|v| v.len()).unwrap_or(0)
}

/// Cut whole submenus until the items fit `limit` bytes: the deepest
/// submenus first, the last ones first, then the top menus. A cut entry
/// keeps its title, loses its items and is disabled. True when something
/// was cut.
pub fn cap(items: &mut Vec<MenuItem>, limit: usize) -> bool {
    let mut cut = false;
    while wire_size(items) > limit {
        let done = (0..DEPTH).rev().any(|depth| cut_at_depth(items, depth));
        if !done {
            // Nothing left to cut: an empty bar fits, so this does not
            // happen, but a loop must end.
            items.clear();
            return true;
        }
        cut = true;
    }
    cut
}

/// Cut the last submenu `depth` levels down. True when one was.
fn cut_at_depth(items: &mut [MenuItem], depth: usize) -> bool {
    for item in items.iter_mut().rev() {
        if item.children.is_empty() {
            continue;
        }
        if depth == 0 {
            item.children.clear();
            item.enabled = false;
            return true;
        }
        if cut_at_depth(&mut item.children, depth - 1) {
            return true;
        }
    }
    false
}

// ---- The fallback state of the host -----------------------------------------

/// The menu state in the session record, for `paddock app status --json`
/// and the Mac checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuRecord {
    /// `tree` or `first-pass`.
    pub shown: String,
    /// The items of the tree, the separators and the submenu entries
    /// included.
    pub items: usize,
    /// The control connection of the agent is down.
    pub away: bool,
}

/// What the host main menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The app menu and the `Window` menu of paddock: no tree yet, or
    /// the app exited.
    FirstPass,
    /// The last tree of the guest.
    Tree,
}

/// Why a press was dropped instead of sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// The host shows the first pass menu: nothing to press in the guest.
    NoTree,
    /// The last tree has no item with that path: a stale entry.
    NoSuchItem,
    /// The item is disabled in the last tree.
    Disabled,
    /// The item opens a submenu; the host shows the submenu itself.
    Submenu,
    /// The control connection of the agent is down.
    Away,
}

/// The menu state of the host: the last tree, and whether the agent is
/// away. A timeout of the `menu` request, a lost connection and a new
/// `hello` keep the last tree; only a `menu` event replaces it and only
/// the app's exit removes it.
#[derive(Debug, Default)]
pub struct Fallback {
    last: Option<Vec<MenuItem>>,
    away: bool,
    /// The agent is away since this many lost connections, for the log.
    gone: u32,
}

impl Fallback {
    pub fn shown(&self) -> Shown {
        match self.last {
            Some(_) => Shown::Tree,
            None => Shown::FirstPass,
        }
    }

    /// The items the host shows: empty for the first pass menu.
    pub fn items(&self) -> &[MenuItem] {
        self.last.as_deref().unwrap_or(&[])
    }

    pub fn away(&self) -> bool {
        self.away
    }

    /// The state for the session record.
    pub fn record(&self) -> MenuRecord {
        MenuRecord {
            shown: match self.shown() {
                Shown::Tree => "tree".into(),
                Shown::FirstPass => "first-pass".into(),
            },
            items: MenuItem::count(self.items()),
            away: self.away,
        }
    }

    /// A `menu` event: the tree replaces the last one. The changes for
    /// the host menus.
    pub fn tree(&mut self, items: Vec<MenuItem>) -> Vec<Change> {
        self.away = false;
        let changes = match &self.last {
            Some(old) => diff(old, &items),
            None => vec![Change::Rebuild(Vec::new())],
        };
        self.last = Some(items);
        changes
    }

    /// The `menu` request timed out or failed: the last tree stays.
    pub fn timeout(&mut self) -> Vec<Change> {
        Vec::new()
    }

    /// The control connection closed: the last tree stays, presses are
    /// dropped until the agent is back.
    pub fn agent_gone(&mut self) -> Vec<Change> {
        self.away = true;
        self.gone += 1;
        Vec::new()
    }

    /// A new `hello`: presses go again, the tree stays until the next
    /// `menu` event.
    pub fn agent_back(&mut self) -> Vec<Change> {
        self.away = false;
        Vec::new()
    }

    /// The app exited: the first pass menu, until the next tree.
    pub fn app_exited(&mut self) -> Vec<Change> {
        match self.last.take() {
            Some(_) => vec![Change::Rebuild(Vec::new())],
            None => Vec::new(),
        }
    }

    /// A press on the item with this Accessibility path: the item, or
    /// why the press is dropped.
    pub fn press(&self, path: &[usize]) -> Result<&MenuItem, Dropped> {
        let items = self.last.as_deref().ok_or(Dropped::NoTree)?;
        let item = MenuItem::find(items, path).ok_or(Dropped::NoSuchItem)?;
        if !item.children.is_empty() {
            return Err(Dropped::Submenu);
        }
        if !item.enabled {
            return Err(Dropped::Disabled);
        }
        if self.away {
            return Err(Dropped::Away);
        }
        Ok(item)
    }
}

#[cfg(test)]
#[path = "menu_tests.rs"]
mod tests;

/// The flag constants of this file against the ones of AppKit, which
/// only a Mac has. `packaging/macos/checks/macvm-menu.sh build` runs it.
#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;
    use objc2_app_kit::{NSControlStateValueOn, NSEventModifierFlags};

    #[test]
    fn flags_match_appkit() {
        assert_eq!(FLAG_SHIFT as usize, NSEventModifierFlags::Shift.bits());
        assert_eq!(FLAG_CONTROL as usize, NSEventModifierFlags::Control.bits());
        assert_eq!(FLAG_OPTION as usize, NSEventModifierFlags::Option.bits());
        assert_eq!(FLAG_COMMAND as usize, NSEventModifierFlags::Command.bits());
        assert_eq!(
            FLAG_FUNCTION as usize,
            NSEventModifierFlags::Function.bits()
        );
        let s = Shortcut::parse("Cmd-Shift-N").unwrap();
        let mask = NSEventModifierFlags::from_bits_retain(s.flags() as usize);
        assert!(mask.contains(NSEventModifierFlags::Command | NSEventModifierFlags::Shift));
        assert_eq!(NSControlStateValueOn, 1);
    }
}
