//! The texts of upgrades and migration: `paddock macvm update` and
//! `rollback`, the previous golden, the agent share and the agent
//! self-update as the host sees it.

use super::shown;
use std::path::Path;

// ---- paddock macvm update -----------------------------------------------

/// `update` without a sealed golden.
pub fn update_needs_golden() -> String {
    "there is no sealed golden to update. Run `paddock macvm setup` first.".into()
}

/// The plan that `update` prints before the build starts.
pub fn update_plan(current: &str, profiles: usize) -> String {
    format!(
        "paddock macvm update builds a new golden next to the current one (macOS {current}) and swaps them once it is sealed. The current golden stays as golden.prev for `paddock macvm rollback`. The {profiles} profile(s) keep their own system disk and their macOS: a data disk mounts only under the system disk that made it. New profiles clone the new golden. `paddock app reset NAME --data` moves a profile to it, after a backup of the pair."
    )
}

/// A sealed `golden.new` is a golden that a rollback set aside.
pub fn update_removes_stale_new(dir: &Path) -> String {
    format!(
        "{} holds the golden that a rollback set aside: it goes, and the update builds anew.",
        shown(dir)
    )
}

/// The newest image this Mac runs is the version of the golden.
pub fn update_same_version(version: &str) -> String {
    format!(
        "the golden already runs macOS {version}, the newest that this Mac runs. `paddock macvm update --force` rebuilds it anyway."
    )
}

/// The build in `golden.new` did not reach the seal.
pub fn new_golden_unsealed(dir: &Path) -> String {
    format!(
        "the build in {} is not sealed, so the golden stays as it is. Run `paddock macvm update` again to continue it.",
        shown(dir)
    )
}

/// The swap is done.
pub fn update_done(old: &str, new: &str, profiles: usize) -> String {
    format!(
        "the golden is macOS {new} now. The previous one (macOS {old}) stays as golden.prev until a profile completes a session on the new golden. The {profiles} profile(s) keep their own system disk (macOS {old}); `paddock app reset NAME --data` moves one to macOS {new} after a backup of the pair."
    )
}

// ---- paddock macvm rollback ---------------------------------------------

/// The question before the previous golden comes back.
pub fn rollback_question(current: &str, prev: &str) -> String {
    format!(
        "paddock macvm rollback replaces the golden (macOS {current}) with the previous one (macOS {prev}). No profile changes: each keeps its own system disk. New profiles clone macOS {prev}."
    )
}

/// The rollback is done.
pub fn rollback_done(now: &str, aside: &Path) -> String {
    format!(
        "the golden is macOS {now} again. The one it replaced is in {}, where `paddock macvm update` rebuilds and `paddock macvm remove` removes it.",
        shown(aside)
    )
}

/// An `update` stopped between its two folder renames, and this start
/// finished it.
pub fn recovered_update() -> String {
    "an earlier `paddock macvm update` stopped between its two folder renames. paddock finished the swap: the new golden is in place.".into()
}

/// A `rollback` stopped between its two folder renames, and this start
/// finished it.
pub fn recovered_rollback() -> String {
    "an earlier `paddock macvm rollback` stopped between its two folder renames. paddock finished it: the previous golden is in place.".into()
}

// ---- Status -------------------------------------------------------------

/// The `prev` line of `paddock macvm status`.
pub fn status_prev(version: &str, removable: bool) -> String {
    if removable {
        format!("  prev     macOS {version}, kept for `paddock macvm rollback`. A profile completed a session on the new golden, so `paddock gc` removes it.")
    } else {
        format!("  prev     macOS {version}, kept for `paddock macvm rollback` until a profile completes a session on the new golden.")
    }
}

// ---- paddock gc ---------------------------------------------------------

pub fn gc_prev_what(version: &str) -> String {
    format!("the previous macOS guest golden (macOS {version})")
}

pub fn gc_prev_reason() -> String {
    "a profile completed a session on the new golden, so a rollback is no longer expected".into()
}

pub fn gc_agent_what(version: &str) -> String {
    format!("the guest agent share of paddock {version}")
}

/// `gc` was asked to remove a path that is not one of its items.
pub fn gc_not_macvm(dir: &Path) -> String {
    format!(
        "{} is not the previous golden or an agent share of the macOS guest, so paddock gc does not remove it",
        shown(dir)
    )
}

pub fn gc_agent_reason(current: &str) -> String {
    format!("this paddock is {current}, and a session shares only the agent of its own version")
}

// ---- The agent share and the self-update --------------------------------

/// The hash file next to the installed agent does not match the binary.
pub fn agent_hash_mismatch(path: &Path, in_file: &str, of_binary: &str) -> String {
    format!(
        "{} does not match its hash file (the file says {in_file}, the binary is {of_binary}), so the agent share is not made. Reinstall paddock, or run packaging/macos/build-guest.sh.",
        shown(path)
    )
}

/// The agent share was not made; the session goes on with the agent of
/// the golden.
pub fn agent_share_failed(error: &str) -> String {
    format!("the agent share was not made: {error}. The session runs with the guest agent of the golden, which cannot update in this session.")
}

/// The `hello` after an update carries the old version again.
pub fn agent_rolled_back(host: &str, running: &str) -> String {
    format!(
        "the guest agent did not update to {host}: the guest runs {running} again, its previous agent, after the new one failed. paddock talks to it as it is. If this repeats, rebuild the guest with `paddock macvm update`."
    )
}

/// The event line when the update request goes out.
pub fn event_agent_update(from: &str, to: &str, downgrade: bool) -> String {
    if downgrade {
        format!("agent update from {from} to {to} (a downgrade: the host version wins)")
    } else {
        format!("agent update from {from} to {to}")
    }
}

/// The event line when the new agent dialed in.
pub fn event_agent_updated(version: &str, secs: f64) -> String {
    format!("agent update done: {version} dialed in {secs:.1} s after the request")
}

/// The event line when the old agent dialed in again after the update.
pub fn event_agent_rollback(running: &str) -> String {
    format!("agent rollback: the guest runs {running} again")
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod tests;
