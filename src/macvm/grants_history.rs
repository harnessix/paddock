//! Best-effort rule history is separate from the version-1 policy. A
//! checkpoint's original policy bytes are retained as unresolved evidence;
//! matching event bodies cannot establish whether occurrences are the same.

use super::text;
use super::{Applies, Change, Grants, What, CHANGES_KEPT};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(super) const FILE: &str = "grants-history.json";
const RECOVERY_PREFIX: &str = "grants-history-recovery-";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryState {
    #[default]
    Missing,
    Available,
    Damaged,
    /// Recovery snapshots exist, or a checkpoint still has history in policy.
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct History {
    v: u32,
    changes: Vec<Change>,
}

#[derive(Debug, Clone)]
pub(super) enum Read {
    Missing,
    Available(Vec<Change>),
    Damaged,
}

/// Recognize both earlier checkpoint encodings, including the marker whose
/// extra field an older writer may have removed.
pub(super) fn normalize(change: &mut Change) {
    if change.current_state.as_deref() == Some("unknown")
        || (change.what == What::Rule && change.value.ends_with(text::HISTORICAL_MARK))
    {
        change.applies = Applies::Unknown;
    }
    if change.applies == Applies::Unknown {
        change.current_state = Some("unknown".into());
    }
}

pub(super) fn needs_migration(g: &Grants) -> bool {
    g.changes.iter().any(|change| {
        let mut change = change.clone();
        normalize(&mut change);
        change.applies == Applies::Unknown
    })
}

pub(super) fn decode(bytes: Option<&[u8]>) -> Read {
    let Some(bytes) = bytes else {
        return Read::Missing;
    };
    let Ok(mut h) = serde_json::from_slice::<History>(bytes) else {
        return Read::Damaged;
    };
    if h.v != 1 {
        return Read::Damaged;
    }
    for change in &mut h.changes {
        normalize(change);
        if change.applies != Applies::Unknown {
            return Read::Damaged;
        }
    }
    Read::Available(h.changes)
}

pub(super) fn read(dir: &Path) -> Read {
    match std::fs::read(dir.join(FILE)) {
        Ok(bytes) => decode(Some(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Damaged,
    }
}

/// Recovery snapshots are intentionally opaque: they may overlap each
/// other or the sidecar. Never infer an occurrence count from their bodies.
pub(super) fn has_recovery(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(entries) => entries.into_iter().any(|entry| match entry {
            Ok(entry) => entry
                .file_name()
                .to_string_lossy()
                .starts_with(RECOVERY_PREFIX),
            Err(_) => true,
        }),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

pub(super) fn split(g: &mut Grants, read: &Read, policy: Option<&[u8]>, recovery: bool) {
    let mut checkpoint = Vec::new();
    g.changes = std::mem::take(&mut g.changes)
        .into_iter()
        .filter_map(|mut change| {
            normalize(&mut change);
            if change.applies == Applies::Unknown {
                checkpoint.push(change);
                None
            } else {
                Some(change)
            }
        })
        .collect();
    g.legacy_history_in_policy = !checkpoint.is_empty();
    g.legacy_policy = if g.legacy_history_in_policy {
        policy.map(|bytes| bytes.to_vec())
    } else {
        None
    };
    g.history_state = match read {
        Read::Missing => HistoryState::Missing,
        Read::Damaged => HistoryState::Damaged,
        Read::Available(changes) => {
            g.history_changes = changes.clone();
            HistoryState::Available
        }
    };
    if recovery || g.legacy_history_in_policy {
        g.history_state = HistoryState::Unresolved;
    }
    g.history_changes.extend(checkpoint);
}

/// Queue a new history occurrence without making it part of policy.
pub(super) fn record(g: &mut Grants, mut change: Change) {
    normalize(&mut change);
    if change.applies == Applies::Unknown {
        g.pending_history.push(change);
    } else {
        g.changes.push(change);
    }
}

pub(super) struct Plan {
    pub policy: Vec<u8>,
    pub sidecar: Option<Vec<u8>>,
    pub recovery: Option<Vec<u8>>,
    pub skipped_history: Option<&'static str>,
}

/// A policy update does not rewrite the sidecar. A checkpoint is archived
/// verbatim, independently of the sidecar and without comparing event bodies.
pub(super) fn plan(g: &Grants, read: &Read) -> Result<Plan> {
    if needs_migration(g) || (g.legacy_history_in_policy && g.legacy_policy.is_none()) {
        bail!(text::grant_checkpoint_unpreservable());
    }
    let mut policy = g.clone();
    policy.v = super::VERSION;
    let start = policy.changes.len().saturating_sub(CHANGES_KEPT);
    policy.changes.drain(..start);
    let (sidecar, skipped_history) = if g.pending_history.is_empty() {
        (None, None)
    } else {
        let existing = match read {
            Read::Missing => Some(Vec::new()),
            Read::Available(changes) => Some(changes.clone()),
            Read::Damaged => None,
        };
        match existing {
            None => (
                None,
                Some("grant history is unreadable; it was not replaced"),
            ),
            Some(changes)
                if g.history_state == HistoryState::Unresolved
                    && changes.len().saturating_add(g.pending_history.len()) > CHANGES_KEPT =>
            {
                (
                    None,
                    Some("unresolved grant history is full; it was not trimmed"),
                )
            }
            Some(mut changes) => {
                changes.extend(g.pending_history.iter().cloned());
                let start = changes.len().saturating_sub(CHANGES_KEPT);
                changes.drain(..start);
                (
                    Some(serde_json::to_vec_pretty(&History { v: 1, changes })?),
                    None,
                )
            }
        }
    };
    Ok(Plan {
        policy: serde_json::to_vec_pretty(&policy)?,
        sidecar,
        recovery: g.legacy_policy.clone(),
        skipped_history,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Recovery,
    Sidecar,
    Policy,
}

/// The recovery write must sync its file and directory before policy. A
/// sidecar failure is reported separately after the policy write succeeds.
pub(super) fn execute(
    plan: &Plan,
    mut write: impl FnMut(Step, &[u8]) -> Result<()>,
) -> Result<Option<String>> {
    if let Some(bytes) = &plan.recovery {
        write(Step::Recovery, bytes)?;
    }
    write(Step::Policy, &plan.policy)?;
    let failed = plan.sidecar.as_ref().and_then(|bytes| {
        write(Step::Sidecar, bytes)
            .err()
            .map(|e| text::grant_history_not_saved(&e))
    });
    Ok(failed.or_else(|| plan.skipped_history.map(str::to_owned)))
}

/// Create an immutable snapshot before changing the policy. A failed write or
/// sync leaves the original policy in place; retries may leave extra snapshots.
pub(super) fn write_recovery(
    dir: &Path,
    bytes: &[u8],
    sync_dir: &mut impl FnMut(&Path) -> std::io::Result<()>,
) -> Result<()> {
    for n in 0..u64::MAX {
        let path = dir.join(format!("{RECOVERY_PREFIX}{n}.json"));
        match crate::privfs::write_new(&path, bytes) {
            Ok(()) => {
                std::fs::File::open(&path)?.sync_all()?;
                sync_dir(dir)?;
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| text::grant_history_writing(&path)),
        }
    }
    bail!(text::grant_history_no_recovery_name())
}
