//! Read the facts of the board from the host state. Reading only: the
//! board never changes a run, a mailbox or a lease.
//!
//! - Running runs: the run records (`state::Running::list`).
//! - Ended runs: the mailboxes with an end, or whose paddock is gone. An
//!   ended run stays for `ENDED_TICKET` on its ticket and for
//!   `ENDED_LOOSE` without one.
//! - Waiting: the approval cards and questions of the run, and `ask` mail
//!   that has no answer.
//! - Idle and stuck: the run record of a detached run.

use super::origin;
use super::tickets::{self, TicketFile};
use super::{Board, Facts, Said, Slot, State};
use crate::env::HostEnv;
use crate::mail::{self, Dir, Kind};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// An ended run with a ticket stays on the board this long.
pub const ENDED_TICKET: u64 = 7 * 24 * 3600;
/// An ended run without a ticket stays this long.
pub const ENDED_LOOSE: u64 = 3600;
/// The kv leases of the compile slots, in the user scope: `compile-slot-N`.
pub const SLOT_PREFIX: &str = "compile-slot-";

/// The board and the ticket files that it read (the text view shows the
/// `Status:` line of a ticket without runs).
pub struct Snapshot {
    pub board: Board,
    pub tickets: BTreeMap<PathBuf, Vec<TicketFile>>,
}

pub fn collect(env: &HostEnv, now: u64) -> Snapshot {
    let pending: Vec<crate::broker::Pending> = crate::broker::pending(env)
        .into_iter()
        .filter(|p| p.expires == 0 || p.expires > now)
        .collect();
    let mut repos: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut repo_of = |p: &Path| -> PathBuf {
        repos
            .entry(p.to_path_buf())
            .or_insert_with(|| {
                let r = crate::speckit::repo_of(p);
                std::fs::canonicalize(&r).unwrap_or(r)
            })
            .clone()
    };
    let mut facts = Vec::new();
    let mut seen = BTreeSet::new();
    for r in crate::state::Running::list(env) {
        seen.insert(r.id.clone());
        let mut f = live(env, &r, &pending);
        f.repo = repo_of(&r.project);
        let pick = r
            .parent
            .as_deref()
            .zip(r.spawn_id.as_deref())
            .and_then(|(p, s)| crate::spawn::model::of_child(env, p, s))
            .and_then(|p| p.model);
        f.model = f
            .origin
            .as_ref()
            .and_then(|o| o.model.clone())
            .or(pick)
            .unwrap_or_default();
        facts.push(f);
    }
    for id in mail::list(env) {
        if seen.contains(&id) {
            continue;
        }
        let Some(info) = mail::run_info(env, &id) else {
            continue;
        };
        // A sandbox run: its id starts with its harness. Other mailboxes
        // (turnout jobs, horse sessions) are not runs of a ticket.
        if info.harness.is_empty() || !id.starts_with(&format!("{}-", info.harness)) {
            continue;
        }
        let ended = match mail::end(env, &id) {
            Some(m) => (m.ts, Some(m.text)),
            None if !mail::events::alive(info.pid, info.pid_start) => {
                (last_write(env, &id).max(info.started), None)
            }
            // Its paddock runs, but its record is not there yet.
            None => continue,
        };
        // Skip old runs before their logs are read: --watch reads often.
        let age = now.saturating_sub(ended.0);
        if age > ENDED_TICKET || (age > ENDED_LOOSE && origin::load(env, &id).is_none()) {
            continue;
        }
        let mut f = from_mail(env, &id);
        f.harness = info.harness.clone();
        f.repo = repo_of(&info.project);
        f.started = info.started;
        f.ended = Some(ended.0.max(f.status.as_ref().map_or(0, |s| s.ts)));
        f.end_text = ended.1.filter(|t| !t.trim().is_empty());
        f.ask = None;
        f.model = f
            .origin
            .as_ref()
            .and_then(|o| o.model.clone())
            .unwrap_or_default();
        facts.push(f);
    }
    let mut files: BTreeMap<PathBuf, Vec<TicketFile>> = BTreeMap::new();
    for f in &facts {
        if !files.contains_key(&f.repo) {
            files.insert(f.repo.clone(), tickets::load(env, &f.repo));
        }
    }
    facts.retain(|f| keep(f, &files, now));
    loft_commits(env, &mut facts);
    files.retain(|repo, _| facts.iter().any(|f| &f.repo == repo));
    let mut board = super::build(&facts, &files, pending.len(), slots(env, now));
    // build() is pure: the loft folders are read here.
    for p in &mut board.projects {
        p.loft = facts
            .iter()
            .find(|f| f.repo.display().to_string() == p.path)
            .and_then(|f| super::loft_of(env, &f.repo));
    }
    board.gateways = crate::agentgw::report::all(env);
    Snapshot {
        board,
        tickets: files,
    }
}

