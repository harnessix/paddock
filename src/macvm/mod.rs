//! The macOS guest route of app mode: paddock's own macOS VM, with each
//! window of the app on the host desktop.
//!
//! The parts that do not need macOS live here as pure code with Linux
//! tests: the layout, the golden and its states, the configure list, the
//! setup driver, `status`, `events`, `logs` and `report`, `proto`, the wire
//! protocol between the host and the guest agent `paddock-guest`
//! (src/guest), the session (its profile, stamp, locks, event log and quit
//! path), the parent side of `paddock app run --backend macvm` in `host`,
//! `crop`, the arithmetic of the viewport windows, `order`, the fold of the
//! window events, `frames`, the frame source, `input`, the input source
//! state machine, and `text`, which holds every text that the user sees on
//! this route, `menu`, the menu bar replica, and `axmirror`, the
//! Accessibility mirror. The Virtualization framework and AppKit parts are
//! in `run.rs`, `windows.rs`, `dock.rs`, `ax_host.rs` and the `vz_mac*`
//! files, macOS only.

pub mod apart;
pub mod apfs;
#[cfg(test)]
pub(crate) mod apfs_fixture;
pub mod appcopy;
pub mod approvals;
pub mod ask_queue;
pub mod ask_status;
#[cfg(target_os = "macos")]
pub mod ax_host;
pub mod axmirror;
pub mod backup;
pub mod bench;
pub mod bench_run;
pub mod clipboard;
pub mod configure;
pub mod configure_quiet;
pub mod connlog;
pub mod cover;
pub mod crop;
pub mod data_disk;
pub mod data_start;
pub mod diskimage;
#[cfg(target_os = "macos")]
pub mod dock;
pub mod download;
pub mod drop;
pub mod du;
pub mod events;
pub mod events_stream;
pub mod failure;
pub mod files_port;
pub mod firewall;
pub mod frames;
pub mod gate;
pub mod golden;
pub mod grants;
pub mod grants_cli;
pub mod grants_live;
pub mod guest_files;
pub mod guided;
pub mod guided_ui;
pub mod host;
pub mod host_app;
pub mod host_mac;
pub mod host_record;
pub mod host_running;
pub mod host_suspend;
pub mod input;
pub mod instance;
#[cfg(target_os = "macos")]
pub mod launch;
pub mod launch_services;
pub mod layout;
pub mod lead;
pub mod limits;
pub mod lock;
pub mod login_route;
pub mod logs;
pub mod manage;
pub mod menu;
pub mod migrate;
pub mod offline;
#[cfg(test)]
pub(crate) mod offline_fake;
pub mod offline_link;
pub mod order;
pub mod owner;
pub mod pair;
#[cfg(target_os = "macos")]
pub mod pasteboard;
pub mod pause;
pub mod place;
pub mod proto;
pub mod proto_files;
pub mod proto_setup;
pub mod provision;
pub mod proxy_header;
pub mod quit;
pub mod report;
pub mod reset;
#[cfg(target_os = "macos")]
pub mod run;
#[cfg(target_os = "macos")]
mod run_agent;
#[cfg(target_os = "macos")]
mod run_apart;
#[cfg(target_os = "macos")]
mod run_app;
#[cfg(target_os = "macos")]
pub mod run_appkit;
#[cfg(target_os = "macos")]
mod run_ax;
#[cfg(target_os = "macos")]
mod run_calls;
#[cfg(target_os = "macos")]
mod run_failure;
#[cfg(target_os = "macos")]
mod run_files;
#[cfg(target_os = "macos")]
mod run_grants;
#[cfg(target_os = "macos")]
mod run_input;
#[cfg(target_os = "macos")]
mod run_lead;
#[cfg(target_os = "macos")]
mod run_login;
#[cfg(target_os = "macos")]
mod run_menu;
#[cfg(target_os = "macos")]
mod run_quit;
#[cfg(target_os = "macos")]
mod run_record;
#[cfg(target_os = "macos")]
mod run_screen;
#[cfg(target_os = "macos")]
mod run_suspend;
#[cfg(target_os = "macos")]
pub mod run_vm;
#[cfg(target_os = "macos")]
mod run_windows;
pub mod screen_lock;
pub mod screens;
pub mod security;
pub mod seed;
pub mod session;
#[cfg(test)]
#[path = "session_flow_tests.rs"]
mod session_flow_tests;
pub mod setup;
pub mod setup_events;
mod setup_offline;
pub mod setup_progress;
pub mod shares;
pub mod shim;
pub mod space;
pub mod start;
pub mod status;
pub mod stubnet;
pub mod suspend;
pub mod tcc;
pub mod text;
#[cfg(target_os = "macos")]
pub mod tis;
pub mod trace;
pub mod version;
#[cfg(target_os = "macos")]
pub mod vz_mac;
#[cfg(target_os = "macos")]
pub mod vz_mac_agent;
#[cfg(target_os = "macos")]
pub mod vz_mac_config;
#[cfg(target_os = "macos")]
pub mod vz_mac_drop;
#[cfg(target_os = "macos")]
pub mod vz_mac_image;
#[cfg(target_os = "macos")]
mod vz_mac_input;
#[cfg(target_os = "macos")]
mod vz_mac_panel;
#[cfg(target_os = "macos")]
mod vz_mac_provision;
#[cfg(target_os = "macos")]
mod vz_mac_session;
#[cfg(target_os = "macos")]
pub mod vz_mac_window;
#[cfg(target_os = "macos")]
pub mod windows;
#[cfg(target_os = "macos")]
mod windows_cover;
#[cfg(target_os = "macos")]
mod windows_delegate;
#[cfg(target_os = "macos")]
mod windows_lead;

#[cfg(test)]
#[path = "readme_tests.rs"]
mod readme;
