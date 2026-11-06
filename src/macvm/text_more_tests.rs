//! One test per function of text.rs, named as the function: the session
//! events and lines, the status, the reset and the firewall panel.

use super as t;
use super::tests::{has, GIB};
use super::{AskButton, ResetTier, SessionEnd, StartKind, StatusRow};
use std::path::Path;
use std::time::Duration;

// ---- Session events and lines --------------------------------------------

#[test]
fn app_exited() {
    let s = t::app_exited("ChatGPT", 6);
    has(&s, &["ChatGPT exited with code 6", "Open ChatGPT again"]);
    assert!(s.contains(&t::open_again("ChatGPT")));
}

#[test]
fn crash_report() {
    let p = Path::new("/Users/me/x/logs/ChatGPT-2026-11-04.ips");
    has(
        &t::crash_report("ChatGPT", p),
        &[
            "ChatGPT",
            "3 times in 60 s",
            "/Users/me/x/logs/ChatGPT-2026-11-04.ips",
        ],
    );
}

#[test]
fn profile_repaired() {
    has(
        &t::profile_repaired(),
        &["the profile was incomplete, paddock rebuilt its system part and kept your data"],
    );
}

#[test]
fn ended_without_save() {
    has(
        &t::ended_without_save(),
        &["the last session ended without a save", "cold boot"],
    );
}

#[test]
fn save_skipped() {
    let s = t::save_skipped(5 * GIB, GIB * 12 / 10);
    has(&s, &["not enough space for the saved state (needs 5 GiB, 1.2 GiB free): the next start is a cold boot", "paddock gc"]);
}

#[test]
fn state_stale() {
    has(
        &t::state_stale("15.1", "15.2"),
        &["the saved state is from macOS 15.1, the host runs 15.2: cold boot"],
    );
}

#[test]
fn copy_signature_broken() {
    let s = t::copy_signature_broken("ChatGPT", "a sealed resource is missing");
    assert!(
        s.contains("ChatGPT") && s.contains("damaged") && s.contains("old copy stays"),
        "{s}"
    );
}

#[test]
fn copy_failed() {
    let s = t::copy_failed("ChatGPT", "ditto: No space left on device");
    has(
        &s,
        &[
            "the copy of ChatGPT into the guest failed: ditto: No space left on device",
            "The old copy stays",
        ],
    );
}

#[test]
fn agent_update_failed() {
    has(
        &t::agent_update_failed(),
        &["old agent stays", "paddock macvm update"],
    );
}

#[test]
fn guest_restarted() {
    has(&t::guest_restarted(), &["120 s", "restarted", "your data"]);
}

// ---- Status --------------------------------------------------------------

#[test]
fn status_label() {
    let labels: Vec<&str> = StatusRow::ALL.iter().map(|r| t::status_label(*r)).collect();
    assert_eq!(
        labels,
        [
            "golden",
            "app",
            "guest",
            "host",
            "data disk",
            "network",
            "shares",
            "logs"
        ]
    );
}

#[test]
fn status_running() {
    let s = t::status_running(
        "chatgpt-desktop",
        812,
        Duration::from_secs(2 * 3600 + 14 * 60 + 5),
        Some(Duration::from_millis(4100)),
    );
    assert_eq!(
        s,
        "chatgpt-desktop   running (pid 812, 2h 14m), warm start in 4.1 s"
    );
    let cold = t::status_running("x", 1, Duration::from_secs(59), None);
    assert_eq!(cold, "x                 running (pid 1, 0h 0m)");
}

#[test]
fn session_end() {
    assert_eq!(t::session_end(SessionEnd::QuitWithSave), "quit with save");
    assert_eq!(t::session_end(SessionEnd::Crashed), "crashed");
    assert_eq!(
        t::session_end(SessionEnd::StoppedWithoutSave),
        "stopped without save"
    );
}

#[test]
fn status_stopped() {
    let s = t::status_stopped(
        "chatgpt-desktop",
        SessionEnd::Crashed,
        StartKind::Cold,
        Some("golden changed"),
    );
    assert_eq!(
        s,
        "chatgpt-desktop   stopped (crashed), next start cold: golden changed"
    );
    let w = t::status_stopped("x", SessionEnd::QuitWithSave, StartKind::Warm, None);
    has(&w, &["quit with save", "next start warm"]);
}

// ---- Reset and backup ----------------------------------------------------

