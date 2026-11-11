//! The launcher event stream: line-delimited JSON of the live state of
//! app mode (sessions, firewall questions, grants, the golden build, the
//! golden and the free space). `paddock events --follow --json` prints it;
//! the Mac launcher reads it and queries the CLI only after a change.
//!
//! - [`schema`]: the one versioned schema of the lines.
//! - [`bus`]: the socket under paddock's run directory, where the
//!   publishers write and the followers listen.
//! - [`follow`]: the loop of `paddock events --follow`.

pub mod bus;
pub mod follow;
pub mod schema;

pub use bus::Bus;
pub use schema::{Event, Line};

/// Publish one event on the bus of `env`. Never fails and never waits
/// long; with no follower it costs one `read_dir`.
pub fn publish(env: &crate::env::HostEnv, event: Event) {
    Bus::of(env).publish(event);
}

static DEFAULT: std::sync::OnceLock<Bus> = std::sync::OnceLock::new();

/// Set the bus of this process: the publishers that have no `HostEnv` at
/// hand (the event log of a session) use it. `paddock` and `paddock-vm`
/// call it once after `HostEnv::detect`; tests never do, so they publish
/// nowhere.
pub fn init(env: &crate::env::HostEnv) {
    let _ = DEFAULT.set(Bus::of(env));
}

/// The bus of [`init`], when it ran.
pub fn default_bus() -> Option<&'static Bus> {
    DEFAULT.get()
}

/// Tests of the socket folders and the doctor of a real Mac.
#[cfg(all(test, target_os = "macos"))]
#[path = "macos_tests.rs"]
mod macos_tests;
