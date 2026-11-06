//! The guest agent `paddock-guest` of paddock's macOS guest VM. It dials the
//! host over vsock, reports the windows of the app with their geometry, moves
//! and raises them on request, launches and quits the app, reports health,
//! runs the configure steps of the golden build, keeps the input source equal
//! to the host's, reads and presses the menu bar of the app, updates itself
//! when the host is another version, and survives its own crash.
//!
//! The pure parts (the window diff, the dispatch, the update, the log
//! rotation, the configure argv, the relaunch counter) have Linux tests.
//! The AppKit, Accessibility and vsock parts are macOS only.

pub mod apps;
#[cfg(target_os = "macos")]
#[path = "apps_macos.rs"]
pub mod apps_macos;
pub mod chrome;
pub mod configure;
pub mod console;
pub mod data;
pub mod data_copy;
pub mod dialogs;
pub mod downloads;
pub mod files;
pub mod identity;
pub mod input_fallback;
pub mod keychain;
pub mod link;
pub mod log;
pub mod main_loop;
pub mod platform;
pub mod proxy;
pub mod shutdown;
pub mod update;
pub mod windows;

#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
pub mod ax_tree;
#[cfg(target_os = "macos")]
pub mod capture;
#[cfg(target_os = "macos")]
pub mod clipboard;
#[cfg(target_os = "macos")]
pub mod dialog_ax;
#[cfg(target_os = "macos")]
pub mod drop;
#[cfg(target_os = "macos")]
pub mod input;
#[cfg(target_os = "macos")]
pub mod link_macos;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod menu;
#[cfg(target_os = "macos")]
pub mod screen;
#[cfg(target_os = "macos")]
pub mod vsock;

/// The folder of the agent in the guest.
pub const SUPPORT_DIR: &str = "/Library/Application Support/paddock";
/// The name of the agent binary.
pub use crate::macvm::guest_files::BINARY;
/// The vsock CID of the host.
pub const HOST_CID: u32 = 2;
