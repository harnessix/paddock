//! One test per function of text/session.rs, named as the function.

use super::super::session as t;
use std::path::Path;
use std::time::Duration;

const GIB: u64 = 1 << 30;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn display_arg_invalid() {
    has(
        &t::display_arg_invalid("x"),
        &["\"x\" is not WxH@S", "2560x1600@2"],
    );
}

#[test]
fn display_arg_range() {
    has(
        &t::display_arg_range("1x1@1"),
        &["1x1@1:", "640 to 16384", "scale 1 to 4"],
    );
}

#[test]
fn bad_app_name() {
    has(&t::bad_app_name("a b"), &["\"a b\"", "letters, digits"]);
}

#[test]
fn macvm_macos_only() {
    has(
        &t::macvm_macos_only(),
        &["--backend macvm", "only on macOS"],
    );
}

#[test]
fn start_fresh() {
    has(&t::start_fresh(), &["new profile", "cold boots"]);
}

#[test]
fn golden_changed() {
    has(
        &t::golden_changed("15.2", "15.3"),
        &["from 15.2 to 15.3", "backs your data up", "cold boots"],
    );
}

#[test]
fn protocol_changed() {
    has(&t::protocol_changed(1, 2), &["from 1 to 2", "cold boot"]);
}

#[test]
fn display_differs() {
    has(&t::display_differs("a", "b"), &["from a to b", "cold boot"]);
}

#[test]
fn state_damaged() {
    has(&t::state_damaged(), &["does not match", "cold boot"]);
}

#[test]
fn stamp_foreign() {
    has(&t::stamp_foreign(), &["another paddock", "cold boot"]);
}

#[test]
fn warm_start() {
    has(&t::warm_start(), &["warm start"]);
}

#[test]
fn cold_boot() {
    assert_eq!(t::cold_boot(), "cold boot");
}

#[test]
fn cannot_clone() {
    assert_eq!(
        t::cannot_clone(Path::new("/g/disk.img"), Path::new("/p/system.img.partial")),
        "cannot clone /g/disk.img to /p/system.img.partial"
    );
}

#[test]
fn machine_id_size() {
    has(&t::machine_id_size(0), &["identifier of 0 bytes"]);
}

#[test]
fn stamp_unreadable() {
    has(
        &t::stamp_unreadable(Path::new("/p/stamp.json"), "Missing"),
        &["/p/stamp.json (Missing)", "after prepare"],
    );
}

#[test]
fn cannot_remove_part() {
    assert_eq!(
        t::cannot_remove_part("State", Path::new("/p")),
        "cannot remove State of /p"
    );
}

#[test]
fn cannot_prepare() {
    assert_eq!(
        t::cannot_prepare(Path::new("/p")),
        "cannot prepare the profile /p"
    );
}

#[test]
fn slot_unreadable() {
    has(
        &t::slot_unreadable(1, "denied"),
        &["slot 1 (cannot open: denied)"],
    );
}

#[test]
fn slot_io() {
    assert_eq!(t::slot_io("e"), "cannot take a VM slot: e");
}

#[test]
fn not_a_folder() {
    assert_eq!(
        t::not_a_folder(Path::new("/x")),
        "/x exists and is not a folder"
    );
}

#[test]
fn cannot_lock() {
    assert_eq!(
        t::cannot_lock(Path::new("/p/profile.lock")),
        "cannot lock /p/profile.lock"
    );
}

#[test]
fn quit_asking() {
    assert_eq!(
        t::quit_asking("Sample Notes"),
        "asking Sample Notes to quit"
    );
}

#[test]
fn quit_app_not_running() {
    has(
        &t::quit_app_not_running("Sample Notes"),
        &["Sample Notes does not run in the guest", "the quit goes on"],
    );
}

#[test]
fn quit_forcing() {
    has(
        &t::quit_forcing("Sample Notes", Duration::from_secs(60)),
        &["Sample Notes did not quit in 60 s, forcing"],
    );
}