#[test]
fn reset_confirm() {
    let system = ["state.vzs", "apps/", "drop/", "logs/"];
    let s = t::reset_confirm("chatgpt-desktop", ResetTier::System, 6 * GIB);
    has(&s, &["paddock app reset chatgpt-desktop removes 6 GiB"]);
    has(&s, &system);
    let keeps = s.split("It keeps: ").nth(1).unwrap();
    has(
        keeps,
        &[
            "data.img",
            "downloads/",
            "backups/",
            "the firewall rules file",
        ],
    );
    assert!(!s.split("It keeps").next().unwrap().contains("data.img"));
    assert!(s.ends_with("Remove it? [y/N]"));

    let d = t::reset_confirm("chatgpt-desktop", ResetTier::Data, 10 * GIB);
    has(&d, &["--data", "data.img", "backups/pre-reset-<time>/"]);
    has(&d, &system);
    has(
        d.split("It keeps: ").nth(1).unwrap(),
        &["downloads/", "backups/", "the firewall rules file"],
    );

    let a = t::reset_confirm("chatgpt-desktop", ResetTier::All, 30 * GIB);
    has(
        &a,
        &[
            "--all",
            "the whole profile folder",
            "the firewall rules file",
            "It keeps: nothing",
            "macvm/trash/",
            "7 days",
        ],
    );

    // Only --data and --all remove the data disk.
    assert!(!ResetTier::System.removes().contains(&"data.img"));
    assert!(ResetTier::Data.removes().contains(&"data.img"));
    for tier in [ResetTier::System, ResetTier::Data, ResetTier::All] {
        for k in tier.keeps() {
            assert!(
                !tier.removes().contains(k),
                "{tier:?} keeps and removes {k}"
            );
        }
    }
}

#[test]
fn backup_made() {
    let p = Path::new("/Users/me/x/backups/2026-11-04T10-00.img");
    has(
        &t::backup_made(p),
        &["/Users/me/x/backups/2026-11-04T10-00.img"],
    );
}

#[test]
fn backup_skipped() {
    has(
        &t::backup_skipped(4 * GIB, GIB),
        &["no backup", "1 GiB free", "needs 4 GiB", "paddock gc"],
    );
}

#[test]
fn restore_done() {
    let s = t::restore_done(
        Path::new("/b/2026-11-02.img"),
        Path::new("/b/pre-restore-2026-11-04.img"),
    );
    has(&s, &["/b/2026-11-02.img", "/b/pre-restore-2026-11-04.img"]);
}

#[test]
fn quit_first() {
    assert_eq!(
        t::quit_first("chatgpt-desktop"),
        "quit the session first (paddock app stop chatgpt-desktop)"
    );
}

// ---- Firewall ------------------------------------------------------------

#[test]
fn ask_panel() {
    let p = Path::new("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT");
    let s = t::ask_panel("ChatGPT", "telemetry.example.com", 443, p);
    has(
        &s,
        &[
            "ChatGPT",
            "telemetry.example.com",
            "port 443",
            &p.display().to_string(),
        ],
    );
}

#[test]
fn ask_button() {
    let labels: Vec<&str> = AskButton::ALL.iter().map(|b| t::ask_button(*b)).collect();
    assert_eq!(
        labels,
        [
            "Allow once",
            "Allow this session",
            "Always allow",
            "Deny",
            "Always deny"
        ]
    );
}

#[test]
fn ask_why() {
    has(
        &t::ask_why(Path::new("/usr/libexec/nsurlsessiond")),
        &["Why?", "/usr/libexec/nsurlsessiond", "in the guest"],
    );
}

// ---- The helpers ---------------------------------------------------------

#[test]
fn sizes_and_times_read_well() {
    assert_eq!(super::gib(0), "0 GiB");
    assert_eq!(super::gib(GIB / 2), "0.5 GiB");
    assert_eq!(super::gib(GIB * 99 / 100), "1 GiB");
    assert_eq!(super::left_text(Duration::from_secs(61)), "2 min");
    assert_eq!(super::fit("abc", 3), "abc");
    assert_eq!(super::fit("abcd", 3), "ab…");
}

// ---- The size and time helpers ------------------------------------------

#[test]
fn gib() {
    assert_eq!(t::gib(23_400_000_000), "21.8 GiB");
    assert_eq!(t::gib(64 << 30), "64 GiB");
    assert_eq!(t::gib(200 << 30), "200 GiB");
    assert_eq!(t::gib((1 << 30) * 12 / 10), "1.2 GiB");
    assert_eq!(t::gib(5 << 20), "5 MiB");
    assert_eq!(t::gib(512 << 20), "0.5 GiB");
    assert_eq!(t::gib(0), "0 GiB");
}

#[test]
fn gb() {
    assert_eq!(t::gb(15_000_000_000), "15 GB");
    assert_eq!(t::gb(1 << 30), "1 GB");
    assert_eq!(t::gb(0), "0 GB");
}

#[test]
fn left_text() {
    assert_eq!(t::left_text(Duration::from_secs(12)), "12 s");
    assert_eq!(t::left_text(Duration::from_secs(185)), "4 min");
    assert_eq!(t::left_text(Duration::from_secs(3900)), "1 h 5 min");
}
