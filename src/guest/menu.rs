//! The menu bar of the app in the guest, over Accessibility:
//! `kAXMenuBarAttribute` of the app two levels deep, into the [`MenuItem`]
//! tree, and `kAXPressAction` on an item by its path. Every element gets the
//! 1 s messaging timeout of `ax.rs`, and the first call that times out ends
//! the read with `Timeout`, so a hung app costs one second, not one per item.
//! The cap is the main loop's.
//!
//! The paths are indices into the `AXChildren` arrays: the menu bar
//! item, then the item in each menu. The Apple menu (index 0) and the
//! app's own menu (index 1) are left out: the host keeps the paddock app
//! menu first.

use super::ax::{attr_result, ax_error, bool_attr, Ax, AX_TIMEOUT};
use super::platform::Fail;
use crate::macvm::menu::{MenuItem, Shortcut, DEPTH};
use crate::macvm::proto::ErrorCode;
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFArray, CFNumber, CFRetained, CFString};
use std::ptr::NonNull;

/// The first menu bar item that the host shows: after the Apple menu and
/// the app menu.
pub const FIRST_MENU: usize = 2;

/// The children of an element, each with the messaging timeout. An
/// element without children gives an empty list; a call that fails
/// gives its error.
pub(super) fn children(el: &AXUIElement) -> Result<Vec<CFRetained<AXUIElement>>, AXError> {
    let arr = match attr_result(el, "AXChildren") {
        Ok(v) => v,
        Err(AXError::NoValue) | Err(AXError::AttributeUnsupported) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let Ok(arr) = arr.downcast::<CFArray>() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.count().max(0) as usize);
    for i in 0..arr.count() {
        // SAFETY: the array holds AXUIElement values that it retains.
        let p = unsafe { arr.value_at_index(i) } as *const AXUIElement;
        let Some(p) = NonNull::new(p.cast_mut()) else {
            continue;
        };
        // SAFETY: a valid element of the array; retain our own reference.
        let el = unsafe { CFRetained::retain(p) };
        let _ = unsafe { el.set_messaging_timeout(AX_TIMEOUT) };
        out.push(el);
    }
    Ok(out)
}

