//! The Accessibility side of the agent: one observer per followed pid, the
//! titles, the window elements, and the move, resize, raise, minimize and
//! close requests. Every element gets a messaging timeout of 1 s, so a hung
//! app cannot hang the agent. The notification callbacks only set flags: the
//! agent thread looks on its next turn. `menu.rs` reads and presses the menu
//! bar with the helpers of this file, `ax_tree.rs` the tree of a window.

use super::platform::{Fail, WindowOp};
use super::windows::{AxInfo, RawWindow};
use crate::macvm::proto::{ErrorCode, Frame, Window};
use crate::macvm::text;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXObserver, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    kCFBooleanTrue, kCFRunLoopCommonModes, CFArray, CFBoolean, CFDictionary, CFRetained, CFRunLoop,
    CFString, CFType, CGPoint, CGSize,
};
use std::collections::{BTreeSet, HashMap};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A window moved, appeared, went or changed focus: look now.
static DIRTY: AtomicBool = AtomicBool::new(false);
/// A title changed: read the titles again.
static TITLES: AtomicBool = AtomicBool::new(false);
/// The focused text field changed its value (the bench).
static VALUE: AtomicBool = AtomicBool::new(false);
/// A menu opened or an item was chosen: read the menu bar again
/// (`menu.rs`).
static MENU: AtomicBool = AtomicBool::new(false);
/// The focused element, a value or the layout changed: read the tree of
/// the watched window again (`ax_tree.rs`).
static TREE: AtomicBool = AtomicBool::new(false);
/// The layout or a title changed: read the title bars again
/// (`ax_chrome.rs`).
static CHROME: AtomicBool = AtomicBool::new(false);

/// The messaging timeout of every element, in seconds.
pub(super) const AX_TIMEOUT: f32 = 1.0;

/// The notifications of one app.
const NOTIFICATIONS: &[&str] = &[
    "AXWindowCreated",
    "AXUIElementDestroyed",
    "AXWindowMoved",
    "AXWindowResized",
    "AXWindowMiniaturized",
    "AXWindowDeminiaturized",
    "AXTitleChanged",
    "AXFocusedWindowChanged",
    "AXApplicationActivated",
    "AXApplicationDeactivated",
    "AXValueChanged",
    "AXMenuOpened",
    "AXMenuItemSelected",
    "AXFocusedUIElementChanged",
    "AXLayoutChanged",
];

/// True once after a notification.
pub fn take_dirty() -> bool {
    DIRTY.swap(false, Ordering::Relaxed)
}

/// True once after a menu notification.
pub fn take_menu_dirty() -> bool {
    MENU.swap(false, Ordering::Relaxed)
}

/// True once after a notification that changes the tree of a window.
pub fn take_tree_dirty() -> bool {
    TREE.swap(false, Ordering::Relaxed)
}

unsafe extern "C-unwind" fn changed(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    notification: NonNull<CFString>,
    _refcon: *mut c_void,
) {
    // SAFETY: the name lives for the callback.
    let name = unsafe { notification.as_ref() }.to_string();
    match name.as_str() {
        "AXValueChanged" => {
            VALUE.store(true, Ordering::Relaxed);
            TREE.store(true, Ordering::Relaxed);
        }
        "AXMenuOpened" | "AXMenuItemSelected" => MENU.store(true, Ordering::Relaxed),
        "AXFocusedUIElementChanged" => TREE.store(true, Ordering::Relaxed),
        "AXLayoutChanged" => {
            TREE.store(true, Ordering::Relaxed);
            CHROME.store(true, Ordering::Relaxed);
        }
        "AXTitleChanged" => {
            TITLES.store(true, Ordering::Relaxed);
            CHROME.store(true, Ordering::Relaxed);
            DIRTY.store(true, Ordering::Relaxed);
        }
        "AXWindowResized" | "AXUIElementDestroyed" => {
            TREE.store(true, Ordering::Relaxed);
            DIRTY.store(true, Ordering::Relaxed);
        }
        _ => DIRTY.store(true, Ordering::Relaxed),
    }
}

#[path = "ax_chrome.rs"]
mod ax_chrome;

