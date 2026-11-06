//! Continue a Codex session at another branch under the same thread id.
//!
//! Codex keys its prompt cache by the root thread id. A fork under a new
//! id starts cold, so paddock continues a chosen branch under the id that
//! the session has: the rollout file is replaced by the projection of the
//! branch, the session head moves to the branch head, and the old head
//! stays in the DAG as a session of its own. One branch runs under an id
//! at a time.
//!
//! [`swap_file`] does this on disk, for a session that no run holds.
//! [`swap_live`] does it for a session in custody: it waits until no turn
//! runs, saves the verdict of the guard, has the harness drop the thread,
//! swaps the file, and has the harness load the thread again from it.
//!
//! The file is replaced by a rename, and the store change is journaled
//! (`swap-journal/<session>.json`): after a crash between the rename and
//! the head move, the next swap of the session finishes or drops the
//! journaled one by what the file holds.

pub mod request;

use super::{check_version, CONTAINER, HARNESS};
use crate::horse::guard::{self, Op, Verdict};
use crate::horse::live::harness::{Live, Pump};
use crate::horse::run::{append_event, RunHorse};
use crate::horse::store::{Hash, Lock, Node, Store};
use crate::horse::{ForkOf, Session};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What a swap did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSwap {
    pub session: String,
    pub thread: String,
    pub from: Option<Hash>,
    pub to: Hash,
    /// The rollout file, which holds the branch now.
    pub path: PathBuf,
    /// The old rollout file, kept as a hard link next to it.
    pub backup: Option<PathBuf>,
    /// The session that holds the old head, when no other session did.
    pub kept: Option<String>,
}

/// What a live swap did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapReport {
    pub file: FileSwap,
    pub verdict: Verdict,
    /// The thread that the harness holds after the reload.
    pub thread: String,
    /// The swap waited this long for the running turn to end.
    pub waited: Duration,
}

/// Why a live swap did not happen, or did not finish.
#[derive(Debug)]
pub enum Failure {
    /// Nothing changed, and the custody goes on.
    Refused(anyhow::Error),
    /// The custody ends. The file may hold the branch already; the next
    /// swap or run resumes from the file.
    Custody(anyhow::Error),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Refused(e) => write!(f, "{e:#}"),
            Failure::Custody(e) => write!(f, "{e:#}; the custody of the run ended"),
        }
    }
}

/// How a live swap waits.
#[derive(Clone, Debug)]
pub struct LiveOpts {
    /// Stop the running turn first (`turn/interrupt`), then wait for its end.
    pub interrupt: bool,
    /// The longest wait for the turn to end.
    pub wait: Duration,
    /// The longest wait of one read of the harness.
    pub tick: Duration,
}

/// The journal of a swap between the file rename and the head move.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Journal {
    from: Option<Hash>,
    to: Hash,
    path: PathBuf,
}

fn journal_ref(session: &str) -> String {
    format!("swap-journal/{session}.json")
}

/// A branch head: a session id (its head), or a node hash or hash prefix
/// of at least 6 digits in a session of the same horse.
pub fn resolve_branch(store: &Store, s: &Session, spec: &str) -> Result<Hash> {
    if spec.starts_with("s-") {
        if let Ok(other) = store.session(spec) {
            return other
                .head
                .with_context(|| format!("session {spec} has no rows"));
        }
    }
    let mut found: Vec<Hash> = Vec::new();
    for id in store.horse(&s.horse)?.sessions {
        let other = store.session(&id)?;
        if other.binding.harness != HARNESS {
            continue;
        }
        if let Ok(h) = store.resolve_node(&other, spec) {
            if !found.contains(&h) {
                found.push(h);
            }
        }
    }
    match found.as_slice() {
        [h] => Ok(h.clone()),
        [] => bail!("no Codex session of horse {} has a node {spec}", s.horse),
        _ => bail!("{spec} names more than one node; give more digits"),
    }
}

/// The rows of the branch, checked: native Codex rows of the pinned
/// version, with the same first row (the same `session_meta`) as the
/// session.
fn branch(store: &Store, s: &Session, to: &Hash) -> Result<Vec<Node>> {
    let nodes = store.chain(Some(to))?;
    for n in &nodes {
        let ok = n.meta.kind == "native"
            && n.meta.harness.as_deref() == Some(HARNESS)
            && n.meta.container.as_deref() == Some(CONTAINER);
        if !ok {
            bail!("node {} is not a native Codex row", n.hash.short());
        }
        check_version(n.meta.version.as_deref().unwrap_or_default())?;
    }
    let root = store.session_nodes(s)?.into_iter().next();
    match (root, nodes.first()) {
        (Some(a), Some(b)) if a.hash == b.hash => Ok(nodes),
        (None, _) => bail!("session {} has no rows to swap", s.id),
        _ => bail!(
            "node {} does not continue thread {}: its first row is another session_meta",
            to.short(),
            s.binding.native_id.as_deref().unwrap_or("?")
        ),
    }
}

