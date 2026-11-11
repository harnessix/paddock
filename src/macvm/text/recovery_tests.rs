//! One test per function of text/recovery.rs, named as the function.

use super::super::recovery as t;
use super::super::tests::{has, GIB};
use super::super::ResetTier;
use std::path::Path;

const P: &str = "/d/profiles.noindex/sample-notes/default";

#[test]
fn data_locked() {
    let s = t::data_locked(Path::new(P), Some(Path::new("/d/b/pre-reset-x")));
    has(
        &s,
        &[
            "sample-notes",
            "formatted",
            "system disk is gone",
            "Nothing was changed",
            "paddock app restore sample-notes /d/b/pre-reset-x",
        ],
    );
    let s = t::data_locked(Path::new(P), None);
    has(&s, &["no backup", "paddock app reset sample-notes --data"]);
}

#[test]
fn golden_newer() {
    has(
        &t::golden_newer("sample-notes", "15.2 (24C101)", "15.3 (24D5)"),
        &[
            "runs macOS 15.2 (24C101)",
            "golden is macOS 15.3 (24D5)",
            "paddock app reset sample-notes --data",
            "backup",
        ],
    );
}

#[test]
fn next_start_refused() {
    has(&t::next_start_refused("locked"), &["refused: locked"]);
}

#[test]
fn reset_done() {
    has(
        &t::reset_done("x", ResetTier::System, 4, None),
        &["reset x", "saved state", "4 entries", "cold boot"],
    );
    has(
        &t::reset_done("x", ResetTier::Data, 9, Some(Path::new("/b/pre-reset-1"))),
        &["disk pair", "/b/pre-reset-1"],
    );
    has(
        &t::reset_done("x", ResetTier::All, 1, Some(Path::new("/t/x-default-1"))),
        &["whole profile", "/t/x-default-1", "7 days", "paddock gc"],
    );
}

#[test]
fn reset_needs_backup() {
    has(
        &t::reset_needs_backup("x", 5 * GIB, GIB),
        &[
            "paddock app reset x stops",
            "5 GiB",
            "1 GiB free",
            "paddock gc",
            "paddock app backups x",
        ],
    );
}

#[test]
fn backup_pair_made() {
    has(
        &t::backup_pair_made(Path::new("/b/manual-1"), 2 * GIB),
        &["disk pair", "/b/manual-1", "2 GiB"],
    );
}

#[test]
fn backup_list_line() {
    has(
        &t::backup_list_line("manual-2026-11-05T10-00-00Z", "2026-11-05 10:00", GIB, true),
        &["manual-2026-11-05T10-00-00Z", "1 GiB", "manual"],
    );
    has(&t::backup_list_line("a", "b", 0, false), &["automatic"]);
}

#[test]
fn backup_list_data_only() {
    has(
        &t::backup_list_data_only("old.img"),
        &["old.img", "data only", "not restorable"],
    );
}

#[test]
fn no_backups() {
    has(
        &t::no_backups("x", Path::new("/p/backups")),
        &["x has no backups", "/p/backups", "paddock app backup x"],
    );
}

#[test]
fn backup_not_found() {
    has(
        &t::backup_not_found("x", "2026", Path::new("/p/backups")),
        &[
            "no backup \"2026\" of x",
            "/p/backups",
            "paddock app backups x",
        ],
    );
}

#[test]
fn restore_refused_data_only() {
    has(
        &t::restore_refused_data_only(Path::new("/b/old.img")),
        &["/b/old.img", "data disk alone", "cannot be restored"],
    );
}

#[test]
fn backup_incomplete() {
    has(
        &t::backup_incomplete(Path::new("/b/x"), "system.img"),
        &["/b/x", "no system.img", "cannot be restored"],
    );
}

#[test]
fn restore_pair_done() {
    has(
        &t::restore_pair_done("x", Path::new("/b/m-1"), Path::new("/b/pre-restore-2")),
        &[
            "x runs from the pair of /b/m-1",
            "cold boot",
            "/b/pre-restore-2",
        ],
    );
}

#[test]
fn backup_written() {
    has(
        &t::backup_written(Path::new("/out")),
        &["disk pair", "/out"],
    );
}

#[test]
fn nothing_to_backup() {
    has(
        &t::nothing_to_backup("x"),
        &["x has no data disk", "nothing to back up"],
    );
}

#[test]
fn backup_retired() {
    has(
        &t::backup_retired(Path::new("/b/a")),
        &["old automatic backup /b/a"],
    );
}

#[test]
fn gc_trash_what() {
    has(
        &t::gc_trash_what("x-default-1"),
        &["x-default-1", "paddock app reset --all"],
    );
}

#[test]
fn gc_trash_reason() {
    has(&t::gc_trash_reason(7), &["7 days"]);
}

#[test]
fn gc_backup_what() {
    has(
        &t::gc_backup_what("x", "pre-reset-1"),
        &["pre-reset-1", "profile x"],
    );
}

#[test]
fn gc_backup_reason() {
    has(
        &t::gc_backup_reason(),
        &["newest 3", "30 days", "manual backups stay"],
    );
}

#[test]
fn disk_low_line() {
    has(
        &t::disk_low_line(900 << 20),
        &[
            "0.9 GiB free",
            "alert",
            "skips the saved state",
            "cold boot",
        ],
    );
}

#[test]
fn agent_timeouts() {
    has(
        &t::agent_timeouts(3),
        &[
            "3 requests in a row",
            "crashed",
            "control connection",
            "launchd",
        ],
    );
}

#[test]
fn wake_line() {
    has(
        &t::wake_line(30),
        &["slept for 30 s", "pings", "tunnels", "firewall questions"],
    );
}

#[test]
fn sleep_line() {
    has(&t::sleep_line(), &["sleeps", "pauses"]);
}

#[test]
fn relaunch_stopped() {
    has(
        &t::relaunch_stopped(
            "Sample Notes",
            Some(Path::new("/p/logs/Sample Notes-1.ips")),
        ),
        &[
            "Sample Notes exited three times",
            "does not open it again",
            "/p/logs/Sample Notes-1.ips",
        ],
    );
    has(
        &t::relaunch_stopped("Sample Notes", None),
        &["no crash report", "DiagnosticReports"],
    );
}

#[test]
fn data_disk_locked_guest() {
    has(
        &t::data_disk_locked_guest("Invalid argument"),
        &[
            "partition table",
            "does not mount (Invalid argument)",
            "another system disk",
            "not touched",
        ],
    );
}

#[test]
fn data_id_differs() {
    has(
        &t::data_id_differs("a-1", "b-2"),
        &["id a-1", "named b-2", "stay as they are"],
    );
}

#[test]
fn no_profile() {
    has(
        &t::no_profile("x", Path::new("/p/x")),
        &["x has no profile", "/p/x"],
    );
}

#[test]
fn exists_already() {
    has(
        &t::exists_already(Path::new("/out")),
        &["/out exists already"],
    );
}

#[test]
fn guest_restart_event() {
    has(
        &t::guest_restart_event(),
        &["guest restart", "120 s", "cold boot"],
    );
}

#[test]
fn agent_connection_closed() {
    has(
        &t::agent_connection_closed(),
        &["control connection closed", "launchd"],
    );
}

#[test]
fn session_ended_without_save_event() {
    has(
        &t::session_ended_without_save_event(812),
        &["session ended without a save", "812"],
    );
}

#[test]
fn host_restarted() {
    has(
        &t::host_restarted(),
        &["restarted on the host", "continues"],
    );
}

#[test]
fn health_back() {
    has(&t::health_back(), &["health back"]);
}
