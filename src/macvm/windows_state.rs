//! `window state` of the control socket: what AppKit says of each host
//! window, for the Mac checks when a window does not show. macOS only.

use super::HostWindows;
use objc2_app_kit::NSWindowOcclusionState;
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
}
