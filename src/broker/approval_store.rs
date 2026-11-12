//! Synchronized approval storage and cleanup.

use super::*;
use crate::state;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex, OnceLock};

#[path = "approval_store/connect.rs"]
mod connect;
pub(crate) use connect::{
    claim_connect, claim_connect_exact, claim_connect_question, is_store_lock_error,
};
#[path = "approval_store/evidence.rs"]
mod evidence;
pub(super) use evidence::read_text as read_evidence_text;
#[cfg(test)]
pub(crate) use evidence::PREPARED_HOOK as EVIDENCE_PREPARED_HOOK;
#[path = "approval_store/lease.rs"]
mod lease;
#[path = "approval_store/listing.rs"]
mod listing;
#[path = "approval_store/mail.rs"]
mod mail;
#[path = "approval_store/outcome.rs"]
mod outcome;
pub(crate) use outcome::UNVERIFIED_RULE;
#[path = "approval_store/publication.rs"]
mod publication;
#[path = "approval_store/report.rs"]
mod report;

pub use listing::{pending, pending_checked};
#[cfg(test)]
pub(super) use mail::try_card_mail;
use mail::{card_mail_key, notify_card};
use outcome::recorded_connect_outcome;
pub(crate) use outcome::{
    confirm_pause_resume, record_connect_outcome_bound, record_hook_connect_selection,
    record_pause_outcome, record_wake_selection, retire_connect_outcome,
};
#[cfg(test)]
pub(crate) use outcome::{record_connect_outcome, OUTCOME_TIME};
pub(super) use publication::try_write_pending;
pub(crate) use publication::update_pending;
pub use publication::write_pending;
#[cfg(test)]
pub(super) use report::cleanup_completed;
use report::consistent_local_outcome;
pub use report::pending_report;
#[cfg(test)]
pub(super) use report::RETENTION_SCAN_HOOK;

#[cfg(test)]
pub(crate) type PidLivenessHook = Box<dyn Fn(u32) -> Option<bool>>;

#[cfg(test)]
pub(crate) type CardMailHooks = (Box<dyn FnOnce() + Send>, Box<dyn FnOnce() + Send>);

#[cfg(test)]
thread_local! {
    pub(crate) static CARD_MAIL_HOOK: std::cell::RefCell<Option<CardMailHooks>> = std::cell::RefCell::new(None);
    pub(crate) static PID_LIVENESS_HOOK: std::cell::RefCell<Option<PidLivenessHook>> = std::cell::RefCell::new(None);
}

fn helper_alive(pid: u32) -> bool {
    #[cfg(test)]
    {
        PID_LIVENESS_HOOK
            .with(|hook| hook.borrow().as_ref().and_then(|f| f(pid)))
            .unwrap_or(true)
    }
    #[cfg(not(test))]
    {
        state::alive(pid)
    }
}

