//! The Text Input Sources API of Carbon (HIToolbox), macOS only, for the
//! shim (the host's sources, the switch to the plain layout and its
//! restore), `paddock app list` (the restore after a crash, item 21) and
//! the guest agent (`input.set`). The API is C, declared here. A source
//! is a `TISInputSourceRef` that the list array owns: a [`List`] keeps
//! the array alive while the pointers are in use.

use super::input::Source;
use super::text;
use objc2::rc::Retained;
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::ptr::NonNull;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    static kTISPropertyInputSourceID: *const CFString;
    static kTISPropertyLocalizedName: *const CFString;
    static kTISPropertyInputSourceType: *const CFString;
    static kTISPropertyInputSourceCategory: *const CFString;
    static kTISPropertyInputSourceLanguages: *const CFString;
    static kTISPropertyInputSourceIsSelectCapable: *const CFString;
    static kTISCategoryKeyboardInputSource: *const CFString;
    static kTISTypeKeyboardLayout: *const CFString;
    static kTISNotifySelectedKeyboardInputSourceChanged: *const CFString;
    fn TISCreateInputSourceList(properties: *const CFDictionary, include_all: u8) -> *mut CFArray;
    fn TISCopyCurrentKeyboardInputSource() -> *mut c_void;
    fn TISGetInputSourceProperty(source: *mut c_void, key: *const CFString) -> *const c_void;
    fn TISEnableInputSource(source: *mut c_void) -> i32;
    fn TISDisableInputSource(source: *mut c_void) -> i32;
    fn TISSelectInputSource(source: *mut c_void) -> i32;
    static kTISPropertyUnicodeKeyLayoutData: *const CFString;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut c_void;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        layout: *const c_void,
        code: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        len: *mut usize,
        chars: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFPreferencesGetAppIntegerValue(
        key: *const CFString,
        application: *const CFString,
        exists: *mut u8,
    ) -> isize;
}

/// The status of a select or an enable of an id that no list has.
pub const NOT_FOUND: i32 = -1;

/// A property of a source as text, when it is a CFString.
fn string(source: *mut c_void, key: *const CFString) -> Option<String> {
    // SAFETY: a valid source and a constant key; the property is a
    // CFString that the source owns.
    let p = unsafe { TISGetInputSourceProperty(source, key) };
    let s = NonNull::new(p.cast_mut())?.cast::<CFString>();
    Some(unsafe { s.as_ref() }.to_string())
}

/// A property of a source as a bool; false when it is not set.
fn boolean(source: *mut c_void, key: *const CFString) -> bool {
    // SAFETY: as in `string`; the property is a CFBoolean.
    let p = unsafe { TISGetInputSourceProperty(source, key) };
    NonNull::new(p.cast_mut())
        .map(|b| unsafe { b.cast::<CFBoolean>().as_ref() }.value())
        .unwrap_or(false)
}

/// A property of a source as texts, when it is a CFArray of CFStrings.
fn strings(source: *mut c_void, key: *const CFString) -> Vec<String> {
    // SAFETY: as in `string`; the property is a CFArray of CFStrings.
    let p = unsafe { TISGetInputSourceProperty(source, key) };
    let Some(arr) = NonNull::new(p.cast_mut()) else {
        return Vec::new();
    };
    let arr = unsafe { arr.cast::<CFArray>().as_ref() };
    (0..arr.count())
        .filter_map(|i| {
            // SAFETY: `i` is within the count; the values are CFStrings.
            let v = unsafe { arr.value_at_index(i) };
            NonNull::new(v.cast_mut()).map(|s| unsafe { s.cast::<CFString>().as_ref() }.to_string())
        })
        .collect()
}

/// A constant of the API as text.
fn constant(key: *const CFString) -> String {
    // SAFETY: the constants are valid CFStrings for the process's life.
    unsafe { &*key }.to_string()
}

/// The input sources of the Mac with their ids: every one
/// (`include_all`), or the enabled ones. The pointers are valid while the
/// list lives.
pub struct List {
    _keep: Option<CFRetained<CFArray>>,
    pub entries: Vec<(String, *mut c_void)>,
}

