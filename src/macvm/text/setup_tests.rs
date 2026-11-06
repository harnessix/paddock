//! One test per function of text/setup.rs, named as the function.

use super::super::setup as t;
use crate::macvm::golden::State;
use std::path::{Path, PathBuf};
use std::time::Duration;

const GIB: u64 = 1 << 30;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn state_starts() {
    has(
        &t::state_starts(State::Install),
        &["step 2 of 7", "install", "minutes"],
    );
    for s in State::ALL {
        has(&t::state_starts(s), &[s.name()]);
    }
}

#[test]
fn setup_continues() {
    has(
        &t::setup_continues(State::Guided),
        &["step 4", "guided", "continues"],
    );
}

#[test]
fn setup_crashed() {
    let s = t::setup_crashed(State::Install, &[PathBuf::from("/g/disk.img.partial")]);
    has(
        &s,
        &[
            "step 2",
            "install",
            "disk.img.partial",
            "runs the step again",
        ],
    );
}

#[test]
fn setup_stopped() {
    has(
        &t::setup_stopped(State::Seed),
        &["step 3", "seed", "paddock macvm setup"],
    );
}

#[test]
fn state_failed() {
    let s = t::state_failed(State::Fetch, "no network.");
    has(
        &s,
        &["step 1", "fetch", "no network.", "paddock macvm setup"],
    );
    assert!(!s.contains("network.."));
}

#[test]
fn setup_no_space() {
    let s = t::setup_no_space(
        15_000_000_000,
        1_000_000_000,
        "the download",
        Path::new("/"),
    );
    has(
        &s,
        &[
            "on /:",
            "the download needs 15 GB",
            "1 GB is free",
            "paddock macvm setup",
        ],
    );
}

#[test]
fn keyring_unavailable() {
    let s = t::keyring_unavailable(Path::new("/k"), "read-only");
    has(&s, &["keyring at /k", "read-only", "did not start"]);
}

#[test]
fn ipsw_missing() {
    has(
        &t::ipsw_missing(Path::new("/x/none.ipsw")),
        &["--ipsw /x/none.ipsw", ".ipsw"],
    );
}

#[test]
fn golden_already() {
    has(
        &t::golden_already("15.2"),
        &["macOS 15.2", "paddock macvm update", "paddock macvm remove"],
    );
}

#[test]
fn image_already_there() {
    has(
        &t::image_already_there(Path::new("/i/15.2.ipsw")),
        &["/i/15.2.ipsw", "already there"],
    );
}

#[test]
fn download_short() {
    has(
        &t::download_short(10, 20),
        &["10 bytes", "said 20", "again"],
    );
}

#[test]
fn download_short_twice() {
    has(
        &t::download_short_twice(10, 20),
        &["10 bytes twice", "said 20"],
    );
}

#[test]
fn image_check_failed() {
    has(
        &t::image_check_failed(Path::new("/i.ipsw"), "bad"),
        &["/i.ipsw failed its check: bad"],
    );
}

#[test]
fn image_not_named() {
    has(&t::image_not_named(), &["no restore image"]);
}

#[test]
fn image_gone() {
    has(&t::image_gone(Path::new("/i.ipsw")), &["/i.ipsw is gone"]);
}

#[test]
fn image_size_differs() {
    has(
        &t::image_size_differs(Path::new("/i.ipsw"), 1, 2),
        &["/i.ipsw has 1 bytes", "said 2"],
    );
}

#[test]
fn installer_left_no() {
    has(
        &t::installer_left_no(Path::new("/g/aux.img.partial")),
        &["left no /g/aux.img.partial"],
    );
}

#[test]
fn installed() {
    let s = t::installed("15.2 (24C101)", Path::new("/g/disk.img"), 20 * GIB);
    has(&s, &["macOS 15.2 (24C101)", "/g/disk.img", "20 GiB used"]);
}

#[test]
fn seal_failed() {
    has(
        &t::seal_failed(Path::new("/g/disk.img"), "busy"),
        &["cannot seal /g/disk.img: busy"],
    );
}

#[test]
fn ask_without_terminal() {
    has(
        &t::ask_without_terminal("Go?"),
        &["Go?\n", "paddock macvm setup --yes"],
    );
}

#[test]
fn ask_prompt() {
    assert_eq!(t::ask_prompt("Go?"), "Go? [y/N] ");
}

#[test]
fn setup_lock_held() {
    has(
        &t::setup_lock_held(Path::new("/m/setup.lock")),
        &["another `paddock macvm setup`", "/m/setup.lock"],
    );
}

#[test]
fn configure_failed() {
    has(
        &t::configure_failed(&["autologin".into(), "proxy-https".into()]),
        &["autologin, proxy-https", "paddock macvm setup"],
    );
}

#[test]
fn configure_no_result() {
    has(
        &t::configure_no_result(),
        &["configure", "without step results"],
    );
}

