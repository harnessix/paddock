//! The failure matrix of a session as code: one [`Failure`] per row,
//! [`respond`] as the table that gives the [`Step`]s of a row, and
//! [`Watch`], the pure fold over time that the shim feeds (health, pong,
//! timeouts, app exits, the wake) and whose steps the shim executes. No
//! framework call here: the tests drive it on Linux.

use super::proto::Request;
use super::session::What;
use super::space::Level;
use super::text;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// `health` silence that sets the badge and sends `ping`.
pub const HEALTH_GAP_BADGE: Duration = Duration::from_secs(10);
/// `health` silence that offers `Restart guest` in the Dock.
pub const HEALTH_GAP_DOCK: Duration = Duration::from_secs(30);
/// `health` silence after which the shim restarts the guest alone.
pub const HEALTH_GAP_RESTART: Duration = Duration::from_secs(120);
/// After a wake, `ping` goes this often until the first `pong`.
pub const WAKE_PING: Duration = Duration::from_secs(2);
/// Timeouts in a row on the control line that count as an agent crash.
pub const TIMEOUT_STRIKES: u32 = 3;
/// App crashes in [`CRASH_WINDOW`] that stop the relaunch.
pub const CRASHES_MAX: u32 = 3;
pub const CRASH_WINDOW: Duration = Duration::from_secs(60);

/// The stage of a `health` silence (row 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// 10 s: the badge and a `ping`.
    Badge,
    /// 30 s: `Restart guest` in the Dock.
    Dock,
    /// 120 s: the shim restarts the guest alone.
    Restart,
}

impl Stage {
    pub fn at(silence: Duration) -> Option<Stage> {
        if silence >= HEALTH_GAP_RESTART {
            Some(Stage::Restart)
        } else if silence >= HEALTH_GAP_DOCK {
            Some(Stage::Dock)
        } else if silence >= HEALTH_GAP_BADGE {
            Some(Stage::Badge)
        } else {
            None
        }
    }
}

/// One row of the failure matrix, in its order.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// The guest kernel panics or hangs: `health` stops.
    GuestHang(Stage),
    /// The guest agent crashes: the control connection closes.
    AgentCrash,
    /// The app crashes in the guest. `crashes` counts this one and the
    /// ones of the last minute.
    AppCrash {
        code: i32,
        report: Option<PathBuf>,
        crashes: u32,
    },
    /// The shim crashed: its record names a dead pid.
    ShimCrash { pid: u32 },
    /// paddock restarts on the host: SIGHUP or a lost parent.
    HostRestart,
    /// The host woke from sleep.
    HostWake { slept: Duration },
    /// A `.partial` or a missing stamp.
    PartialProfile,
    /// The host disk is full, at one of the two levels.
    DiskFull(Level),
    /// The saved state did not restore.
    StateNotRestored { error: String },
    /// No VM slot.
    SlotsTaken { line: String },
    /// A vsock request got no reply. `strikes` counts the ones in a row.
    RequestTimeout { kind: String, strikes: u32 },
    /// The agent refused the hello and could not update.
    HelloRefused { line: String },
    /// The app copy failed half way.
    AppCopyFailed { app: String, reason: String },
    /// The marker process died.
    MarkerDied,
    /// The proxy socket is gone: the shim crash row.
    ProxyGone { pid: u32 },
    /// The golden is missing or unsealed.
    GoldenMissing,
    /// A second start of the same profile.
    SecondStart { pid: u32 },
}

/// The rows of the matrix.
pub const ROWS: usize = 17;

/// A Dock menu entry that a row adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    RestartGuest,
    OpenAgain,
}

/// What the shim does for a row, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Badge(String),
    ClearBadge,
    DockEntry(Entry),
    /// A request to the agent.
    Send(Request),
    /// Stop the VM; `force` skips the guest shutdown.
    StopVm {
        force: bool,
    },
    /// Boot the same profile again with the same arguments.
    ColdBoot,
    Discard(What),
    /// A backup of the pair with this reason.
    Backup(String),
    Line(String),
    Event {
        kind: &'static str,
        text: String,
    },
    CloseWindows,
    /// The idle proxy tunnels of the session.
    CloseTunnels,
    /// Deny the firewall questions from before the sleep once.
    DenyStale,
    /// Close the control connection, so the agent dials again or
    /// launchd restarts it.
    DropAgent,
    /// Whether the Dock relaunches the app on a click.
    Relaunch(bool),
    /// The next quit skips the save.
    SkipSave,
    RestartMarker,
    Exit(i32),
}

/// What the table needs of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ctx {
    pub app: String,
}

