//! Differences between Linux and macOS.
//!
//! Linux keeps the calls the sandbox already uses. macOS uses a native
//! stand-in where one exists. A feature with no native implementation returns
//! [`unavailable`] so the caller refuses it out loud.

use anyhow::Error;

/// `feature` has no native macOS implementation in this build.
pub fn unavailable(feature: &str) -> Error {
    anyhow::anyhow!("{feature} is not available on macOS yet")
}

/// Point standard output and error at /dev/null.
pub fn quiet_stdio() {
    // SAFETY: open(2) and dup2(2) on our own descriptors.
    unsafe {
        let fd = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if fd >= 0 {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
            libc::close(fd);
        }
    }
}

pub mod nofile;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::{arm_parent_death_signal, be_idle_io, children_of, parent_of, peer_gone};
#[cfg(target_os = "macos")]
pub use macos::{arm_parent_death_signal, be_idle_io, bsd_info, children_of, parent_of, peer_gone};
