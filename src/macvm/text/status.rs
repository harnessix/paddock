//! The texts of observability: the rows of `paddock app status`, the
//! lines of `paddock app log`, the profile and slot lines of `paddock
//! macvm status`, and the headings of the `macvm` section of `paddock
//! doctor --report`.

use super::{gib, shown, Holder, Share, StartKind};
use std::path::Path;

/// The width of the label column of `paddock app status`.
pub const STATUS_LABEL_WIDTH: usize = 18;

// ---- `paddock app status` -----------------------------------------------

/// One row: the label, then the value at the column.
pub fn status_row(label: &str, value: &str) -> String {
    format!("{label:<STATUS_LABEL_WIDTH$}{value}")
}

/// The head line of a profile that never ran.
pub fn status_never_started(name: &str) -> String {
    format!(
        "{name:<STATUS_LABEL_WIDTH$}never started, next start {}",
        next_start_text(
            StartKind::Cold,
            Some(super::cold_why(super::ColdWhy::First))
        )
    )
}

/// The next start of a stopped profile: `warm`, `cold: <why>` or `cold`.
pub fn next_start_text(next: StartKind, why: Option<&str>) -> String {
    match (next, why) {
        (StartKind::Warm, _) => "warm".to_string(),
        (StartKind::Cold, Some(w)) => format!("cold: {w}"),
        (StartKind::Cold, None) => "cold".to_string(),
    }
}

/// `macOS 15.2 (24C101), agent 0.9.0, protocol 1.3`.
pub fn status_golden_row(golden: &str, agent: &str, protocol: &str) -> String {
    format!("macOS {golden}, agent {agent}, protocol {protocol}")
}

/// The golden row before the first start: nothing is stamped yet.
pub fn status_golden_unknown() -> String {
    "not started on this golden yet".into()
}

/// `ChatGPT 1.2025.300 (copied 2026-11-02), running, 2 windows`.
pub fn status_app_row(
    name: &str,
    version: &str,
    copied: Option<&str>,
    running: Option<bool>,
    windows: Option<usize>,
) -> String {
    let mut s = if version.is_empty() {
        name.to_string()
    } else {
        format!("{name} {version}")
    };
    if let Some(c) = copied {
        s.push_str(&format!(" (copied {c})"));
    }
    match running {
        Some(true) => s.push_str(", running"),
        Some(false) => s.push_str(", not running"),
        None => {}
    }
    match windows {
        Some(1) => s.push_str(", 1 window"),
        Some(n) => s.push_str(&format!(", {n} windows")),
        None => {}
    }
    s
}

/// The app row of a session without an app of its own.
pub fn status_app_none() -> String {
    "none (the session runs the guest without an app)".into()
}

/// The guest row of a running session.
pub fn status_guest_row(
    answering: bool,
    health_age_s: u64,
    load: f64,
    mem_free: u64,
    data_free: Option<u64>,
) -> String {
    let state = if answering {
        "healthy"
    } else {
        "not answering"
    };
    let data = match data_free {
        Some(b) => format!("data disk {} free", gib(b)),
        None => "data disk not mounted".to_string(),
    };
    format!(
        "{state} (last health {health_age_s} s ago), load {load:.1}, {} free RAM, {data}",
        gib(mem_free)
    )
}

/// The guest row while no `health` came yet.
pub fn status_guest_booting() -> String {
    "booting (no health yet)".into()
}

/// The guest row of a stopped profile.
pub fn status_guest_stopped() -> String {
    "stopped".into()
}

/// `profile 6.3 GiB (data 4.1 GiB, state 4.0 GiB), disk 120 GiB free`.
pub fn status_host_row(profile: u64, data: u64, state: u64, disk_free: u64) -> String {
    format!(
        "profile {} (data {}, state {}), disk {} free",
        gib(profile),
        gib(data),
        gib(state),
        gib(disk_free)
    )
}

/// `ask, 14 allowed, 2 denied in this session (last: host:443)`.
pub fn status_network_row(mode: &str, allowed: u64, denied: u64, last: Option<&str>) -> String {
    let mut s = format!("{mode}, {allowed} allowed, {denied} denied in this session");
    if let Some(l) = last {
        s.push_str(&format!(" (last: {l})"));
    }
    s
}

/// The network row of a stopped profile.
pub fn status_network_stopped() -> String {
    "no session".into()
}

/// The value of the head line of a stopped profile, after the name.
pub fn stopped_head(end: &str, next: &str) -> String {
    format!("stopped ({end}), next start {next}")
}