/// The failure matrix: the steps of one row.
pub fn respond(f: &Failure, ctx: &Ctx) -> Vec<Step> {
    use Step::*;
    match f {
        Failure::GuestHang(Stage::Badge) => vec![
            Badge(text::guest_not_answering()),
            Send(Request::Ping { t: 0 }),
            Event {
                kind: "health",
                text: text::guest_not_answering(),
            },
        ],
        Failure::GuestHang(Stage::Dock) => vec![
            DockEntry(Entry::RestartGuest),
            Event {
                kind: "health",
                text: text::guest_not_answering(),
            },
        ],
        Failure::GuestHang(Stage::Restart) => vec![
            Line(text::restarting_guest()),
            Badge(text::restarting_guest()),
            Event {
                kind: "guest",
                text: text::guest_restart_event(),
            },
            CloseWindows,
            StopVm { force: true },
            Discard(What::State),
            ColdBoot,
        ],
        Failure::AgentCrash => vec![Event {
            kind: "agent",
            text: text::agent_connection_closed(),
        }],
        Failure::AppCrash {
            code,
            report,
            crashes,
        } => {
            let mut v = vec![
                Line(text::app_exited(&ctx.app, *code)),
                Event {
                    kind: "app",
                    text: text::app_exited(&ctx.app, *code),
                },
                CloseWindows,
            ];
            if *crashes >= CRASHES_MAX {
                let line = text::relaunch_stopped(&ctx.app, report.as_deref());
                v.push(Line(line.clone()));
                v.push(Event {
                    kind: "app",
                    text: line,
                });
                v.push(Relaunch(false));
            } else {
                v.push(DockEntry(Entry::OpenAgain));
                v.push(Relaunch(true));
            }
            v
        }
        Failure::ShimCrash { pid } | Failure::ProxyGone { pid } => vec![
            Event {
                kind: "crashed",
                text: text::session_ended_without_save_event(*pid),
            },
            Line(text::ended_without_save()),
        ],
        Failure::HostRestart => vec![Event {
            kind: "host",
            text: text::host_restarted(),
        }],
        Failure::HostWake { slept } => vec![
            Badge(text::waking()),
            Send(Request::Ping { t: 0 }),
            CloseTunnels,
            DenyStale,
            Event {
                kind: "wake",
                text: text::wake_line(slept.as_secs()),
            },
        ],
        Failure::PartialProfile => vec![
            Backup("pre-repair".into()),
            Discard(What::Partials),
            Discard(What::Stamp),
            Discard(What::State),
            Line(text::profile_repaired()),
        ],
        Failure::DiskFull(Level::Warn2G) => vec![Badge(text::disk_warn(super::space::WARN))],
        Failure::DiskFull(Level::Low1G) => vec![
            Badge(text::disk_warn(super::space::LOW)),
            Send(Request::DiskLow),
            Line(text::disk_low_line(super::space::LOW)),
            Event {
                kind: "disk",
                text: text::disk_low_line(super::space::LOW),
            },
            SkipSave,
        ],
        Failure::StateNotRestored { error } => vec![
            Line(text::restore_failed(error)),
            Event {
                kind: "restore",
                text: text::restore_failed(error),
            },
            Discard(What::State),
            ColdBoot,
        ],
        Failure::SlotsTaken { line } => vec![Line(line.clone()), Exit(super::host::EXIT_REFUSED)],
        Failure::RequestTimeout { kind, strikes } => {
            let line = text::request_timeout(kind, Duration::ZERO);
            let mut v = vec![Event {
                kind: "timeout",
                text: line,
            }];
            if *strikes >= TIMEOUT_STRIKES {
                v.push(Line(text::agent_timeouts(*strikes)));
                v.push(DropAgent);
                v.extend(respond(&Failure::AgentCrash, ctx));
            }
            v
        }
        Failure::HelloRefused { line } => vec![
            Line(line.clone()),
            Event {
                kind: "error",
                text: line.clone(),
            },
            Exit(1),
        ],
        Failure::AppCopyFailed { app, reason } => vec![
            Line(text::copy_failed(app, reason)),
            Event {
                kind: "copy",
                text: text::copy_failed(app, reason),
            },
        ],
        Failure::MarkerDied => vec![RestartMarker],
        Failure::GoldenMissing => vec![
            Line(text::golden_unsealed()),
            Exit(super::host::EXIT_REFUSED),
        ],
        Failure::SecondStart { pid } => vec![
            Line(text::profile_running(&ctx.app, *pid)),
            Exit(super::host::EXIT_REFUSED),
        ],
    }
}

/// The arguments of a boot, as the guest restart repeats them: the
/// restart boots the same profile with the same shares and the same
/// token (row 1). `restart` is the identity, and the test holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootArgs {
    pub profile: PathBuf,
    pub shares: Vec<String>,
    pub token: String,
    pub display: String,
    pub cpus: u32,
    pub memory_mib: u64,
}

/// The arguments of the cold boot after a guest restart.
pub fn restart(original: &BootArgs) -> BootArgs {
    original.clone()
}

