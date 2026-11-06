//! The Accessibility elements of a host window, macOS only: one
//! `NSAccessibilityElement` per node of the mirror of `axmirror.rs`, with the
//! role, the label, the value and the enabled state of the guest element, and
//! a frame that follows the host window wherever the user drags it. VoiceOver
//! and every other Accessibility client see them as the children of the
//! session window. A press (`accessibilityPerformPress`) and a value
//! (`setAccessibilityValue:`) go to the session as [`HostEvent`]s with the
//! element id; the session checks the id against the last tree before
//! anything goes to the guest, so a stale element never acts. Main thread
//! only.
//!
//! `windows.rs` holds one [`Elements`] per session window and answers
//! `accessibilityChildren`, the hit test and the focused element from it;
//! `run_ax.rs` drives the changes.

use super::axmirror::{host_rect, window_point, Change, Mirror, Node};
use super::crop::Rect;
use super::proto::Frame;
use super::windows::{HostEvent, HostSink, SessionWindow};
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityElement, NSAccessibilityFocusedUIElementChangedNotification,
    NSAccessibilityLayoutChangedNotification, NSAccessibilityPostNotification,
};
use objc2_foundation::{NSArray, NSNumber, NSPoint, NSRect, NSSize, NSString};
use std::cell::Cell;
use std::collections::BTreeMap;

/// The roles whose value is a number for Accessibility clients.
const NUMBER_ROLES: &[&str] = &[
    "AXCheckBox",
    "AXRadioButton",
    "AXDisclosureTriangle",
    "AXSlider",
    "AXProgressIndicator",
    "AXIncrementor",
    "AXValueIndicator",
    "AXScrollBar",
];

pub struct ElementIvars {
    id: u64,
    role: String,
    /// The window the element belongs to; weak, so the window's
    /// elements never keep the window.
    window: Weak<SessionWindow>,
    /// The frame relative to the window, guest points, origin top left.
    frame: Cell<Frame>,
    sink: HostSink,
}

define_class!(
    #[unsafe(super(NSAccessibilityElement))]
    #[thread_kind = MainThreadOnly]
    #[name = "PaddockMirrorElement"]
    #[ivars = ElementIvars]
    pub struct MirrorElement;

    impl MirrorElement {
        /// The frame in screen points, from the window's frame now, so
        /// a dragged window keeps its elements in place.
        #[unsafe(method(accessibilityFrame))]
        fn accessibility_frame(&self) -> NSRect {
            let Some(window) = self.ivars().window.load() else {
                return NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            };
            screen_frame(window.frame(), self.ivars().frame.get())
        }

        /// A press of an Accessibility client: to the session by id.
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_perform_press(&self) -> bool {
            (self.ivars().sink)(HostEvent::AxPress(self.ivars().id));
            true
        }

        /// A value of an Accessibility client: to the session by id, as
        /// text. The element keeps it until the next tree says otherwise.
        #[unsafe(method(setAccessibilityValue:))]
        fn set_accessibility_value(&self, value: Option<&AnyObject>) {
            if let Some(text) = value.and_then(value_text) {
                (self.ivars().sink)(HostEvent::AxSet(self.ivars().id, text));
            }
            let _: () = unsafe { msg_send![super(self), setAccessibilityValue: value] };
        }
    }
);

