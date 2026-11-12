//! The second read of the grants of a start, under the lock of the grants
//! commands, held until the session record exists.

use super::{load, missing_after_save, text, unavailable_policy, Grants, Lock, Profile, LOCK};
use anyhow::{bail, Result};

/// How long a start waits for another grants command.
pub const START_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The policy of a start, read again under the lock of the grants
/// commands. Hold it until the session record exists: a change before
/// the lock is in this read, a change after it finds the session.
pub struct StartPolicy {
    _lock: Lock,
    /// The saved network word, already checked; `None`: the default.
    pub network: Option<String>,
    pub warning: Option<String>,
}

/// The network word of a start under the confirmed policy, with its lock;
/// `None` after a printed refusal. An explicit word wins.
pub fn start_network(
    profile: &Profile,
    earlier: &Grants,
    explicit: Option<String>,
) -> Option<(StartPolicy, String)> {
    let policy = confirm_start(profile, earlier)
        .map_err(|e| eprintln!("{e:#}"))
        .ok()?;
    if policy.network != earlier.network {
        policy.warning.iter().for_each(|w| eprintln!("{w}"));
    }
    let word = explicit.unwrap_or_else(|| policy.network.clone().unwrap_or_else(|| "ask".into()));
    Some((policy, word))
}

/// Confirm the earlier read of a start. Changed saved folders refuse the
/// start; the network word is always the one of this read.
pub fn confirm_start(profile: &Profile, earlier: &Grants) -> Result<StartPolicy> {
    confirm_start_within(profile, earlier, START_LOCK_WAIT)
}

/// The lock is held by another process: the only error worth waiting on.
fn busy(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::raw_os_error)
        == Some(libc::EWOULDBLOCK)
}

pub(super) fn confirm_start_within(
    profile: &Profile,
    earlier: &Grants,
    wait: std::time::Duration,
) -> Result<StartPolicy> {
    let until = std::time::Instant::now() + wait;
    let lock = loop {
        match Lock::take_with(profile, true) {
            Ok(lock) => break lock,
            Err(e) if !busy(&e) => return Err(e),
            Err(_) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Err(e) => return Err(e.context(text::grants_busy_at_start(&profile.dir.join(LOCK)))),
        }
    };
    let (fresh, warning) = match missing_after_save(profile) {
        Some(line) => (unavailable_policy(), Some(line)),
        None => match load(profile) {
            Ok(g) => (g, None),
            Err(e) => (
                unavailable_policy(),
                Some(text::grants_skipped(&format!("{e:#}"))),
            ),
        },
    };
    if fresh.shares != earlier.shares {
        bail!(text::grants_changed_at_start());
    }
    Ok(StartPolicy {
        _lock: lock,
        network: fresh.network,
        warning,
    })
}
