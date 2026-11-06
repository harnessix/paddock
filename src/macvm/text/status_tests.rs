//! One test per function of text/status.rs, named as the function.

use super::super::status as t;
use super::super::{ColdWhy, Holder, Share, StartKind};
use std::path::Path;

const GIB: u64 = 1 << 30;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn status_row() {
    assert_eq!(t::status_row("golden", "x"), "golden            x");
    assert_eq!(
        t::status_row("golden", "x").find('x'),
        Some(t::STATUS_LABEL_WIDTH)
    );
}

#[test]
fn status_never_started() {
    assert_eq!(
        t::status_never_started("vscode"),
        "vscode            never started, next start cold: the first start of this profile"
    );
}

#[test]
fn next_start_text() {
    assert_eq!(t::next_start_text(StartKind::Warm, None), "warm");
    assert_eq!(t::next_start_text(StartKind::Cold, None), "cold");
    assert_eq!(
        t::next_start_text(
            StartKind::Cold,
            Some(super::super::cold_why(ColdWhy::NewGolden))
        ),
        "cold: the guest image changed"
    );
}

#[test]
fn status_golden_row() {
    assert_eq!(
        t::status_golden_row("15.2 (24C101)", "0.9.0", "1.3"),
        "macOS 15.2 (24C101), agent 0.9.0, protocol 1.3"
    );
}

#[test]
fn status_golden_unknown() {
    has(&t::status_golden_unknown(), &["not started"]);
}

#[test]
fn status_app_row() {
    assert_eq!(
        t::status_app_row(
            "ChatGPT",
            "1.2025.300",
            Some("2026-11-02"),
            Some(true),
            Some(2)
        ),
        "ChatGPT 1.2025.300 (copied 2026-11-02), running, 2 windows"
    );
    assert_eq!(
        t::status_app_row("ChatGPT", "1.2025.300", None, Some(false), Some(1)),
        "ChatGPT 1.2025.300, not running, 1 window"
    );
    assert_eq!(
        t::status_app_row("ChatGPT", "", None, None, None),
        "ChatGPT"
    );
}

#[test]
fn status_app_none() {
    has(&t::status_app_none(), &["none"]);
}

#[test]
fn status_guest_row() {
    assert_eq!(
        t::status_guest_row(true, 1, 0.4, 19 * GIB / 10, Some(142 * GIB / 10)),
        "healthy (last health 1 s ago), load 0.4, 1.9 GiB free RAM, data disk 14.2 GiB free"
    );
    has(
        &t::status_guest_row(false, 42, 1.25, GIB, None),
        &[
            "not answering (last health 42 s ago)",
            "load 1.2",
            "data disk not mounted",
        ],
    );
}

#[test]
fn status_guest_booting() {
    has(&t::status_guest_booting(), &["booting"]);
}

#[test]
fn status_guest_stopped() {
    assert_eq!(t::status_guest_stopped(), "stopped");
}

#[test]
fn status_host_row() {
    assert_eq!(
        t::status_host_row(63 * GIB / 10, 41 * GIB / 10, 4 * GIB, 120 * GIB),
        "profile 6.3 GiB (data 4.1 GiB, state 4 GiB), disk 120 GiB free"
    );
}

#[test]
fn status_network_row() {
    assert_eq!(
        t::status_network_row("ask", 14, 2, Some("telemetry.example.com:443")),
        "ask, 14 allowed, 2 denied in this session (last: telemetry.example.com:443)"
    );
    assert_eq!(
        t::status_network_row("none", 0, 0, None),
        "none, 0 allowed, 0 denied in this session"
    );
}

#[test]
fn status_network_stopped() {
    assert_eq!(t::status_network_stopped(), "no session");
}

#[test]
fn stopped_head() {
    assert_eq!(
        t::stopped_head("quit with save", "warm"),
        "stopped (quit with save), next start warm"
    );
}

#[test]
fn status_shares_row() {
    let shares = [
        Share {
            path: Path::new("/Users/u/src/web"),
            writable: true,
        },
        Share {
            path: Path::new("/Users/u/src/api"),
            writable: false,
        },
    ];
    assert_eq!(
        t::status_shares_row(&shares, Path::new("/p/downloads")),
        "/Users/u/src/web (rw), /Users/u/src/api (ro), downloads -> /p/downloads"
    );
    assert_eq!(
        t::status_shares_row(&[], Path::new("/p/downloads")),
        "no workspace folders, downloads -> /p/downloads"
    );
}

