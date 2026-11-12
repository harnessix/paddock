//! Persist selected answers and their application outcomes.

use super::*;

#[cfg(test)]
thread_local! {
    pub(crate) static OUTCOME_TIME: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// The rule word of a selection that closed as a once denial without its
/// effect, because no fence orders it after the last physical wake.
pub(crate) const UNVERIFIED_RULE: &str = "unverified";

fn outcome_time() -> u64 {
    #[cfg(test)]
    if let Some(now) = OUTCOME_TIME.with(|clock| clock.get()) {
        return now;
    }
    state::now()
}

/// Preserve a hook selection and its question before entering rule locks.
/// The answer contains the card too, so interruption between these two
/// writes still leaves the selected word and its context together.
pub(crate) fn record_hook_connect_selection(
    env: &HostEnv,
    card: &Pending,
    selected: ConnectAnswer,
) -> io::Result<()> {
    let card = card.cleaned();
    if !valid_id(&card.id) || card.capability != CONNECT_CAPABILITY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid connect card",
        ));
    }
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, &card.id)?;
    for ext in ["json", "answer", "owner", "done", "cleanup", "retired"] {
        match std::fs::symlink_metadata(dir.join(format!("{}.{ext}", card.id))) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "connect selection id is in use",
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let answer =
        serde_json::json!({ "reply": selected.word(), "source": "approver", "request": &card });
    crate::privfs::write_new(&dir.join(format!("{}.answer", card.id)), answer.to_string())?;
    crate::privfs::write_atomic(
        &dir.join(format!("{}.json", card.id)),
        serde_json::to_vec(&card).map_err(io::Error::other)?,
    )
}

