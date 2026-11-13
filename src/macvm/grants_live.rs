//! The grants of a running session in the shim, pure: the `grant` lines
//! of the control socket, their answer, and the state that they change:
//! the shares of the VM, the open file handles per share that the guest
//! agent counts (`shares.open`), the last use of each share, the window
//! in which the app must open its files again, the clipboard directions
//! and the network mode. `run_grants.rs` (macOS) applies a planned change
//! to the VM and commits it only when the framework took it.
//!
//! The lines, after `grant `:
//!
//! - `share add ro|rw PATH`, `share remove PATH`, `share mode ro|rw PATH`
//! - `network ask|allowlist|none`
//! - `clipboard on|off on|off` (to the app, from the app)
//! - `list`
//! - `rule add JSON` (a [`RuleAdd`]: a rule of this session only, also
//!   one with an end time), `rule remove ID`
//!
//! The answer is `ok JSON` ([`Answer`]) or `error TEXT`. The path comes
//! last and may hold spaces.

use super::grants::{Applies, Clipboard, NETWORK_MODES};
use super::host_record::RecordShare;
use super::shares::{normalize, Share, WS_PREFIX};
use super::text;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Deliver an answer only while its caller can still wait for it.
pub fn deliver_before_deadline(
    reply: &std::sync::mpsc::Sender<Result<String, String>>,
    deadline: std::time::Instant,
    answer: Result<String, String>,
    restore: impl FnOnce(),
) -> bool {
    if std::time::Instant::now() < deadline && reply.send(answer).is_ok() {
        true
    } else {
        restore();
        false
    }
}

/// How long a share shows `reopen-files` after a change while the app
/// held files of it open.
pub const REOPEN_SECS: u64 = 30;
/// How often the shim asks the guest for the open file handles while a
/// workspace folder is shared.
pub const HANDLES_EVERY: Duration = Duration::from_secs(10);
/// The prefix of the control lines.
pub const PREFIX: &str = "grant";

/// One `grant` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    ShareAdd {
        path: PathBuf,
        read_only: bool,
    },
    ShareRemove {
        path: PathBuf,
    },
    ShareMode {
        path: PathBuf,
        read_only: bool,
    },
    Network {
        mode: String,
    },
    Clipboard(Clipboard),
    List,
    ListFresh,
    /// A rule of the running session only: the firewall of the shim
    /// takes it, the rules file does not.
    RuleAdd(RuleAdd),
    /// Remove a rule of the running session by its id.
    RuleRemove {
        id: String,
    },
}

/// The rule of `grant rule add`, with the confirmation of a rule for
/// every host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleAdd {
    pub rule: super::firewall::Rule,
    #[serde(default)]
    pub yes_all_hosts: bool,
}

fn mode_word(read_only: bool) -> &'static str {
    if read_only {
        "ro"
    } else {
        "rw"
    }
}

fn parse_mode(w: &str) -> Result<bool, String> {
    match w {
        "ro" => Ok(true),
        "rw" => Ok(false),
        _ => Err(text::grant_bad_mode(w)),
    }
}

