//! `window state` of the control socket: what AppKit says of each host
//! window, for the Mac checks when a window does not show, and `window
//! host-click` of a bench session. macOS only.

use super::HostWindows;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSWindowOcclusionState,
};
use objc2_foundation::NSPoint;
use std::collections::BTreeMap;

impl HostWindows {
    /// What AppKit says of each host window, for `window state` (a Mac
    /// check reads it when a window does not show).
    pub fn states(&self) -> BTreeMap<u32, serde_json::Value> {
        self.windows
            .iter()
            .map(|(id, w)| {
                let f = w.frame();
                let view = w.view().map(|v| v.frame());
                let v = serde_json::json!({
                    "visible": w.isVisible(),
                    "active_space": w.isOnActiveSpace(),
                    "occlusion_visible": w.occlusionState().contains(NSWindowOcclusionState::Visible),
                    "on_screen": w.screen().is_some(),
                    "level": w.level(),
                    "alpha": w.alphaValue(),
                    "miniaturized": w.isMiniaturized(),
                    "frame": [f.origin.x, f.origin.y, f.size.width, f.size.height],
                    "view": view.map(|r| [r.origin.x, r.origin.y, r.size.width, r.size.height]),
                    "parent": w.parentWindow().is_some(),
                });
                (*id, v)
            })
            .collect()
    }

    /// A left click of the host at point `(x, y)` of the host window of
    /// `id`, in points from the top left of its content, y down: a press
    /// and a release through `NSApplication`, as a real click goes. The
    /// guest point (guest points, y down) under the click, from the place
    /// of the view, or None without that window or its view.
    pub fn host_click(&self, id: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        let w = self.windows.get(&id)?;
        let view = w.view()?;
        let mtm = MainThreadMarker::from(&**w);
        let content_h = w
            .contentView()
            .map(|c| c.frame().size.height)
            .unwrap_or_else(|| w.frame().size.height);
        let at = NSPoint::new(x, content_h - y);
        let v = view.frame().origin;
        let guest = (at.x - v.x, self.guest_display().h - (at.y - v.y));
        let app = NSApplication::sharedApplication(mtm);
        let number = w.windowNumber();
        for kind in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
            let ev = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                at,
                NSEventModifierFlags::empty(),
                0.0,
                number,
                None,
                0,
                1,
                1.0,
            )?;
            app.sendEvent(&ev);
        }
        Some(guest)
    }
}