fn legacy_pid_dead(pid: i32) -> bool {
    #[cfg(test)]
    {
        PID_LIVENESS_HOOK.with(|hook| hook.borrow().as_ref().and_then(|f| f(pid as u32)))
            == Some(false)
    }
    #[cfg(not(test))]
    {
        (unsafe { libc::kill(pid, 0) }) != 0
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

pub fn approvals_dir(env: &HostEnv) -> PathBuf {
    env.paddock_run().join("approvals")
}

// This inode must not be removed: another process may already have it open.
// Every store mutation below takes this lock, including card rewrites.
pub(super) struct ApprovalLock {
    _scope: evidence::Scope,
    _file: File,
}

pub(super) fn lock_approvals(dir: &Path, id: &str) -> io::Result<ApprovalLock> {
    lock_approvals_with(dir, id, None, false)
}

fn lock_approvals_with(
    dir: &Path,
    id: &str,
    extra: Option<&Path>,
    nonblocking: bool,
) -> io::Result<ApprovalLock> {
    // No untrusted evidence path is opened after this process takes the lock.
    // Evidence that a lock holder changed during the wait is prepared again
    // without the lock. The last try keeps what it has, as unknown evidence.
    for tries_left in (0..LOCK_TRIES).rev() {
        let file = crate::privfs::append(&dir.join(".lock"))?;
        let prepared = evidence::prepare(dir, id, extra);
        retry_interrupted(|| {
            let flags = libc::LOCK_EX | if nonblocking { libc::LOCK_NB } else { 0 };
            if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })?;
        evidence::verify_lock(&prepared, dir, &file)?;
        if tries_left > 0 && !evidence::still_current(&prepared) {
            continue; // Closing the file releases the lock.
        }
        return Ok(ApprovalLock {
            _scope: evidence::enter(prepared),
            _file: file,
        });
    }
    unreachable!("the last try returns")
}

/// Lock attempts when the evidence changes during the wait for the lock.
const LOCK_TRIES: u32 = 3;

// The lock is on the owner inode, not a PID. The kernel releases it when
// the owner exits, including on a crash; PID reuse cannot revive a card.
const LEASE: &str = "lease-v1";
fn owners() -> &'static Mutex<HashMap<PathBuf, File>> {
    static OWNERS: OnceLock<Mutex<HashMap<PathBuf, File>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn release_owner(dir: &Path, id: &str) {
    owners()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&dir.join(format!("{id}.owner")));
}

pub(crate) fn abandon_owner(env: &HostEnv, id: &str) {
    release_owner(&approvals_dir(env), id);
}

#[derive(PartialEq, Eq)]
enum OwnerState {
    Live,
    Dead,
    Unknown,
    Legacy,
}

fn owner_state(dir: &Path, id: &str) -> io::Result<OwnerState> {
    let path = dir.join(format!("{id}.owner"));
    let text = match evidence::read_text(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(result) = evidence::missing_locked(&path) {
                return result.map(|()| OwnerState::Legacy);
            }
            return match std::fs::symlink_metadata(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(OwnerState::Legacy),
                Err(e) => Err(e),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "approval owner exists but cannot be read",
                )),
            };
        }
        Err(e) => return Err(e),
    };
    if text != LEASE {
        // Older PID-only records do not establish the identity of a live
        // owner. A nonexistent PID can still establish death.
        let dead = text
            .parse::<i32>()
            .ok()
            .filter(|pid| *pid > 0)
            .is_some_and(legacy_pid_dead);
        return Ok(if dead {
            OwnerState::Dead
        } else {
            OwnerState::Unknown
        });
    }
    let file = evidence::open_regular(&path, true)?;
    if evidence::read_open(&file)? != LEASE.as_bytes() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "approval owner changed",
        ));
    }
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(OwnerState::Dead)
    } else {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            Ok(OwnerState::Live)
        } else {
            Err(e)
        }
    }
}

pub(super) fn require_live_owner(dir: &Path, p: &Pending) -> io::Result<()> {
    match owner_state(dir, &p.id)? {
        OwnerState::Live | OwnerState::Legacy => Ok(()),
        OwnerState::Dead | OwnerState::Unknown => Err(io::Error::other(
            "approval owner is no longer known to be waiting",
        )),
    }
}

pub(super) fn retry_interrupted(mut attempt: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    loop {
        match attempt() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    REMOVE_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook(path)?;
        }
        Ok::<(), io::Error>(())
    })?;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn answer_text(dir: &Path, id: &str) -> io::Result<Option<String>> {
    #[cfg(test)]
    ANSWER_READ_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook()?;
        }
        Ok::<(), io::Error>(())
    })?;
    let path = dir.join(format!("{id}.answer"));
    match evidence::read_text(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(result) = evidence::missing_locked(&path) {
                return result.map(|()| None);
            }
            match std::fs::symlink_metadata(&path) {
                Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(None),
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "approval answer {} exists but cannot be read",
                        path.display()
                    ),
                )),
                Err(error) => Err(error),
            }
        }
        Err(e) => Err(e),
    }
}