fn parse_switch(w: &str) -> Result<bool, String> {
    match w {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(text::grant_bad_switch(w)),
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

impl Op {
    /// The op of the rest of a line after `grant `.
    pub fn parse(rest: &str) -> Result<Op, String> {
        let bad = || text::grant_bad_line(rest);
        let path_of = |p: &str| {
            let p = Path::new(p);
            if p.is_absolute() {
                Ok(normalize(p))
            } else {
                Err(bad())
            }
        };
        let (head, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        match head {
            "list" if tail.is_empty() => Ok(Op::List),
            "list" if tail == "fresh" => Ok(Op::ListFresh),
            "network" => {
                if NETWORK_MODES.contains(&tail) {
                    Ok(Op::Network {
                        mode: tail.to_string(),
                    })
                } else {
                    Err(text::grant_bad_network(tail))
                }
            }
            "clipboard" => {
                let mut w = tail.split(' ');
                match (w.next(), w.next(), w.next()) {
                    (Some(a), Some(b), None) => Ok(Op::Clipboard(Clipboard {
                        to_app: parse_switch(a)?,
                        from_app: parse_switch(b)?,
                    })),
                    _ => Err(bad()),
                }
            }
            "rule" => {
                let (verb, rest) = tail.split_once(' ').ok_or_else(bad)?;
                match verb {
                    "add" => serde_json::from_str(rest)
                        .map(Op::RuleAdd)
                        .map_err(|_| bad()),
                    "remove" if !rest.is_empty() && !rest.contains(' ') => Ok(Op::RuleRemove {
                        id: rest.to_string(),
                    }),
                    _ => Err(bad()),
                }
            }
            "share" => {
                let (verb, rest) = tail.split_once(' ').ok_or_else(bad)?;
                match verb {
                    "remove" => Ok(Op::ShareRemove {
                        path: path_of(rest)?,
                    }),
                    "add" | "mode" => {
                        let (m, p) = rest.split_once(' ').ok_or_else(bad)?;
                        let read_only = parse_mode(m)?;
                        let path = path_of(p)?;
                        Ok(if verb == "add" {
                            Op::ShareAdd { path, read_only }
                        } else {
                            Op::ShareMode { path, read_only }
                        })
                    }
                    _ => Err(bad()),
                }
            }
            _ => Err(bad()),
        }
    }

    /// The whole control line.
    pub fn line(&self) -> String {
        let rest = match self {
            Op::ShareAdd { path, read_only } => {
                let (m, p) = (mode_word(*read_only), path.display());
                format!("share add {m} {p}") // not user text
            }
            Op::ShareRemove { path } => format!("share remove {}", path.display()), // not user text
            Op::ShareMode { path, read_only } => {
                let (m, p) = (mode_word(*read_only), path.display());
                format!("share mode {m} {p}") // not user text
            }
            Op::Network { mode } => format!("network {mode}"), // not user text
            Op::Clipboard(c) => format!("clipboard {} {}", on_off(c.to_app), on_off(c.from_app)), // not user text
            Op::List => "list".into(),
            Op::ListFresh => "list fresh".into(),
            Op::RuleAdd(r) => {
                let json = serde_json::to_string(r).unwrap_or_default();
                format!("rule add {json}") // not user text
            }
            Op::RuleRemove { id } => format!("rule remove {id}"), // not user text
        };
        format!("{PREFIX} {rest}") // not user text
    }
}

/// One workspace folder in the answer of `list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveShare {
    pub name: String,
    pub path: PathBuf,
    pub read_only: bool,
    pub guest: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_handles: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_use: Option<u64>,
    pub applies: Option<Applies>,
}

/// The answer of a `grant` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub applies: Applies,
    /// The share name of an added folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where the app sees the folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<PathBuf>,
    /// The files that the app held open in the folder at the change, as
    /// last counted. None when the guest has not counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_handles: Option<u32>,
    /// The app held files of the folder open: it must open them again.
    #[serde(default)]
    pub reopen_needed: bool,
    /// The connections that a switch to `none` closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<u64>,
    /// The workspace folders, for `list`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<LiveShare>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<Clipboard>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// The rule that `rule add` or `rule remove` changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<super::firewall::Rule>,
}

impl Answer {
    pub fn live() -> Self {
        Answer {
            applies: Applies::Live,
            name: None,
            guest: None,
            open_handles: None,
            reopen_needed: false,
            closed: None,
            shares: Vec::new(),
            clipboard: None,
            network: None,
            rule: None,
        }
    }
}

/// The last count of the open file handles of the guest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Handles {
    /// Seconds since the epoch.
    pub t: u64,
    pub per_share: BTreeMap<String, u32>,
}

