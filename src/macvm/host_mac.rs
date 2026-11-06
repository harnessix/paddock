//! What the parent `paddock` reads of the Mac itself for the macOS guest
//! route: the screens, for the display of a new session and the mixed scale
//! note of the start lines, and the restore of a host input source that a
//! dead session left switched (item 21 of the security checklist). The Mac
//! calls are behind `cfg(target_os = "macos")`; the rest runs everywhere.

use super::host::{alive, sweep, DEFAULT_DISPLAY};
use super::layout::Layout;
use super::text;
use crate::env::HostEnv;

/// The sweep of the dead sessions, and the host input source of each one
/// that had switched it put back (item 21 of the checklist): the lines
/// that name the restored sources, for the user.
pub fn sweep_and_restore(layout: &Layout) -> Vec<String> {
    let mut restored = Vec::new();
    for s in sweep(&layout.sessions, &alive, &mut |src| {
        restored.push(src.to_string())
    }) {
        log::info!("macvm: the session of {} (pid {}) ended", s.app, s.pid);
    }
    restored
        .iter()
        .filter_map(|id| restore_input_source(id))
        .map(|name| text::input_source_restored(&name))
        .collect()
}

/// Select the input source `id` on the host again. The name of the
/// source when it took; None when it did not, with the reason logged.
#[cfg(target_os = "macos")]
fn restore_input_source(id: &str) -> Option<String> {
    match super::tis::select(id) {
        Ok(()) => Some(super::tis::name_of(id).unwrap_or_else(|| id.to_string())),
        Err(e) => {
            log::warn!("macvm: {e}");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn restore_input_source(id: &str) -> Option<String> {
    log::warn!("macvm: cannot restore the input source {id} on this platform");
    None
}

/// The host screens, for the display of a new session and the mixed scale
/// note of the start lines. None off the main thread or without a window
/// server.
#[cfg(target_os = "macos")]
pub fn host_screens() -> Option<super::crop::ScreenSet> {
    let mtm = objc2::MainThreadMarker::new()?;
    let set = super::windows::screens(mtm);
    (!set.scales.is_empty()).then_some(set)
}

#[cfg(not(target_os = "macos"))]
pub fn host_screens() -> Option<super::crop::ScreenSet> {
    None
}

/// The display argument of a session: `PADDOCK_MACVM_DISPLAY`, else the
/// union of the host screens at the main screen's scale, else the
/// default.
pub fn display_arg(env: &HostEnv) -> String {
    if let Some(d) = env.var("PADDOCK_MACVM_DISPLAY") {
        return d.to_string();
    }
    host_screens()
        .and_then(|s| s.session_display())
        .map(|d| d.to_string())
        .unwrap_or_else(|| DEFAULT_DISPLAY.to_string())
}
