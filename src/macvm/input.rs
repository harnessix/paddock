//! The input sources of a session, pure with Linux tests. The view sends
//! key codes, not characters, so the guest must hold the host's input
//! source at every key event. An input method that composes (Pinyin,
//! Kotoeri, Hangul, Telex) must compose in the guest, because the view
//! cannot receive marked text: while a session window is key the host goes
//! to the plain layout of the input method and the guest to the input
//! method itself. On resign, and at the end of the shim, the host gets its
//! source back. The saved source goes into the session record, so `paddock
//! app list` restores it after a crash (item 21 of the checklist).
//!
//! [`InputSync`] is the state machine. The AppKit side (`run_input.rs`,
//! `tis.rs`) feeds it the host facts and runs the [`Action`]s: a
//! `TISSelectInputSource` on the host, an `input.set` to the guest, the
//! badge, the restore.

use super::text;
use std::collections::BTreeMap;

/// The plain layout of the input methods that type Latin letters.
pub const ABC: &str = "com.apple.keylayout.ABC";
/// The US layout, the second choice when ABC is not enabled.
pub const US: &str = "com.apple.keylayout.US";

/// One enabled input source of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `com.apple.keylayout.US`, `com.apple.inputmethod.SCIM.ITABC`.
    pub id: String,
    /// The localized name, `Pinyin - Simplified`.
    pub name: String,
    /// A plain keyboard layout (`TISTypeKeyboardLayout`). False for an
    /// input method or one of its modes: those compose.
    pub layout: bool,
    /// The languages, `zh-Hans`, `ja`.
    pub langs: Vec<String>,
}

impl Source {
    pub fn layout(id: &str, name: &str, lang: &str) -> Source {
        Source {
            id: id.into(),
            name: name.into(),
            layout: true,
            langs: vec![lang.into()],
        }
    }

    pub fn method(id: &str, name: &str, lang: &str) -> Source {
        Source {
            id: id.into(),
            name: name.into(),
            layout: false,
            langs: vec![lang.into()],
        }
    }
}

/// What the host has: its enabled sources, the current one and the two
/// accessibility settings that the guest follows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostState {
    pub sources: Vec<Source>,
    pub current: String,
    pub reduce_motion: bool,
    pub increase_contrast: bool,
}

/// What the AppKit side does, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `TISSelectInputSource` on the host.
    HostSelect(String),
    /// `input.set` to the guest.
    GuestSet {
        sources: Vec<String>,
        current: String,
        reduce_motion: bool,
        increase_contrast: bool,
    },
    /// The badge of the session windows: the text, or None to clear it.
    Badge(Option<String>),
    /// `TISSelectInputSource` of the saved source on the host, at resign
    /// and at exit.
    Restore(String),
}

/// The plain layout of each input method of macOS and of the third party
/// ones of `guest::windows::THIRD_PARTY_INPUT_METHODS`. A key matches an
/// id that equals it or starts with it and a dot, so a mode of an input
/// method (`com.apple.inputmethod.SCIM.ITABC`) finds its method
/// (`com.apple.inputmethod.SCIM`). Every one types Latin letters, so the
/// plain layout is ABC.
pub const PLAIN_LAYOUTS: &[(&str, &str)] = &[
    // Chinese, Simplified: Pinyin, Shuangpin, Wubi.
    ("com.apple.inputmethod.SCIM", ABC),
    // Chinese, Traditional: Pinyin, Zhuyin, Cangjie, Sucheng.
    ("com.apple.inputmethod.TCIM", ABC),
    // Japanese: Kotoeri, romaji and kana typing.
    ("com.apple.inputmethod.Kotoeri", ABC),
    // Korean: 2-Set, 3-Set, 390 Sebulshik, Romaja.
    ("com.apple.inputmethod.Korean", ABC),
    // Vietnamese: Telex, VNI, VIQR.
    ("com.apple.inputmethod.VietnameseIM", ABC),
    // Transliteration input methods of Hindi, Tamil and the others.
    ("com.apple.inputmethod.Transliteration", ABC),
    ("com.sogou.inputmethod.sogou", ABC),
    ("com.baidu.inputmethod.BaiduIM", ABC),
    ("com.tencent.inputmethod.QQInput", ABC),
    ("im.rime.inputmethod.Squirrel", ABC),
    ("com.googlecode.rimeime", ABC),
    ("com.google.inputmethod.Japanese", ABC),
    ("jp.sourceforge.inputmethod.aquaskk", ABC),
    ("com.valloric.inputmethod.openvanilla", ABC),
];