/// The window id of an Accessibility window element, by the private
/// call that the window managers use, or None.
pub fn window_id(el: &AXUIElement) -> Option<u32> {
    extern "C" {
        fn _AXUIElementGetWindow(element: &AXUIElement, out: *mut u32) -> AXError;
    }
    let mut id: u32 = 0;
    // SAFETY: a valid element and an out pointer.
    let err = unsafe { _AXUIElementGetWindow(el, &mut id) };
    (err == AXError::Success && id != 0).then_some(id)
}

/// The Accessibility state of the agent.
pub struct Ax {
    trusted: bool,
    apps: HashMap<u32, CFRetained<AXUIElement>>,
    observers: HashMap<u32, CFRetained<AXObserver>>,
    /// The element of each window id, found once.
    elements: HashMap<u32, CFRetained<AXUIElement>>,
    titles: HashMap<u32, String>,
    /// The title bar of each window (`ax_chrome.rs`).
    bars: HashMap<u32, ax_chrome::Bar>,
    last_value: Option<(u32, String)>,
}

/// True when the agent has the Accessibility permission now. The
/// permission can arrive while the agent runs: the guided step of the
/// golden build turns it on in System Settings.
pub fn trusted_now() -> bool {
    // SAFETY: a plain query.
    unsafe { AXIsProcessTrusted() }
}

/// Ask macOS to show the Accessibility dialog for this process, once.
/// The guided step of the golden build tells the user to click it.
fn prompt() {
    // SAFETY: the key is a constant of the framework.
    let key = unsafe { kAXTrustedCheckOptionPrompt };
    let options = CFDictionary::from_slices(&[key], &[CFBoolean::new(true)]);
    // SAFETY: a dictionary with the one documented option. The typed
    // dictionary is the same object as the untyped one.
    let untyped: &CFDictionary =
        unsafe { &*(&*options as *const CFDictionary<CFString, CFBoolean>).cast() };
    let _ = unsafe { AXIsProcessTrustedWithOptions(Some(untyped)) };
}

impl Ax {
    pub fn new() -> Self {
        let trusted = trusted_now();
        if !trusted {
            log::warn!("no Accessibility permission: no titles, no window requests");
            prompt();
        }
        Ax {
            trusted,
            apps: HashMap::new(),
            observers: HashMap::new(),
            elements: HashMap::new(),
            titles: HashMap::new(),
            bars: HashMap::new(),
            last_value: None,
        }
    }

    pub fn trusted(&self) -> bool {
        self.trusted
    }

    pub(super) fn app(&mut self, pid: u32) -> CFRetained<AXUIElement> {
        self.apps
            .entry(pid)
            .or_insert_with(|| {
                // SAFETY: a pid; the element is ours.
                let el = unsafe { AXUIElement::new_application(pid as i32) };
                let _ = unsafe { el.set_messaging_timeout(AX_TIMEOUT) };
                el
            })
            .clone()
    }

