//! The Accessibility tree of a window of the app in the guest: the children
//! of the window element four levels deep, with the role, the title, the
//! value, the frame relative to the window, the enabled and focused states,
//! into [`Node`]s; `kAXPressAction` on an element by its path; and the value
//! of an element set by its path. Every element gets the 1 s messaging
//! timeout of `ax.rs`, and the first call that times out ends the read with
//! `Timeout`. The read stops at [`MAX_NODES`] nodes, and says so. The byte
//! cap is the main loop's.
//!
//! A press or a set names the role the host saw: when the element at the
//! path is another role now, nothing is pressed and the reply says so, so
//! a stale element of the host never acts on the wrong control.

use super::ax::{attr_result, ax_error, bool_attr, frame_of, Ax};
use super::menu::{children, text_attr};
use super::platform::Fail;
use crate::macvm::axmirror::{Node, DEPTH, MAX_NODES};
use crate::macvm::proto::{ErrorCode, Frame, Window};
use crate::macvm::text;
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFBoolean, CFNumber, CFRetained, CFString, CFType};

/// The value of an element as text: a string as is, a number as its
/// digits, a boolean as `1` or `0`. None for no value or another type.
fn value_text(el: &AXUIElement) -> Result<Option<String>, AXError> {
    match attr_result(el, "AXValue") {
        Ok(v) => Ok(text_of(&v)),
        Err(AXError::CannotComplete) => Err(AXError::CannotComplete),
        Err(_) => Ok(None),
    }
}

fn text_of(v: &CFType) -> Option<String> {
    if let Some(s) = v.downcast_ref::<CFString>() {
        return Some(s.to_string());
    }
    if let Some(n) = v.downcast_ref::<CFNumber>() {
        return Some(if n.is_float_type() {
            n.as_f64().map(|f| f.to_string()).unwrap_or_default()
        } else {
            n.as_i64().map(|i| i.to_string()).unwrap_or_default()
        });
    }
    if let Some(b) = v.downcast_ref::<CFBoolean>() {
        return Some(if b.as_bool() { "1" } else { "0" }.into());
    }
    None
}

/// How many nodes the read may still add, and whether it stopped.
struct Budget {
    left: usize,
    cut: bool,
}

/// The children of `parent`, `level` 1 for the children of the window,
/// down to [`DEPTH`] levels.
fn read_list(
    parent: &AXUIElement,
    path: &[usize],
    window: Frame,
    level: usize,
    budget: &mut Budget,
) -> Result<Vec<Node>, AXError> {
    let mut out = Vec::new();
    for (i, el) in children(parent)?.iter().enumerate() {
        if budget.left == 0 {
            budget.cut = true;
            break;
        }
        budget.left -= 1;
        let mut node_path = path.to_vec();
        node_path.push(i);
        let role = text_attr(el, "AXRole")?;
        let mut title = text_attr(el, "AXTitle")?;
        if title.is_empty() {
            title = text_attr(el, "AXDescription")?;
        }
        let value = value_text(el)?;
        let enabled = bool_attr(el, "AXEnabled").unwrap_or(true);
        let focused = bool_attr(el, "AXFocused").unwrap_or(false);
        let frame = frame_of(el)
            .map(|f| Frame::new(f.x - window.x, f.y - window.y, f.w, f.h))
            .unwrap_or_default();
        let children = if level < DEPTH {
            read_list(el, &node_path, window, level + 1, budget)?
        } else {
            Vec::new()
        };
        out.push(Node {
            role,
            title,
            value,
            frame,
            path: node_path,
            enabled,
            focused,
            children,
        });
    }
    Ok(out)
}

impl Ax {
    /// The tree of window `w`: its children four levels deep, not capped
    /// by bytes. The flag says the read stopped at [`MAX_NODES`].
    pub fn tree(&mut self, w: &Window) -> Result<(Vec<Node>, bool), Fail> {
        let el = self.tree_window(w)?;
        let frame = frame_of(&el).unwrap_or(w.frame);
        let mut budget = Budget {
            left: MAX_NODES,
            cut: false,
        };
        let nodes = read_list(&el, &[], frame, 1, &mut budget)
            .map_err(|e| ax_error(e, &format!("tree of window {}", w.id)))?; // not user text
        Ok((nodes, budget.cut))
    }