/// `~/src/web (rw), downloads -> PATH`.
pub fn status_shares_row(shares: &[Share<'_>], downloads: &Path) -> String {
    let mut parts: Vec<String> = shares
        .iter()
        .map(|s| {
            format!(
                "{} ({})",
                shown(s.path),
                if s.writable { "rw" } else { "ro" }
            )
        })
        .collect();
    if parts.is_empty() {
        parts.push("no workspace folders".into());
    }
    parts.push(format!("downloads -> {}", shown(downloads)));
    parts.join(", ")
}

/// The logs row: the folder with a trailing slash.
pub fn status_logs_row(dir: &Path) -> String {
    format!("{}/", shown(dir).trim_end_matches('/'))
}

/// `paddock app status NAME` without a profile folder.
pub fn status_no_profile(name: &str) -> String {
    format!("no profile {name} in the macOS guest (paddock app list shows them)")
}

// ---- `paddock app log` --------------------------------------------------

/// One event of `paddock app log --events`: the time, the kind and the
/// text, then the numbers in brackets.
pub fn event_line(t: &str, kind: &str, text: &str, numbers: &[(String, String)]) -> String {
    let mut s = format!("{t}  {kind:<8} {text}");
    if !numbers.is_empty() {
        let ns: Vec<String> = numbers.iter().map(|(k, v)| format!("{k}={v}")).collect();
        s.push_str(&format!("  [{}]", ns.join(", ")));
    }
    s
}

/// A log that does not exist yet.
pub fn log_missing(path: &Path) -> String {
    format!("no log at {} yet", shown(path))
}

/// `--follow` ends because the session ended.
pub fn log_follow_ended(name: &str) -> String {
    format!("the session of {name} ended")
}

/// `paddock app log NAME` without `--events`: the connection log of
/// the firewall is not there, so the boundary log of the profile follows.
pub fn connections_fallback(path: &Path) -> String {
    format!(
        "no connection log at {} yet; the boundary log of the profile follows",
        shown(path)
    )
}

// ---- `paddock macvm status` -----------------------------------------------

/// One profile line with its state.
pub fn status_profile_state(app: &str, home: &str, size: u64, state: &str) -> String {
    format!("  profile  {app}/{home} {}, {state}", gib(size))
}

/// The slots line: `1 of 2 in use (chatgpt-desktop (you, pid 812))`.
pub fn status_slots(holders: &[Option<Holder>]) -> String {
    let used: Vec<String> = holders.iter().flatten().map(holder_short).collect();
    let mut s = format!("  slots    {} of {} in use", used.len(), holders.len());
    if !used.is_empty() {
        s.push_str(&format!(" ({})", used.join(", ")));
    }
    s
}

/// A slot holder in a few words.
pub fn holder_short(h: &Holder) -> String {
    match h {
        Holder::Mine { app, pid } => format!("{app} (you, pid {pid})"),
        Holder::User { name } => format!("a VM of user {name}"),
        Holder::OtherProgram => "a VM of another program".into(),
    }
}

// ---- `paddock doctor --report` --------------------------------------------

/// A file of the bundle that is not there.
pub fn report_missing(path: &Path) -> String {
    format!("missing: {}", shown(path))
}

/// The heading of the tail of a log.
pub fn report_tail_heading(path: &Path, lines: usize) -> String {
    format!("{} (last {lines} lines):", shown(path))
}

/// The counts of the connection log by verdict.
pub fn report_connections(counts: &[(String, u64)]) -> String {
    if counts.is_empty() {
        return "connections: none logged".into();
    }
    let parts: Vec<String> = counts.iter().map(|(v, n)| format!("{n} {v}")).collect();
    format!("connections: {}", parts.join(", "))
}

/// The heading of one file of the bundle: `manifest PATH:`.
pub fn report_file_heading(kind: &str, path: &Path) -> String {
    format!("{kind} {}:", shown(path))
}

/// The heading of one profile of the bundle.
pub fn report_profile_heading(app: &str, home: &str, dir: &Path) -> String {
    format!("profile {app}/{home} ({}):", shown(dir))
}

/// What a line with a password becomes in the bundle.
pub fn report_password_line() -> String {
    "<line with a password removed>".into()
}

/// The note on top of a section with `--no-hosts`.
pub fn report_no_hosts_note(count: usize) -> String {
    format!("{count} host names replaced with host-N (--no-hosts)")
}

/// The line of `paddock app run` under the test flag that prints the
/// session secrets to the parent's stderr.
pub fn secrets_printed(token: &str, password: Option<&str>) -> String {
    match password {
        Some(p) => format!("PADDOCK_MACVM_PRINT_SECRETS: token={token} password={p}"),
        None => format!("PADDOCK_MACVM_PRINT_SECRETS: token={token}"),
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
