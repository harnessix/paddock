//! Claim connect evidence without consuming it.

use super::*;

// Counts and explanatory text may be rewritten; these fields identify the
// connection the in-memory question actually represents.
pub(super) fn same_identity(a: &Pending, b: &Pending) -> bool {
    a.id == b.id
        && a.capability == CONNECT_CAPABILITY
        && b.capability == CONNECT_CAPABILITY
        && a.created == b.created
        && a.sandbox == b.sandbox
        && a.harness == b.harness
        && a.project == b.project
        && match (&a.connect, &b.connect) {
            (Some(a), Some(b)) => {
                a.pid == b.pid
                    && a.profile == b.profile
                    && a.app == b.app
                    && a.process_app == b.process_app
                    && a.parent_app == b.parent_app
                    && a.team == b.team
                    && a.host == b.host
                    && a.port == b.port
                    && a.path == b.path
                    && a.asked_at == b.asked_at
                    && a.notice == b.notice
            }
            _ => false,
        }
}

pub(crate) fn claim_connect(env: &HostEnv, id: &str, withdraw: bool) -> io::Result<Option<String>> {
    claim(env, id, withdraw, None, false)
}

pub(crate) fn claim_connect_exact(
    env: &HostEnv,
    card: &Pending,
    withdraw: bool,
) -> io::Result<Option<String>> {
    claim(env, &card.id, withdraw, Some(card), true)
}

pub(crate) fn claim_connect_question(
    env: &HostEnv,
    card: &Pending,
    withdraw: bool,
) -> io::Result<Option<String>> {
    claim(env, &card.id, withdraw, Some(card), false)
}

/// The store lock could not be taken, so nothing is known about the card
/// or its answer. Every other claim error comes from the card or the
/// answer itself.
#[derive(Debug)]
struct StoreLock(io::Error);

impl std::fmt::Display for StoreLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot lock the approval store: {}", self.0)
    }
}

impl std::error::Error for StoreLock {}

/// True when a claim failed because the store lock could not be taken.
pub(crate) fn is_store_lock_error(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<StoreLock>())
}

fn claim(
    env: &HostEnv,
    id: &str,
    withdraw: bool,
    expected: Option<&Pending>,
    exact: bool,
) -> io::Result<Option<String>> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id).map_err(|e| io::Error::new(e.kind(), StoreLock(e)))?;
    let text = evidence::read_text(dir.join(format!("{id}.json")))?;
    let card: Pending =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if card.id != id
        || card.capability != CONNECT_CAPABILITY
        || expected.is_some_and(|p| {
            if exact {
                p != &card
            } else {
                !same_identity(p, &card)
            }
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "connect card identity changed",
        ));
    }
    let answer = answer_text(&dir, id)?;
    if let Some(text) = &answer {
        if !answer_valid(&card, text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid connect answer retained",
            ));
        }
        if let Some(expected) = expected {
            let selected = serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|v| v.get("request").cloned())
                .and_then(|v| serde_json::from_value::<Pending>(v).ok());
            if selected.as_ref().is_none_or(|p| {
                if exact {
                    p != expected
                } else {
                    !same_identity(p, expected)
                }
            }) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "selection presentation is unknown; retained for review",
                ));
            }
        }
    } else if withdraw {
        close_locked(&dir, id, false)?;
    }
    Ok(answer)
}