#[test]
fn agent_missing() {
    has(
        &t::agent_missing("seed"),
        &[
            "seed:",
            "paddock-guest",
            "build-guest.sh",
            "paddock macvm setup",
        ],
    );
}

#[test]
fn manifest_not_toml() {
    has(
        &t::manifest_not_toml(Path::new("/g/manifest.toml")),
        &["/g/manifest.toml is not valid TOML"],
    );
}

#[test]
fn manifest_schema() {
    let s = t::manifest_schema(Path::new("/g/manifest.toml"), Some(2), 1);
    has(
        &s,
        &[
            "schema 2",
            "knows schema 1",
            "paddock macvm remove",
            "paddock macvm setup",
        ],
    );
    has(
        &t::manifest_schema(Path::new("/m"), None, 1),
        &["schema none"],
    );
}

#[test]
fn manifest_unknown_state() {
    has(
        &t::manifest_unknown_state(Path::new("/m"), "flux"),
        &[
            "/m names the unknown state \"flux\"",
            "paddock macvm remove",
        ],
    );
}

#[test]
fn manifest_invalid() {
    has(
        &t::manifest_invalid(Path::new("/m")),
        &["/m is not a golden manifest"],
    );
}

#[test]
fn manifest_unserializable() {
    has(&t::manifest_unserializable(), &["manifest"]);
}

#[test]
fn fake_free_invalid() {
    has(
        &t::fake_free_invalid("1X"),
        &["PADDOCK_MACVM_FAKE_FREE=\"1X\"", "1G"],
    );
}

#[test]
fn no_new_golden() {
    has(
        &t::no_new_golden(Path::new("/g.new")),
        &["no new golden in /g.new"],
    );
}

#[test]
fn no_prev_golden() {
    has(
        &t::no_prev_golden(Path::new("/g.prev")),
        &["no previous golden in /g.prev"],
    );
}

#[test]
fn cannot_move_aside() {
    assert_eq!(
        t::cannot_move_aside(Path::new("/g")),
        "cannot move /g aside"
    );
}

#[test]
fn status_head() {
    has(&t::status_head(Path::new("/m")), &["macOS guest in /m:"]);
}

#[test]
fn status_golden_none() {
    has(
        &t::status_golden_none(),
        &["  golden   none", "paddock macvm setup"],
    );
}

#[test]
fn status_golden_unreadable() {
    has(
        &t::status_golden_unreadable("bad toml"),
        &["  golden   bad toml"],
    );
}

#[test]
fn status_golden_unsealed() {
    has(
        &t::status_golden_unsealed("15.2", State::Guided),
        &["macOS 15.2", "step 4 (guided)", "paddock macvm setup"],
    );
}

#[test]
fn status_golden_sealed() {
    let s = t::status_golden_sealed("15.2", "2026-11-04T10:00:00Z", 23 * GIB, true);
    has(&s, &["macOS 15.2", "sealed 2026-11-04T10:00:00Z", "23 GiB"]);
    assert!(!s.contains("flags"));
    has(
        &t::status_golden_sealed("15.2", "?", GIB, false),
        &["seal flags are missing"],
    );
}

#[test]
fn status_golden_older() {
    has(
        &t::status_golden_older("15.2", "15.3"),
        &["(15.2)", "macOS 15.3", "paddock macvm update"],
    );
}

#[test]
fn status_host() {
    assert_eq!(t::status_host("15.3"), "  host     macOS 15.3");
}

#[test]
fn status_profiles_none() {
    has(&t::status_profiles_none(), &["profiles none"]);
}

#[test]
fn status_profile() {
    has(
        &t::status_profile("chatgpt-desktop", "default", 3 * GIB),
        &["chatgpt-desktop/default 3 GiB"],
    );
}

#[test]
fn status_sessions_none() {
    has(&t::status_sessions_none(), &["sessions none"]);
}

#[test]
fn status_session() {
    has(
        &t::status_session("chatgpt-desktop--default", 812, "/p"),
        &["chatgpt-desktop--default (pid 812) /p"],
    );
}

#[test]
fn remove_while_running() {
    has(
        &t::remove_while_running(812),
        &["pid 812", "paddock macvm remove"],
    );
}

#[test]
fn remove_golden_question() {
    has(
        &t::remove_golden_question("15.2", 23 * GIB),
        &["macOS 15.2", "23 GiB", "--profiles"],
    );
}

#[test]
fn remove_profiles_question() {
    has(
        &t::remove_profiles_question(2, 5 * GIB),
        &["2 profile(s)", "5 GiB", "trash", "7 days"],
    );
}

#[test]
fn needs_apple_silicon() {
    has(&t::needs_apple_silicon(), &["Apple silicon"]);
}

#[test]
fn no_virtualization() {
    has(&t::no_virtualization(), &["virtual machines"]);
}

