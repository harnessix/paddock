//! One test per function of text/datadisk.rs, named as the function.

use super::super::datadisk as t;
use std::path::Path;
use std::time::Duration;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn data_preparing() {
    has(&t::data_preparing(), &["preparing the data disk", "home"]);
}

#[test]
fn data_prepared() {
    // The Mac check greps for these words.
    assert_eq!(t::data_prepared(""), "the data disk is prepared");
    has(
        &t::data_prepared("The volume appears to be OK"),
        &[
            "the data disk is prepared",
            "verify: The volume appears to be OK",
        ],
    );
}

#[test]
fn data_prepare_timeout() {
    has(
        &t::data_prepare_timeout(Duration::from_secs(120)),
        &[
            "not prepared",
            "data.prepare in 120 s",
            "next start tries again",
        ],
    );
}

#[test]
fn data_prepare_warm() {
    has(
        &t::data_prepare_warm(),
        &["not prepared yet", "saved state", "next start prepares"],
    );
}

#[test]
fn data_no_save_unprepared() {
    has(&t::data_no_save_unprepared(), &["no save", "boots cold"]);
}

#[test]
fn data_agent_restarts() {
    has(
        &t::data_agent_restarts(3),
        &["restarted 3 times", "data.prepare"],
    );
}

#[test]
fn data_no_password() {
    has(
        &t::data_no_password(),
        &[
            "not prepared",
            "no guest password",
            "keyring",
            "without a shutdown",
        ],
    );
}

#[test]
fn shutdown_no_session_password() {
    has(
        &t::shutdown_no_session_password(),
        &["no guest password", "shutdown"],
    );
}

#[test]
fn session_password_missing() {
    has(
        &t::session_password_missing(Path::new("/k/guest-password")),
        &["warning", "/k/guest-password", "paddock macvm setup"],
    );
}

#[test]
fn keyring_password_damaged() {
    has(
        &t::keyring_password_damaged(Path::new("/k/p")),
        &["/k/p", "damaged"],
    );
}

#[test]
fn keyring_password_unreadable() {
    has(
        &t::keyring_password_unreadable(Path::new("/k/p"), "denied"),
        &["cannot read", "/k/p", "denied"],
    );
}

#[test]
fn data_verified() {
    has(
        &t::data_verified("OK"),
        &["verified after an unclean end: OK"],
    );
}

#[test]
fn data_step_failed() {
    assert_eq!(
        t::data_step_failed("mount the volume", "e"),
        "mount the volume: e"
    );
}

#[test]
fn status_data_row() {
    has(
        &t::status_data_row(true, Some("old")),
        &["prepared", "data.img"],
    );
    assert!(!t::status_data_row(true, Some("old")).contains("old"));
    assert_eq!(t::status_data_row(false, Some("e")), "not prepared: e");
    has(&t::status_data_row(false, None), &["not prepared yet"]);
}

#[test]
fn console_start() {
    assert_eq!(
        t::console_start("0.9.0", "1.4", 42),
        "paddock-guest 0.9.0 starts, protocol 1.4, pid 42"
    );
}

#[test]
fn console_no_device() {
    has(
        &t::console_no_device("cu.a"),
        &["no virtio serial console", "cu.a"],
    );
}

#[test]
fn console_write_failed() {
    has(
        &t::console_write_failed("/dev/cu.virtio", "busy"),
        &["/dev/cu.virtio", "busy"],
    );
}

#[test]
fn data_prepare_cancelled() {
    has(&t::data_prepare_cancelled(), &["quit", "next cold start"]);
}

#[test]
fn data_prepare_busy() {
    has(&t::data_prepare_busy(), &["prepares the data disk"]);
}

#[test]
fn data_prepare_stopped_by_quit() {
    has(
        &t::data_prepare_stopped_by_quit(),
        &["quit came while", "nothing is saved", "next cold start"],
    );
}

#[test]
fn data_copy_timed_out() {
    has(
        &t::data_copy_timed_out(Duration::from_secs(75)),
        &["did not end in 75 s", "1 s per 50 MB"],
    );
}

#[test]
fn data_copy_failed() {
    assert_eq!(
        t::data_copy_failed("/Users/paddock/Music", "No space left"),
        "cannot copy /Users/paddock/Music: No space left"
    );
}

#[test]
fn data_copy_progress() {
    assert_eq!(
        t::data_copy_progress(3, 9, 1 << 30, 3 << 30),
        "copying the home: 3 of 9 parts, 1 GiB of 3 GiB"
    );
}

#[test]
fn data_copy_measured() {
    has(
        &t::data_copy_measured(Some(2 << 30), Duration::from_secs(100)),
        &["2 GiB", "2 min"],
    );
    has(
        &t::data_copy_measured(None, Duration::from_secs(1800)),
        &["could not measure", "30 min"],
    );
}

#[test]
fn data_copy_done() {
    assert_eq!(t::data_copy_done(9, 0, &[]), "copied the home: 9 parts");
    has(
        &t::data_copy_done(9, 2, &["~/Library/Containers".into()]),
        &[
            "9 parts",
            "2 files could not be read",
            "~/Library/Containers",
        ],
    );
}

#[test]
fn log_fetch_missing() {
    has(&t::log_fetch_missing(20), &["20 s", "logs/agent.log"]);
}

#[test]
fn cfprefsd_restart_failed() {
    let s = t::cfprefsd_restart_failed("No matching processes");
    assert!(
        s.contains("settings") && s.contains("No matching processes"),
        "{s}"
    );
}