// Called with the store lock. If card or answer removal fails, the answer
// remains available for a retry. Once the answer is removed, report it even
// if a later sidecar cleanup fails; otherwise the caller would lose it.
fn close_locked(dir: &Path, id: &str, only_if_answered: bool) -> io::Result<Option<String>> {
    let answer = answer_text(dir, id)?;
    if only_if_answered && answer.is_none() {
        return Ok(None);
    }
    remove_if_present(&dir.join(format!("{id}.json")))?;
    remove_if_present(&dir.join(format!("{id}.answer")))?;
    let cleanup = remove_if_present(&dir.join(format!("{id}.done")))
        .and_then(|()| remove_if_present(&dir.join(format!("{id}.owner"))));
    if let Err(e) = cleanup {
        if answer.is_none() {
            return Err(e);
        }
        log::warn!("cannot remove approval {id} sidecar: {e}");
    }
    release_owner(dir, id);
    Ok(answer)
}

// Do not wait for a contended store after a deadline or cancellation. A
// read answer is confirmed even if cleanup fails. A .done marker records
// that the owner read it, not that the requested action completed.
pub(super) fn close_owned(
    env: &HostEnv,
    id: &str,
    retry: &dyn Fn() -> bool,
) -> io::Result<Option<String>> {
    let dir = approvals_dir(env);
    loop {
        let result = (|| {
            let _lock = lock_approvals_with(&dir, id, None, true)?;
            let answer = answer_text(&dir, id)?;
            if answer.is_some() {
                if let Err(e) =
                    crate::privfs::write_atomic(&dir.join(format!("{id}.done")), b"read")
                {
                    log::warn!("cannot record cleanup of approval {id}: {e}");
                }
            }
            match close_locked(&dir, id, false) {
                Ok(_) => Ok(answer),
                Err(e) if answer.is_some() => {
                    log::warn!("cannot clean answered approval {id}: {e}");
                    Ok(answer)
                }
                Err(e) => Err(e),
            }
        })();
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock)
            && retry()
        {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        release_owner(&dir, id);
        return result;
    }
}

/// Close a card and return any answer published before closure. An error
/// means the caller must retry or report failure; it must not forget a card.
pub(crate) fn close_pending(env: &HostEnv, id: &str) -> io::Result<Option<String>> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    close_locked(&dir, id, false)
}

/// Consume only an answered card, leaving an unanswered one visible.
pub(crate) fn take_answer(env: &HostEnv, id: &str) -> io::Result<Option<String>> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    close_locked(&dir, id, true)
}

// With no consumer, retain an accepted answer instead of deleting it.
pub(crate) fn withdraw_unanswered(env: &HostEnv, id: &str) -> io::Result<bool> {
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    if answer_text(&dir, id)?.is_some() {
        return Ok(false);
    }
    close_locked(&dir, id, false)?;
    Ok(true)
}

// A dead lease cannot prove that an answer was delivered or acted on, even
// when .done says it was read. Keep the answer for recovery in either case.
fn recover_dead_locked(dir: &Path, id: &str) -> io::Result<bool> {
    if owner_state(dir, id)? != OwnerState::Dead {
        return Ok(false);
    }
    let answer = answer_text(dir, id)?;
    if answer.is_none() {
        match std::fs::symlink_metadata(dir.join(format!("{id}.done"))) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "answer-read marker without an answer; completion is unconfirmed",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        close_locked(dir, id, false)?;
    } else {
        if std::fs::symlink_metadata(dir.join(format!("{id}.json"))).is_ok() {
            log::warn!("approval {id} has an answer and no owner; retaining it for recovery");
        }
        // No owner can use this card. Retain the answer and owner as an
        // orphan so its uncertain outcome can be investigated.
        remove_if_present(&dir.join(format!("{id}.json")))?;
    }
    Ok(true)
}

fn recover_sidecars(env: &HostEnv, strict: bool) -> io::Result<()> {
    let dir = approvals_dir(env);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if strict {
                missing_card(&dir)?;
            }
            return Ok(());
        }
        Err(e) if strict => return Err(e),
        Err(_) => return Ok(()),
    };
    let mut ids = std::collections::BTreeSet::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if strict => return Err(e),
            Err(_) => continue,
        };
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext == "owner" || ext == "answer" || ext == "done")
        {
            if let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|id| valid_id(id))
            {
                ids.insert(id.to_owned());
            }
        }
    }
    for id in ids {
        let result = (|| {
            let _lock = lock_approvals(&dir, &id)?;
            let owner = owner_state(&dir, &id)?;
            if strict && owner == OwnerState::Unknown {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "approval owner liveness is unknown; retaining its card and answer",
                ));
            }
            if owner == OwnerState::Legacy
                && std::fs::symlink_metadata(dir.join(format!("{id}.json"))).is_err()
                && std::fs::symlink_metadata(dir.join(format!("{id}.answer"))).is_ok()
            {
                log::debug!("approval {id} has an ownerless answer; retaining it for recovery");
            }
            recover_dead_locked(&dir, &id)
        })();
        if let Err(e) = result {
            if strict {
                return Err(io::Error::new(
                    e.kind(),
                    format!("cannot recover approval {id}: {e}"),
                ));
            }
            log::warn!("cannot recover approval {id}: {e}");
        }
    }
    Ok(())
}

