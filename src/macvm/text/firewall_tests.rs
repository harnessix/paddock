use crate::macvm::text as t;
use std::path::Path;

fn has(s: &str, parts: &[&str]) {
    for p in parts {
        assert!(s.contains(p), "{s:?} lacks {p:?}");
    }
}

#[test]
fn rules_file_invalid() {
    has(
        &t::rules_file_invalid(Path::new("/c/firewall/a.toml"), "line 3"),
        &["/c/firewall/a.toml", "line 3", "paddock app rules"],
    );
}

#[test]
fn rules_file_header() {
    let h = t::rules_file_header();
    assert!(h.starts_with("# The firewall rules"), "{h}");
    assert!(
        h.lines().all(|l| l.starts_with('#')),
        "every line is a comment"
    );
    assert!(h.contains("paddock app rules NAME"));
}

#[test]
fn all_hosts_needs_flag() {
    has(
        &t::all_hosts_needs_flag(),
        &["host = \"*\"", "--yes-all-hosts"],
    );
}

#[test]
fn bad_host_glob() {
    has(&t::bad_host_glob("a b"), &["\"a b\"", "*.name"]);
}

#[test]
fn bad_port() {
    has(&t::bad_port("70000"), &["\"70000\"", "65535"]);
}

#[test]
fn bad_verdict() {
    has(&t::bad_verdict("maybe"), &["\"maybe\"", "allow or deny"]);
}

#[test]
fn bad_scope() {
    has(&t::bad_scope("week"), &["\"week\"", "always or session"]);
}

#[test]
fn no_rule() {
    has(
        &t::no_rule("rule-1", "vscode"),
        &["rule-1", "paddock app rules vscode list"],
    );
}

#[test]
fn rule_is_seed() {
    has(
        &t::rule_is_seed("seed-1", "chatgpt-desktop"),
        &["seed-1", "profile chatgpt-desktop", "--deny"],
    );
}

#[test]
fn rule_is_session() {
    has(
        &t::rule_is_session("session-1", "vscode"),
        &["session-1", "running session of vscode"],
    );
}

#[test]
fn rules_header() {
    has(
        &t::rules_header("vscode", Path::new("/c/f/vscode.toml")),
        &["vscode", "/c/f/vscode.toml"],
    );
}

#[test]
fn rules_empty() {
    has(
        &t::rules_empty("vscode"),
        &[
            "vscode has no firewall rules",
            "paddock app rules vscode add",
        ],
    );
}

#[test]
fn rule_line() {
    let l = t::rule_line(
        "rule-1a2b3c",
        "allow",
        "com.openai.chat",
        "*.openai.com",
        "443",
        "always",
        "user",
        "2026-11-04T10:22:00Z",
    );
    has(
        &l,
        &[
            "rule-1a2b3c",
            "allow",
            "com.openai.chat",
            "*.openai.com:443",
            "always, by user, 2026-11-04T10:22:00Z",
        ],
    );
}

#[test]
fn rule_added() {
    has(
        &t::rule_added("rule-1", Path::new("/c/a.toml")),
        &["Added rule rule-1", "/c/a.toml"],
    );
}

#[test]
fn rule_removed() {
    has(
        &t::rule_removed("rule-1", Path::new("/c/a.toml")),
        &["Removed rule rule-1", "/c/a.toml"],
    );
}

#[test]
fn rules_need_macvm() {
    has(&t::rules_need_macvm("claude"), &["claude", "macOS guest"]);
}

#[test]
fn ask_summary() {
    assert_eq!(
        t::ask_summary("ChatGPT", "telemetry.example.com", 443),
        "ChatGPT wants to connect to telemetry.example.com port 443"
    );
}

#[test]
fn ask_count() {
    assert_eq!(t::ask_count(1), "");
    assert_eq!(t::ask_count(3), "3 connections wait on this answer");
}

#[test]
fn connect_answers() {
    has(
        &t::connect_answers(),
        &[
            "--allow-once",
            "--allow-session",
            "--allow-always",
            "--deny",
            "--deny-always",
        ],
    );
}

#[test]
fn connect_answered() {
    assert_eq!(
        t::connect_answered("c-1", "allow-always"),
        "Answered c-1: allow-always"
    );
}

#[test]
fn connection_line() {
    let l = t::connection_line(
        "10:22:05",
        "allow",
        "com.openai.chat",
        "chatgpt.com",
        443,
        "rule-1a2b3c",
        1,
        Some((1536, 2 * 1024 * 1024, 4)),
    );
    has(
        &l,
        &[
            "10:22:05",
            "allow",
            "com.openai.chat",
            "chatgpt.com:443",
            "rule-1a2b3c",
            "1.5 KiB up, 2.0 MiB down, 4 s",
        ],
    );
    assert!(!l.contains(" x1"));
    let d = t::connection_line(
        "10:22:05",
        "deny",
        "curl",
        "example.com",
        443,
        "timeout",
        3,
        None,
    );
    has(&d, &["deny", "example.com:443", "timeout", "x3"]);
    assert!(!d.contains(" up,"));
}

#[test]
fn kib() {
    assert_eq!(t::kib(512), "512 B");
    assert_eq!(t::kib(1536), "1.5 KiB");
    assert_eq!(t::kib(3 * 1024 * 1024), "3.0 MiB");
}

#[test]
fn connection_refused() {
    has(
        &t::connection_refused("example.com", 443, "timeout"),
        &["refused example.com:443", "timeout"],
    );
}