#[test]
fn quit_force_failed() {
    has(
        &t::quit_force_failed("Sample Notes", Duration::from_secs(70)),
        &["in 70 s", "without a save"],
    );
}

#[test]
fn pause_failed() {
    has(&t::pause_failed("e"), &["did not pause (e)", "cold boot"]);
}

#[test]
fn save_failed() {
    has(&t::save_failed("e"), &["save failed (e)", "cold boot"]);
}

#[test]
fn resume_failed() {
    assert_eq!(t::resume_failed("e"), "resume failed: e");
}

#[test]
fn shutdown_refused() {
    has(
        &t::shutdown_refused("exit 1: Sorry, try again."),
        &[
            "did not shut down (exit 1: Sorry, try again.)",
            "stopping the VM",
        ],
    );
}

#[test]
fn shutdown_timeout() {
    has(
        &t::shutdown_timeout(Duration::from_secs(30)),
        &["did not stop in 30 s", "stopping the VM"],
    );
}

#[test]
fn stop_failed() {
    assert_eq!(t::stop_failed("e"), "the stop failed: e");
}

#[test]
fn quit_begin() {
    assert_eq!(t::quit_begin("signal"), "quit (signal)");
}

#[test]
fn second_signal() {
    has(&t::second_signal(), &["hard stop"]);
}

#[test]
fn state_commit_failed() {
    has(&t::state_commit_failed("e"), &["saved state: e"]);
}

#[test]
fn stamp_write_failed() {
    assert_eq!(t::stamp_write_failed("e"), "cannot write the stamp: e");
}

#[test]
fn helper_line() {
    assert_eq!(t::helper_line("x"), "paddock-vm: x");
}

#[test]
fn restore_unsupported() {
    has(
        &t::restore_unsupported("e"),
        &["cannot be restored here (e)", "cold boot"],
    );
}

#[test]
fn restore_failed() {
    has(
        &t::restore_failed("e"),
        &["did not restore (e)", "cold boot"],
    );
}

#[test]
fn vm_running() {
    has(
        &t::vm_running("warm", Duration::from_millis(1500)),
        &["runs (warm), 1500 ms"],
    );
}

#[test]
fn no_control_socket() {
    assert_eq!(t::no_control_socket("e"), "no control socket: e");
}

#[test]
fn vm_start_failed() {
    let s = t::vm_start_failed("limit", true);
    has(&s, &["cannot start the VM: limit", "Tart, UTM"]);
    assert_eq!(t::vm_start_failed("e", false), "cannot start the VM: e");
}

#[test]
fn guest_stopped_after() {
    has(
        &t::guest_stopped_after(Duration::from_millis(20)),
        &["guest stopped, 20 ms"],
    );
}

#[test]
fn vm_failed() {
    assert_eq!(t::vm_failed("e"), "the VM stopped with an error: e");
}

#[test]
fn agent_dialed() {
    has(
        &t::agent_dialed("0.2.1", Duration::from_millis(2500)),
        &["agent 0.2.1 dialed in after 2.5 s"],
    );
}

#[test]
fn agent_updating() {
    has(
        &t::agent_updating("0.2.0", "0.2.1"),
        &["agent 0.2.0 updates itself to 0.2.1"],
    );
}

#[test]
fn agent_no_update_file() {
    has(
        &t::agent_no_update_file(Path::new("/a/paddock-guest")),
        &["/a/paddock-guest has no hash file", "stays"],
    );
}

#[test]
fn data_prepare_failed() {
    assert_eq!(
        t::data_prepare_failed("e"),
        "the data disk was not prepared: e"
    );
}

#[test]
fn request_failed() {
    assert_eq!(
        t::request_failed("app.quit", "e"),
        "the guest answered an error to app.quit: e"
    );
}

#[test]
fn request_timeout() {
    has(
        &t::request_timeout("app.quit", Duration::from_secs(60)),
        &["answer app.quit within 60 s"],
    );
}

#[test]
fn hello_restore_cold() {
    has(
        &t::hello_restore_cold(),
        &["after the restore", "cold boot"],
    );
}