/// Check that a session can swap at all, before a wait.
fn check_session(s: &Session) -> Result<String> {
    if s.binding.harness != HARNESS || s.binding.container != CONTAINER {
        bail!(
            "a swap under the same id is for Codex sessions, whose cache is keyed by the thread id; session {} is a {} session, so fork it instead",
            s.id,
            s.binding.harness
        );
    }
    check_version(&s.binding.version)?;
    s.binding
        .native_id
        .clone()
        .with_context(|| format!("session {} has no Codex thread yet: project it first", s.id))
}

/// The rollout path of a session under a Codex home.
fn rollout_path(store: &Store, s: &Session, codex_home: &Path) -> Result<PathBuf> {
    let rel = String::from_utf8(
        store
            .get(s.native_header.as_ref().context("missing rollout path")?)?
            .raw,
    )?;
    let rel = Path::new(&rel);
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("invalid rollout path");
    }
    Ok(codex_home.join("sessions").join(rel))
}

fn file_rows(path: &Path) -> Result<Vec<Vec<u8>>> {
    let bytes = std::fs::read(path)?;
    let mut rows: Vec<Vec<u8>> = bytes.split(|b| *b == b'\n').map(<[u8]>::to_vec).collect();
    if rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    Ok(rows)
}

fn raws(nodes: &[Node]) -> Vec<Vec<u8>> {
    nodes.iter().map(|n| n.raw.clone()).collect()
}

/// Keep `old` in a session of its own, unless a session has it as head.
fn keep(locked: &Lock<'_>, s: &Session, old: &Hash) -> Result<Option<String>> {
    let store = locked.store;
    for id in store.horse(&s.horse)?.sessions {
        if id != s.id && store.session(&id)?.head.as_ref() == Some(old) {
            return Ok(None);
        }
    }
    let k = locked.new_session(
        &s.horse,
        crate::horse::Binding {
            native_id: None,
            ..s.binding.clone()
        },
    )?;
    locked.update_session(&k.id, |k| {
        k.head = Some(old.clone());
        k.native_header = s.native_header.clone();
        k.fork_of = Some(ForkOf {
            session: s.id.clone(),
            node: old.clone(),
        });
        k.lineage = s.lineage.clone();
    })?;
    Ok(Some(k.id))
}

/// Move the head and keep the old one: the store half of a swap.
fn move_head(locked: &Lock<'_>, s: &Session, thread: &str, to: &Hash) -> Result<Option<String>> {
    let kept = match &s.head {
        Some(old) => keep(locked, s, old)?,
        None => None,
    };
    locked.update_session(&s.id, |x| x.head = Some(to.clone()))?;
    locked
        .store
        .write_ref(&format!("codex/{thread}.json"), s.id.as_bytes())?;
    Ok(kept)
}

/// Finish or drop a journaled swap of a session by what its file holds.
fn recover(locked: &Lock<'_>, s: &Session, thread: &str) -> Result<()> {
    let store = locked.store;
    let Some(b) = store.read_ref(&journal_ref(&s.id))? else {
        return Ok(());
    };
    let j: Journal = serde_json::from_slice(&b).context("the swap journal is damaged")?;
    let on_disk = file_rows(&j.path).ok();
    let to = raws(&store.chain(Some(&j.to))?);
    if on_disk.as_ref() == Some(&to) && s.head.as_ref() != Some(&j.to) {
        log::warn!(
            "horse: finishing the swap of session {} to {} that a crash interrupted",
            s.id,
            j.to.short()
        );
        move_head(locked, s, thread, &j.to)?;
    }
    store.remove_ref(&journal_ref(&s.id))
}

/// Swap the rollout of a session that no run holds for the projection of
/// the branch that ends at `to`, under the same thread id. `codex_home`
/// is the Codex home that holds `sessions/`. The caller saved the verdict
/// of the guard before.
pub fn swap_file(store: &Store, session: &str, to: &Hash, codex_home: &Path) -> Result<FileSwap> {
    let locked = store.lock()?;
    let s = store.session(session)?;
    let thread = check_session(&s)?;
    recover(&locked, &s, &thread)?;
    let s = store.session(session)?;
    if s.head.as_ref() == Some(to) {
        bail!("session {session} is at node {} already", to.short());
    }
    let rows = raws(&branch(store, &s, to)?);
    let path = rollout_path(store, &s, codex_home)?;
    if !path.is_file() {
        bail!(
            "no rollout of thread {thread} at {}: project the session first",
            path.display()
        );
    }
    let j = Journal {
        from: s.head.clone(),
        to: to.clone(),
        path: path.clone(),
    };
    store.write_ref(&journal_ref(session), &serde_json::to_vec(&j)?)?;
    let backup = match super::project::publish(codex_home, &path, &thread, &rows, true) {
        Ok(b) => b,
        Err(e) => {
            store.remove_ref(&journal_ref(session))?;
            return Err(e);
        }
    };
    let kept = move_head(&locked, &s, &thread, to)?;
    store.remove_ref(&journal_ref(session))?;
    drop(locked);
    if let Err(e) = store.refresh_guard(session) {
        log::warn!("horse guard {session}: {e:#}");
    }
    Ok(FileSwap {
        session: session.into(),
        thread,
        from: s.head,
        to: to.clone(),
        path,
        backup,
        kept,
    })
}