impl MirrorElement {
    fn new(
        mtm: MainThreadMarker,
        id: u64,
        node: &Node,
        window: &Retained<SessionWindow>,
        sink: HostSink,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ElementIvars {
            id,
            role: node.role.clone(),
            window: Weak::from_retained(window),
            frame: Cell::new(node.frame),
            sink,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setAccessibilityElement(true);
        this.setAccessibilityRole(Some(&NSString::from_str(&node.role)));
        let window_any: &AnyObject = window;
        // SAFETY: the window is an NSWindow, which is what the attribute
        // takes; the parent is set by the caller.
        unsafe {
            this.setAccessibilityWindow(Some(window_any));
            this.setAccessibilityTopLevelUIElement(Some(window_any));
        }
        this.update(node);
        this
    }

    pub fn id(&self) -> u64 {
        self.ivars().id
    }

    /// The label, the value, the enabled and focused states and the
    /// frame of the node.
    fn update(&self, node: &Node) {
        let label = (!node.title.is_empty()).then(|| NSString::from_str(&node.title));
        self.setAccessibilityLabel(label.as_deref());
        self.setAccessibilityEnabled(node.enabled);
        self.setAccessibilityFocused(node.focused);
        self.ivars().frame.set(node.frame);
        let value: Option<Retained<AnyObject>> = node.value.as_deref().map(|v| {
            let number = NUMBER_ROLES
                .contains(&self.ivars().role.as_str())
                .then(|| v.trim().parse::<f64>().ok())
                .flatten();
            match number {
                Some(n) if n.fract() == 0.0 => NSNumber::new_i64(n as i64).into(),
                Some(n) => NSNumber::new_f64(n).into(),
                None => NSString::from_str(v).into(),
            }
        });
        // SAFETY: a string or a number, which the attribute takes.
        unsafe { self.setAccessibilityValue(value.as_deref()) };
    }

    /// Give the element its children and tell each who its parent is.
    fn set_children(&self, children: &[Retained<MirrorElement>]) {
        let this: &AnyObject = self;
        let mut any: Vec<Retained<AnyObject>> = Vec::with_capacity(children.len());
        for c in children {
            // SAFETY: the parent is an accessibility element.
            unsafe { c.setAccessibilityParent(Some(this)) };
            any.push(as_any(c.clone()));
        }
        // SAFETY: an array of accessibility elements.
        unsafe { self.setAccessibilityChildren(Some(&NSArray::from_retained_slice(&any))) };
    }
}

/// The value an Accessibility client set, as text.
fn value_text(v: &AnyObject) -> Option<String> {
    if let Some(s) = v.downcast_ref::<NSString>() {
        return Some(s.to_string());
    }
    if let Some(n) = v.downcast_ref::<NSNumber>() {
        return Some(n.stringValue().to_string());
    }
    None
}

/// The screen frame of an element: `axmirror::host_rect` in AppKit's
/// types.
fn screen_frame(window: NSRect, rel: Frame) -> NSRect {
    let r = host_rect(rect_of(window), rel);
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

fn rect_of(r: NSRect) -> Rect {
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

fn as_any(el: Retained<MirrorElement>) -> Retained<AnyObject> {
    el.into()
}

/// What changed for the Accessibility clients after an `apply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Posted {
    pub layout: bool,
    pub focus: bool,
}

impl Posted {
    /// Tell the Accessibility clients of `window`.
    pub fn post(self, window: &SessionWindow) {
        let window_any: &AnyObject = window;
        // SAFETY: the window is an accessibility element, and the
        // notification names are constants of AppKit.
        unsafe {
            if self.layout {
                NSAccessibilityPostNotification(
                    window_any,
                    NSAccessibilityLayoutChangedNotification,
                );
            }
            if self.focus {
                NSAccessibilityPostNotification(
                    window_any,
                    NSAccessibilityFocusedUIElementChangedNotification,
                );
            }
        }
    }
}

/// The elements of one session window: by id, with the roots in order,
/// and the copy of the tree for the hit test and the focused element.
#[derive(Default)]
pub struct Elements {
    by_id: BTreeMap<u64, Retained<MirrorElement>>,
    roots: Vec<u64>,
    nodes: Vec<Node>,
    ids: BTreeMap<Vec<usize>, u64>,
    focused: Option<u64>,
}

impl Elements {
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// The roots as the window's `accessibilityChildren`.
    pub fn roots(&self) -> Retained<NSArray> {
        let any: Vec<Retained<AnyObject>> = self
            .roots
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .map(|el| as_any(el.clone()))
            .collect();
        NSArray::from_retained_slice(&any)
    }

    /// The deepest element under a screen point.
    pub fn hit(&self, window: NSRect, p: NSPoint) -> Option<Retained<AnyObject>> {
        let node = Node::hit(&self.nodes, window_point(rect_of(window), p.x, p.y))?;
        let id = self.ids.get(&node.path)?;
        self.by_id.get(id).map(|el| as_any(el.clone()))
    }

    /// The focused element of the last tree.
    pub fn focused(&self) -> Option<Retained<AnyObject>> {
        let id = self.focused?;
        self.by_id.get(&id).map(|el| as_any(el.clone()))
    }

    /// Everything goes.
    pub fn clear(&mut self) {
        *self = Elements::default();
    }

    /// Run the changes of the mirror on the elements of `window`. The
    /// caller posts the notifications of the result once it let go of
    /// the elements, because a client may query them at once.
    pub fn apply(
        &mut self,
        window: &Retained<SessionWindow>,
        mirror: &Mirror,
        changes: &[Change],
        sink: &HostSink,
    ) -> Posted {
        let mtm = MainThreadMarker::from(&**window);
        for c in changes {
            match c {
                Change::Rebuild(position) if position.is_empty() => {
                    self.by_id.clear();
                    self.roots.clear();
                    let window_any: &AnyObject = window;
                    for node in mirror.nodes() {
                        let el = self.build(mtm, node, mirror, window, sink);
                        // SAFETY: the window is an NSWindow, which is what
                        // the parent attribute takes.
                        unsafe { el.setAccessibilityParent(Some(window_any)) };
                        self.roots.push(el.id());
                    }
                }
                Change::Rebuild(position) => {
                    let parent = mirror
                        .id_of(position)
                        .and_then(|id| self.by_id.get(&id).cloned());
                    let Some(parent) = parent else {
                        continue;
                    };
                    let stale: Vec<u64> = self
                        .ids
                        .iter()
                        .filter(|(path, _)| {
                            path.len() > position.len() && path.starts_with(position)
                        })
                        .map(|(_, id)| *id)
                        .collect();
                    for id in stale {
                        self.by_id.remove(&id);
                    }
                    let children: Vec<Retained<MirrorElement>> = Node::at(mirror.nodes(), position)
                        .map(|n| n.children.as_slice())
                        .unwrap_or(&[])
                        .iter()
                        .map(|n| self.build(mtm, n, mirror, window, sink))
                        .collect();
                    parent.set_children(&children);
                }
                Change::Update(position) => {
                    let el = mirror
                        .id_of(position)
                        .and_then(|id| self.by_id.get(&id).cloned());
                    if let (Some(el), Some(node)) = (el, Node::at(mirror.nodes(), position)) {
                        el.update(node);
                    }
                }
            }
        }
        self.nodes = mirror.nodes().to_vec();
        self.ids = mirror.ids().clone();
        let focused = Node::focused(&self.nodes).and_then(|n| self.ids.get(&n.path).copied());
        let focus_changed = focused != self.focused;
        self.focused = focused;
        Posted {
            layout: !changes.is_empty(),
            focus: focus_changed,
        }
    }

    /// One element for a node and its subtree, into the map.
    fn build(
        &mut self,
        mtm: MainThreadMarker,
        node: &Node,
        mirror: &Mirror,
        window: &Retained<SessionWindow>,
        sink: &HostSink,
    ) -> Retained<MirrorElement> {
        let id = mirror.id_of(&node.path).unwrap_or(0);
        let el = MirrorElement::new(mtm, id, node, window, sink.clone());
        let children: Vec<Retained<MirrorElement>> = node
            .children
            .iter()
            .map(|n| self.build(mtm, n, mirror, window, sink))
            .collect();
        if !children.is_empty() {
            el.set_children(&children);
        }
        self.by_id.insert(id, el.clone());
        el
    }
}