    /// `kAXPressAction` on the element at `path` of window `w`, when its
    /// role is still `role`.
    pub fn tree_press(&mut self, w: &Window, path: &[usize], role: &str) -> Result<(), Fail> {
        let what = format!("element {path:?} of window {}", w.id); // not user text
        let el = self.tree_element(w, path, role, &what)?;
        // SAFETY: a valid action name on a valid element.
        match unsafe { el.perform_action(&CFString::from_static_str("AXPress")) } {
            AXError::Success => Ok(()),
            e => Err(ax_error(e, &what)),
        }
    }

    /// Set `kAXValueAttribute` of the element at `path` of window `w`:
    /// a number when the element holds a number, a boolean when it holds
    /// one, else the text.
    pub fn tree_set(
        &mut self,
        w: &Window,
        path: &[usize],
        role: &str,
        value: &str,
    ) -> Result<(), Fail> {
        let what = format!("value of element {path:?} of window {}", w.id); // not user text
        let el = self.tree_element(w, path, role, &what)?;
        let current = attr_result(&el, "AXValue").ok();
        let number = current
            .as_ref()
            .and_then(|c| c.downcast_ref::<CFNumber>().map(|n| n.is_float_type()));
        let boolean = current
            .as_ref()
            .is_some_and(|c| c.downcast_ref::<CFBoolean>().is_some());
        let name = CFString::from_static_str("AXValue");
        // SAFETY: a valid attribute name and a value of the attribute's type.
        let r = unsafe {
            match number {
                Some(true) => match value.trim().parse::<f64>() {
                    Ok(f) => el.set_attribute_value(&name, &CFNumber::new_f64(f)),
                    Err(_) => return Err(not_a_number(value)),
                },
                Some(false) => match value.trim().parse::<i64>() {
                    Ok(i) => el.set_attribute_value(&name, &CFNumber::new_i64(i)),
                    Err(_) => return Err(not_a_number(value)),
                },
                None if boolean => {
                    let on = matches!(value.trim(), "1" | "true" | "yes");
                    el.set_attribute_value(&name, CFBoolean::new(on))
                }
                None => el.set_attribute_value(&name, &CFString::from_str(value)),
            }
        };
        match r {
            AXError::Success => Ok(()),
            e => Err(ax_error(e, &what)),
        }
    }

    /// The window element, or the failure for the reply.
    fn tree_window(&mut self, w: &Window) -> Result<CFRetained<AXUIElement>, Fail> {
        if !self.trusted() {
            return Err((
                ErrorCode::Accessibility,
                "the guest agent has no Accessibility permission".into(),
            ));
        }
        self.element(w.pid, w.id, w.frame)
            .ok_or((ErrorCode::NoSuchWindow, text::window_no_ax_element(w.id)))
    }

    /// The element at `path` below the window, with its role checked
    /// against the one the host saw.
    fn tree_element(
        &mut self,
        w: &Window,
        path: &[usize],
        role: &str,
        what: &str,
    ) -> Result<CFRetained<AXUIElement>, Fail> {
        let mut el = self.tree_window(w)?;
        if path.is_empty() {
            return Err((ErrorCode::BadRequest, "an empty element path".into()));
        }
        for index in path {
            el = children(&el)
                .map_err(|e| ax_error(e, what))?
                .get(*index)
                .cloned()
                .ok_or((ErrorCode::BadRequest, text::ax_element_gone(path)))?;
        }
        let now = text_attr(&el, "AXRole").map_err(|e| ax_error(e, what))?;
        if now != role {
            return Err((
                ErrorCode::BadRequest,
                text::ax_element_changed(path, role, &now),
            ));
        }
        Ok(el)
    }
}

fn not_a_number(value: &str) -> Fail {
    (ErrorCode::BadRequest, text::ax_value_not_a_number(value))
}