#[test]
fn status_logs_row() {
    assert_eq!(t::status_logs_row(Path::new("/p/logs")), "/p/logs/");
    assert_eq!(t::status_logs_row(Path::new("/p/logs/")), "/p/logs/");
}

#[test]
fn status_no_profile() {
    has(
        &t::status_no_profile("x"),
        &["no profile x", "paddock app list"],
    );
}

#[test]
fn event_line() {
    assert_eq!(
        t::event_line("2026-11-04T10:00:00Z", "quit", "asked", &[]),
        "2026-11-04T10:00:00Z  quit     asked"
    );
    assert_eq!(
        t::event_line(
            "T",
            "restore",
            "done",
            &[("secs".into(), "4.1".into()), ("bytes".into(), "7".into())]
        ),
        "T  restore  done  [secs=4.1, bytes=7]"
    );
}

#[test]
fn log_missing() {
    assert_eq!(
        t::log_missing(Path::new("/p/logs/shim.log")),
        "no log at /p/logs/shim.log yet"
    );
}

#[test]
fn log_follow_ended() {
    assert_eq!(
        t::log_follow_ended("chatgpt-desktop"),
        "the session of chatgpt-desktop ended"
    );
}

#[test]
fn connections_fallback() {
    has(
        &t::connections_fallback(Path::new("/r/connections.jsonl")),
        &["no connection log at /r/connections.jsonl", "boundary log"],
    );
}

#[test]
fn status_profile_state() {
    assert_eq!(
        t::status_profile_state(
            "chatgpt-desktop",
            "default",
            63 * GIB / 10,
            "running (pid 812, 2h 14m)"
        ),
        "  profile  chatgpt-desktop/default 6.3 GiB, running (pid 812, 2h 14m)"
    );
}

#[test]
fn status_slots() {
    assert_eq!(t::status_slots(&[None, None]), "  slots    0 of 2 in use");
    let one = [
        Some(Holder::Mine {
            app: "chatgpt-desktop".into(),
            pid: 812,
        }),
        None,
    ];
    assert_eq!(
        t::status_slots(&one),
        "  slots    1 of 2 in use (chatgpt-desktop (you, pid 812))"
    );
    let two = [
        Some(Holder::Mine {
            app: "a".into(),
            pid: 1,
        }),
        Some(Holder::User { name: "ana".into() }),
    ];
    assert_eq!(
        t::status_slots(&two),
        "  slots    2 of 2 in use (a (you, pid 1), a VM of user ana)"
    );
}

#[test]
fn holder_short() {
    assert_eq!(
        t::holder_short(&Holder::OtherProgram),
        "a VM of another program"
    );
    assert_eq!(
        t::holder_short(&Holder::User { name: "ana".into() }),
        "a VM of user ana"
    );
}

#[test]
fn report_missing() {
    assert_eq!(
        t::report_missing(Path::new("/p/stamp.json")),
        "missing: /p/stamp.json"
    );
}

#[test]
fn report_tail_heading() {
    assert_eq!(
        t::report_tail_heading(Path::new("/p/logs/shim.log"), 200),
        "/p/logs/shim.log (last 200 lines):"
    );
}

#[test]
fn report_connections() {
    assert_eq!(t::report_connections(&[]), "connections: none logged");
    assert_eq!(
        t::report_connections(&[("allowed".into(), 14), ("denied".into(), 2)]),
        "connections: 14 allowed, 2 denied"
    );
}

#[test]
fn report_file_heading() {
    assert_eq!(
        t::report_file_heading("manifest", Path::new("/g/manifest.toml")),
        "manifest /g/manifest.toml:"
    );
}

#[test]
fn report_profile_heading() {
    assert_eq!(
        t::report_profile_heading("vscode", "default", Path::new("/p/vscode/default")),
        "profile vscode/default (/p/vscode/default):"
    );
}

#[test]
fn report_password_line() {
    has(&t::report_password_line(), &["password removed"]);
}

#[test]
fn report_no_hosts_note() {
    assert_eq!(
        t::report_no_hosts_note(3),
        "3 host names replaced with host-N (--no-hosts)"
    );
}

#[test]
fn secrets_printed() {
    assert_eq!(
        t::secrets_printed("t0", Some("p")),
        "PADDOCK_MACVM_PRINT_SECRETS: token=t0 password=p"
    );
    assert_eq!(
        t::secrets_printed("t0", None),
        "PADDOCK_MACVM_PRINT_SECRETS: token=t0"
    );
}
