//! What `paddock app run NAME` prints before the session runs, and the
//! numbers behind it: the seconds that each kind of start takes and the
//! reason of a cold one.
//!
//! Pure: the lines are built from a [`Plan`] with no disk and no VM, so a
//! Linux test holds every line of every start kind.

use super::appcopy::HostBundle;
use super::session::{Display, Profile, Reason, Refusal, Start};
use super::text::{self, ColdWhy, Share, StartKind};
use std::path::Path;

/// The seconds of a warm start (6 s p50).
pub const WARM_SECONDS: u32 = 6;
/// The seconds of a cold boot with the guest disk in place (45 s p95).
pub const COLD_SECONDS: u32 = 40;
/// The seconds of a start that clones the golden disk first.
pub const CLONE_SECONDS: u32 = 60;

/// Everything the start lines name.
#[derive(Debug, Clone)]
pub struct Plan<'a> {
    /// The profile name, which every line that names a setting uses.
    pub app: &'a str,
    /// The golden version, `15.2 (24C101)`.
    pub golden: &'a str,
    pub memory_mib: u64,
    pub display: Display,
    /// The host screens have two scales.
    pub mixed_scale: bool,
    /// The workspace folders of the user, without paddock's own shares.
    pub shares: Vec<Share<'a>>,
    pub downloads: &'a Path,
    /// `ask`, `egress`, `api`, `allowlist` or `none`.
    pub network: &'a str,
    pub start: Start,
    /// The app and the size of a copy that this start makes, or None when
    /// the guest already has the app's version.
    pub copy: Option<(&'a str, u32)>,
}

/// How long a start takes, for the line that promises it.
pub fn seconds(start: Start) -> u32 {
    match start {
        Start::Warm => WARM_SECONDS,
        _ if start.clones() => CLONE_SECONDS,
        _ => COLD_SECONDS,
    }
}

/// Cold or warm, for the line.
pub fn kind(start: Start) -> StartKind {
    match start {
        Start::Warm => StartKind::Warm,
        _ => StartKind::Cold,
    }
}

/// The short reason of a cold start, or None for a warm one.
pub fn why(start: Start) -> Option<ColdWhy> {
    match start {
        Start::Warm => None,
        Start::Fresh => Some(ColdWhy::First),
        Start::Repair => Some(ColdWhy::Repaired),
        Start::Stale(Reason::GoldenChanged) => Some(ColdWhy::NewGolden),
        Start::Stale(Reason::HostChanged) => Some(ColdWhy::NewHost),
        Start::Stale(_) => Some(ColdWhy::NoState),
        Start::Cold => Some(ColdWhy::NoState),
    }
}

/// The start lines of `paddock app run`, in order: the backend and its
/// reason, the memory, the display, the shares, the downloads path, the
/// network mode, the start kind with its seconds, the copy with its
/// seconds when there is one, and the sentence about the terminal.
pub fn lines(p: &Plan<'_>) -> Vec<String> {
    let mut out = vec![
        text::start_backend(&text::backend_reason(p.golden)),
        text::start_memory(p.app, (p.memory_mib >> 10) as u32),
        text::start_display(p.display.w, p.display.h, p.display.scale, p.mixed_scale),
        text::start_shares(p.app, &p.shares),
        text::start_downloads(p.downloads),
        text::start_network(p.network),
        text::start_kind(
            kind(p.start),
            seconds(p.start),
            why(p.start).map(text::cold_why),
        ),
    ];
    if let Some((app, secs)) = p.copy {
        out.push(text::start_copy(app, secs));
    }
    out.push(text::start_continues(p.app));
    out
}

/// What the parent predicts for the start line, from the profile folder
/// alone. It reads no host fact that only the shim has (the macOS build,
/// the framework version), so a start that the shim turns into a cold one
/// for those reasons is predicted warm here. The shim decides for real,
/// under the locks, and prints its own line. A `DataLocked` refusal is
/// final here too: the parent prints it and starts no shim.
pub fn predict(profile: &Profile, golden: &str, display: Display) -> Result<Start, Refusal> {
    use super::session::{Now, Stamp};
    let stamp = Stamp::read(&profile.stamp()).ok();
    let now = Now {
        golden: golden.to_string(),
        host_build: stamp
            .as_ref()
            .map(|s| s.host_build.clone())
            .unwrap_or_default(),
        vz: stamp.as_ref().map(|s| s.vz.clone()).unwrap_or_default(),
        protocol: stamp
            .as_ref()
            .map(|s| s.protocol)
            .unwrap_or(super::proto::VERSION),
        paddock: stamp
            .as_ref()
            .map(|s| s.paddock.clone())
            .unwrap_or_default(),
        agent: stamp.as_ref().map(|s| s.agent.clone()).unwrap_or_default(),
        display,
    };
    super::session::decide_files(profile, &now)
}

/// The copy part of a plan: the app's name and the seconds, when the
/// guest needs a new copy of this bundle.
pub fn copy_part(bundle: &HostBundle, needs_copy: bool) -> Option<(&str, u32)> {
    needs_copy.then(|| (bundle.display_name.as_str(), bundle.copy_seconds()))
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