/// Record the local application result, keeping the card and selection as
/// evidence. This is not a delivery or crash-durability acknowledgement.
#[cfg(test)]
pub(crate) fn record_connect_outcome(
    env: &HostEnv,
    id: &str,
    selected: ConnectAnswer,
    allowed: bool,
    word: &str,
    by: &str,
    no_waiter: bool,
) -> io::Result<()> {
    record_connect_outcome_inner(env, id, selected, allowed, word, by, no_waiter, None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_connect_outcome_bound(
    env: &HostEnv,
    id: &str,
    selected: ConnectAnswer,
    allowed: bool,
    word: &str,
    by: &str,
    no_waiter: bool,
    binding: &(Pending, String),
) -> io::Result<()> {
    record_connect_outcome_inner(
        env,
        id,
        selected,
        allowed,
        word,
        by,
        no_waiter,
        Some(binding),
    )
}

#[allow(clippy::too_many_arguments)]
fn record_connect_outcome_inner(
    env: &HostEnv,
    id: &str,
    selected: ConnectAnswer,
    allowed: bool,
    word: &str,
    by: &str,
    no_waiter: bool,
    binding: Option<&(Pending, String)>,
) -> io::Result<()> {
    let state = if word == "stale" {
        LocalApplication::Failed
    } else if word == UNVERIFIED_RULE {
        LocalApplication::Refused
    } else if no_waiter && matches!(selected, ConnectAnswer::AllowOnce | ConnectAnswer::Deny) {
        LocalApplication::NoWaiter
    } else if word == "once" {
        LocalApplication::DecisionPrepared
    } else {
        LocalApplication::Applied
    };
    let scope = if word == "once" || word == UNVERIFIED_RULE {
        ApplicationScope::Connection
    } else if word.starts_with("session-") {
        ApplicationScope::Session
    } else if word.starts_with("rule-") {
        ApplicationScope::File
    } else {
        ApplicationScope::Unknown
    };
    record_local_outcome(
        env,
        id,
        selected,
        allowed,
        word,
        by,
        state,
        scope,
        Vec::new(),
        binding.map(|b| &b.0),
        binding.map(|b| b.1.as_str()),
    )
}

/// The card was selected, but the existing store format cannot establish
/// whether that selection preceded the physical wake.
pub(crate) fn record_wake_selection(
    env: &HostEnv,
    id: &str,
    selected: ConnectAnswer,
    by: &str,
    expected: &Pending,
    expected_answer: &str,
) -> io::Result<()> {
    record_local_outcome(
        env,
        id,
        selected,
        false,
        "stale",
        by,
        LocalApplication::Unknown,
        ApplicationScope::Unknown,
        Vec::new(),
        Some(expected),
        Some(expected_answer),
    )
}

/// The notice is recorded before the queued decisions leave memory. A deny
/// notice cannot assert that those decisions were subsequently settled.
pub(crate) fn record_pause_outcome(
    env: &HostEnv,
    id: &str,
    selected: ConnectAnswer,
    by: &str,
    affected: Vec<AffectedDecision>,
    expected: &Pending,
    expected_answer: &str,
) -> io::Result<()> {
    let state = if selected == ConnectAnswer::Deny {
        LocalApplication::AffectedUnconfirmed
    } else {
        LocalApplication::NoticePending
    };
    record_local_outcome(
        env,
        id,
        selected,
        selected.allows(),
        "pause-notice",
        by,
        state,
        ApplicationScope::Notice,
        affected,
        Some(expected),
        Some(expected_answer),
    )
}

#[allow(clippy::too_many_arguments)]
fn record_local_outcome(
    env: &HostEnv,
    id: &str,
    selected: ConnectAnswer,
    allowed: bool,
    word: &str,
    by: &str,
    state: LocalApplication,
    scope: ApplicationScope,
    affected: Vec<AffectedDecision>,
    expected: Option<&Pending>,
    expected_answer: Option<&str>,
) -> io::Result<()> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let card_text = evidence::read_text(dir.join(format!("{id}.json")))?;
    let card: Pending = serde_json::from_str(&card_text).map_err(io::Error::other)?;
    let answer = answer_text(&dir, id)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "connect selection is missing"))?;
    if card.id != id
        || card.capability != CONNECT_CAPABILITY
        || card
            .connect
            .as_ref()
            .is_none_or(|connect| connect.notice != (scope == ApplicationScope::Notice))
        || !answer_valid(&card, &answer)
        || ConnectAnswer::parse_file(&answer) != Some(selected)
        || expected.is_some_and(|snapshot| snapshot != &card)
        || expected_answer.is_some_and(|snapshot| snapshot != answer)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "connect selection changed",
        ));
    }
    let outcome = StoredConnectOutcome {
        kind: "connect-application-v2".into(),
        local: ConnectOutcome {
            id: id.into(),
            request: card,
            selected: selected.word().into(),
            local_allowed: if matches!(
                state,
                LocalApplication::Failed
                    | LocalApplication::NoWaiter
                    | LocalApplication::AffectedUnconfirmed
                    | LocalApplication::NoticePending
                    | LocalApplication::Unknown
            ) {
                None
            } else {
                Some(allowed)
            },
            rule: word.into(),
            by: by.into(),
            state,
            scope,
            reason: match (state, scope, selected) {
                (LocalApplication::Failed, _, _) => {
                    Some("selection could not be safely applied or revalidated".into())
                }
                (LocalApplication::NoWaiter, _, _) => {
                    Some("no connection was waiting for this once decision".into())
                }
                (LocalApplication::DecisionPrepared, _, _) => Some(
                    "local result prepared; downstream admission and durability are unconfirmed"
                        .into(),
                ),
                (LocalApplication::AffectedUnconfirmed, _, _) => {
                    Some("affected decisions are not confirmed settled".into())
                }
                (LocalApplication::NoticePending, _, _) => {
                    Some("resume has not been recorded as applied".into())
                }
                (LocalApplication::Refused, _, _) => Some(
                    "selection ordering relative to host wake is unknown; the connection was denied once and no rule was applied"
                        .into(),
                ),
                (LocalApplication::Unknown, _, _) => Some(
                    "selection ordering relative to host wake is unknown; no rule was applied"
                        .into(),
                ),
                (
                    LocalApplication::Applied,
                    ApplicationScope::Session,
                    ConnectAnswer::DenyAlways,
                ) => Some("persistent deny failed; session fallback installed".into()),
                _ => None,
            },
            recorded_at: Some(outcome_time()),
            affected,
        },
        selection: answer,
    };
    let path = dir.join(format!("{id}.done"));
    let bytes = serde_json::to_vec(&outcome).map_err(io::Error::other)?;
    if scope == ApplicationScope::Notice {
        match evidence::read(&path) {
            Ok(existing) => {
                // A previous write may have succeeded despite an error. Only
                // the identical selection and target set may be resumed.
                let stored: StoredConnectOutcome =
                    serde_json::from_slice(&existing).map_err(io::Error::other)?;
                if !consistent_local_outcome(&stored) {
                    return Err(io::Error::other("invalid existing pause outcome"));
                }
                let mut old: serde_json::Value =
                    serde_json::from_slice(&existing).map_err(io::Error::other)?;
                let mut new: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                old["recorded_at"] = serde_json::Value::Null;
                new["recorded_at"] = serde_json::Value::Null;
                return if old == new {
                    Ok(())
                } else {
                    Err(io::Error::other(
                        "existing pause outcome does not match selection",
                    ))
                };
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => missing_card(&path)?,
            Err(e) => return Err(e),
        }
    }
    crate::privfs::write_new(&path, bytes)
}