/// The loft commits of each run: one read of the history per project.
fn loft_commits(env: &HostEnv, facts: &mut [Facts]) {
    let mut by_repo: HashMap<PathBuf, HashMap<String, (u64, u64)>> = HashMap::new();
    for f in facts.iter_mut() {
        let commits = by_repo.entry(f.repo.clone()).or_insert_with(|| {
            let loft = crate::loft::Loft::of(env, &f.repo);
            crate::loft::git::run_commits(&loft)
        });
        if let Some(&(n, last)) = commits.get(&f.id) {
            f.loft_commits = n;
            f.loft_last = Some(last);
        }
    }
}

/// The facts of a running run: its mailbox, its run record and its cards.
fn live(env: &HostEnv, r: &crate::state::Running, pending: &[crate::broker::Pending]) -> Facts {
    let mut f = from_mail(env, &r.id);
    f.harness = r.harness.clone();
    f.started = r.started;
    f.approvals = r.approvals.clone();
    if let Some(rec) = crate::detach::Record::load(env, &r.id) {
        f.idle_since = rec.idle_since;
        f.stuck_since = rec.stuck.map(|s| s.since);
    }
    f.cards = pending
        .iter()
        .filter(|p| p.sandbox == r.id)
        .map(|p| Said {
            ts: p.connect.as_ref().map_or(p.created, |c| c.asked_at),
            text: if p.summary.is_empty() {
                p.capability.clone()
            } else {
                p.summary.clone()
            },
        })
        .collect();
    f
}

/// The state of one running run by the rules of the board, without the
/// rest of the board: the badge of a pane of `paddock split`. None: the
/// run does not run.
pub fn live_state(env: &HostEnv, id: &str, now: u64) -> Option<State> {
    let r = crate::state::Running::list(env)
        .into_iter()
        .find(|r| r.id == id)?;
    let pending: Vec<crate::broker::Pending> = crate::broker::pending(env)
        .into_iter()
        .filter(|p| p.sandbox == id && (p.expires == 0 || p.expires > now))
        .collect();
    Some(super::rules::run_state(&live(env, &r, &pending)).0)
}

/// An ended run stays for a while: longer when it has a ticket.
pub fn keep(f: &Facts, files: &BTreeMap<PathBuf, Vec<TicketFile>>, now: u64) -> bool {
    let Some(end) = f.ended else { return true };
    let age = now.saturating_sub(end);
    let has_ticket = f.origin.as_ref().is_some_and(|o| {
        files
            .get(&f.repo)
            .is_some_and(|t| tickets::owner(o, t).is_some())
    });
    age <= if has_ticket {
        ENDED_TICKET
    } else {
        ENDED_LOOSE
    }
}

/// The last write into a mailbox, for a run that ended without an end.
fn last_write(env: &HostEnv, id: &str) -> u64 {
    let Ok(d) = mail::dir(env, id) else { return 0 };
    ["up.jsonl", "down.jsonl", "run.json"]
        .iter()
        .filter_map(|f| std::fs::metadata(d.join(f)).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|t| t.as_secs())
        .max()
        .unwrap_or(0)
}

/// The facts that the mailbox and the prompt record hold.
fn from_mail(env: &HostEnv, id: &str) -> Facts {
    let up = mail::read(env, id, Dir::Up);
    let down = mail::read(env, id, Dir::Down);
    let last_of = |k: Kind| {
        up.iter().rev().find(|m| m.kind == k).map(|m| Said {
            ts: m.ts,
            text: m.text.clone(),
        })
    };
    let ask = up
        .iter()
        .filter(|m| m.kind == Kind::Ask)
        .find(|a| !down.iter().any(|d| d.reply_to == Some(a.seq)))
        .map(|m| Said {
            ts: m.ts,
            text: m.text.clone(),
        });
    Facts {
        id: id.to_string(),
        status: last_of(Kind::Status),
        say: last_of(Kind::Say),
        ask,
        unread_mail: !mail::unread_down(env, id).is_empty(),
        origin: origin::load(env, id),
        ..Facts::default()
    }
}

/// The held compile slots. A kv store that paddock cannot read gives none.
fn slots(env: &HostEnv, now: u64) -> Vec<Slot> {
    let leases = match crate::kv::host::user_leases(env, SLOT_PREFIX, now) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("cannot read the compile slots: {e:#}");
            return Vec::new();
        }
    };
    leases
        .into_iter()
        .filter_map(|(name, holder)| {
            let n = name.strip_prefix(SLOT_PREFIX)?.parse().ok()?;
            Some(Slot { slot: n, holder })
        })
        .collect()
}