/// The grants state of a running session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    /// The app of the session, for the lines.
    pub app: String,
    /// Every share of the VM, paddock's own included.
    pub shares: Vec<Share>,
    /// The number of the next `ws-<n>`: never one used in this session,
    /// so a removed name does not come back with other files.
    next: usize,
    pub handles: Option<Handles>,
    last_use: BTreeMap<String, u64>,
    reopen_until: BTreeMap<String, u64>,
    pub clipboard: Clipboard,
    pub network: String,
}

/// A change before the VM took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// The state after the change.
    pub live: Live,
    pub answer: Answer,
    /// The new share set of the VM, when the shares change.
    pub vm_shares: Option<Vec<Share>>,
}

impl Live {
    pub fn new(app: &str, shares: Vec<Share>, clipboard: Clipboard, network: &str) -> Self {
        let next = shares
            .iter()
            .filter_map(|s| s.name.strip_prefix(WS_PREFIX)?.parse::<usize>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        Live {
            app: app.to_string(),
            shares,
            next,
            handles: None,
            last_use: BTreeMap::new(),
            reopen_until: BTreeMap::new(),
            clipboard,
            network: network.to_string(),
        }
    }

    fn workspace(&self, path: &Path) -> Option<&Share> {
        let path = normalize(path);
        self.shares
            .iter()
            .find(|s| s.is_workspace() && s.host == path)
    }

    /// The open file handles of a share, from the last count.
    pub fn open_in(&self, name: &str) -> Option<u32> {
        self.handles
            .as_ref()
            .map(|h| h.per_share.get(name).copied().unwrap_or(0))
    }

    /// The status of a share now.
    pub fn applies_of(&self, name: &str, now: u64) -> Applies {
        match self.reopen_until.get(name) {
            Some(until) if *until > now => Applies::ReopenFiles,
            _ => Applies::Live,
        }
    }

    /// Validate a live share against the same protected folders as startup.
    pub fn plan_checked(
        &self,
        op: &Op,
        now: u64,
        home: &Path,
        paddock: &[PathBuf],
    ) -> Result<Planned, String> {
        if let Op::ShareAdd { path, read_only } = op {
            let (path, _) = super::grants::check_folder(&self.app, path, home, home, paddock, true)
                .map_err(|e| format!("{e:#}"))?;
            return self.plan(
                &Op::ShareAdd {
                    path,
                    read_only: *read_only,
                },
                now,
            );
        }
        self.plan(op, now)
    }

    /// The change of `op`, or the line that refuses it. Nothing changes
    /// in `self`: the caller commits `Planned::live`.
    pub fn plan(&self, op: &Op, now: u64) -> Result<Planned, String> {
        let mut live = self.clone();
        let mut answer = Answer::live();
        let mut vm_shares = None;
        match op {
            Op::ShareAdd { path, read_only } => {
                if let Some(s) = self.workspace(path) {
                    return Err(text::grant_already_shared(&self.app, &s.host, s.read_only));
                }
                let name = format!("{WS_PREFIX}{}", live.next); // not user text
                live.next += 1;
                let share = Share {
                    name: name.clone(),
                    host: normalize(path),
                    read_only: *read_only,
                };
                answer.guest = Some(share.guest_path());
                answer.name = Some(name);
                // The workspace folders before paddock's own, as at a start.
                let at = live
                    .shares
                    .iter()
                    .position(|s| !s.is_workspace())
                    .unwrap_or(live.shares.len());
                live.shares.insert(at, share);
                vm_shares = Some(live.shares.clone());
            }
            Op::ShareRemove { path } => {
                let Some(s) = self.workspace(path).cloned() else {
                    return Err(text::grant_not_shared(&self.app, path));
                };
                answer.name = Some(s.name.clone());
                answer.open_handles = self.open_in(&s.name);
                answer.reopen_needed = answer.open_handles.is_some_and(|n| n > 0);
                live.shares.retain(|x| x.name != s.name);
                live.reopen_until.remove(&s.name);
                live.last_use.remove(&s.name);
                vm_shares = Some(live.shares.clone());
            }
            Op::ShareMode { path, read_only } => {
                let Some(s) = self.workspace(path).cloned() else {
                    return Err(text::grant_not_shared(&self.app, path));
                };
                answer.name = Some(s.name.clone());
                answer.guest = Some(s.guest_path());
                answer.open_handles = self.open_in(&s.name);
                if s.read_only != *read_only {
                    answer.reopen_needed = answer.open_handles.is_some_and(|n| n > 0);
                    if let Some(x) = live.shares.iter_mut().find(|x| x.name == s.name) {
                        x.read_only = *read_only;
                    }
                    if answer.reopen_needed {
                        live.reopen_until.insert(s.name.clone(), now + REOPEN_SECS);
                        answer.applies = Applies::ReopenFiles;
                    }
                    vm_shares = Some(live.shares.clone());
                }
            }
            Op::Network { mode } => {
                live.network = mode.clone();
                answer.network = Some(mode.clone());
            }
            Op::Clipboard(c) => {
                live.clipboard = *c;
                answer.clipboard = Some(*c);
            }
            // The firewall takes these: `gate::Firewall::rule_op`.
            Op::RuleAdd(_) | Op::RuleRemove { .. } => {}
            Op::List | Op::ListFresh => {
                answer.shares = self.list(now);
                answer.clipboard = Some(self.clipboard);
                answer.network = Some(self.network.clone());
            }
        }
        Ok(Planned {
            live,
            answer,
            vm_shares,
        })
    }

    /// The workspace folders with their counts.
    pub fn list(&self, now: u64) -> Vec<LiveShare> {
        self.shares
            .iter()
            .filter(|s| s.is_workspace())
            .map(|s| LiveShare {
                name: s.name.clone(),
                path: s.host.clone(),
                read_only: s.read_only,
                guest: s.guest_path(),
                open_handles: self.open_in(&s.name),
                last_use: self.last_use.get(&s.name).copied(),
                applies: Some(self.applies_of(&s.name, now)),
            })
            .collect()
    }

    /// A count of the guest: the open file handles per share name.
    pub fn on_handles(&mut self, per_share: BTreeMap<String, u32>, now: u64) {
        for (name, n) in &per_share {
            if *n > 0 {
                self.last_use.insert(name.clone(), now);
            }
        }
        self.reopen_until.retain(|_, until| *until > now);
        self.handles = Some(Handles { t: now, per_share });
    }

    /// True while a workspace folder is shared: the shim counts handles.
    pub fn has_workspace(&self) -> bool {
        self.shares.iter().any(Share::is_workspace)
    }

    /// The workspace folders for the session record.
    pub fn record(&self, now: u64) -> Vec<RecordShare> {
        self.shares
            .iter()
            .filter(|s| s.is_workspace())
            .map(|s| RecordShare {
                path: s.host.clone(),
                read_only: s.read_only,
                name: s.name.clone(),
                open_handles: self.open_in(&s.name),
                handles_t: self.handles.as_ref().map(|h| h.t),
                last_use: self.last_use.get(&s.name).copied(),
                reopen_until: self.reopen_until.get(&s.name).copied().filter(|u| *u > now),
            })
            .collect()
    }
}

/// The open files of the guest under the share root from the output of
/// `lsof -F n` (one `n<path>` line per open file, `p<pid>` before the
/// files of each process), counted per share name. The agent's own
/// files and lines outside the root are left out.
pub fn count_open(lsof: &str, root: &str, own_pid: u32) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let prefix = format!("{root}/"); // not user text
    let mut skip = false;
    for line in lsof.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            skip = pid.parse::<u32>().ok() == Some(own_pid);
            continue;
        }
        if skip {
            continue;
        }
        let Some(path) = line.strip_prefix('n') else {
            continue;
        };
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        let name = rest.split('/').next().unwrap_or("");
        if !name.is_empty() {
            *out.entry(name.to_string()).or_insert(0) += 1;
        }
    }
    out
}

#[cfg(test)]
#[path = "grants_live_tests.rs"]
mod tests;
