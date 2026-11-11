//! Every text that the user sees on the macOS guest route.
//!
//! The rule: a text says in one sentence what happened and in one
//! sentence what the reader does next (the command, the path, the pid).
//! Each text is a function here with typed arguments. No handler builds
//! a user text itself: it calls a function of this file. The test
//! `no_user_text_outside_text_rs` holds the rule for `src/macvm/` and
//! `src/guest/`, and each function has a test of the names that it must
//! contain. A badge is at most `BADGE_MAX` characters.
//!
//! The texts of one part of the route live in a child module, so no file
//! passes the size limit: `setup` (the golden build, status and remove),
//! `io` (a failed file or command), `guest` (the guest agent and the
//! protocol), `session` (the app session, its shim and its parent),
//! `windows` (the host windows, the menus and the control socket),
//! `apps` (the host bundle, the copy into the guest, the shim bundle),
//! `network` (the stub network and the proxy), `login` (the links and the login callbacks), `files` (the shares, the
//! drops and the clipboard), `panel` (the guided window of the setup)
//! and `upgrade` (`macvm update`, `rollback`, the agent self-update). Every child is re-exported here, so a caller
//! writes `text::name` whatever the file.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

#[cfg(test)]
#[path = "text_more_tests.rs"]
mod more_tests;
#[cfg(test)]
#[path = "text_rules_tests.rs"]
mod rules;
#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;

pub mod apps;
pub mod asks;
pub mod bench;
pub mod configure;
pub mod datadisk;
pub mod du;
pub mod files;
pub mod firewall;
pub mod freeze;
pub mod grants;
pub mod guest;
pub mod io;
pub mod launcher;
pub mod login;
pub mod network;
pub mod offline;
pub mod panel;
pub mod recovery;
pub mod screen;
pub mod session;
pub mod setup;
pub mod status;
pub mod suspend;
pub mod upgrade;
pub mod windows;

pub use apps::*;
pub use asks::*;
pub use bench::*;
pub use configure::*;
pub use datadisk::*;
pub use du::*;
pub use files::*;
pub use firewall::*;
pub use freeze::*;
pub use grants::*;
pub use guest::*;
pub use io::*;
pub use launcher::*;
pub use login::*;
pub use network::*;
pub use offline::*;
pub use panel::*;
pub use recovery::*;
pub use screen::*;
pub use session::*;
pub use setup::*;
pub use status::*;
pub use suspend::*;
pub use upgrade::*;
pub use windows::*;

/// The longest badge, in characters. A badge sits on the title bar of a
/// window, next to the paddock mark.
pub const BADGE_MAX: usize = 47;

/// The size of the golden build, as `paddock macvm setup` tells it before
/// the download. Apple gives the restore image size in GB (10^9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoldenSize {
    pub download_gb: u32,
    pub disk_gb: u32,
}

/// The holder of one of the two VM slots of the Mac.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    /// A session of this user: the profile and the shim pid.
    Mine { app: String, pid: u32 },
    /// A session of another user of the Mac. paddock reads only the name.
    User { name: String },
    /// The framework refused with both slots free: a VM of another program.
    OtherProgram,
}

/// A step of `paddock macvm setup` that prints progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupPhase {
    Download,
    Install,
}

/// The six steps of the guided setup panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelStep {
    Language,
    Skips,
    Account,
    /// The install command of the seed, in Terminal of the guest.
    Seed,
    Accessibility,
    Done,
}

impl PanelStep {
    pub const ALL: [PanelStep; 6] = [
        PanelStep::Language,
        PanelStep::Skips,
        PanelStep::Account,
        PanelStep::Seed,
        PanelStep::Accessibility,
        PanelStep::Done,
    ];

    /// The number that the panel shows, 1 to 6.
    pub const fn number(self) -> u8 {
        match self {
            PanelStep::Language => 1,
            PanelStep::Skips => 2,
            PanelStep::Account => 3,
            PanelStep::Seed => 4,
            PanelStep::Accessibility => 5,
            PanelStep::Done => 6,
        }
    }
}

/// A folder that the session shares with the guest.
#[derive(Debug, Clone, PartialEq)]
pub struct Share<'a> {
    pub path: &'a Path,
    pub writable: bool,
}

/// How the session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartKind {
    Cold,
    Warm,
}

/// A row label of `paddock app status NAME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusRow {
    Golden,
    App,
    Guest,
    Host,
    Data,
    Network,
    Shares,
    Logs,
}

impl StatusRow {
    pub const ALL: [StatusRow; 8] = [
        StatusRow::Golden,
        StatusRow::App,
        StatusRow::Guest,
        StatusRow::Host,
        StatusRow::Data,
        StatusRow::Network,
        StatusRow::Shares,
        StatusRow::Logs,
    ];
}