/// A number attribute, or None when the item has none.
fn number_attr(el: &AXUIElement, name: &'static str) -> Result<Option<i64>, AXError> {
    match attr_result(el, name) {
        Ok(v) => Ok(v.downcast::<CFNumber>().ok().and_then(|n| n.as_i64())),
        Err(AXError::NoValue) | Err(AXError::AttributeUnsupported) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A string attribute, with a timeout as the error and anything else as
/// an empty text.
pub(super) fn text_attr(el: &AXUIElement, name: &'static str) -> Result<String, AXError> {
    match attr_result(el, name) {
        Ok(v) => Ok(v
            .downcast::<CFString>()
            .ok()
            .map(|s| s.to_string())
            .unwrap_or_default()),
        Err(AXError::CannotComplete) => Err(AXError::CannotComplete),
        Err(_) => Ok(String::new()),
    }
}

/// The items of one menu element, `level` 1 for a top menu, with the
/// submenus down to [`DEPTH`]. A submenu below that stays an entry
/// without items and is disabled, so the host never presses it.
fn read_items(menu: &AXUIElement, path: &[usize], level: usize) -> Result<Vec<MenuItem>, AXError> {
    let mut out = Vec::new();
    for (i, el) in children(menu)?.iter().enumerate() {
        let mut item_path = path.to_vec();
        item_path.push(i);
        let title = text_attr(el, "AXTitle")?;
        let enabled = bool_attr(el, "AXEnabled").unwrap_or(true);
        let checked = !text_attr(el, "AXMenuItemMarkChar")?.is_empty();
        let cmd_char = text_attr(el, "AXMenuItemCmdChar")?;
        let mask = number_attr(el, "AXMenuItemCmdModifiers")?.unwrap_or(0);
        let glyph = number_attr(el, "AXMenuItemCmdGlyph")?.filter(|g| *g > 0);
        let shortcut = Shortcut::from_ax(&cmd_char, mask, glyph);
        let subs = children(el)?;
        let (children, enabled) = match subs.first() {
            None => (Vec::new(), enabled),
            Some(sub) if level < DEPTH => (read_items(sub, &item_path, level + 1)?, enabled),
            Some(_) => (Vec::new(), false),
        };
        out.push(MenuItem {
            title,
            path: item_path,
            shortcut,
            enabled,
            checked,
            children,
        });
    }
    Ok(out)
}

impl Ax {
    /// The menu bar of the app `pid`: one entry per menu after the Apple
    /// and the app menu, two levels deep, not capped.
    pub fn menu(&mut self, pid: u32) -> Result<Vec<MenuItem>, Fail> {
        if !self.trusted() {
            return Err((
                ErrorCode::Accessibility,
                "the guest agent has no Accessibility permission".into(),
            ));
        }
        let app = self.app(pid);
        let bar = attr_result(&app, "AXMenuBar")
            .map_err(|e| ax_error(e, "menu bar"))?
            .downcast::<AXUIElement>()
            .map_err(|_| (ErrorCode::Io, "the menu bar is not an element".into()))?;
        // SAFETY: a valid element.
        let _ = unsafe { bar.set_messaging_timeout(AX_TIMEOUT) };
        let read = || -> Result<Vec<MenuItem>, AXError> {
            let mut out = Vec::new();
            for (i, top) in children(&bar)?.iter().enumerate().skip(FIRST_MENU) {
                let title = text_attr(top, "AXTitle")?;
                let enabled = bool_attr(top, "AXEnabled").unwrap_or(true);
                let items = match children(top)?.first() {
                    Some(menu) => read_items(menu, &[i], 1)?,
                    None => Vec::new(),
                };
                out.push(MenuItem {
                    title,
                    path: vec![i],
                    shortcut: None,
                    enabled,
                    checked: false,
                    children: items,
                });
            }
            Ok(out)
        };
        read().map_err(|e| ax_error(e, "menu bar"))
    }

    /// `kAXPressAction` on the item at `path` of the menu bar of `pid`.
    pub fn menu_press(&mut self, pid: u32, path: &[usize]) -> Result<(), Fail> {
        if !self.trusted() {
            return Err((
                ErrorCode::Accessibility,
                "the guest agent has no Accessibility permission".into(),
            ));
        }
        let what = format!("menu item {path:?}"); // not user text
        let app = self.app(pid);
        let bar = attr_result(&app, "AXMenuBar")
            .map_err(|e| ax_error(e, &what))?
            .downcast::<AXUIElement>()
            .map_err(|_| (ErrorCode::Io, "the menu bar is not an element".into()))?;
        // SAFETY: a valid element.
        let _ = unsafe { bar.set_messaging_timeout(AX_TIMEOUT) };
        let missing = || (ErrorCode::BadRequest, format!("no menu item at {path:?}")); // not user text
        let (first, rest) = path.split_first().ok_or_else(missing)?;
        let mut el = children(&bar)
            .map_err(|e| ax_error(e, &what))?
            .get(*first)
            .cloned()
            .ok_or_else(missing)?;
        for index in rest {
            let menu = children(&el)
                .map_err(|e| ax_error(e, &what))?
                .first()
                .cloned()
                .ok_or_else(missing)?;
            el = children(&menu)
                .map_err(|e| ax_error(e, &what))?
                .get(*index)
                .cloned()
                .ok_or_else(missing)?;
        }
        // SAFETY: a valid action name on a valid element.
        match unsafe { el.perform_action(&CFString::from_static_str("AXPress")) } {
            AXError::Success => Ok(()),
            e => Err(ax_error(e, &what)),
        }
    }
}
