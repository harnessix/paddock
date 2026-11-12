//! Inspect recorded outcomes and unresolved approvals.

use super::*;

// A valid JSON object alone is not proof of a completed local application.
pub(super) fn consistent_local_outcome(stored: &StoredConnectOutcome) -> bool {
    let local = &stored.local;
    let card = &local.request;
    let Some(connect) = &card.connect else {
        return false;
    };
    let Some(selected) = ConnectAnswer::parse_file(&stored.selection) else {
        return false;
    };
    if stored.kind != "connect-application-v2"
        || card.id != local.id
        || card.capability != CONNECT_CAPABILITY
        || local.recorded_at.is_none()
        || local.selected != selected.word()
        || !answer_valid(card, &stored.selection)
    {
        return false;
    }
    let once = matches!(selected, ConnectAnswer::AllowOnce | ConnectAnswer::Deny);
    match local.state {
        LocalApplication::DecisionPrepared => {
            !connect.notice
                && once
                && local.scope == ApplicationScope::Connection
                && local.rule == "once"
                && local.local_allowed == Some(selected.allows())
        }
        LocalApplication::NoWaiter => {
            !connect.notice
                && once
                && local.scope == ApplicationScope::Connection
                && local.rule == "once"
                && local.local_allowed.is_none()
        }
        LocalApplication::Failed => {
            !connect.notice && local.rule == "stale" && local.local_allowed.is_none()
        }
        LocalApplication::NoticePending => {
            connect.notice
                && selected == ConnectAnswer::AllowOnce
                && local.scope == ApplicationScope::Notice
                && local.rule == "pause-notice"
                && local.local_allowed.is_none()
        }
        LocalApplication::AffectedUnconfirmed => {
            connect.notice
                && selected == ConnectAnswer::Deny
                && local.scope == ApplicationScope::Notice
                && local.rule == "pause-notice"
                && local.local_allowed.is_none()
        }
        LocalApplication::Applied if connect.notice => {
            selected == ConnectAnswer::AllowOnce
                && local.scope == ApplicationScope::Notice
                && local.rule == "pause-resumed"
                && local.local_allowed == Some(true)
        }
        LocalApplication::Applied => {
            !once
                && local.local_allowed == Some(selected.allows())
                && match local.scope {
                    ApplicationScope::Session => local.rule.starts_with("session-"),
                    ApplicationScope::File => local.rule.starts_with("rule-"),
                    _ => false,
                }
        }
        LocalApplication::Unknown => local.local_allowed.is_none(),
        LocalApplication::Refused => {
            !connect.notice
                && local.scope == ApplicationScope::Connection
                && local.rule == UNVERIFIED_RULE
                && local.local_allowed == Some(false)
        }
    }
}

pub(super) const COMPLETED_KEEP: usize = 256;
const COMPLETED_MAX_AGE: u64 = 7 * 24 * 60 * 60;

// Large or unreadable files are evidence, not candidates for deletion. The
// limit also bounds bytes read and parsed in any one store critical section.
#[cfg(test)]
thread_local! {
    pub(crate) static RETENTION_SCAN_HOOK: std::cell::RefCell<Option<Box<dyn Fn()>>> = std::cell::RefCell::new(None);
}

fn small_regular(path: &Path) -> Option<Vec<u8>> {
    evidence::read(path).ok()
}

// Caller holds the store lock. All eligibility checks precede ranking.
fn eligible(dir: &Path, id: &str, now: u64) -> Option<(u64, Vec<u8>)> {
    if !valid_id(id) {
        return None;
    }
    for ext in ["owner", "cleanup"] {
        if !matches!(std::fs::symlink_metadata(dir.join(format!("{id}.{ext}"))),
            Err(e) if e.kind() == io::ErrorKind::NotFound)
        {
            return None;
        }
    }
    let bytes = small_regular(&dir.join(format!("{id}.done")))?;
    if small_regular(&dir.join(format!("{id}.retired")))? != bytes {
        return None;
    }
    let stored: StoredConnectOutcome = serde_json::from_slice(&bytes).ok()?;
    let at = stored.local.recorded_at?;
    if stored.local.id != id
        || at > now
        || !consistent_local_outcome(&stored)
        || !matches!(
            stored.local.state,
            LocalApplication::Applied
                | LocalApplication::DecisionPrepared
                | LocalApplication::NoWaiter
                | LocalApplication::Refused
        )
    {
        return None;
    }
    let card: Pending =
        serde_json::from_slice(&small_regular(&dir.join(format!("{id}.json")))?).ok()?;
    let answer = small_regular(&dir.join(format!("{id}.answer")))?;
    if card != stored.local.request || answer.as_slice() != stored.selection.as_bytes() {
        return None;
    }
    Some((at, bytes))
}