/// How the last session of a profile ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    QuitWithSave,
    QuitWithoutSave,
    Crashed,
    StoppedWithoutSave,
    NeverStarted,
}

/// The tiers of `paddock app reset NAME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetTier {
    System,
    Data,
    All,
}

impl ResetTier {
    /// What the tier removes: the system disk stays with a formatted data
    /// disk.
    pub fn removes(self) -> &'static [&'static str] {
        const SYSTEM: &[&str] = &[
            "state.vzs",
            "apps/",
            "drop/",
            "logs/",
            "the app version of the stamp",
        ];
        const DATA: &[&str] = &[
            "system.img",
            "aux.img",
            "machine-id",
            "data.img",
            "state.vzs",
            "stamp.json",
            "apps/",
            "drop/",
            "logs/",
        ];
        const ALL: &[&str] = &["the whole profile folder", "the firewall rules file"];
        match self {
            ResetTier::System => SYSTEM,
            ResetTier::Data => DATA,
            ResetTier::All => ALL,
        }
    }

    /// What the tier keeps.
    pub fn keeps(self) -> &'static [&'static str] {
        match self {
            ResetTier::System => &[
                "system.img",
                "aux.img",
                "machine-id",
                "data.img",
                "stamp.json",
                "downloads/",
                "backups/",
                "the firewall rules file",
            ],
            ResetTier::Data => &["downloads/", "backups/", "the firewall rules file"],
            ResetTier::All => &[],
        }
    }

    fn flag(self) -> &'static str {
        match self {
            ResetTier::System => "",
            ResetTier::Data => " --data",
            ResetTier::All => " --all",
        }
    }
}

/// The five answers of the firewall panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskButton {
    AllowOnce,
    AllowSession,
    AlwaysAllow,
    Deny,
    AlwaysDeny,
}

impl AskButton {
    pub const ALL: [AskButton; 5] = [
        AskButton::AllowOnce,
        AskButton::AllowSession,
        AskButton::AlwaysAllow,
        AskButton::Deny,
        AskButton::AlwaysDeny,
    ];
}

/// A size in GiB with one decimal, without `.0`: `5 GiB`, `1.2 GiB`. No
/// decimal from 100 GiB on, and MiB under 0.1 GiB, as `du -h` does.
pub fn gib(bytes: u64) -> String {
    let tenths = (bytes as f64 / (1u64 << 30) as f64 * 10.0).round() as u64;
    if tenths == 0 && bytes > 0 {
        format!("{} MiB", bytes >> 20)
    } else if tenths.is_multiple_of(10) || tenths >= 1000 {
        format!("{} GiB", tenths / 10)
    } else {
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    }
}

/// A size in decimal gigabytes with no decimal: `15 GB`. Apple gives the
/// restore image size this way, and the README says it this way.
pub fn gb(bytes: u64) -> String {
    format!("{} GB", (bytes as f64 / 1e9).round() as u64)
}