// An older card has no owner lease. Inspect its own sandbox, rather than a
// best-effort list which silently skips unreadable or malformed records.
fn legacy_sandbox_gone(env: &HostEnv, id: &str) -> io::Result<bool> {
    if !crate::view::valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid legacy sandbox id",
        ));
    }
    let path = state::Running::file(env, id);
    let text = match evidence::read_text(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // A missing record or directory is not proof that the waiter
            // exited: without its PID, liveness cannot be established.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy sandbox has no readable info.json",
            ));
        }
        Err(e) => return Err(e),
    };
    let running: state::Running =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if running.id != id || running.pid == 0 || running.pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid legacy sandbox identity",
        ));
    }
    #[cfg(test)]
    {
        Ok(!helper_alive(running.pid))
    }
    #[cfg(not(test))]
    {
        if unsafe { libc::kill(running.pid as i32, 0) } == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(true),
            Some(libc::EPERM) => Ok(false),
            _ => Err(error),
        }
    }
}

// NotFound may describe a dangling link, not a concurrently removed card.
fn missing_card(path: &Path) -> io::Result<()> {
    if let Some(result) = evidence::missing_locked(path) {
        return result;
    }
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // A missing leaf can be caused by a dangling link higher up.
            for ancestor in path.ancestors().skip(1) {
                match std::fs::symlink_metadata(ancestor) {
                    Ok(m) if m.file_type().is_symlink() => {
                        if let Err(error) = std::fs::metadata(ancestor) {
                            return Err(io::Error::new(
                                error.kind(),
                                format!(
                                    "approval ancestor {} cannot be read: {error}",
                                    ancestor.display()
                                ),
                            ));
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }
        Err(e) => Err(e),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("approval {} exists but cannot be read", path.display()),
        )),
    }
}

fn answer_valid(p: &Pending, text: &str) -> bool {
    if p.capability == CONNECT_CAPABILITY {
        return ConnectAnswer::parse_file(text).is_some_and(|answer| {
            !p.connect.as_ref().is_some_and(|c| c.notice)
                || matches!(answer, ConnectAnswer::AllowOnce | ConnectAnswer::Deny)
        });
    }
    if let Some((answer, _)) = AnswerFile::parse(text) {
        return if p.capability == ASK_CAPABILITY || !p.choices.is_empty() {
            answer == Answer::Deny
        } else {
            !matches!(answer, Answer::Session | Answer::DenySession) || p.session_allowed
        };
    }
    let reply = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("reply").and_then(|s| s.as_str()).map(str::to_owned));
    match reply {
        Some(reply) if p.capability == ASK_CAPABILITY => {
            !reply.trim().is_empty() && reply.chars().count() <= MAX_REPLY
        }
        Some(reply) => p.choices.contains(&reply),
        None => false,
    }
}

/// Confirm the state of the exact displayed connect card under the store
/// lock. A failed or partial read is not evidence that a panel can close.
pub fn connect_card_retired(env: &HostEnv, expected: &Pending) -> io::Result<bool> {
    let id = &expected.id;
    if !valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid approval id",
        ));
    }
    let dir = approvals_dir(env);
    let _lock = lock_approvals(&dir, id)?;
    let path = dir.join(format!("{id}.json"));
    let text = match evidence::read_text(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            missing_card(&path)?;
            // An orphan may be a partially published or damaged selection.
            for ext in ["answer", "done", "owner", "cleanup", "retired"] {
                let sidecar = dir.join(format!("{id}.{ext}"));
                match std::fs::symlink_metadata(&sidecar) {
                    Ok(_) => return Ok(false),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => missing_card(&sidecar)?,
                    Err(e) => return Err(e),
                }
            }
            return Ok(true);
        }
        Err(e) => return Err(e),
    };
    let card: Pending = serde_json::from_str(&text).map_err(io::Error::other)?;
    if !connect::same_identity(&card, expected) {
        return Ok(false);
    }
    match answer_text(&dir, id)? {
        Some(answer) if answer_valid(&card, &answer) => Ok(true),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid connect selection",
        )),
        None => Ok(false),
    }
}