/// Scan without the global store lock, then validate and remove one identity
/// at a time under that lock. A separate nonblocking maintenance lock prevents
/// two collectors from ranking the same snapshot and deleting each other's
/// retained candidates. Unknown evidence is never made eligible by the cap.
pub(in super::super) fn cleanup_completed(dir: &Path, now: u64, keep: usize) -> io::Result<()> {
    let maintenance = crate::privfs::append(&dir.join(".retention-lock"))?;
    if unsafe { libc::flock(maintenance.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        #[cfg(test)]
        RETENTION_SCAN_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow().as_ref() {
                hook();
            }
        });
        let path = entry?.path();
        if !path.extension().is_some_and(|ext| ext == "retired") {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| valid_id(s))
        else {
            continue;
        };
        let _lock = lock_approvals(dir, id)?;
        if let Some((at, bytes)) = eligible(dir, id, now) {
            candidates.push((at, id.to_string(), bytes));
        }
    }
    candidates.sort_by(|a, b| (b.0, &b.1).cmp(&(a.0, &a.1)));
    for (index, (at, id, bytes)) in candidates.into_iter().enumerate() {
        if index < keep && now.saturating_sub(at) <= COMPLETED_MAX_AGE {
            continue;
        }
        let _lock = lock_approvals(dir, &id)?;
        if eligible(dir, &id, now) != Some((at, bytes.clone())) {
            continue;
        }
        let intent = dir.join(format!("{id}.cleanup"));
        // The intent contains the complete selected answer and result, and
        // remains in place if any unlink is interrupted.
        crate::privfs::write_new(&intent, &bytes)?;
        for ext in ["json", "answer", "done", "retired"] {
            remove_if_present(&dir.join(format!("{id}.{ext}")))?;
        }
        remove_if_present(&intent)?;
    }
    Ok(())
}