/// The named input methods, as ids, for the table test.
pub const NAMED_METHODS: &[&str] = &[
    "com.apple.inputmethod.SCIM.ITABC",
    "com.apple.inputmethod.SCIM.WBX",
    "com.apple.inputmethod.TCIM.Zhuyin",
    "com.apple.inputmethod.TCIM.Cangjie",
    "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese",
    "com.apple.inputmethod.Korean.2SetKorean",
    "com.apple.inputmethod.VietnameseIM.VietnameseTelex",
];

/// The table entry of an input method id, when one matches.
pub fn table_layout(method: &str) -> Option<&'static str> {
    PLAIN_LAYOUTS
        .iter()
        .filter(|(k, _)| method == *k || method.starts_with(&format!("{k}.")))
        .max_by_key(|(k, _)| k.len())
        .map(|(_, l)| *l)
}

/// The two-letter language of a tag, `zh` of `zh-Hans`.
fn lang_root(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// The plain layout that the host selects while a session window is key
/// and the guest has `method`. The table first, when the host has that
/// layout enabled; then the generic rule, the enabled layout of the
/// method's language (what `TISCopyInputSourceForLanguage` gives); then
/// ABC or US when the host has one; then the first enabled layout. ABC
/// without any host fact.
pub fn plain_layout(method: &Source, host: &[Source]) -> String {
    let enabled = |id: &str| host.iter().any(|s| s.layout && s.id == id);
    if let Some(l) = table_layout(&method.id) {
        if enabled(l) || host.is_empty() {
            return l.to_string();
        }
    }
    let roots: Vec<&str> = method.langs.iter().map(|l| lang_root(l)).collect();
    if let Some(l) = host
        .iter()
        .find(|s| s.layout && s.langs.iter().any(|l| roots.contains(&lang_root(l))))
    {
        return l.id.clone();
    }
    for id in [ABC, US] {
        if enabled(id) {
            return id.to_string();
        }
    }
    host.iter()
        .find(|s| s.layout)
        .map(|s| s.id.clone())
        .unwrap_or_else(|| ABC.to_string())
}

/// True for an id that composes: not a plain layout. Without a host entry
/// the id itself tells: every input method has `inputmethod` in its id.
fn composes(id: &str, host: &[Source]) -> bool {
    match host.iter().find(|s| s.id == id) {
        Some(s) => !s.layout,
        None => id.contains("inputmethod"),
    }
}

/// The state machine of the input sources.
#[derive(Debug, Default)]
pub struct InputSync {
    key: bool,
    /// The host source to restore: Some while the host is on a plain
    /// layout that the shim selected.
    saved: Option<String>,
    /// The plain layout the shim selected, to tell its own change
    /// notification from the user's.
    plain: Option<String>,
    /// What the host shows now.
    host_shown: Option<String>,
    /// What the guest holds, from the last `input.set`.
    guest: Option<String>,
    /// The last `input.set`, so the same one goes once.
    sent: Option<Action>,
    badge: Option<String>,
    names: BTreeMap<String, String>,
}

impl InputSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// The host source to put back if the shim dies: for the record.
    pub fn saved(&self) -> Option<&str> {
        self.saved.as_deref()
    }

    /// The guest's input source as the shim knows it.
    pub fn guest(&self) -> Option<&str> {
        self.guest.as_deref()
    }

    pub fn is_key(&self) -> bool {
        self.key
    }

    /// The name of the guest's source while it differs from what the host
    /// shows, for the badge of the marker. None while they match, or before
    /// the guest has one.
    pub fn guest_differs(&self) -> Option<String> {
        let guest = self.guest.as_deref()?;
        (self.host_shown.as_deref() != Some(guest)).then(|| self.name(guest))
    }

    fn learn(&mut self, host: &HostState) {
        for s in &host.sources {
            self.names.insert(s.id.clone(), s.name.clone());
        }
    }

    /// The name of a source for a badge: the localized name, or the last
    /// part of the id.
    pub fn name(&self, id: &str) -> String {
        self.names
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.rsplit('.').next().unwrap_or(id).to_string())
    }

    /// The `hello` of the agent: the guest gets the host's sources and
    /// the source it should hold now. No host switch and no badge: those
    /// come with the key window.
    pub fn on_hello(&mut self, host: &HostState) -> Vec<Action> {
        self.learn(host);
        self.sent = None;
        let target = match (&self.saved, self.key) {
            (Some(s), true) => s.clone(),
            _ => host.current.clone(),
        };
        let mut out = Vec::new();
        self.guest_set(host, &target, &mut out);
        out
    }

    /// A session window became key.
    pub fn on_key(&mut self, host: &HostState) -> Vec<Action> {
        self.learn(host);
        self.key = true;
        let mut out = Vec::new();
        let target = if self.plain.as_deref() == Some(host.current.as_str()) {
            // The host is still on the plain layout of a key window that
            // resigned a moment ago: the saved source is the one.
            self.saved.clone().unwrap_or_else(|| host.current.clone())
        } else {
            host.current.clone()
        };
        self.host_shown = Some(host.current.clone());
        self.switch_host(host, &target, &mut out);
        self.guest_set(host, &target, &mut out);
        self.update_badge(&target, &mut out);
        out
    }

    /// The host's current source changed (`kTISNotifySelectedKeyboardInputSourceChanged`).
    pub fn on_host_change(&mut self, host: &HostState) -> Vec<Action> {
        self.learn(host);
        if !self.key {
            return Vec::new();
        }
        if self.plain.as_deref() == Some(host.current.as_str()) {
            // The echo of the shim's own switch.
            self.host_shown = Some(host.current.clone());
            return Vec::new();
        }
        let mut out = Vec::new();
        let target = host.current.clone();
        self.host_shown = Some(target.clone());
        self.saved = None;
        self.plain = None;
        self.switch_host(host, &target, &mut out);
        self.guest_set(host, &target, &mut out);
        self.update_badge(&target, &mut out);
        out
    }

    /// The Globe key while a session window is key: the guest goes to
    /// the next enabled source of the host's list. The host keeps its
    /// plain layout.
    pub fn on_globe(&mut self, host: &HostState) -> Vec<Action> {
        self.learn(host);
        if !self.key || host.sources.is_empty() {
            return Vec::new();
        }
        let current = self
            .guest
            .clone()
            .or_else(|| self.saved.clone())
            .unwrap_or_else(|| host.current.clone());
        let at = host.sources.iter().position(|s| s.id == current);
        let next = match at {
            Some(i) => host.sources[(i + 1) % host.sources.len()].id.clone(),
            None => host.sources[0].id.clone(),
        };
        let mut out = Vec::new();
        self.guest_set(host, &next, &mut out);
        self.update_badge(&next, &mut out);
        out
    }

    /// The last session window resigned key: the host gets its source
    /// back and the badge goes.
    pub fn on_resign(&mut self) -> Vec<Action> {
        self.key = false;
        self.release()
    }

    /// The shim ends: the same as a resign, also while key.
    pub fn on_exit(&mut self) -> Vec<Action> {
        self.key = false;
        self.release()
    }

    fn release(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(saved) = self.saved.take() {
            self.plain = None;
            self.host_shown = Some(saved.clone());
            out.push(Action::Restore(saved));
        }
        if self.badge.take().is_some() {
            out.push(Action::Badge(None));
        }
        out
    }

    /// The reply of the guest to `input.set`: the ids it does not have.
    /// The badge names the current one when it is missing, else the
    /// first missing one.
    pub fn on_guest_reply(&mut self, missing: &[String]) -> Vec<Action> {
        let Some(id) = missing
            .iter()
            .find(|m| Some(m.as_str()) == self.guest.as_deref())
            .or_else(|| missing.first())
        else {
            return Vec::new();
        };
        let badge = text::layout_missing(&self.name(id));
        self.set_badge(Some(badge))
    }

    /// `input.set` failed or timed out.
    pub fn on_guest_error(&mut self) -> Vec<Action> {
        self.set_badge(Some(text::keyboard_layout_not_set()))
    }

    /// The host goes to the plain layout of `target` when `target`
    /// composes and the host is on it.
    fn switch_host(&mut self, host: &HostState, target: &str, out: &mut Vec<Action>) {
        if !composes(target, &host.sources) {
            return;
        }
        if self.host_shown.as_deref() != Some(target) {
            return;
        }
        let method = host
            .sources
            .iter()
            .find(|s| s.id == target)
            .cloned()
            .unwrap_or_else(|| Source::method(target, &self.name(target), ""));
        let plain = plain_layout(&method, &host.sources);
        if plain == target {
            return;
        }
        self.saved = Some(target.to_string());
        self.plain = Some(plain.clone());
        self.host_shown = Some(plain.clone());
        out.push(Action::HostSelect(plain));
    }

    fn guest_set(&mut self, host: &HostState, target: &str, out: &mut Vec<Action>) {
        let mut sources: Vec<String> = host.sources.iter().map(|s| s.id.clone()).collect();
        if !sources.iter().any(|s| s == target) {
            sources.push(target.to_string());
        }
        let action = Action::GuestSet {
            sources,
            current: target.to_string(),
            reduce_motion: host.reduce_motion,
            increase_contrast: host.increase_contrast,
        };
        self.guest = Some(target.to_string());
        if self.sent.as_ref() == Some(&action) {
            return;
        }
        self.sent = Some(action.clone());
        out.push(action);
    }

    /// The badge shows the guest's source while it differs from what the
    /// host shows.
    fn update_badge(&mut self, target: &str, out: &mut Vec<Action>) {
        let badge = if self.host_shown.as_deref() == Some(target) {
            None
        } else {
            Some(text::input_source(&self.name(target)))
        };
        out.extend(self.set_badge(badge));
    }

    fn set_badge(&mut self, badge: Option<String>) -> Vec<Action> {
        if self.badge == badge {
            return Vec::new();
        }
        self.badge = badge.clone();
        vec![Action::Badge(badge)]
    }
}

