//! The texts of `paddock macvm bench`: the refusals of the command, the
//! refusals of the bench ops in the guest, and the lines of the script.

/// A release build without `PADDOCK_MACVM_BENCH=1`.
pub fn bench_release_refused() -> String {
    "paddock macvm bench runs in a development build. In a release build set PADDOCK_MACVM_BENCH=1 to run it.".into()
}

/// A bench op in a session that did not start with `--bench`.
pub fn bench_op_refused() -> String {
    "refused: the session did not start with --bench".into()
}

/// `bench.animate` or `bench.text` on a window with no text area.
pub fn bench_no_text_area(window: u32) -> String {
    format!("window {window} has no text area or text field")
}

/// `bench.pasteboard` with a word other than `get` or `set`.
pub fn bench_pasteboard_action(word: &str) -> String {
    format!("bench.pasteboard takes get or set, not {word}")
}

/// `bench.spawn_child` without a command.
pub fn bench_no_argv() -> String {
    "bench.spawn_child needs a command in argv".into()
}

/// `bench.dock_menu` before the app of the session is known.
pub fn bench_no_app() -> String {
    "no app runs in this session yet".into()
}

/// `paddock macvm bench` on Linux.
pub fn bench_macos_only() -> String {
    "paddock macvm bench runs on macOS only: it drives a session of the macOS guest.".into()
}

/// A word that is not a case.
pub fn bench_unknown_case(word: &str, names: &str) -> String {
    format!("{word} is not a bench case. The cases: {names}.")
}

/// The session of the app runs without `--bench`.
pub fn bench_needs_bench_session(app: &str) -> String {
    format!(
        "{app} runs without --bench. Stop it (paddock app stop {app}) and start it again with paddock app run {app} --bench, or let the bench start it."
    )
}

/// The line of a case that ran, for the terminal.
pub fn bench_case_line(name: &str, p50: f64, p95: f64, samples: usize, unit: &str) -> String {
    format!("{name}: p50 {p50} {unit}, p95 {p95} {unit}, {samples} samples")
}

/// The line of a missed target, for the script's exit.
pub fn bench_miss_line(miss: &dyn std::fmt::Display) -> String {
    format!("MISS {miss}")
}

/// Every target met.
pub fn bench_all_met(count: usize) -> String {
    format!("every target met ({count} measures)")
}

/// A target lowered or raised by the environment.
pub fn bench_env_target(names: &[String]) -> String {
    format!("targets from the environment: {}", names.join(", "))
}

/// A tool the case needs is not there.
pub fn bench_tool_missing(what: &str, var: &str) -> String {
    format!("{what} is missing: set {var}")
}

/// The case waited too long for a state of the session.
pub fn bench_wait_timeout(what: &str, secs: u64) -> String {
    format!("waited {secs} s for {what}")
}

/// A command of the bench failed.
pub fn bench_command_failed(what: &str, detail: &str) -> String {
    format!("{what} failed: {detail}")
}

/// The control socket of the session cannot be reached.
pub fn bench_control_unreachable(path: &std::path::Path) -> String {
    format!("cannot connect to the session at {}", path.display())
}

/// A target of targets.json does not parse.
pub fn bench_target_invalid(name: &str, e: &dyn std::fmt::Display) -> String {
    format!("target {name}: {e}")
}

/// A case that the event log does not measure.
pub fn bench_not_an_event_case(name: &str) -> String {
    format!("{name} is not an event log case")
}

/// The event log has no line that starts the case.
pub fn bench_no_start_line(name: &str) -> String {
    format!("no start line of {name} in the event log")
}

/// The event log has no line that ends the case after its start.
pub fn bench_no_end_line(name: &str) -> String {
    format!("no end line of {name} after its start in the event log")
}

/// The header of the table of `bench report`.
pub fn bench_table_header() -> String {
    format!(
        "{:<10} {:>9} {:>9} {:>9} {:>7} {:<5} {:<24} {}\n",
        "measure", "p50", "p95", "mean", "n", "unit", "target", "result"
    )
}

/// One row of the table: the measure, or dashes when the case did not run.
pub fn bench_table_row(
    name: &str,
    m: Option<&super::super::bench::Measure>,
    unit: &str,
    want: &str,
    result: &str,
) -> String {
    match m {
        Some(m) => format!(
            "{:<10} {:>9.3} {:>9.3} {:>9.3} {:>7} {:<5} {:<24} {}\n",
            name, m.p50, m.p95, m.mean, m.samples, m.unit, want, result
        ),
        None => format!(
            "{:<10} {:>9} {:>9} {:>9} {:>7} {:<5} {:<24} {}\n",
            name, "-", "-", "-", 0, unit, want, result
        ),
    }
}

#[cfg(test)]
#[path = "bench_tests.rs"]
mod tests;
