//! The texts of `paddock freeze` of a guest session, the screen change
//! and the limits of the host.

use super::BADGE_MAX;

// ---- Freeze -------------------------------------------------------------

/// A `pause` line while the session quits.
pub fn pause_while_quitting() -> String {
    "the session is quitting, so it cannot be frozen".into()
}

/// A `pause` line while a `resume` runs.
pub fn pause_while_resuming() -> String {
    "the VM is resuming. Wait a second, then freeze it again".into()
}

/// A `resume` line while a `pause` runs.
pub fn resume_while_pausing() -> String {
    "the VM is pausing. Wait a second, then thaw it again".into()
}

/// The framework refused the pause.
pub fn pause_failed_freeze(e: &str) -> String {
    format!("the VM did not pause ({e}). It keeps running")
}

/// The framework refused the resume.
pub fn resume_failed_freeze(e: &str) -> String {
    format!("the VM did not resume ({e}). It stays paused: paddock thaw tries again, paddock app stop quits it")
}

/// The framework says that the VM cannot pause or resume in its state.
pub fn vm_state_refused() -> String {
    "the VM cannot do that in its state now (it is starting, saving or stopping)".into()
}

/// The badge of a frozen session.
pub fn frozen_badge() -> String {
    let s = "frozen: paddock thaw resumes it".to_string();
    debug_assert!(s.chars().count() <= BADGE_MAX);
    s
}

/// The event line of a freeze.
pub fn frozen_line() -> String {
    "the VM is paused (paddock freeze)".into()
}

/// The event line of a thaw.
pub fn thawed_line(secs: u64) -> String {
    format!("the VM runs again after {secs} s paused (paddock thaw)")
}

/// `paddock app stop` of a frozen session: the thaw comes first.
pub fn thaw_before_quit(name: &str) -> String {
    format!("{name} is frozen: paddock thaws it first, so the app can save its work, then quits it")
}

// ---- Screens and limits -------------------------------------------------

/// Two screens with two scales.
pub fn mixed_scales() -> String {
    "the screens have two scales: the guest draws at the main screen's scale, so windows on the other screen are soft".into()
}

/// The event line of a screen change, with the windows that moved.
pub fn screens_changed(screens: usize, moved: usize) -> String {
    format!("the screens changed ({screens} now): {moved} windows moved onto a screen")
}

#[cfg(test)]
#[path = "freeze_tests.rs"]
mod tests;