/// A time left: `40 s`, `6 min`, `1 h 5 min`.
pub fn left_text(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s.div_ceil(60)),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// A name cut to `max` characters, with `…` at the end when cut.
fn fit(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let mut s: String = name.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

fn shown(p: &Path) -> String {
    p.display().to_string()
}

// ---- Refusals -----------------------------------------------------------

/// `paddock app run` with no golden.
pub fn no_golden(app: &str, size: GoldenSize, minutes: u32) -> String {
    format!(
        "{app} runs in paddock's macOS guest. The guest needs a one-time setup of about {minutes} minutes ({} GB download, {} GB on disk): paddock macvm setup",
        size.download_gb, size.disk_gb
    )
}

/// The golden exists but is not sealed: the setup stopped half way.
pub fn golden_unsealed() -> String {
    "The setup of paddock's macOS guest did not finish. Continue it where it stopped: paddock macvm setup".into()
}

/// Both VM slots of the Mac are taken.
pub fn third_session(holders: &[Holder]) -> String {
    let names: Vec<String> = holders
        .iter()
        .map(|h| match h {
            Holder::Mine { app, pid } => format!("{app} (you, pid {pid})"),
            Holder::User { name } => format!("a VM of user {name}"),
            Holder::OtherProgram => "a VM of another program (Tart, UTM)".into(),
        })
        .collect();
    let who = match names.as_slice() {
        [] => "two VMs of another program (Tart, UTM)".to_string(),
        [one] => format!("{one} and one more VM"),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    };
    let mine = holders.iter().find_map(|h| match h {
        Holder::Mine { app, .. } => Some(app.as_str()),
        _ => None,
    });
    let next = match mine {
        Some(app) => format!("Stop one (paddock app stop {app}), then try again"),
        None => "Stop one, then try again".to_string(),
    };
    format!("two macOS VMs already run on this Mac: {who}. {next}")
}

/// A second start of a running profile.
pub fn profile_running(name: &str, pid: u32) -> String {
    format!("{name} already runs (pid {pid}). Use its windows, or stop it first: paddock app stop {name}")
}

/// Not enough free space before a spend.
pub fn no_space(needed: u64, free: u64, what: &str) -> String {
    format!(
        "not enough space for {what} (needs {}, {} free). Free space on the disk, for example with paddock gc, then try again",
        gib(needed),
        gib(free)
    )
}

/// An App Store bundle, which needs an Apple ID.
pub fn mas_bundle(app: &Path) -> String {
    format!(
        "{} comes from the App Store, and the guest has no Apple ID to run it. Install the app from the vendor's website into /Applications, then run paddock app run again",
        shown(app)
    )
}

/// A host before macOS 14.
pub fn host_too_old(version: &str) -> String {
    format!("This Mac runs macOS {version}, and paddock's macOS guest needs macOS 14 or later. Update macOS in System Settings > General > Software Update")
}

/// A Mac with an Intel processor.
pub fn not_apple_silicon() -> String {
    "paddock's macOS guest needs a Mac with Apple silicon, and this Mac has an Intel processor. Desktop apps do not run here; agents still run with paddock run".into()
}

/// `vm_memory` asks for more than the host has.
pub fn memory_too_large(name: &str, asked_gib: u32, host_gib: u32) -> String {
    let max = (host_gib / 2).max(1);
    format!(
        "{name} asks for {asked_gib} GiB of guest memory, and this Mac has {host_gib} GiB. Set vm_memory = \"{max}G\" or less in [apps.{name}] of ~/.config/paddock/config.toml"
    )
}

/// The secrets detector found a file in a shared folder.
pub fn env_file(name: &str, path: &Path) -> String {
    format!(
        "{} looks like a secrets file, and paddock does not share it with the guest. Move it out of the folder, or set unprotected_workspaces = true in [apps.{name}] of ~/.config/paddock/config.toml",
        shown(path)
    )
}

/// A folder that the route never shares.
pub fn share_refused(name: &str, path: &Path, reason: &str) -> String {
    format!(
        "paddock does not share {} with the guest: {reason}. Name a project folder instead, for example paddock app run {name} ~/src/web",
        shown(path)
    )
}

/// `--network host`, which the guest route does not have.
pub fn network_host_refused() -> String {
    "the network mode host is not available in paddock's macOS guest. Leave out --network for the default ask, or use --network egress".into()
}

/// `--network ask` outside the macOS guest route.
pub fn ask_mode_elsewhere() -> String {
    "the network mode ask works only for desktop apps in paddock's macOS guest. Use --network egress, api, allowlist or none here".into()
}

/// The data disk does not mount. `backup` is the newest.
pub fn data_unmountable(name: &str, backup: Option<&str>) -> String {
    match backup {
        Some(b) => format!(
            "the data disk of {name} does not mount. Put the newest backup in its place: paddock app restore {name} {b}"
        ),
        None => format!(
            "the data disk of {name} does not mount, and it has no backup. A new empty data disk, which removes the login: paddock app reset {name} --data"
        ),
    }
}

// ---- Setup --------------------------------------------------------------

/// The question before the download.
pub fn fetch_question(url: &str, version: &str, size: GoldenSize, minutes: u32) -> String {
    format!(
        "paddock downloads macOS {version} from Apple ({url}): {} GB download, {} GB on disk, about {minutes} minutes in all. Download and install? [y/N]",
        size.download_gb, size.disk_gb
    )
}

/// One progress line, printed at each 5 percent. `left` is `None` while
/// the time left is not known yet.
pub fn progress(step: SetupPhase, percent: u8, left: Option<Duration>) -> String {
    let what = match step {
        SetupPhase::Download => "download",
        SetupPhase::Install => "install",
    };
    match left {
        Some(left) => format!(
            "{what} {}%, about {} left",
            percent.min(100),
            left_text(left)
        ),
        None => format!("{what} {}%", percent.min(100)),
    }
}

/// The `Stop` button of the panel was used.
pub fn panel_stopped() -> String {
    "The setup stopped and keeps what is done. Continue it later: paddock macvm setup".into()
}

/// The seal finished.
pub fn golden_ready(version: &str, size: u64) -> String {
    format!(
        "golden ready (macOS {version}, {}). Start an app: paddock app run chatgpt-desktop",
        gib(size)
    )
}

/// The guest account password is not in paddock's keyring.
pub fn keyring_missing() -> String {
    "The password of the guest account is not in paddock's keyring, so paddock cannot configure the guest. Remove the guest and build it again (your profiles stay): paddock macvm remove, then paddock macvm setup".into()
}

// ---- Start lines --------------------------------------------------------

pub fn start_backend(reason: &str) -> String {
    format!("backend   macvm, paddock's macOS guest ({reason})")
}

pub fn start_memory(name: &str, gib: u32) -> String {
    format!("memory    {gib} GiB for the guest (vm_memory in [apps.{name}] changes it)")
}

/// The guest display: the union of the host screens, in points.
pub fn start_display(w: u32, h: u32, scale: u32, mixed: bool) -> String {
    let mut s = format!("display   {w} x {h} points at {scale}x");
    if mixed {
        let _ = write!(
            s,
            ", the screens have two scales, so windows on the other screen are soft"
        );
    }
    s
}

pub fn start_shares(name: &str, list: &[Share<'_>]) -> String {
    if list.is_empty() {
        return format!(
            "shares    0 folders (name folders after the profile: paddock app run {name} ~/src/web)"
        );
    }
    let items: Vec<String> = list
        .iter()
        .map(|s| {
            format!(
                "{} ({})",
                shown(s.path),
                if s.writable { "rw" } else { "ro" }
            )
        })
        .collect();
    format!("shares    {} folder(s): {}", list.len(), items.join(", "))
}

pub fn start_downloads(path: &Path) -> String {
    format!("downloads {}", shown(path))
}

pub fn start_network(mode: &str) -> String {
    let what = match mode {
        "ask" => "paddock asks before the first connection to each new host",
        "egress" => "every host is allowed",
        "api" => "only the API hosts of the profile",
        "allowlist" => "only the hosts of the allowlist",
        "none" => "no connection",
        _ => "",
    };
    if what.is_empty() {
        format!("network   {mode}")
    } else {
        format!("network   {mode}: {what}")
    }
}

/// Cold or warm, with the time that it takes and the reason of a cold one.
pub fn start_kind(kind: StartKind, seconds: u32, reason: Option<&str>) -> String {
    let k = match kind {
        StartKind::Cold => "cold",
        StartKind::Warm => "warm",
    };
    match reason {
        Some(r) => format!("start     {k} start, about {seconds} s ({r})"),
        None => format!("start     {k} start, about {seconds} s"),
    }
}

/// The copy of the app into the guest, once per app version.
pub fn start_copy(app: &str, seconds: u32) -> String {
    format!("copy      then the copy of {app} into the guest, about {seconds} s, once per version")
}

pub fn start_continues(name: &str) -> String {
    format!("the session continues after this terminal closes. Quit it from its Dock menu, or: paddock app stop {name}")
}

// ---- Badges (at most BADGE_MAX characters) ------------------------------

pub fn guest_not_answering() -> String {
    "guest not answering".into()
}

pub fn restarting_guest() -> String {
    "restarting the guest".into()
}

pub fn layout_missing(name: &str) -> String {
    format!(
        "keyboard layout {} not in the guest",
        fit(name, BADGE_MAX - 33)
    )
}

/// The guest input source while it differs from the host's plain layout.
pub fn input_source(name: &str) -> String {
    format!("input: {}", fit(name, BADGE_MAX - 7))
}

pub fn full_screen_refused() -> String {
    "full screen is not available in the guest".into()
}

pub fn disk_warn(free: u64) -> String {
    format!("disk low: {} free", gib(free))
}

pub fn keyboard_layout_not_set() -> String {
    "keyboard layout not set".into()
}

// ---- Session events and lines -------------------------------------------

pub fn app_exited(app: &str, code: i32) -> String {
    format!("{app} exited with code {code}. Open it again from its Dock menu: Open {app} again")
}

/// Three crashes in 60 s: paddock stops the relaunch.
pub fn crash_report(app: &str, path: &Path) -> String {
    format!(
        "{app} crashed 3 times in 60 s, so paddock does not open it again. The crash report of the guest: {}",
        shown(path)
    )
}

pub fn profile_repaired() -> String {
    "the profile was incomplete, paddock rebuilt its system part and kept your data. The start continues with a cold boot".into()
}

pub fn ended_without_save() -> String {
    "the last session ended without a save, so this start is a cold boot. Your data is on the data disk".into()
}

pub fn save_skipped(needed: u64, free: u64) -> String {
    format!(
        "not enough space for the saved state (needs {}, {} free): the next start is a cold boot. Free space, for example with paddock gc",
        gib(needed),
        gib(free)
    )
}

/// The saved state is from another host macOS build.
pub fn state_stale(old: &str, new: &str) -> String {
    format!("the saved state is from macOS {old}, the host runs {new}: cold boot. Nothing to do, the next quit saves again")
}

pub fn copy_failed(app: &str, reason: &str) -> String {
    format!("the copy of {app} into the guest failed: {reason}. The old copy stays, and the next start tries again")
}

pub fn agent_update_failed() -> String {
    "the guest agent did not update, so the old agent stays. Try again: paddock macvm update".into()
}

pub fn guest_restarted() -> String {
    "the guest did not answer for 120 s, so paddock restarted it. The app copy and your data stay"
        .into()
}

// ---- Status -------------------------------------------------------------

pub fn status_label(row: StatusRow) -> &'static str {
    match row {
        StatusRow::Golden => "golden",
        StatusRow::App => "app",
        StatusRow::Guest => "guest",
        StatusRow::Host => "host",
        StatusRow::Data => "data disk",
        StatusRow::Network => "network",
        StatusRow::Shares => "shares",
        StatusRow::Logs => "logs",
    }
}

/// The head line of a running profile.
pub fn status_running(name: &str, pid: u32, up: Duration, warm: Option<Duration>) -> String {
    let m = up.as_secs() / 60;
    let mut s = format!(
        "{name:<STATUS_LABEL_WIDTH$}running (pid {pid}, {}h {}m)",
        m / 60,
        m % 60
    );
    if let Some(w) = warm {
        let _ = write!(s, ", warm start in {:.1} s", w.as_secs_f64());
    }
    s
}

pub fn session_end(end: SessionEnd) -> &'static str {
    match end {
        SessionEnd::QuitWithSave => "quit with save",
        SessionEnd::QuitWithoutSave => "quit without save",
        SessionEnd::Crashed => "crashed",
        SessionEnd::StoppedWithoutSave => "stopped without save",
        SessionEnd::NeverStarted => "never started",
    }
}

