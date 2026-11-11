//! `paddock app status NAME`: one screen that says what a session does and
//! what the next start will do, also as JSON.
//!
//! Pure: [`build`] turns the [`Inputs`] (the record of the session, the
//! stamp, the sizes, the counts of the connection log) into a [`Status`],
//! and [`render`] prints it. [`read`] gathers the inputs from the disk.
//! No socket is opened: the shim writes what the screen needs into the
//! record of the session (`health`, `start`, `app_info`, `network`,
//! `menu`).

use super::host::{self, Session};
use super::layout::Layout;
use super::session::{Profile, Stamp, Start};
use super::start;
use super::text::{self, Share, StatusRow};
use crate::env::HostEnv;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

/// The file of the connection log of a session, next to its control socket.
/// The firewall of the session writes it.
pub const CONNECTIONS_FILE: &str = "connections.jsonl";
/// `health` silence after which the guest row says `not answering`
/// (the first event of the failure matrix).
pub const HEALTH_LOST_SECS: u64 = 10;

/// What `build` needs.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub name: String,
    pub profile: Profile,
    /// The record of the session, when its shim lives.
    pub record: Option<Session>,
    pub stamp: Option<Stamp>,
    /// How the next start of a stopped profile runs, or why it cannot.
    pub next: Result<Start, super::session::Refusal>,
    /// The used bytes of the profile folder, the data disk and the state.
    pub profile_bytes: u64,
    pub data_bytes: u64,
    pub state_bytes: u64,
    /// The free bytes of the volume.
    pub disk_free: u64,
    /// The workspace folders: `(path, writable)`.
    pub shares: Vec<(PathBuf, bool)>,
    /// The counts of the connection log of the running session.
    pub connections: Counts,
    /// The firewall questions of the running session (`questions.json`).
    pub asks: super::ask_status::AskStatus,
    /// Seconds since the epoch.
    pub now: u64,
}

/// The counts of a connection log by verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub allowed: u64,
    pub denied: u64,
    /// Every other verdict word with its count, sorted.
    pub other: Vec<(String, u64)>,
    /// `host:port` of the last line.
    pub last: Option<String>,
    /// Every host name of the log, in order of first sight.
    pub hosts: Vec<String>,
}

impl Counts {
    /// Every verdict with its count, `allowed` and `denied` first.
    pub fn by_verdict(&self) -> Vec<(String, u64)> {
        let mut v = Vec::new();
        if self.allowed > 0 || self.denied > 0 || self.other.is_empty() {
            v.push(("allowed".to_string(), self.allowed));
            v.push(("denied".to_string(), self.denied));
        }
        v.extend(self.other.iter().cloned());
        v
    }
}

/// Count the lines of a connection log: one JSON object per line with
/// `verdict` (`allow`, `deny`, `ask`), `host`, `port` and the `count` of
/// a merged line (`connlog.rs`). A line that does not parse is skipped.
/// Nothing but the host, the port, the verdict and the count is read.
pub fn count_connections(bytes: &[u8]) -> Counts {
    let mut c = Counts::default();
    let mut other = std::collections::BTreeMap::<String, u64>::new();
    for line in String::from_utf8_lossy(bytes).lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let verdict = v
            .get("verdict")
            .or_else(|| v.get("decision"))
            .and_then(|x| x.as_str())
            .unwrap_or("other");
        let n = v.get("count").and_then(|n| n.as_u64()).unwrap_or(1);
        match verdict {
            "allowed" | "allow" => c.allowed += n,
            "denied" | "deny" => c.denied += n,
            w => *other.entry(w.to_string()).or_default() += n,
        }
        if let Some(host) = v.get("host").and_then(|h| h.as_str()) {
            if !c.hosts.iter().any(|h| h == host) {
                c.hosts.push(host.to_string());
            }
            c.last = Some(match v.get("port").and_then(|p| p.as_u64()) {
                Some(p) => format!("{host}:{p}"), // not user text
                None => host.to_string(),
            });
        }
    }
    c.other = other.into_iter().collect();
    c
}

