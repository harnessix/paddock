//! The size changes of the caller's terminal (SIGWINCH).

use std::sync::atomic::{AtomicU64, Ordering};

/// The size changes so far. Each relay compares it with the count it saw
/// last, so no relay takes a change away from another one.
static WINCH: AtomicU64 = AtomicU64::new(0);

extern "C" fn on_winch(_: libc::c_int) {
    WINCH.fetch_add(1, Ordering::Relaxed);
}

/// The count now: a relay that starts sees the changes after it.
pub(super) fn seen() -> u64 {
    WINCH.load(Ordering::Relaxed)
}

/// True when the size changed since `seen`, which moves on.
pub(super) fn take_winch(seen: &mut u64) -> bool {
    let now = WINCH.load(Ordering::Relaxed);
    std::mem::replace(seen, now) != now
}

pub(super) fn install_winch() -> libc::sigaction {
    // SAFETY: installs a handler that only adds to an atomic counter.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_winch as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGWINCH, &action, &mut old);
        old
    }
}

/// Tests that start a relay hold this for reading. A test that raises
/// SIGWINCH holds it for writing: the signal reaches every relay of the
/// test process.
#[cfg(test)]
pub(crate) static TEST_SIGNAL: std::sync::RwLock<()> = std::sync::RwLock::new(());