// Re-read the exact snapshot and liveness under the same lock as writers.
pub(super) fn reap_stale(env: &HostEnv, p: &Pending) -> io::Result<bool> {
    let dir = approvals_dir(env);
    let legacy = crate::view::valid_id(&p.sandbox).then(|| state::Running::file(env, &p.sandbox));
    let _lock = lock_approvals_with(&dir, &p.id, legacy.as_deref(), false)?;
    let path = dir.join(format!("{}.json", p.id));
    let text = match evidence::read_text(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            missing_card(&path)?;
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    let current: Pending =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if &current != p {
        return Ok(false);
    }
    let owner = owner_state(&dir, &p.id)?;
    if owner == OwnerState::Dead {
        return recover_dead_locked(&dir, &p.id);
    }
    if owner == OwnerState::Unknown {
        return Ok(false);
    }
    let legacy_owner = owner == OwnerState::Legacy;
    let gone = if p.capability == ASK_CAPABILITY
        && legacy_owner
        && state::now().saturating_sub(p.created) > 60
    {
        legacy_sandbox_gone(env, &p.sandbox)?
    } else {
        p.connect.as_ref().is_some_and(|c| !helper_alive(c.pid))
    };
    if gone {
        let answer = answer_text(&dir, &p.id)?;
        if answer.is_none() {
            close_locked(&dir, &p.id, false)?;
        } else {
            log::debug!("keeping accepted answer for stale approval {}", p.id);
        }
    }
    Ok(gone)
}

/// These states describe only the helper's local application, not delivery,
/// final connection admission, or crash-durable persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalApplication {
    Applied,
    DecisionPrepared,
    Failed,
    NoWaiter,
    AffectedUnconfirmed,
    NoticePending,
    Unknown,
    /// The selection closed its question as a once denial without its
    /// effect, because its order against the last wake is unknown.
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationScope {
    Connection,
    Session,
    File,
    Notice,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectedDecision {
    pub id: String,
    pub app: String,
    pub parent_app: Option<String>,
    pub team: Option<String>,
    pub host: String,
    pub port: u16,
    pub path: Option<String>,
    pub guest_pid: Option<u32>,
    pub waiting: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectOutcome {
    pub id: String,
    pub request: Pending,
    pub selected: String,
    /// The local verdict, or None when there was no confirmed decision to
    /// deliver. This never confirms admission at the later hold.
    pub local_allowed: Option<bool>,
    pub rule: String,
    pub by: String,
    pub state: LocalApplication,
    pub scope: ApplicationScope,
    pub reason: Option<String>,
    pub recorded_at: Option<u64>,
    pub affected: Vec<AffectedDecision>,
}

#[derive(Serialize, Deserialize)]
struct StoredConnectOutcome {
    kind: String,
    #[serde(flatten)]
    local: ConnectOutcome,
    // This and local.request bind cleanup to the selected identity.
    selection: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalUnresolved {
    pub id: String,
    pub reason: String,
    pub selected: Option<String>,
    pub request: Option<Pending>,
}

/// An incomplete listing keeps healthy cards visible and identifies the
/// records that could not be inspected. No uncertain sidecar is removed.
pub struct PendingReport {
    pub pending: Vec<Pending>,
    pub diagnostics: Vec<String>,
    pub outcomes: Vec<ConnectOutcome>,
    pub unresolved: Vec<ApprovalUnresolved>,
}

pub(super) enum PublishError {
    Contended,
    Failed(io::Error),
}

impl From<io::Error> for PublishError {
    fn from(error: io::Error) -> Self {
        Self::Failed(error)
    }
}