/// The key code of the Fn (Globe) key.
pub const FN_KEY_CODE: u16 = 63;

/// A key event of a session window, reduced to what the Globe catch needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    /// `flagsChanged`: the key code and whether the Function flag is on.
    Flags { code: u16, function: bool },
    /// A `keyDown` or `keyUp`.
    Key,
}

/// What the window does with the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Globe {
    /// On to the view, as every other event.
    Pass,
    /// A Fn flags change: not for the guest.
    Swallow,
    /// A lone press of the Globe key: switch the guest's source.
    Fire,
}

/// The Globe key catch, pure: a press and release of the Fn key with no
/// other key in between fires once, on the release. Fn held with another
/// key (Fn-Delete, Fn-arrows) is a combination: the keys pass, with their
/// own Function flag, and nothing fires. The two flags changes of the Fn
/// key itself never reach the guest, so the guest's own Globe setting does
/// not switch a second time.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GlobeKey {
    down: bool,
    combo: bool,
}

impl GlobeKey {
    pub fn step(&mut self, ev: KeyEvent) -> Globe {
        match ev {
            KeyEvent::Flags { code, function } if code == FN_KEY_CODE => {
                if function {
                    self.down = true;
                    self.combo = false;
                    Globe::Swallow
                } else {
                    let fire = self.down && !self.combo;
                    self.down = false;
                    self.combo = false;
                    if fire {
                        Globe::Fire
                    } else {
                        Globe::Swallow
                    }
                }
            }
            KeyEvent::Flags { .. } => Globe::Pass,
            KeyEvent::Key => {
                if self.down {
                    self.combo = true;
                }
                Globe::Pass
            }
        }
    }
}

impl Action {
    /// A short text for the logs: the kind and the id, no name.
    pub fn kind(&self) -> String {
        match self {
            Action::HostSelect(id) => format!("host select {id}"), // not user text
            Action::GuestSet {
                sources, current, ..
            } => format!("input.set {current} of {}", sources.len()), // not user text
            Action::Badge(Some(_)) => "badge".into(),
            Action::Badge(None) => "badge cleared".into(),
            Action::Restore(id) => format!("host restore {id}"), // not user text
        }
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