impl List {
    pub fn new(include_all: bool) -> List {
        // SAFETY: no filter; the list is a +1 reference.
        let list = unsafe { TISCreateInputSourceList(std::ptr::null(), u8::from(include_all)) };
        let Some(list) = NonNull::new(list) else {
            return List {
                _keep: None,
                entries: Vec::new(),
            };
        };
        let list: CFRetained<CFArray> = unsafe { CFRetained::from_raw(list) };
        let mut entries = Vec::new();
        for i in 0..list.count() {
            // SAFETY: the array holds TISInputSourceRef values that it retains.
            let src = unsafe { list.value_at_index(i) }.cast_mut();
            if let Some(id) = string(src, unsafe { kTISPropertyInputSourceID }) {
                entries.push((id, src));
            }
        }
        List {
            _keep: Some(list),
            entries,
        }
    }

    /// The source of an id.
    fn find(&self, id: &str) -> Option<*mut c_void> {
        self.entries.iter().find(|(x, _)| x == id).map(|(_, s)| *s)
    }

    /// Enable the source of an id: the OSStatus (0 on success), or None
    /// when the list has no such id.
    pub fn enable(&self, id: &str) -> Option<i32> {
        // SAFETY: a valid source of this list, which is alive.
        self.find(id)
            .map(|src| unsafe { TISEnableInputSource(src) })
    }

    /// Select the source of an id: the OSStatus, or None for an unknown id.
    pub fn select(&self, id: &str) -> Option<i32> {
        // SAFETY: a valid source of this list, which is alive.
        self.find(id)
            .map(|src| unsafe { TISSelectInputSource(src) })
    }

    /// The source of an id, from the enabled list first and then from
    /// every source (a disabled one can still be selected after an
    /// enable).
    pub fn find_any(id: &str) -> Option<(List, *mut c_void)> {
        for all in [false, true] {
            let list = List::new(all);
            if let Some(src) = list.find(id) {
                return Some((list, src));
            }
        }
        None
    }
}

/// The enabled keyboard input sources of the Mac that the user can
/// select: the layouts and the input methods with their modes, not the
/// palettes (Emoji & Symbols) and not the holders of modes.
pub fn enabled_sources() -> Vec<Source> {
    let list = List::new(false);
    let (category, layout_type) = unsafe {
        (
            constant(kTISCategoryKeyboardInputSource),
            constant(kTISTypeKeyboardLayout),
        )
    };
    let mut out = Vec::new();
    for (id, src) in &list.entries {
        // SAFETY: the keys are constants of the API.
        let (cat, kind, selectable, name, langs) = unsafe {
            (
                string(*src, kTISPropertyInputSourceCategory),
                string(*src, kTISPropertyInputSourceType),
                boolean(*src, kTISPropertyInputSourceIsSelectCapable),
                string(*src, kTISPropertyLocalizedName),
                strings(*src, kTISPropertyInputSourceLanguages),
            )
        };
        if cat.as_deref() != Some(category.as_str()) || !selectable {
            continue;
        }
        out.push(Source {
            id: id.clone(),
            name: name.unwrap_or_else(|| id.clone()),
            layout: kind.as_deref() == Some(layout_type.as_str()),
            langs,
        });
    }
    out
}

/// The current keyboard input source.
pub fn current() -> Option<String> {
    // SAFETY: a +1 reference, or null.
    let src = unsafe { TISCopyCurrentKeyboardInputSource() };
    let id = string(src, unsafe { kTISPropertyInputSourceID });
    // SAFETY: the copy is a +1 reference; the drop releases it.
    let _owned: Option<CFRetained<CFType>> =
        NonNull::new(src.cast::<CFType>()).map(|p| unsafe { CFRetained::from_raw(p) });
    id
}