// Inspect retained evidence without replaying it. Invalid or unreadable
// pieces stay unknown; an answer-only hook file can carry its own request.
fn unresolved_with_evidence(dir: &Path, id: &str, reason: String) -> ApprovalUnresolved {
    let mut item = ApprovalUnresolved {
        id: id.into(),
        reason,
        selected: None,
        request: None,
    };
    let Ok(_lock) = lock_approvals(dir, id) else {
        return item;
    };
    let mut answer = answer_text(dir, id).ok().flatten();
    let mut request = evidence::read(dir.join(format!("{id}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Pending>(&bytes).ok());
    if request.is_none() {
        request = answer
            .as_deref()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
            .and_then(|value| value.get("request").cloned())
            .and_then(|value| serde_json::from_value::<Pending>(value).ok());
    }
    if request.is_none() || answer.is_none() {
        if let Some(stored) = evidence::read(dir.join(format!("{id}.cleanup")))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StoredConnectOutcome>(&bytes).ok())
        {
            if stored.kind == "connect-application-v2"
                && stored.local.id == id
                && request
                    .as_ref()
                    .is_none_or(|card| card == &stored.local.request)
                && answer
                    .as_deref()
                    .is_none_or(|text| text == stored.selection.as_str())
            {
                if request.is_none() {
                    request = Some(stored.local.request);
                }
                if answer.is_none() {
                    answer = Some(stored.selection);
                }
            }
        }
    }
    if let Some(card) =
        request.filter(|card| card.id == id && card.capability == CONNECT_CAPABILITY)
    {
        if let Some(text) = answer.as_deref().filter(|text| answer_valid(&card, text)) {
            item.selected = ConnectAnswer::parse_file(text).map(|answer| answer.word().to_string());
        }
        item.request = Some(card.cleaned());
    }
    item
}

pub fn pending_report(env: &HostEnv) -> io::Result<PendingReport> {
    let dir = approvals_dir(env);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            missing_card(&dir)?;
            return Ok(PendingReport {
                pending: Vec::new(),
                diagnostics: Vec::new(),
                outcomes: Vec::new(),
                unresolved: Vec::new(),
            });
        }
        Err(e) => return Err(e),
    };
    let mut ids = std::collections::BTreeSet::new();
    let mut report = PendingReport {
        pending: Vec::new(),
        diagnostics: Vec::new(),
        outcomes: Vec::new(),
        unresolved: Vec::new(),
    };
    if let Err(e) = cleanup_completed(&dir, state::now(), COMPLETED_KEEP) {
        report
            .diagnostics
            .push(format!("completed approval cleanup incomplete: {e}"));
    }
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                report
                    .diagnostics
                    .push(format!("cannot inspect approval directory entry: {e}"));
                continue;
            }
        };
        let path = entry.path();
        if !path.extension().is_some_and(|ext| {
            ext == "json"
                || ext == "owner"
                || ext == "answer"
                || ext == "done"
                || ext == "cleanup"
                || ext == "retired"
        }) {
            continue;
        }
        match path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|id| valid_id(id))
        {
            Some(id) => {
                ids.insert(id.to_string());
            }
            None => report
                .diagnostics
                .push(format!("invalid approval sidecar {}", path.display())),
        }
    }
    for id in ids {
        let inspected = (|| -> io::Result<Option<Pending>> {
            let _lock = lock_approvals(&dir, &id)?;
            match std::fs::symlink_metadata(dir.join(format!("{id}.cleanup"))) {
                Ok(_) => return Err(io::Error::other("cleanup interrupted; retained for review")),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let path = dir.join(format!("{id}.json"));
            let card = match evidence::read_text(&path) {
                Ok(text) => {
                    let card: Pending = serde_json::from_str(&text)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    if card.id != id {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "card identity does not match its filename",
                        ));
                    }
                    Some(card)
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    missing_card(&path)?;
                    None
                }
                Err(e) => return Err(e),
            };
            let owner = owner_state(&dir, &id)?;
            if owner == OwnerState::Unknown {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "owner liveness is unknown; retained for recovery",
                ));
            }
            let answer = answer_text(&dir, &id)?;
            if let (Some(card), Some(text)) = (&card, &answer) {
                if !answer_valid(card, text) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid answer sidecar; retained for recovery",
                    ));
                }
                if card.capability == CONNECT_CAPABILITY {
                    if let Some(outcome) = recorded_connect_outcome(&dir, &id, card, text)? {
                        let future_time = outcome.recorded_at.is_some_and(|at| at > state::now());
                        let retirement_missing = matches!(
                            outcome.state,
                            LocalApplication::Applied
                                | LocalApplication::DecisionPrepared
                                | LocalApplication::NoWaiter
                                | LocalApplication::Refused
                        ) && match (
                            small_regular(&dir.join(format!("{id}.done"))),
                            small_regular(&dir.join(format!("{id}.retired"))),
                        ) {
                            (Some(done), Some(retired)) => done != retired,
                            _ => true,
                        };
                        if future_time
                            || retirement_missing
                            || matches!(
                                outcome.state,
                                LocalApplication::Failed
                                    | LocalApplication::AffectedUnconfirmed
                                    | LocalApplication::NoticePending
                                    | LocalApplication::Unknown
                            )
                        {
                            let reason = if future_time {
                                "result time is in the future; retained for review".into()
                            } else if retirement_missing {
                                "local result retirement is unconfirmed; retained for review".into()
                            } else {
                                format!(
                                    "local application {:?}; retained for review",
                                    outcome.state
                                )
                            };
                            report.diagnostics.push(format!("approval {id}: {reason}"));
                            report.unresolved.push(ApprovalUnresolved {
                                id: id.clone(),
                                reason,
                                selected: Some(outcome.selected.clone()),
                                request: Some(outcome.request.cleaned()),
                            });
                        }
                        report.outcomes.push(outcome);
                        return Ok(None);
                    }
                }
            }
            if owner == OwnerState::Dead {
                recover_dead_locked(&dir, &id)?;
            }
            if answer.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "answer retained; delivery and completion are unconfirmed",
                ));
            }
            if owner == OwnerState::Dead {
                return Ok(None);
            }
            if card.is_none() {
                for ext in ["done", "retired"] {
                    match std::fs::symlink_metadata(dir.join(format!("{id}.{ext}"))) {
                        Ok(_) => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "orphaned outcome marker; completion is unconfirmed",
                            ));
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(card)
        })();
        let card = match inspected {
            Ok(Some(card)) => card,
            Ok(None) => continue,
            Err(e) => {
                report.diagnostics.push(format!("approval {id}: {e}"));
                report
                    .unresolved
                    .push(unresolved_with_evidence(&dir, &id, e.to_string()));
                continue;
            }
        };
        let candidate = (card.capability == ASK_CAPABILITY
            && state::now().saturating_sub(card.created) > 60)
            || card.connect.as_ref().is_some_and(|c| !helper_alive(c.pid));
        if candidate {
            match reap_stale(env, &card) {
                Ok(true) => {
                    // An answer can arrive between the inspection and the
                    // stale-card check. Recovery keeps it, not its outcome.
                    let answer = lock_approvals(&dir, &id).and_then(|_lock| answer_text(&dir, &id));
                    match answer {
                        Ok(Some(_)) => {
                            let reason = "answer retained; delivery and completion are unconfirmed"
                                .to_string();
                            report.diagnostics.push(format!("approval {id}: {reason}"));
                            report
                                .unresolved
                                .push(unresolved_with_evidence(&dir, &id, reason));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            let reason = format!("cannot inspect retained answer: {e}");
                            report.diagnostics.push(format!("approval {id}: {reason}"));
                            report
                                .unresolved
                                .push(unresolved_with_evidence(&dir, &id, reason));
                        }
                    }
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    let reason = format!("cannot establish liveness: {e}");
                    report.diagnostics.push(format!("approval {id}: {reason}"));
                    report
                        .unresolved
                        .push(unresolved_with_evidence(&dir, &id, reason));
                    continue;
                }
            }
        }
        report.pending.push(card);
    }
    report
        .pending
        .sort_by_key(|p| (p.capability == ASK_CAPABILITY, p.created));
    Ok(report)
}