    /// Keep one observer per pid of the set, and no other.
    pub fn observe(&mut self, pids: &BTreeSet<u32>) {
        if !self.trusted {
            return;
        }
        self.observers.retain(|pid, obs| {
            let keep = pids.contains(pid);
            if !keep {
                // SAFETY: the source of an observer that we made.
                let src = unsafe { obs.run_loop_source() };
                if let Some(rl) = CFRunLoop::main() {
                    rl.remove_source(Some(&src), unsafe { kCFRunLoopCommonModes });
                }
            }
            keep
        });
        let gone: Vec<u32> = self
            .apps
            .keys()
            .filter(|p| !pids.contains(*p))
            .copied()
            .collect();
        for pid in gone {
            self.apps.remove(&pid);
        }
        self.elements.retain(|_, el| {
            let mut pid: libc::pid_t = 0;
            // SAFETY: a valid element and out pointer.
            let r = unsafe { el.pid(NonNull::from(&mut pid)) };
            r == AXError::Success && pids.contains(&(pid as u32))
        });
        self.bars.retain(|id, _| self.elements.contains_key(id));
        for &pid in pids {
            if self.observers.contains_key(&pid) {
                continue;
            }
            let mut raw: *mut AXObserver = std::ptr::null_mut();
            // SAFETY: the callback has the documented signature, and
            // `raw` is a valid out pointer.
            let err =
                unsafe { AXObserver::create(pid as i32, Some(changed), NonNull::from(&mut raw)) };
            if err != AXError::Success {
                log::debug!("no observer for pid {pid}: {err:?}");
                continue;
            }
            let Some(raw) = NonNull::new(raw) else {
                continue;
            };
            // SAFETY: AXObserverCreate returns a +1 reference.
            let obs = unsafe { CFRetained::from_raw(raw) };
            let app = self.app(pid);
            for n in NOTIFICATIONS {
                let name = CFString::from_static_str(n);
                // SAFETY: a valid element and notification name, no refcon.
                let _ = unsafe { obs.add_notification(&app, &name, std::ptr::null_mut()) };
            }
            // SAFETY: the source of an observer that we made.
            let src = unsafe { obs.run_loop_source() };
            if let Some(rl) = CFRunLoop::main() {
                rl.add_source(Some(&src), unsafe { kCFRunLoopCommonModes });
            }
            self.observers.insert(pid, obs);
        }
        DIRTY.store(true, Ordering::Relaxed);
    }

    /// The facts of one window. Without the permission: defaults.
    pub fn info(&mut self, w: &RawWindow, front: Option<u32>) -> AxInfo {
        if !self.trusted {
            return AxInfo {
                resizable: true,
                movable: true,
                ..AxInfo::default()
            };
        }
        if TITLES.swap(false, Ordering::Relaxed) {
            self.titles.clear();
        }
        let Some(el) = self.element(w.pid, w.id, w.bounds) else {
            return AxInfo {
                resizable: true,
                movable: true,
                ..AxInfo::default()
            };
        };
        let title = match self.titles.get(&w.id) {
            Some(t) => t.clone(),
            None => {
                let t = string_attr(&el, "AXTitle").unwrap_or_default();
                self.titles.insert(w.id, t.clone());
                t
            }
        };
        let hint = string_attr(&el, "AXSubrole").or_else(|| string_attr(&el, "AXRole"));
        let minimized = bool_attr(&el, "AXMinimized").unwrap_or(false);
        let focused =
            front == Some(w.pid) && bool_attr(&el, "AXMain").unwrap_or(false) && !minimized;
        let full_screen = bool_attr(&el, "AXFullScreen").unwrap_or(false);
        let (title_bar, controls) = if full_screen {
            (None, Vec::new())
        } else {
            self.bar(w.id, &el, w.bounds)
        };
        AxInfo {
            title,
            hint,
            minimized,
            focused,
            resizable: settable(&el, "AXSize"),
            movable: settable(&el, "AXPosition"),
            title_bar,
            controls,
            full_screen,
        }
    }

    /// The element of a window id: by `_AXUIElementGetWindow`, else by
    /// the frame.
    pub(super) fn element(
        &mut self,
        pid: u32,
        id: u32,
        bounds: Frame,
    ) -> Option<CFRetained<AXUIElement>> {
        if let Some(el) = self.elements.get(&id) {
            return Some(el.clone());
        }
        let app = self.app(pid);
        let windows = array_attr(&app, "AXWindows")?;
        let mut by_frame = None;
        for i in 0..windows.count() {
            // SAFETY: the array holds AXUIElement values that it retains.
            let w = unsafe { windows.value_at_index(i) } as *const AXUIElement;
            let Some(w) = NonNull::new(w.cast_mut()) else {
                continue;
            };
            // SAFETY: a valid element of the array; retain our own reference.
            let w = unsafe { CFRetained::retain(w) };
            let _ = unsafe { w.set_messaging_timeout(AX_TIMEOUT) };
            match window_id(&w) {
                Some(wid) => {
                    self.elements.entry(wid).or_insert_with(|| w.clone());
                    if wid == id {
                        return Some(w);
                    }
                }
                None => {
                    if by_frame.is_none() && frame_of(&w).is_some_and(|f| f.near(&bounds, 2.0)) {
                        by_frame = Some(w);
                    }
                }
            }
        }
        let el = by_frame?;
        self.elements.insert(id, el.clone());
        Some(el)
    }