/// The character of the key `code` with cmd, and with shift when
/// `shift`, on the current keyboard layout: what the menus of an app match
/// a shortcut against. None for a layout without Unicode data, a dead key
/// or a key with no single character. It must run on the main thread:
/// since macOS 14 the TIS calls assert it and end the process. On another
/// thread it calls nothing and gives None.
pub fn key_char(code: u16, shift: bool) -> Option<char> {
    if objc2::MainThreadMarker::new().is_none() {
        log::warn!("tis: key_char called off the main thread");
        return None;
    }
    // kUCKeyActionDown; cmdKey and shiftKey of Events.h, shifted right by
    // 8; kUCKeyTranslateNoDeadKeysMask.
    const ACTION_DOWN: u16 = 0;
    const CMD: u32 = 0x01;
    const SHIFT: u32 = 0x02;
    const NO_DEAD_KEYS: u32 = 1;
    // SAFETY: a +1 reference, or null; the drop below releases it.
    let src = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    let _owned: Option<CFRetained<CFType>> =
        NonNull::new(src.cast::<CFType>()).map(|p| unsafe { CFRetained::from_raw(p) });
    if src.is_null() {
        return None;
    }
    // SAFETY: a valid source; the property is a CFData that it owns.
    let data = unsafe { TISGetInputSourceProperty(src, kTISPropertyUnicodeKeyLayoutData) };
    if data.is_null() {
        return None;
    }
    // SAFETY: the bytes of the CFData, alive while `src` is.
    let layout = unsafe { CFDataGetBytePtr(data) };
    if layout.is_null() {
        return None;
    }
    let modifiers = CMD | if shift { SHIFT } else { 0 };
    let mut dead = 0u32;
    let mut len = 0usize;
    let mut buf = [0u16; 4];
    // SAFETY: a valid layout, a buffer of `buf.len()` units and its length.
    let r = unsafe {
        UCKeyTranslate(
            layout.cast(),
            code,
            ACTION_DOWN,
            modifiers,
            u32::from(LMGetKbdType()),
            NO_DEAD_KEYS,
            &mut dead,
            buf.len(),
            &mut len,
            buf.as_mut_ptr(),
        )
    };
    if r != 0 || len == 0 {
        return None;
    }
    let mut it = char::decode_utf16(buf[..len.min(buf.len())].iter().copied());
    match (it.next(), it.next()) {
        (Some(Ok(c)), None) => Some(c),
        _ => None,
    }
}

/// The localized name of a source, enabled or not.
pub fn name_of(id: &str) -> Option<String> {
    let (_list, src) = List::find_any(id)?;
    string(src, unsafe { kTISPropertyLocalizedName })
}

/// Select a source by id. The error names the id and the OSStatus.
pub fn select(id: &str) -> Result<(), String> {
    let Some((_list, src)) = List::find_any(id) else {
        return Err(text::input_source_select_failed(id, NOT_FOUND));
    };
    // SAFETY: a valid source of the list that `_list` keeps alive.
    let r = unsafe { TISSelectInputSource(src) };
    if r == 0 {
        Ok(())
    } else {
        Err(text::input_source_select_failed(id, r))
    }
}

/// Disable a source by id (the check scripts restore the Mac).
pub fn disable(id: &str) -> bool {
    let Some((_list, src)) = List::find_any(id) else {
        return false;
    };
    // SAFETY: a valid source of the list that `_list` keeps alive.
    unsafe { TISDisableInputSource(src) == 0 }
}

/// The distributed notification of a change of the current source, as
/// the name `NSDistributedNotificationCenter` takes.
pub fn changed_notification() -> Retained<NSString> {
    NSString::from_str(&constant(unsafe {
        kTISNotifySelectedKeyboardInputSourceChanged
    }))
}

/// True when the Globe (Fn) key alone changes the input source on this
/// Mac: `AppleFnUsageType` of `com.apple.HIToolbox` is 1, or is not set
/// (the default on a Mac with a Globe key). Then the shim catches the key
/// while a session window is key and switches the guest instead.
pub fn globe_switches_sources() -> bool {
    let key = CFString::from_str("AppleFnUsageType");
    let app = CFString::from_str("com.apple.HIToolbox");
    let mut exists: u8 = 0;
    // SAFETY: two valid CFStrings and a place for the flag.
    let value = unsafe {
        CFPreferencesGetAppIntegerValue(
            std::ptr::from_ref(&*key),
            std::ptr::from_ref(&*app),
            &mut exists,
        )
    };
    exists == 0 || value == 1
}

#[cfg(test)]
#[path = "tis_tests.rs"]
mod tests;