/// The state word of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    /// Saved by automatic suspend: the shim waits in the Dock, without a
    /// VM slot.
    Suspended,
    Stopped,
    NeverStarted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppStatus {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copied: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuestStatus {
    pub answering: bool,
    pub health_age_s: u64,
    pub load: f64,
    pub mem_free: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_free: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostStatus {
    pub profile_bytes: u64,
    pub data_bytes: u64,
    pub state_bytes: u64,
    pub disk_free: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkStatus {
    pub mode: String,
    pub allowed: u64,
    pub denied: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShareStatus {
    pub path: PathBuf,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogPaths {
    pub dir: PathBuf,
    pub shim: PathBuf,
    pub events: PathBuf,
    pub console: PathBuf,
    pub agent: PathBuf,
    pub boundary: PathBuf,
}

/// The screen, as the struct that `--json` prints.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub name: String,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_s: Option<u64>,
    /// `warm`, `cold`, `fresh` or `repair`, once the shim knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_kind: Option<String>,
    /// Seconds from the boot to the hello.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_secs: Option<f64>,
    /// How the last session ended, for a stopped profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_end: Option<String>,
    /// What the next start does: `warm` or `cold: <why>`.
    pub next_start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub golden: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<AppStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest: Option<GuestStatus>,
    /// Seconds since the last `health`, for the scripts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_age_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub windows: Option<usize>,
    /// The free space of the data disk in GiB, for the scripts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_free_gib: Option<f64>,
    pub host: HostStatus,
    /// The data disk: prepared, or why not.
    pub data: String,
    pub network: NetworkStatus,
    /// The open firewall questions of a running session, and the guest
    /// system hosts denied without asking.
    pub questions: super::ask_status::AskStatus,
    pub shares: Vec<ShareStatus>,
    pub downloads: PathBuf,
    pub logs: LogPaths,
    /// The menu bar replica of the session, while it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<super::menu::MenuRecord>,
    /// The Accessibility mirror of the session, once an Accessibility
    /// client asked for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax: Option<super::axmirror::AxRecord>,
    /// Why a suspended session was suspended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspend_reason: Option<String>,
}

/// What the next start does, for each kind of [`Start`].
pub fn next_start(s: Start) -> String {
    text::next_start_text(start::kind(s), start::why(s).map(text::cold_why))
}

/// The same, or the refusal of a start that cannot happen.
pub fn next_start_of(s: &Result<Start, super::session::Refusal>) -> String {
    match s {
        Ok(s) => next_start(*s),
        Err(r) => text::next_start_refused(&r.to_string()),
    }
}

/// The status from the inputs. Pure.
pub fn build(i: &Inputs) -> Status {
    let p = &i.profile;
    let logs = LogPaths {
        dir: p.logs(),
        shim: p.shim_log(),
        events: p.events_log(),
        console: p.console_log(),
        agent: p.agent_log(),
        boundary: p.boundary_log(),
    };
    let host = HostStatus {
        profile_bytes: i.profile_bytes,
        data_bytes: i.data_bytes,
        state_bytes: i.state_bytes,
        disk_free: i.disk_free,
    };
    let shares = i
        .shares
        .iter()
        .map(|(path, writable)| ShareStatus {
            path: path.clone(),
            writable: *writable,
        })
        .collect();
    let (golden, agent, protocol) = match &i.stamp {
        Some(s) => (
            Some(s.golden.clone()),
            Some(s.agent.clone()),
            Some(format!("{}.{}", s.protocol.major, s.protocol.minor)), // not user text
        ),
        None => (None, None, None),
    };
    let app_from_stamp = |running: Option<bool>, windows: Option<usize>| {
        i.stamp
            .as_ref()
            .and_then(|s| s.app.as_ref())
            .map(|a| AppStatus {
                name: i
                    .record
                    .as_ref()
                    .and_then(|r| r.app_info.as_ref())
                    .map(|x| x.name.clone())
                    .unwrap_or_else(|| i.name.clone()),
                version: a.version.clone(),
                copied: a.copied.clone(),
                running,
                windows,
            })
    };
    let data = text::status_data_row(
        i.stamp.as_ref().is_some_and(|s| s.data_prepared),
        i.stamp.as_ref().and_then(|s| s.data_error.as_deref()),
    );
    let state = match (&i.record, &i.stamp) {
        (Some(r), _) if r.suspended => State::Suspended,
        (Some(_), _) => State::Running,
        (None, Some(_)) => State::Stopped,
        (None, None) => State::NeverStarted,
    };
    match &i.record {
        Some(r) => {
            let health = r.health.as_ref();
            let guest = health.map(|h| GuestStatus {
                answering: i.now.saturating_sub(h.t) < HEALTH_LOST_SECS,
                health_age_s: i.now.saturating_sub(h.t),
                load: h.load,
                mem_free: h.mem_free,
                data_free: h.data_disk_free,
            });
            let windows = health.map(|h| h.windows);
            let app = match &r.app_info {
                Some(a) => Some(AppStatus {
                    name: a.name.clone(),
                    version: a.version.clone(),
                    copied: i
                        .stamp
                        .as_ref()
                        .and_then(|s| s.app.as_ref())
                        .and_then(|s| s.copied.clone()),
                    running: health.map(|h| h.app_running),
                    windows,
                }),
                None => app_from_stamp(health.map(|h| h.app_running), windows),
            };
            Status {
                name: i.name.clone(),
                state,
                pid: Some(r.pid),
                uptime_s: Some(i.now.saturating_sub(r.started)),
                start_kind: r.start.as_ref().map(|s| s.kind.clone()),
                start_secs: r.start.as_ref().map(|s| s.secs),
                last_end: None,
                next_start: next_start_of(&i.next),
                golden,
                agent,
                protocol,
                app,
                health_age_s: guest.as_ref().map(|g| g.health_age_s),
                windows,
                data_free_gib: guest
                    .as_ref()
                    .and_then(|g| g.data_free)
                    .map(|b| (b as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0),
                guest,
                host,
                data,
                network: NetworkStatus {
                    mode: r.network_mode.clone(),
                    allowed: i.connections.allowed,
                    denied: i.connections.denied,
                    last: i.connections.last.clone(),
                },
                questions: i.asks.clone(),
                shares,
                downloads: p.downloads(),
                logs,
                menu: r.menu.clone(),
                ax: r.ax.clone(),
                suspend_reason: r.suspend_reason.clone(),
            }
        }
        None => Status {
            name: i.name.clone(),
            state,
            pid: None,
            uptime_s: None,
            start_kind: None,
            start_secs: None,
            last_end: (state == State::Stopped)
                .then(|| text::session_end(host::end_of(i.stamp.as_ref())).to_string()),
            next_start: next_start_of(&i.next),
            golden,
            agent,
            protocol,
            app: app_from_stamp(None, None),
            guest: None,
            health_age_s: None,
            windows: None,
            data_free_gib: None,
            host,
            data,
            network: NetworkStatus {
                mode: String::new(),
                allowed: 0,
                denied: 0,
                last: None,
            },
            questions: Default::default(),
            shares,
            downloads: p.downloads(),
            logs,
            menu: None,
            ax: None,
            suspend_reason: None,
        },
    }
}

/// The status screen.
pub fn render(s: &Status) -> String {
    let head = match s.state {
        State::Running => text::status_running(
            &s.name,
            s.pid.unwrap_or(0),
            Duration::from_secs(s.uptime_s.unwrap_or(0)),
            match (s.start_kind.as_deref(), s.start_secs) {
                (Some("warm"), Some(secs)) => Some(Duration::from_secs_f64(secs.max(0.0))),
                _ => None,
            },
        ),
        State::Suspended => text::status_row(&s.name, &text::list_suspended(s.pid.unwrap_or(0))),
        State::Stopped => text::status_row(
            &s.name,
            &text::stopped_head(s.last_end.as_deref().unwrap_or(""), &s.next_start),
        ),
        State::NeverStarted => text::status_never_started(&s.name),
    };
    let golden = match (&s.golden, &s.agent, &s.protocol) {
        (Some(g), Some(a), Some(p)) => text::status_golden_row(g, a, p),
        _ => text::status_golden_unknown(),
    };
    let app = match &s.app {
        Some(a) => text::status_app_row(
            &a.name,
            &a.version,
            a.copied.as_deref(),
            a.running,
            a.windows,
        ),
        None => text::status_app_none(),
    };
    let guest = match (s.state, &s.guest) {
        (State::Running, Some(g)) => {
            text::status_guest_row(g.answering, g.health_age_s, g.load, g.mem_free, g.data_free)
        }
        (State::Running, None) => text::status_guest_booting(),
        _ => text::status_guest_stopped(),
    };
    let host = text::status_host_row(
        s.host.profile_bytes,
        s.host.data_bytes,
        s.host.state_bytes,
        s.host.disk_free,
    );
    let network = match s.state {
        State::Running => text::status_network_row(
            &s.network.mode,
            s.network.allowed,
            s.network.denied,
            s.network.last.as_deref(),
        ),
        _ => text::status_network_stopped(),
    };
    let shares: Vec<Share<'_>> = s
        .shares
        .iter()
        .map(|x| Share {
            path: &x.path,
            writable: x.writable,
        })
        .collect();
    let rows = [
        golden,
        app,
        guest,
        host,
        s.data.clone(),
        network,
        text::status_shares_row(&shares, &s.downloads),
        text::status_logs_row(&s.logs.dir),
    ];
    let mut out = head;
    out.push('\n');
    for (row, value) in StatusRow::ALL.iter().zip(rows.iter()) {
        out.push_str(&text::status_row(text::status_label(*row), value));
        out.push('\n');
    }
    if s.state == State::Running {
        out.push_str(&questions_block(&s.questions));
    }
    out
}

/// The hosts of the guest system that the screen lists at most.
pub const SYSTEM_LINES: usize = 10;

/// The `questions` row of a running session, each open question under it,
/// and the guest system hosts denied most.
pub fn questions_block(q: &super::ask_status::AskStatus) -> String {
    let mut out = text::status_row(
        "questions",
        &text::status_questions_row(q.open.len(), q.paused, q.system_denied()),
    );
    out.push('\n');
    for o in &q.open {
        out.push_str(&text::status_question_line(
            &o.app, &o.host, o.port, o.waiting, o.shown,
        ));
        out.push('\n');
    }
    for h in q.system.iter().take(SYSTEM_LINES) {
        out.push_str(&text::status_system_line(
            &h.host, h.port, &h.process, h.count,
        ));
        out.push('\n');
    }
    out
}

/// The inputs of a profile from the disk. `None` when the name is not a
/// profile name.
pub fn read(env: &HostEnv, name: &str, n: u32) -> Option<Inputs> {
    if !host::valid_name(name) {
        return None;
    }
    let layout = Layout::new(env);
    let home = super::instance::folder(n);
    let profile = layout.profile(name, &home);
    let record = host::running(&layout, name, &home);
    let stamp = Stamp::read(&profile.stamp()).ok();
    let golden = layout
        .golden
        .read_manifest()
        .ok()
        .flatten()
        .map(|m| m.version_text())
        .unwrap_or_default();
    let display = env
        .var("PADDOCK_MACVM_DISPLAY")
        .unwrap_or(host::DEFAULT_DISPLAY)
        .parse()
        .unwrap_or(super::session::Display {
            w: 2560,
            h: 1600,
            scale: 2,
        });
    let shares = match &record {
        Some(r) => r
            .shares
            .iter()
            .map(|s| (s.path.clone(), !s.read_only))
            .collect(),
        None => crate::app::roots(env, name, &[])
            .map(|r| r.into_iter().map(|p| (p, true)).collect())
            .unwrap_or_default(),
    };
    let connections = record
        .as_ref()
        .map(|_| super::connlog::counts(&profile.connections_log()))
        .unwrap_or_default();
    let asks = record
        .as_ref()
        .map(|_| {
            super::ask_status::read(&super::ask_status::path_next_to(&profile.connections_log()))
        })
        .unwrap_or_default();
    Some(Inputs {
        name: super::instance::label(name, n),
        next: start::predict(&profile, &golden, display),
        profile_bytes: super::golden::used_bytes_in(&profile.dir),
        data_bytes: super::golden::used_bytes(&profile.data()),
        state_bytes: super::golden::used_bytes(&profile.state()),
        disk_free: super::session::free_bytes(&profile.dir),
        shares,
        connections,
        asks,
        now: now_secs(),
        record,
        stamp,
        profile,
    })
}

/// `paddock app status NAME [--instance N] [--json]`.
pub fn cmd(env: &HostEnv, name: &str, n: u32, json: bool) -> anyhow::Result<i32> {
    let Some(inputs) = read(env, name, n) else {
        anyhow::bail!(text::bad_app_name(name));
    };
    if !inputs.profile.dir.is_dir() && inputs.record.is_none() {
        println!("{}", text::status_no_profile(name));
        return Ok(1);
    }
    let status = build(&inputs);
    if json {
        let mut v = serde_json::to_value(&status)?;
        super::grants_cli::extend_status(&mut v, &inputs.profile, inputs.record.as_ref());
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        print!("{}", render(&status));
    }
    Ok(0)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
