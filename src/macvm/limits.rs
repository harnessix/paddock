//! The limits of the macOS guest route: the table of what a session costs,
//! which the README repeats, and the checks that refuse a start with one
//! line: a third session, a host before macOS 14, a Mac without Apple
//! silicon, guest memory above half of the host's. The mixed scale of two
//! screens is a note, not a refusal. Pure, except [`host_facts`], which
//! asks macOS.

use super::crop::ScreenSet;
use super::host::{self, Session};
use super::text::{self, Holder};

/// The oldest host macOS: save and restore of the VM state and
/// `VZMacKeyboardConfiguration` need macOS 14.
pub const MIN_MACOS_MAJOR: u32 = 14;
/// Apple's limit on macOS VMs at once on one Mac.
pub const MAX_SESSIONS: usize = 2;

/// One row of the table of limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub what: &'static str,
    pub value: &'static str,
    pub told: &'static str,
}

/// The table of limits: what paddock tells the user before it spends
/// time or disk. The `paddock help macos` topic
/// has each row, which a test checks.
pub const TABLE: &[Row] = &[
    Row {
        what: "Sessions at once",
        value: "2 (Apple's limit on macOS VMs)",
        told: "the third `paddock app run` suspends the least recently used session, or stops and names the two with `[macvm] auto_suspend = false`",
    },
    Row {
        what: "RAM per session",
        value: "4 GiB, `vm_memory = \"8G\"` in `[apps.NAME]` for editors, at most half of the Mac's memory",
        told: "the start lines, `paddock macvm status`",
    },
    Row {
        what: "CPUs per session",
        value: "4",
        told: "the start lines",
    },
    Row {
        what: "Golden image on disk",
        value: "20 to 25 GB after the seal, plus the 15 GB IPSW until it is removed",
        told: "`paddock macvm setup` before the download, `paddock macvm status`",
    },
    Row {
        what: "Profile on disk",
        value: "1 to 5 GB of system writes, a data disk up to 20 GB, a saved state of the RAM size",
        told: "`paddock app list`, `paddock macvm status`",
    },
    Row {
        what: "First golden build",
        value: "10 to 20 minutes of download, 10 to 20 minutes of install, 3 minutes of first boots and configure",
        told: "`paddock macvm setup` before it starts",
    },
    Row {
        what: "First start of a profile",
        value: "25 to 40 s cold boot, plus 20 to 40 s for the copy of the app",
        told: "the start lines",
    },
    Row {
        what: "Later start",
        value: "3 to 6 s restore",
        told: "the start lines",
    },
    Row {
        what: "Quit",
        value: "2 to 8 s for the save",
        told: "the quit lines",
    },
    Row {
        what: "Input to screen",
        value: "one display frame, 16 ms at 60 Hz, plus the guest's own time",
        told: "this README",
    },
];

/// What the checks need to know about the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFacts {
    /// `sw_vers -productVersion`: `15.2`. Empty when unknown.
    pub macos: String,
    /// `hw.optional.arm64` is 1, also for a process under Rosetta.
    pub apple_silicon: bool,
}

/// The major version of `15.2`.
pub fn macos_major(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse().ok()
}

/// The refusal of a host that cannot run the guest. An unknown version
/// is not refused: the framework says more at the start.
pub fn host_refusal(h: &HostFacts) -> Option<String> {
    if !h.apple_silicon {
        return Some(text::not_apple_silicon());
    }
    match macos_major(&h.macos) {
        Some(major) if major < MIN_MACOS_MAJOR => Some(text::host_too_old(&h.macos)),
        _ => None,
    }
}

/// The refusal of a third session of this user: the line names the two
/// that run. `live` are the session records with a live shim; a second
/// start of the same instance (`key`) is the profile lock's line, not
/// this one. A suspended session holds no slot.
pub fn third_session_refusal(live: &[Session], key: &str) -> Option<String> {
    let others: Vec<&Session> = live
        .iter()
        .filter(|s| s.key() != key && !s.suspended)
        .collect();
    if others.len() < MAX_SESSIONS {
        return None;
    }
    let holders: Vec<Holder> = others
        .iter()
        .take(MAX_SESSIONS)
        .map(|s| Holder::Mine {
            app: s.label(),
            pid: s.pid,
        })
        .collect();
    Some(text::third_session(&holders))
}

/// The refusal of guest memory above half of the host's.
pub fn memory_refusal(name: &str, mib: u64, host_gib: Option<u32>) -> Option<String> {
    let host = host_gib?;
    let asked = (mib >> 10) as u32;
    (asked > host / 2).then(|| text::memory_too_large(name, asked.max(1), host))
}

/// The note of two screens with two scales: the guest draws at the main
/// screen's scale, so windows on the other screen are soft.
pub fn scale_note(screens: &ScreenSet) -> Option<String> {
    screens.mixed_scale().then(text::mixed_scales)
}

/// Every refusal before a start: the host, then a third session. With
/// automatic suspend on, a third session is not refused here: the start
/// suspends one (`host_suspend::make_room`).
pub fn refusal(
    facts: Option<&HostFacts>,
    sessions_dir: &std::path::Path,
    key: &str,
    auto_suspend: bool,
) -> Option<String> {
    if let Some(line) = facts.and_then(host_refusal) {
        return Some(line);
    }
    if auto_suspend {
        return None;
    }
    let live: Vec<Session> = host::sessions(sessions_dir)
        .into_iter()
        .map(|(_, s)| s)
        .filter(|s| host::alive(s.pid))
        .collect();
    third_session_refusal(&live, key)
}

/// The facts of this Mac. None off macOS.
pub fn host_facts() -> Option<HostFacts> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let read = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default()
    };
    let arm = read("/usr/sbin/sysctl", &["-n", "hw.optional.arm64"]);
    Some(HostFacts {
        macos: read("/usr/bin/sw_vers", &["-productVersion"]),
        // A Mac whose sysctl says nothing is not refused here: the
        // framework refuses a VM on an Intel Mac at the start too.
        apple_silicon: arm == "1" || (arm.is_empty() && cfg!(target_arch = "aarch64")),
    })
}

#[cfg(test)]
#[path = "limits_tests.rs"]
mod tests;
