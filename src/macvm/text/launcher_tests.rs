//! One test per function of text/launcher.rs, named as the function.

use super::super::launcher as t;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn pause_requested() {
    has(&t::pause_requested(), &["pauses", "--resume"]);
}

#[test]
fn pause_no_build() {
    has(
        &t::pause_no_build(),
        &["No golden build runs", "paddock macvm setup"],
    );
}

#[test]
fn pause_past_download() {
    has(
        &t::pause_past_download("install"),
        &["install state", "Only the download"],
    );
}

#[test]
fn resume_running() {
    has(&t::resume_running(), &["continues"]);
}

const GIB: u64 = 1 << 30;

#[test]
fn doctor_chip_ok() {
    has(
        &t::doctor_chip_ok(Some("Apple M2 Pro")),
        &["Apple silicon", "M2 Pro"],
    );
    assert_eq!(t::doctor_chip_ok(None), "This Mac has Apple silicon.");
}

#[test]
fn doctor_macos_ok() {
    assert_eq!(t::doctor_macos_ok("15.1"), "macOS 15.1.");
}

#[test]
fn doctor_macos_unknown() {
    has(&t::doctor_macos_unknown(), &["sw_vers"]);
}

#[test]
fn doctor_space() {
    assert_eq!(
        t::doctor_space(184 * GIB, 40 * GIB),
        "184 GiB free, 40 GiB needed."
    );
    has(
        &t::doctor_space(12 * GIB, 40 * GIB),
        &["Needs 40 GiB, 12 GiB free", "check again"],
    );
}

#[test]
fn doctor_memory() {
    assert_eq!(
        t::doctor_memory(16 * GIB, false),
        "16 GiB: each app gets up to 8 GiB."
    );
    assert_eq!(
        t::doctor_memory(8 * GIB, true),
        "8 GiB: apps get 4 GiB each, which is the minimum."
    );
}

#[test]
fn doctor_memory_unknown() {
    has(&t::doctor_memory_unknown(), &["4 GiB"]);
}

#[test]
fn doctor_entitlement() {
    has(
        &t::doctor_entitlement(Some(true)),
        &["has the Virtualization"],
    );
    has(&t::doctor_entitlement(Some(false)), &["lacks", "Homebrew"]);
    has(&t::doctor_entitlement(None), &["not installed", "Homebrew"]);
}

#[test]
fn doctor_slots() {
    assert_eq!(t::doctor_slots(1, 2), "1 of 2 macOS VMs run.");
    has(&t::doctor_slots(2, 2), &["2 of 2", "Stop one"]);
}

#[test]
fn setup_helper_exit() {
    has(&t::setup_helper_exit(3), &["exit code 3", "continues"]);
}

#[test]
fn add_name_taken() {
    has(
        &t::add_name_taken("sample-mail"),
        &["sample-mail", "--name"],
    );
}

#[test]
fn add_no_golden() {
    has(&t::add_no_golden(), &["base image", "paddock macvm setup"]);
}

#[test]
fn add_low_space() {
    assert_eq!(
        t::add_low_space(27 * GIB / 10, 11 * GIB / 10),
        "Needs 2.7 GiB, 1.1 GiB free. Free up space, then add the app again."
    );
}

#[test]
fn add_already() {
    has(
        &t::add_already(
            "sample-mail",
            std::path::Path::new("/Applications/Sample Mail.app"),
        ),
        &[
            "/Applications/Sample Mail.app",
            "paddock app run sample-mail",
        ],
    );
}

#[test]
fn add_would() {
    has(
        &t::add_would("Sample Mail", "sample-mail"),
        &["Sample Mail", "sample-mail", "--dry-run"],
    );
}

#[test]
fn add_done() {
    has(
        &t::add_done("Sample Mail", "sample-mail"),
        &[
            "Added Sample Mail as sample-mail",
            "paddock app install sample-mail",
        ],
    );
}

#[test]
fn app_not_added() {
    has(&t::app_not_added("x"), &["No app x", "paddock app list"]);
}

#[test]
fn app_remove_question() {
    has(
        &t::app_remove_question("sample-mail", 2 * GIB, false),
        &["2 GiB", "trash folder", "original app stays"],
    );
    has(
        &t::app_remove_question("sample-mail", 2 * GIB, true),
        &["delete its data now"],
    );
}

#[test]
fn app_removed() {
    assert_eq!(
        t::app_removed("sample-mail", 66 * GIB / 10, None),
        "Removed sample-mail: 6.6 GiB freed."
    );
    has(
        &t::app_removed(
            "sample-mail",
            GIB,
            Some(std::path::Path::new("/t/sample-mail")),
        ),
        &["1 GiB freed", "/t/sample-mail", "7 days"],
    );
}