/// A helper calls this only after a local result with no waiter, or after a
/// pause resume is confirmed. The marker matches the complete outcome; an
/// absent acknowledgement leaves the evidence ineligible for cleanup.
pub(crate) fn retire_connect_outcome(env: &HostEnv, id: &str) -> io::Result<()> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let bytes = evidence::read(dir.join(format!("{id}.done")))?;
    let stored: StoredConnectOutcome = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let card: Pending = serde_json::from_slice(&evidence::read(dir.join(format!("{id}.json")))?)
        .map_err(io::Error::other)?;
    let answer = answer_text(&dir, id)?.ok_or_else(|| io::Error::other("selection missing"))?;
    if stored.local.id != id
        || !consistent_local_outcome(&stored)
        || stored.local.request != card
        || stored.selection != answer
        || !matches!(
            stored.local.state,
            LocalApplication::Applied
                | LocalApplication::DecisionPrepared
                | LocalApplication::NoWaiter
                | LocalApplication::Refused
        )
    {
        return Err(io::Error::other("outcome cannot be retired"));
    }
    crate::privfs::write_new(&dir.join(format!("{id}.retired")), bytes)
}

/// Confirm the local resume only after the in-memory queue was resumed.
/// A failed confirmation is retained for review and is not retried.
pub(crate) fn confirm_pause_resume(env: &HostEnv, id: &str) -> io::Result<()> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let path = dir.join(format!("{id}.done"));
    let mut stored: StoredConnectOutcome =
        serde_json::from_slice(&evidence::read(&path)?).map_err(io::Error::other)?;
    let card: Pending = serde_json::from_slice(&evidence::read(dir.join(format!("{id}.json")))?)
        .map_err(io::Error::other)?;
    let answer =
        answer_text(&dir, id)?.ok_or_else(|| io::Error::other("pause selection missing"))?;
    if stored.kind != "connect-application-v2"
        || stored.local.id != id
        || !consistent_local_outcome(&stored)
        || stored.local.state != LocalApplication::NoticePending
        || stored.local.scope != ApplicationScope::Notice
        || stored.local.selected != ConnectAnswer::AllowOnce.word()
        || stored.local.request != card
        || stored.selection != answer
        || !card.connect.as_ref().is_some_and(|connect| connect.notice)
        || ConnectAnswer::parse_file(&answer) != Some(ConnectAnswer::AllowOnce)
        || !answer_valid(&card, &answer)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "pause outcome identity changed",
        ));
    }
    stored.local.state = LocalApplication::Applied;
    stored.local.local_allowed = Some(true);
    stored.local.rule = "pause-resumed".into();
    stored.local.reason = None;
    crate::privfs::write_atomic(
        &path,
        serde_json::to_vec(&stored).map_err(io::Error::other)?,
    )
}

pub(super) fn recorded_connect_outcome(
    dir: &Path,
    id: &str,
    card: &Pending,
    answer: &str,
) -> io::Result<Option<ConnectOutcome>> {
    let text = match evidence::read_text(dir.join(format!("{id}.done"))) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            missing_card(&dir.join(format!("{id}.done")))?;
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(io::Error::other)?;
    let selected = ConnectAnswer::parse_file(answer);
    if value.get("kind").and_then(|v| v.as_str()) == Some("connect-application-v2") {
        let stored: StoredConnectOutcome =
            serde_json::from_value(value).map_err(io::Error::other)?;
        if stored.local.id != id
            || &stored.local.request != card
            || stored.selection != answer
            || Some(stored.local.selected.as_str()) != selected.map(|a| a.word())
            || !consistent_local_outcome(&stored)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "connect outcome identity changed",
            ));
        }
        let mut local = stored.local;
        local.request = local.request.cleaned();
        return Ok(Some(local));
    }
    if value.get("kind").and_then(|v| v.as_str()) != Some("connect-application-v1")
        || value.get("selected").and_then(|v| v.as_str()) != selected.map(|a| a.word())
        || value.get("allowed").and_then(|v| v.as_bool()).is_none()
        || value.get("rule").and_then(|v| v.as_str()).is_none()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid connect outcome",
        ));
    }
    let rule = value["rule"].as_str().unwrap_or_default();
    let state = if rule == "stale" {
        LocalApplication::Failed
    } else if rule == "pause-resumed" && selected == Some(ConnectAnswer::Deny) {
        LocalApplication::AffectedUnconfirmed
    } else {
        // V1 did not bind its result to a card identity or record when it
        // completed. It cannot prove that this is the same selection.
        LocalApplication::Unknown
    };
    Ok(Some(ConnectOutcome {
        id: id.into(),
        request: card.cleaned(),
        selected: selected.map(|a| a.word()).unwrap_or("").into(),
        local_allowed: if matches!(
            state,
            LocalApplication::Failed
                | LocalApplication::AffectedUnconfirmed
                | LocalApplication::Unknown
        ) {
            None
        } else {
            value["allowed"].as_bool()
        },
        rule: rule.into(),
        by: value["by"].as_str().unwrap_or("unknown").into(),
        state,
        scope: ApplicationScope::Unknown,
        reason: Some("legacy outcome lacks an identity snapshot and completion details".into()),
        recorded_at: None,
        affected: Vec::new(),
    }))
}