/// The head line of a stopped profile: the last end and the next start.
pub fn status_stopped(name: &str, end: SessionEnd, next: StartKind, why: Option<&str>) -> String {
    status_row(
        name,
        &stopped_head(session_end(end), &next_start_text(next, why)),
    )
}

// ---- Reset and backup ---------------------------------------------------

/// The confirmation of `paddock app reset NAME [--data | --all]`.
pub fn reset_confirm(name: &str, tier: ResetTier, size: u64) -> String {
    let removes = tier.removes().join(", ");
    let keeps = match tier.keeps() {
        [] => "nothing".to_string(),
        k => k.join(", "),
    };
    let backup = match tier {
        ResetTier::System => "",
        ResetTier::Data => {
            "\npaddock first makes a backup of the disk pair in backups/pre-reset-<time>/."
        }
        ResetTier::All => {
            "\npaddock first moves the folder to macvm/trash/, where it stays for 7 days."
        }
    };
    format!(
        "paddock app reset {name}{} removes {} of {name}: {removes}.\nIt keeps: {keeps}.{backup}\nRemove it? [y/N]",
        tier.flag(),
        gib(size)
    )
}

pub fn backup_made(path: &Path) -> String {
    format!("backup of the data disk: {}", shown(path))
}

pub fn backup_skipped(needed: u64, free: u64) -> String {
    format!(
        "no backup, because the disk has {} free and the backup needs {}. Free space, for example with paddock gc, to get one next time",
        gib(free),
        gib(needed)
    )
}

pub fn restore_done(from: &Path, old: &Path) -> String {
    format!(
        "the data disk is now {}. The data disk before it: {}",
        shown(from),
        shown(old)
    )
}

pub fn quit_first(name: &str) -> String {
    format!("quit the session first (paddock app stop {name})")
}

// ---- Firewall panel -----------------------------------------------------

pub fn ask_panel(app: &str, host: &str, port: u16, path: &Path) -> String {
    format!(
        "{app} wants to connect to {host} port {port}.\n{}",
        ask_why(path)
    )
}

pub fn ask_button(b: AskButton) -> &'static str {
    match b {
        AskButton::AllowOnce => "Allow once",
        AskButton::AllowSession => "Allow this session",
        AskButton::AlwaysAllow => "Always allow",
        AskButton::Deny => "Deny",
        AskButton::AlwaysDeny => "Always deny",
    }
}

/// The `Why?` line: the process in the guest that connects.
pub fn ask_why(path: &Path) -> String {
    format!(
        "Why? The process {} in the guest opened the connection.",
        shown(path)
    )
}