/// The verdict of the guard for a swap, saved before anything is written.
pub fn judge(store: &Store, session: &str, to: &Hash) -> Result<Verdict> {
    let v = guard::check(
        store,
        session,
        &Op::Swap { to: to.clone() },
        crate::horse::now_ms(),
    )?;
    store.save_verdict(&v)?;
    Ok(v)
}

/// The run event of a swap.
pub fn record(store: &Store, s: &Session, kind: &str, f: &FileSwap) -> Result<Hash> {
    let rh = RunHorse {
        horse: s.horse.clone(),
        session: s.id.clone(),
        harness: HARNESS.into(),
    };
    append_event(
        store,
        &rh,
        &json!({
            "type": kind,
            "session": f.session,
            "from": f.from,
            "to": f.to,
            "thread": f.thread,
            "kept": f.kept,
            "at_ms": crate::horse::now_ms(),
        }),
    )
}

/// Swap a session that `live` holds: wait for a safe point (after
/// `turn/interrupt` with `opts.interrupt`), save the verdict of the guard,
/// let the harness drop the thread, swap the file, and let the harness
/// load the thread again from the file. One event `swap.live`.
pub fn swap_live(
    store: &Store,
    session: &str,
    to: &Hash,
    live: &mut dyn Live,
    opts: &LiveOpts,
) -> std::result::Result<SwapReport, Failure> {
    use Failure::{Custody, Refused};
    let s = store.session(session).map_err(Refused)?;
    let thread = check_session(&s).map_err(Refused)?;
    if !live.sessions().iter().any(|x| x.session == session) {
        return Err(Refused(anyhow::anyhow!(
            "session {session} is not in this custody"
        )));
    }
    if !live.reloads() {
        return Err(Refused(anyhow::anyhow!(
            "the run holds session {session} in a TUI, which cannot load the thread again while it runs; swap it after the run"
        )));
    }
    branch(store, &s, to).map_err(Refused)?;
    let started = Instant::now();
    if opts.interrupt && !live.safe(Some(session)) {
        live.interrupt().map_err(Custody)?;
    }
    while !live.safe(Some(session)) {
        if started.elapsed() >= opts.wait {
            return Err(Refused(anyhow::anyhow!(
                "the running turn of session {session} did not end within {} s; nothing changed (--interrupt stops it)",
                opts.wait.as_secs()
            )));
        }
        if live.pump(opts.tick).map_err(Custody)? == Pump::Closed {
            return Err(Refused(anyhow::anyhow!(
                "codex exited before session {session} was idle; nothing changed"
            )));
        }
    }
    let waited = started.elapsed();
    let verdict = judge(store, session, to).map_err(Refused)?;
    let mut done: Option<FileSwap> = None;
    let r = live.reload(session, &mut |root: &Path| {
        let home = root
            .parent()
            .context("the sessions folder of the custody has no parent")?;
        done = Some(swap_file(store, session, to, home)?);
        Ok(())
    });
    let r = r.map_err(|e| {
        let state = if done.is_some() {
            format!(
                "the rollout of thread {thread} holds branch {} now",
                to.short()
            )
        } else {
            format!("the rollout of thread {thread} did not change")
        };
        Custody(e.context(format!(
            "codex did not load thread {thread} again ({state}; the next run resumes from the file)"
        )))
    })?;
    r.written.map_err(Refused)?;
    let file = done.ok_or_else(|| Refused(anyhow::anyhow!("the swap did not run")))?;
    if r.native != thread {
        return Err(Custody(anyhow::anyhow!(
            "codex loaded thread {} instead of {thread} after the swap; the rollout holds branch {} now, and the next run resumes from it",
            r.native,
            to.short()
        )));
    }
    record(store, &s, "swap.live", &file).map_err(Custody)?;
    Ok(SwapReport {
        file,
        verdict,
        thread: r.native,
        waited,
    })
}

#[cfg(test)]
#[path = "swap_tests.rs"]
mod tests;
