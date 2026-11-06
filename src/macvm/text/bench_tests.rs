//! One test per function of text/bench.rs, named as the function.

use super::super::bench as t;

#[test]
fn bench_release_refused() {
    assert!(t::bench_release_refused().contains("PADDOCK_MACVM_BENCH=1"));
}

#[test]
fn bench_op_refused() {
    assert!(t::bench_op_refused().contains("--bench"));
}

#[test]
fn bench_no_text_area() {
    assert!(t::bench_no_text_area(7).contains("window 7"));
}

#[test]
fn bench_pasteboard_action() {
    assert!(t::bench_pasteboard_action("drop").contains("not drop"));
}

#[test]
fn bench_no_argv() {
    assert!(t::bench_no_argv().contains("argv"));
}

#[test]
fn bench_no_app() {
    assert!(t::bench_no_app().contains("no app"));
}

#[test]
fn bench_macos_only() {
    assert!(t::bench_macos_only().contains("macOS only"));
}

#[test]
fn bench_unknown_case() {
    let s = t::bench_unknown_case("fast", "warm, cold");
    assert!(s.contains("fast") && s.contains("warm, cold"), "{s}");
}

#[test]
fn bench_needs_bench_session() {
    let s = t::bench_needs_bench_session("chatgpt-desktop");
    assert!(s.contains("paddock app run chatgpt-desktop --bench"), "{s}");
}

#[test]
fn bench_case_line() {
    let s = t::bench_case_line("warm", 4.5, 6.0, 5, "s");
    assert_eq!(s, "warm: p50 4.5 s, p95 6 s, 5 samples");
}

#[test]
fn bench_miss_line() {
    assert_eq!(t::bench_miss_line(&"warm p95 12 s"), "MISS warm p95 12 s");
}

#[test]
fn bench_all_met() {
    assert!(t::bench_all_met(13).contains("13"));
}

#[test]
fn bench_env_target() {
    assert!(t::bench_env_target(&["warm".into()]).contains("warm"));
}

#[test]
fn bench_tool_missing() {
    let s = t::bench_tool_missing("the video file", "PADDOCK_MACVM_BENCH_VIDEO");
    assert!(s.contains("PADDOCK_MACVM_BENCH_VIDEO"), "{s}");
}

#[test]
fn bench_wait_timeout() {
    assert!(t::bench_wait_timeout("the first window", 45).contains("45 s"));
}

#[test]
fn bench_command_failed() {
    assert!(t::bench_command_failed("du", "no such file").contains("du failed"));
}

#[test]
fn bench_control_unreachable() {
    assert!(t::bench_control_unreachable(std::path::Path::new("/c.sock")).contains("/c.sock"));
}

#[test]
fn bench_target_invalid() {
    assert_eq!(t::bench_target_invalid("warm", &"bad"), "target warm: bad");
}

#[test]
fn bench_not_an_event_case() {
    assert!(t::bench_not_an_event_case("input").starts_with("input is not"));
}

#[test]
fn bench_no_start_line() {
    assert!(t::bench_no_start_line("quit").contains("no start line of quit"));
}

#[test]
fn bench_no_end_line() {
    assert!(t::bench_no_end_line("warm").contains("no end line of warm"));
}

#[test]
fn bench_table_header() {
    let h = t::bench_table_header();
    assert!(h.starts_with("measure") && h.ends_with("result\n"), "{h}");
}

#[test]
fn bench_table_row() {
    let m = crate::macvm::bench::Measure::from_samples("warm", "s", &[4.0, 6.0]);
    let r = t::bench_table_row("warm", Some(&m), "s", "p95<=10", "ok");
    assert!(
        r.starts_with("warm") && r.contains("p95<=10") && r.ends_with("ok\n"),
        "{r}"
    );
    let r = t::bench_table_row("disk", None, "gib", "p95<=6", "MISS disk: no measure");
    assert!(r.contains(" - ") && r.ends_with("no measure\n"), "{r}");
}