#[test]
fn hello_timeout() {
    has(
        &t::hello_timeout(Duration::from_secs(90), Path::new("/p/logs/console.log")),
        &["within 90 s", "/p/logs/console.log"],
    );
}

#[test]
fn no_socket_device() {
    has(&t::no_socket_device(), &["no socket device"]);
}

#[test]
fn no_connection() {
    assert_eq!(t::no_connection(), "no connection");
}

#[test]
fn vm_cannot_stop() {
    has(&t::vm_cannot_stop(), &["cannot stop"]);
}

#[test]
fn guest_port_failed() {
    assert_eq!(
        t::guest_port_failed(1030, "e"),
        "cannot connect to guest port 1030: e"
    );
}

#[test]
fn guest_port_timeout() {
    assert_eq!(
        t::guest_port_timeout(1030),
        "guest port 1030 did not answer"
    );
}

#[test]
fn token_unreadable() {
    has(&t::token_unreadable(), &["token", "parent"]);
}

#[test]
fn token_invalid() {
    has(&t::token_invalid(), &["no valid session token"]);
}

#[test]
fn token_pipe_failed() {
    assert_eq!(t::token_pipe_failed("make"), "cannot make the token pipe");
}

#[test]
fn signal_pipe_failed() {
    has(&t::signal_pipe_failed(), &["signal pipe"]);
}

#[test]
fn stop_timeout() {
    has(
        &t::stop_timeout("x", 812, Duration::from_secs(120)),
        &["session of x (pid 812)", "in 120 s"],
    );
}

#[test]
fn list_line() {
    assert_eq!(
        t::list_line("dead", "stopped (never started)"),
        "dead             macOS guest, stopped (never started)"
    );
}

#[test]
fn list_running() {
    assert_eq!(t::list_running(812, "2h 3m"), "running (pid 812, 2h 3m)");
}

#[test]
fn list_stopped() {
    assert_eq!(t::list_stopped("crashed"), "stopped (crashed)");
}

#[test]
fn hardware_model_invalid() {
    has(
        &t::hardware_model_invalid(),
        &["hardware model", "not valid"],
    );
}

#[test]
fn hardware_model_unsupported_here() {
    has(
        &t::hardware_model_unsupported_here(),
        &["not supported on this Mac", "paddock macvm setup"],
    );
}

#[test]
fn machine_id_damaged() {
    has(
        &t::machine_id_damaged(Path::new("/p/machine-id")),
        &["/p/machine-id is damaged", "paddock app reset"],
    );
}

#[test]
fn disk_serial_invalid() {
    has(&t::disk_serial_invalid("x", "e"), &["serial \"x\": e"]);
}

#[test]
fn share_name_invalid() {
    has(&t::share_name_invalid("x", "e"), &["share name \"x\": e"]);
}

#[test]
fn cpus_out_of_range() {
    assert_eq!(
        t::cpus_out_of_range(99, 1, 8),
        "--cpus 99 is outside 1 to 8 on this Mac"
    );
}

#[test]
fn memory_out_of_range() {
    has(
        &t::memory_out_of_range(99, 1024, 65536),
        &["--memory 99 MiB is outside 1024 to 65536 MiB"],
    );
}

#[test]
fn backup_skipped_used() {
    has(
        &t::backup_skipped_used(5 * GIB, GIB),
        &["skipped: 5 GiB used, 1 GiB free"],
    );
}

#[test]
fn guest_dialog_dismissed() {
    has(
        &t::guest_dialog_dismissed("DiskArbitrationAgent", "[Ignore]: pressed Ignore"),
        &["DiskArbitrationAgent", "dismissed", "pressed Ignore"],
    );
}

#[test]
fn guest_dialog_open() {
    has(
        &t::guest_dialog_open("DiskArbitrationAgent", "no permission"),
        &[
            "could not dismiss",
            "no permission",
            "choose Ignore",
            "never Eject",
        ],
    );
}
