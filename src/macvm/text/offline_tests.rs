//! One test per function of text/offline.rs, named as the function.

use super::super::offline as t;
use crate::macvm::diskimage::Fsck;
use crate::macvm::provision::Why;
use std::path::Path;
use std::time::Duration;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn apfs_damaged() {
    has(
        &t::apfs_damaged("no GPT header"),
        &["guest disk", "APFS", "no GPT header"],
    );
}

#[test]
fn apfs_damaged_at() {
    has(
        &t::apfs_damaged_at("invalid block size", 4095),
        &["APFS", "invalid block size 4095"],
    );
}

#[test]
fn apfs_unexpected_owner() {
    has(
        &t::apfs_unexpected_owner("Users/x", 501, 20),
        &["Users/x", "501:20", "99:99", "nothing was changed"],
    );
}

#[test]
fn apfs_path_missing() {
    has(
        &t::apfs_path_missing("Library/a/b", "a"),
        &["Library/a/b", "no entry a"],
    );
}

#[test]
fn apfs_write_failed() {
    has(
        &t::apfs_write_failed(7, "disk full"),
        &["block 7", "disk full"],
    );
}

#[test]
fn plist_unreadable() {
    has(
        &t::plist_unreadable("no <plist>"),
        &["property list", "no <plist>"],
    );
}

#[test]
fn disk_command_timeout() {
    has(
        &t::disk_command_timeout("hdiutil attach x", Duration::from_secs(600)),
        &["hdiutil attach x", "600 s", "stopped"],
    );
}

#[test]
fn disk_command_failed() {
    has(
        &t::disk_command_failed("hdiutil detach /dev/disk4", "", "busy"),
        &["detach", "failed: busy"],
    );
    has(&t::disk_command_failed("x", "a", "b"), &["a / b"]);
    has(&t::disk_command_failed("x", "", ""), &["no output"]);
}

#[test]
fn attach_no_disk() {
    has(
        &t::attach_no_disk(Path::new("/g/disk.img")),
        &["/g/disk.img", "whole disk"],
    );
}

#[test]
fn attach_no_data_volume() {
    has(
        &t::attach_no_data_volume(Path::new("/g/disk.img")),
        &["/g/disk.img", "Data volume", "detached"],
    );
}

#[test]
fn attach_no_store() {
    has(
        &t::attach_no_store(Path::new("/g/disk.img")),
        &["/g/disk.img", "APFS partition"],
    );
}

#[test]
fn cannot_clone_disk() {
    has(
        &t::cannot_clone_disk(Path::new("/g/disk.img"), "not supported"),
        &["/g/disk.img", "not supported", "APFS"],
    );
}

#[test]
fn seed_restore_failed() {
    has(
        &t::seed_restore_failed(
            Path::new("/g/disk.img.before-seed"),
            Path::new("/g/disk.img"),
            "busy",
        ),
        &["/g/disk.img.before-seed", "/g/disk.img", "busy", "by hand"],
    );
}

#[test]
fn tcc_db_missing() {
    has(
        &t::tcc_db_missing(Path::new("/m/TCC.db")),
        &["/m/TCC.db", "first boot"],
    );
}

#[test]
fn tcc_schema_unknown() {
    has(
        &t::tcc_schema_unknown(Path::new("/m/TCC.db"), "auth_value"),
        &["/m/TCC.db", "auth_value", "nothing was changed"],
    );
}

#[test]
fn tcc_no_access_table() {
    has(
        &t::tcc_no_access_table(),
        &["access table", "nothing was changed"],
    );
}

#[test]
fn tcc_integrity() {
    has(
        &t::tcc_integrity(Path::new("/m/TCC.db"), "row 3 missing"),
        &["/m/TCC.db", "integrity", "row 3 missing"],
    );
}

#[test]
fn tcc_wal_left() {
    has(
        &t::tcc_wal_left(Path::new("/m/TCC.db-wal")),
        &["/m/TCC.db-wal"],
    );
}

#[test]
fn offline_off_by_flag() {
    has(&t::offline_off_by_flag(), &["PADDOCK_MACVM_GUIDED=1"]);
}

#[test]
fn offline_no_tool() {
    has(&t::offline_no_tool(), &["cannot mount the guest disk"]);
}

