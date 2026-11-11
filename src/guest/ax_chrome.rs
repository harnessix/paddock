//! The title bar of a window from Accessibility, macOS only: the frames of
//! `AXTitleUIElement`, the window buttons and the `AXToolbar` with its
//! items, into the height and the buttons of `chrome.rs`. Read once per
//! window and size, and again after a layout notification.

use super::{element_attr, frame_of, string_attr, Ax, CHROME};
use crate::guest::chrome::{self, BarParts};
use crate::guest::menu::children;
use crate::macvm::proto::Frame;
use objc2_application_services::AXUIElement;
use std::sync::atomic::Ordering;

/// The buttons of the window itself.
const BUTTONS: [&str; 4] = [
    "AXCloseButton",
    "AXMinimizeButton",
    "AXZoomButton",
    "AXFullScreenButton",
];
/// The roles of a toolbar's children that a press drags: the title and
/// the icons are no buttons.
const NOT_CONTROLS: [&str; 2] = ["AXStaticText", "AXImage"];
/// The most children of a window or a toolbar that the read looks at.
const CHILDREN_MAX: usize = 64;

/// The cached title bar of one window.
pub(super) struct Bar {
    size: (f64, f64),
    height: Option<f64>,
    controls: Vec<Frame>,
}

/// The parts of the title bar of the window element `el`.
fn parts(el: &AXUIElement) -> BarParts {
    let mut p = BarParts {
        title: element_attr(el, "AXTitleUIElement").and_then(|t| frame_of(&t)),
        ..BarParts::default()
    };
    for name in BUTTONS {
        if let Some(f) = element_attr(el, name).and_then(|b| frame_of(&b)) {
            p.buttons.push(f);
        }
    }
    let kids = children(el).unwrap_or_default();
    let toolbar = kids
        .iter()
        .take(CHILDREN_MAX)
        .find(|c| string_attr(c, "AXRole").as_deref() == Some("AXToolbar"));
    if let Some(t) = toolbar {
        p.toolbar = frame_of(t);
        for item in children(t).unwrap_or_default().iter().take(CHILDREN_MAX) {
            let role = string_attr(item, "AXRole").unwrap_or_default();
            if NOT_CONTROLS.contains(&role.as_str()) {
                continue;
            }
            if let Some(f) = frame_of(item) {
                p.items.push(f);
            }
        }
    }
    p
}

impl Ax {
    /// The title bar height and buttons of window `id` with element `el`
    /// and frame `bounds`, from the cache when the size is the same and
    /// no layout changed since.
    pub(super) fn bar(
        &mut self,
        id: u32,
        el: &AXUIElement,
        bounds: Frame,
    ) -> (Option<f64>, Vec<Frame>) {
        if CHROME.swap(false, Ordering::Relaxed) {
            self.bars.clear();
        }
        let size = (bounds.w, bounds.h);
        if let Some(b) = self.bars.get(&id) {
            if b.size == size {
                return (b.height, b.controls.clone());
            }
        }
        let p = parts(el);
        let height = chrome::title_bar(bounds, &p);
        let controls =
            chrome::controls(bounds, &p, height.unwrap_or(crate::macvm::proto::TITLE_BAR));
        self.bars.insert(
            id,
            Bar {
                size,
                height,
                controls: controls.clone(),
            },
        );
        (height, controls)
    }
}