/// The fold over time: the shim feeds what it sees, executes the steps.
#[derive(Debug)]
pub struct Watch {
    ctx: Ctx,
    last_health: Option<Instant>,
    stage: Option<Stage>,
    strikes: u32,
    crashes: Vec<Instant>,
    /// The last `ping` of a wake, until the first `pong` or `health`.
    waking: Option<Instant>,
}

impl Watch {
    pub fn new(app: &str) -> Self {
        Self {
            ctx: Ctx { app: app.into() },
            last_health: None,
            stage: None,
            strikes: 0,
            crashes: Vec::new(),
            waking: None,
        }
    }

    pub fn ctx(&self) -> &Ctx {
        &self.ctx
    }

    pub fn stage(&self) -> Option<Stage> {
        self.stage
    }

    pub fn strikes(&self) -> u32 {
        self.strikes
    }

    pub fn waking(&self) -> bool {
        self.waking.is_some()
    }

    /// Whether the Dock relaunches the app: under three crashes a minute.
    pub fn relaunch_allowed(&self, now: Instant) -> bool {
        self.crashes_in_window(now) < CRASHES_MAX
    }

    fn crashes_in_window(&self, now: Instant) -> u32 {
        self.crashes
            .iter()
            .filter(|t| now.duration_since(**t) <= CRASH_WINDOW)
            .count() as u32
    }

    /// A `health` or a `pong`: the guest answers. Every silence state
    /// goes, with the badge.
    pub fn alive(&mut self, now: Instant) -> Vec<Step> {
        self.last_health = Some(now);
        self.strikes = 0;
        let mut v = Vec::new();
        if self.stage.take().is_some() || self.waking.take().is_some() {
            v.push(Step::ClearBadge);
            v.push(Step::Event {
                kind: "health",
                text: text::health_back(),
            });
        }
        v
    }

    /// The agent connection is up again after a restart: the counters
    /// of the old connection do not carry over.
    pub fn connected(&mut self, now: Instant) {
        self.last_health = Some(now);
        self.strikes = 0;
    }

    /// The periodic tick: the health gap stages, the wake pings.
    pub fn tick(&mut self, now: Instant) -> Vec<Step> {
        let mut v = Vec::new();
        if let Some(at) = self.waking {
            if now.duration_since(at) >= WAKE_PING {
                self.waking = Some(now);
                v.push(Step::Send(Request::Ping { t: 0 }));
            }
        }
        if let Some(last) = self.last_health {
            let next = Stage::at(now.duration_since(last));
            if next > self.stage {
                // Each stage once, in order, even when a tick skips one.
                let from = self.stage;
                for s in [Stage::Badge, Stage::Dock, Stage::Restart] {
                    if Some(s) > from && Some(s) <= next {
                        self.stage = Some(s);
                        v.extend(respond(&Failure::GuestHang(s), &self.ctx));
                    }
                }
                if next == Some(Stage::Restart) {
                    // The cold boot starts the clock again.
                    self.last_health = None;
                    self.stage = None;
                }
            }
        }
        v
    }

    /// A request on the control line got no reply.
    pub fn timeout(&mut self, kind: &str) -> Vec<Step> {
        self.strikes += 1;
        let v = respond(
            &Failure::RequestTimeout {
                kind: kind.into(),
                strikes: self.strikes,
            },
            &self.ctx,
        );
        if self.strikes >= TIMEOUT_STRIKES {
            self.strikes = 0;
            self.last_health = None;
            self.stage = None;
        }
        v
    }

    /// The control connection closed.
    pub fn agent_gone(&mut self) -> Vec<Step> {
        self.strikes = 0;
        respond(&Failure::AgentCrash, &self.ctx)
    }

    /// `app_exited` outside the quit path.
    pub fn app_exited(&mut self, code: i32, report: Option<PathBuf>, now: Instant) -> Vec<Step> {
        self.crashes
            .retain(|t| now.duration_since(*t) <= CRASH_WINDOW);
        if code != 0 {
            self.crashes.push(now);
        }
        respond(
            &Failure::AppCrash {
                code,
                report,
                crashes: self.crashes_in_window(now),
            },
            &self.ctx,
        )
    }

    /// The VM runs again after `paddock thaw`: the silence of the pause is
    /// paddock's, so the health clock and the strikes start again.
    pub fn thawed(&mut self, now: Instant) {
        self.last_health = Some(now);
        self.stage = None;
        self.strikes = 0;
    }

    /// `NSWorkspaceDidWakeNotification`.
    pub fn woke(&mut self, slept: Duration, now: Instant) -> Vec<Step> {
        self.waking = Some(now);
        // The silence of the sleep is not the guest's.
        self.last_health = Some(now);
        self.stage = None;
        respond(&Failure::HostWake { slept }, &self.ctx)
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