#[test]
fn seed_offline_skipped() {
    has(
        &t::seed_offline_skipped("why"),
        &["(why)", "command to type"],
    );
}

#[test]
fn seed_offline_done() {
    has(
        &t::seed_offline_done(9, &Fsck::Clean),
        &["9 files", "no error", "nothing to type"],
    );
    has(
        &t::seed_offline_done(9, &Fsck::Skipped("Permission denied".into())),
        &["did not run", "Permission denied"],
    );
}

#[test]
fn seed_offline_failed() {
    has(
        &t::seed_offline_failed("attach failed"),
        &["as it was", "attach failed", "command to type"],
    );
}

#[test]
fn trust_offline_done() {
    has(&t::trust_offline_done(), &["Accessibility", "boots again"]);
}

#[test]
fn trust_reported() {
    has(
        &t::trust_reported(),
        &["reports the Accessibility permission"],
    );
}

#[test]
fn trust_not_reported() {
    has(&t::trust_not_reported(), &["no Accessibility permission"]);
}

#[test]
fn trust_offline_failed() {
    has(
        &t::trust_offline_failed("no TCC.db"),
        &["no TCC.db", "Turn on paddock-guest"],
    );
}

#[test]
fn guided_why() {
    has(&t::guided_why(&Why::Forced), &["PADDOCK_MACVM_GUIDED=1"]);
    has(
        &t::guided_why(&Why::HostOld("26.4".into())),
        &["macOS 26.4", "macOS 27"],
    );
    has(
        &t::guided_why(&Why::GuestOld("15.2".into())),
        &["guest is macOS 15.2"],
    );
    has(&t::guided_why(&Why::Missing("VZX".into())), &["VZX"]);
    has(&t::guided_why(&Why::FirstBootUsed), &["first boot"]);
    has(
        &t::guided_why(&Why::Refused("bad".into())),
        &["refused", "bad"],
    );
}

#[test]
fn route_guided() {
    has(
        &t::route_guided(&Why::FirstBootUsed),
        &["Setup Assistant", "first boot"],
    );
}

#[test]
fn unattended_banner() {
    has(&t::unattended_banner(), &["nothing to do", "Stop"]);
}

#[test]
fn seeded_agent_late() {
    has(
        &t::seeded_agent_late(300, Path::new("/g/logs/console.log"), "sudo /bin/sh x"),
        &[
            "300 s",
            "/g/logs/console.log",
            "paddock-firstboot.log",
            "sudo /bin/sh x",
        ],
    );
}

#[test]
fn status_offline_failed() {
    has(
        &t::status_offline_failed("seed offline", "no APFS"),
        &["seed offline failed", "no APFS"],
    );
}

#[test]
fn downloads_no_home() {
    has(
        &t::downloads_no_home(Path::new("/m/Data/Users/paddock")),
        &["/m/Data/Users/paddock", "not made yet"],
    );
}

#[test]
fn downloads_not_a_folder() {
    has(
        &t::downloads_not_a_folder(Path::new("/m/Users/paddock/Downloads")),
        &[
            "/m/Users/paddock/Downloads",
            "not a folder",
            "leaves it alone",
        ],
    );
}

#[test]
fn downloads_not_empty() {
    has(
        &t::downloads_not_empty(
            Path::new("/m/Users/paddock/Downloads"),
            &["a.pdf".into(), "sub".into()],
        ),
        &[
            "/m/Users/paddock/Downloads",
            "(a.pdf, sub)",
            "run paddock macvm setup again",
        ],
    );
}

#[test]
fn downloads_offline_done() {
    has(
        &t::downloads_offline_done(true),
        &["now a link", "stopped guest"],
    );
    has(&t::downloads_offline_done(false), &["already"]);
}

#[test]
fn downloads_offline_failed() {
    has(
        &t::downloads_offline_failed("no home"),
        &["(no home)", "downloads-link fails"],
    );
}

#[test]
fn data_format_failed() {
    has(
        &t::data_format_failed("diskutil: permission denied"),
        &[
            "not formatted on the host",
            "guest agent formats it",
            "permission denied",
        ],
    );
}

#[test]
fn data_format_no_table() {
    has(&t::data_format_no_table(), &["no partition table"]);
}

#[test]
fn data_format_size_changed() {
    has(&t::data_format_size_changed(), &["size"]);
}