    /// One request on a window.
    /// Run `op` on the window. A move sets the position only when it
    /// changes (a live resize keeps the top left corner, and each set is
    /// a round trip to the app), then the size, and gives the frame that
    /// Accessibility reads back after it.
    pub fn op(&mut self, w: &Window, op: WindowOp) -> Result<Option<Frame>, Fail> {
        if !self.trusted {
            return Err((
                ErrorCode::Accessibility,
                "the guest agent has no Accessibility permission".into(),
            ));
        }
        let Some(el) = self.element(w.pid, w.id, w.frame) else {
            return Err((ErrorCode::NoSuchWindow, text::window_no_ax_element(w.id)));
        };
        let r = match op {
            WindowOp::Move(f) => {
                let mut p = CGPoint::new(f.x, f.y);
                let mut s = CGSize::new(f.w, f.h);
                // SAFETY: the value pointers are valid for the call.
                let pos =
                    unsafe { AXValue::new(AXValueType::CGPoint, NonNull::from(&mut p).cast()) };
                let size =
                    unsafe { AXValue::new(AXValueType::CGSize, NonNull::from(&mut s).cast()) };
                let (Some(pos), Some(size)) = (pos, size) else {
                    return Err((ErrorCode::Internal, "cannot make an AXValue".into()));
                };
                let same_place = (w.frame.x - f.x).abs() < 0.5 && (w.frame.y - f.y).abs() < 0.5;
                let same_size = (w.frame.w - f.w).abs() < 0.5 && (w.frame.h - f.h).abs() < 0.5;
                let mut r = AXError::Success;
                if !same_place || same_size {
                    // SAFETY: valid attribute names and values.
                    r = unsafe {
                        el.set_attribute_value(&CFString::from_static_str("AXPosition"), &pos)
                    };
                }
                if r == AXError::Success && !same_size {
                    r = unsafe {
                        el.set_attribute_value(&CFString::from_static_str("AXSize"), &size)
                    };
                }
                if r == AXError::Success {
                    return Ok(frame_of(&el));
                }
                r
            }
            WindowOp::Raise | WindowOp::Focus => {
                // SAFETY: a valid action name.
                let r = unsafe { el.perform_action(&CFString::from_static_str("AXRaise")) };
                if let Some(app) =
                    NSRunningApplication::runningApplicationWithProcessIdentifier(w.pid as i32)
                {
                    app.activateWithOptions(NSApplicationActivationOptions::empty());
                }
                r
            }
            WindowOp::Minimize => {
                // SAFETY: a constant of Core Foundation.
                let t: &CFBoolean = unsafe { kCFBooleanTrue }
                    .ok_or((ErrorCode::Internal, "no kCFBooleanTrue".into()))?;
                unsafe { el.set_attribute_value(&CFString::from_static_str("AXMinimized"), t) }
            }
            WindowOp::Close => match element_attr(&el, "AXCloseButton") {
                Some(button) => unsafe {
                    button.perform_action(&CFString::from_static_str("AXPress"))
                },
                None => AXError::ActionUnsupported,
            },
        };
        if r == AXError::InvalidUIElement {
            self.elements.remove(&w.id);
        }
        ax_result(r, op).map(|()| None)
    }

    /// The focused text field of the front app and its value, when it
    /// changed. `pid` is the app of the session.
    pub fn value(&mut self, pid: u32) -> Option<(u32, String)> {
        if !self.trusted || !VALUE.swap(false, Ordering::Relaxed) {
            return None;
        }
        let app = self.app(pid);
        let field = element_attr(&app, "AXFocusedUIElement")?;
        let text = string_attr(&field, "AXValue")?;
        let window = element_attr(&field, "AXWindow")?;
        let id = window_id(&window).or_else(|| {
            let f = frame_of(&window)?;
            self.elements
                .iter()
                .find(|(_, el)| frame_of(el).is_some_and(|x| x.near(&f, 1.0)))
                .map(|(id, _)| *id)
        })?;
        let v = (id, text);
        if self.last_value.as_ref() == Some(&v) {
            return None;
        }
        self.last_value = Some(v.clone());
        Some(v)
    }
}