#[test]
fn main_thread_only() {
    has(
        &t::main_thread_only("paddock-vm macvm-setup"),
        &["paddock-vm macvm-setup must run on the main thread"],
    );
}

#[test]
fn parent_ended() {
    has(&t::parent_ended(), &["paddock macvm setup ended"]);
}

#[test]
fn restore_image_no_url() {
    has(&t::restore_image_no_url(), &["no URL"]);
}

#[test]
fn image_unsupported() {
    has(
        &t::image_unsupported("16.0", "25A1"),
        &["macOS 16.0 (25A1)", "no supported configuration"],
    );
}

#[test]
fn hardware_model_unsupported() {
    has(
        &t::hardware_model_unsupported("16.0"),
        &["macOS 16.0", "not supported on this Mac"],
    );
}

#[test]
fn image_lost_configuration() {
    has(&t::image_lost_configuration(), &["lost its configuration"]);
}

#[test]
fn guest_shut_down_in_guided() {
    has(
        &t::guest_shut_down_in_guided(),
        &["shut down", "guided step"],
    );
}

#[test]
fn guest_stopped_with_error() {
    has(
        &t::guest_stopped_with_error("boom"),
        &["stopped with an error: boom"],
    );
}

#[test]
fn guided_window_gone() {
    has(&t::guided_window_gone(), &["window is gone"]);
}

#[test]
fn guest_no_shutdown() {
    has(
        &t::guest_no_shutdown(Duration::from_secs(300)),
        &["within 5 min", "Apple menu", "paddock macvm setup"],
    );
}

#[test]
fn main_thread_no_boot() {
    has(&t::main_thread_no_boot(), &["main thread", "boot"]);
}

#[test]
fn golden_start_failed() {
    has(
        &t::golden_start_failed("e"),
        &["cannot start the golden: e"],
    );
}

#[test]
fn golden_stopped_at_start() {
    has(
        &t::golden_stopped_at_start(Some("e")),
        &["stopped at the start: e"],
    );
    assert_eq!(
        t::golden_stopped_at_start(None),
        "the golden stopped at the start"
    );
}

#[test]
fn golden_start_timeout() {
    has(
        &t::golden_start_timeout(Duration::from_secs(60)),
        &["did not start within 1 min"],
    );
}

#[test]
fn stop_request_failed() {
    has(&t::stop_request_failed("e"), &["stop request failed: e"]);
}

#[test]
fn cannot_open_for_vm() {
    assert_eq!(
        t::cannot_open_for_vm(Path::new("/d"), "e"),
        "cannot open /d: e"
    );
}

#[test]
fn cannot_make_for_vm() {
    assert_eq!(
        t::cannot_make_for_vm(Path::new("/d"), "e"),
        "cannot make /d: e"
    );
}

#[test]
fn stub_network_failed() {
    assert_eq!(
        t::stub_network_failed("make"),
        "cannot make the stub network"
    );
}

#[test]
fn configuration_invalid() {
    has(&t::configuration_invalid("e"), &["not valid: e"]);
}

#[test]
fn installer_no_start() {
    has(&t::installer_no_start(), &["installer did not start"]);
}

#[test]
fn install_failed() {
    has(&t::install_failed("e"), &["install of macOS failed: e"]);
}

#[test]
fn installer_no_result() {
    has(&t::installer_no_result(), &["without a result"]);
}

#[test]
fn fetch_failed() {
    has(&t::fetch_failed("e"), &["newest macOS", ": e"]);
}

#[test]
fn fetch_timeout() {
    has(&t::fetch_timeout(), &["Apple did not answer"]);
}

#[test]
fn image_unusable() {
    has(
        &t::image_unusable(Path::new("/i"), "e"),
        &["/i is not a usable restore image: e"],
    );
}

#[test]
fn image_load_timeout() {
    has(
        &t::image_load_timeout(Path::new("/i")),
        &["load of /i did not end"],
    );
}

#[test]
fn downloading() {
    assert_eq!(t::downloading("https://x"), "downloading https://x");
}

#[test]
fn download_failed() {
    has(
        &t::download_failed("exit status: 22", "404"),
        &["failed (exit status: 22): 404"],
    );
}

#[test]
fn seed_prepared() {
    has(
        &t::seed_prepared(Path::new("/g/seed"), true),
        &["/g/seed holds the agent", "read-only"],
    );
    has(
        &t::seed_prepared(Path::new("/g/seed"), false),
        &["no agent"],
    );
}

#[test]
fn seed_install_line() {
    has(
        &t::seed_install_line("sudo /bin/sh x"),
        &["Terminal of the guest", "password", ": sudo /bin/sh x"],
    );
}

#[test]
fn aux_busy_wait() {
    has(&t::aux_busy_wait(), &["still holds the golden", "a minute"]);
}
