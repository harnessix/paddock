//! macOS: read a disk dialog of the guest through Accessibility and press
//! its Ignore button (`dialogs.rs` holds the rules and the tests).

use super::ax::{self, string_attr, Ax};
use super::dialogs::{self, Seen};
use super::menu::children;
use super::windows::RawWindow;
use crate::macvm::text;
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString};

/// How deep the walk goes below the dialog window.
const DEPTH: usize = 4;

/// Read the texts and the buttons of the dialog window `w`, and press
/// Ignore when it is a disk dialog.
pub fn dismiss(ax: &mut Ax, w: &RawWindow) -> Seen {
    let mut seen = Seen::default();
    if !ax::trusted_now() {
        seen.why = Some("the agent has no Accessibility permission".into());
        return seen;
    }
    let Some(win) = ax.element(w.pid, w.id, w.bounds) else {
        seen.why = Some("the dialog has no Accessibility window".into());
        return seen;
    };
    let mut buttons = Vec::new();
    collect(&win, 0, &mut seen, &mut buttons);
    if !dialogs::relevant(&w.owner, &seen.buttons) {
        return seen;
    }
    let Some(i) = dialogs::ignore_button(&seen.buttons) else {
        seen.why = Some("the dialog has no Ignore button".into());
        return seen;
    };
    let Some(button) = buttons.get(i) else {
        seen.why = Some("the Ignore button is gone".into());
        return seen;
    };
    // SAFETY: a valid element with the messaging timeout of `children`.
    let r = unsafe { button.perform_action(&CFString::from_static_str("AXPress")) };
    if r == AXError::Success {
        seen.pressed = Some(seen.buttons[i].clone());
    } else {
        seen.why = Some(text::dialog_press_failed(r.0));
    }
    seen
}

/// The static texts and the buttons below `el`, in order.
fn collect(
    el: &AXUIElement,
    depth: usize,
    seen: &mut Seen,
    buttons: &mut Vec<CFRetained<AXUIElement>>,
) {
    if depth > DEPTH {
        return;
    }
    let Ok(kids) = children(el) else {
        return;
    };
    for k in kids {
        match string_attr(&k, "AXRole").as_deref() {
            Some("AXButton") => {
                seen.buttons
                    .push(string_attr(&k, "AXTitle").unwrap_or_default());
                buttons.push(k);
            }
            Some("AXStaticText") => {
                if let Some(v) = string_attr(&k, "AXValue").filter(|v| !v.trim().is_empty()) {
                    seen.texts.push(v);
                }
            }
            _ => collect(&k, depth + 1, seen, buttons),
        }
    }
}