impl Default for Ax {
    fn default() -> Self {
        Self::new()
    }
}

/// The error of a request from an Accessibility result.
fn ax_result(r: AXError, op: WindowOp) -> Result<(), Fail> {
    match r {
        AXError::Success => Ok(()),
        AXError::InvalidUIElement => Err((ErrorCode::NoSuchWindow, "the window is gone".into())),
        other => Err(ax_error(other, &format!("{op:?}"))),
    }
}

/// The failure of an Accessibility call that did not succeed, for the
/// reply: a timeout, the disabled API, or the raw code.
pub(super) fn ax_error(r: AXError, what: &str) -> Fail {
    match r {
        AXError::CannotComplete => (
            ErrorCode::Timeout,
            text::ax_timeout(what, Duration::from_secs_f32(AX_TIMEOUT)),
        ),
        AXError::APIDisabled => (
            ErrorCode::Accessibility,
            "the Accessibility API is disabled for the agent".into(),
        ),
        other => (ErrorCode::Io, text::ax_failed(what, other.0)),
    }
}

/// One attribute, with the error of the call.
pub(super) fn attr_result(
    el: &AXUIElement,
    name: &'static str,
) -> Result<CFRetained<CFType>, AXError> {
    let mut v: *const CFType = std::ptr::null();
    // SAFETY: a valid element, name and out pointer.
    let err =
        unsafe { el.copy_attribute_value(&CFString::from_static_str(name), NonNull::from(&mut v)) };
    if err != AXError::Success {
        return Err(err);
    }
    let p = NonNull::new(v.cast_mut()).ok_or(AXError::NoValue)?;
    // SAFETY: the copy returns a +1 reference.
    Ok(unsafe { CFRetained::from_raw(p) })
}

fn attr(el: &AXUIElement, name: &'static str) -> Option<CFRetained<CFType>> {
    attr_result(el, name).ok()
}

pub(super) fn string_attr(el: &AXUIElement, name: &'static str) -> Option<String> {
    attr(el, name)?
        .downcast::<CFString>()
        .ok()
        .map(|s| s.to_string())
}

pub(super) fn bool_attr(el: &AXUIElement, name: &'static str) -> Option<bool> {
    attr(el, name)?
        .downcast::<CFBoolean>()
        .ok()
        .map(|b| b.value())
}

pub(super) fn element_attr(
    el: &AXUIElement,
    name: &'static str,
) -> Option<CFRetained<AXUIElement>> {
    let v = attr(el, name)?.downcast::<AXUIElement>().ok()?;
    // SAFETY: a valid element.
    let _ = unsafe { v.set_messaging_timeout(AX_TIMEOUT) };
    Some(v)
}

fn array_attr(el: &AXUIElement, name: &'static str) -> Option<CFRetained<CFArray>> {
    attr(el, name)?.downcast::<CFArray>().ok()
}

fn settable(el: &AXUIElement, name: &'static str) -> bool {
    let mut yes: u8 = 0;
    // SAFETY: a valid element, name and out pointer.
    let err = unsafe {
        el.is_attribute_settable(&CFString::from_static_str(name), NonNull::from(&mut yes))
    };
    err == AXError::Success && yes != 0
}

/// The frame of a window element, in guest points.
pub(super) fn frame_of(w: &AXUIElement) -> Option<Frame> {
    let pos = attr(w, "AXPosition")?.downcast::<AXValue>().ok()?;
    let size = attr(w, "AXSize")?.downcast::<AXValue>().ok()?;
    let mut p = CGPoint::new(0.0, 0.0);
    let mut s = CGSize::new(0.0, 0.0);
    // SAFETY: the out pointers match the value types.
    let ok = unsafe {
        pos.value(AXValueType::CGPoint, NonNull::from(&mut p).cast())
            && size.value(AXValueType::CGSize, NonNull::from(&mut s).cast())
    };
    ok.then(|| Frame::new(p.x, p.y, s.width, s.height))
}
