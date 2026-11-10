//! Continue a Grok session at another branch under the same session id.
//!
//! Grok keys its prompt cache by the session id, as Codex keys it by the
//! thread id. So paddock continues a chosen branch under the id that the
//! session has: the files of the session folder are replaced by the
//! projection of the branch, the session head moves to the branch head,
//! and the old head stays in the DAG as a session of its own. One branch
//! runs under an id at a time.
//!
//! The store change is journaled (`swap-journal/<session>.json`): after a
//! crash between the file change and the head move, the next swap of the
//! session finishes or drops the journaled one by what the files hold.

use super::replay::{replay, Files};
use super::{check_version, stored_summary, CONTAINER, HARNESS, UPDATES_FILE};
use crate::horse::run::{append_event, RunHorse};
use crate::horse::store::{Hash, Lock, Node, Store};
use crate::horse::{ForkOf, Session};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

/// What a swap did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSwap {
    pub session: String,
    /// The Grok session id, which stays.
    pub native: String,
    pub from: Option<Hash>,
    pub to: Hash,
    /// The session folder, which holds the branch now.
    pub path: PathBuf,
    /// The folder with the old files, as hard links.
    pub backup: Option<PathBuf>,
    /// The session that holds the old head, when no other session did.
    pub kept: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Journal {
    from: Option<Hash>,
    to: Hash,
    path: PathBuf,
}

fn journal_ref(session: &str) -> String {
    format!("swap-journal/{session}.json")
}

/// The rows of the branch, checked: native Grok rows of the pinned
/// version, with the same first node as the session.
pub fn branch(store: &Store, s: &Session, to: &Hash) -> Result<Vec<Node>> {
    let nodes = store.chain(Some(to))?;
    for n in &nodes {
        let ok = n.meta.kind == "native" && n.meta.harness.as_deref() == Some(HARNESS);
        if !ok {
            bail!("node {} is not a native Grok row", n.hash.short());
        }
        check_version(n.meta.version.as_deref().unwrap_or_default())?;
    }
    let root = store.session_nodes(s)?.into_iter().next();
    match (root, nodes.first()) {
        (Some(a), Some(b)) if a.hash == b.hash => Ok(nodes),
        (None, _) => bail!("session {} has no rows to swap", s.id),
        _ => bail!(
            "node {} does not continue Grok session {}: its first row is another one",
            to.short(),
            s.binding.native_id.as_deref().unwrap_or("?")
        ),
    }
}

/// Check that a session can swap at all.
pub fn check_session(s: &Session) -> Result<String> {
    if s.binding.harness != HARNESS || s.binding.container != CONTAINER {
        bail!("session {} is not a Grok session", s.id);
    }
    check_version(&s.binding.version)?;
    s.binding
        .native_id
        .clone()
        .with_context(|| format!("session {} has no Grok id yet: project it first", s.id))
}

/// A branch head: a session id (its head), or a node hash or hash prefix
/// of at least 6 digits in a Grok session of the same horse.
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
        [] => bail!("no Grok session of horse {} has a node {spec}", s.horse),
        _ => bail!("{spec} names more than one node; give more digits"),
    }
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

fn move_head(locked: &Lock<'_>, s: &Session, native: &str, to: &Hash) -> Result<Option<String>> {
    let kept = match &s.head {
        Some(old) => keep(locked, s, old)?,
        None => None,
    };
    locked.update_session(&s.id, |x| x.head = Some(to.clone()))?;
    locked
        .store
        .write_ref(&format!("grok/{native}.json"), s.id.as_bytes())?;
    Ok(kept)
}

/// The rows of the two files in a session folder.
type Rows = (Vec<Vec<u8>>, Vec<Vec<u8>>);

fn on_disk(dir: &Path) -> Option<Rows> {
    let (u, _) = super::read_lines(&dir.join(UPDATES_FILE)).ok()?;
    let (c, _) = super::read_lines(&dir.join(super::CHAT_FILE)).ok()?;
    Some((u, c))
}

/// Finish or drop a journaled swap of a session by what its files hold.
fn recover(locked: &Lock<'_>, s: &Session, native: &str) -> Result<()> {
    let store = locked.store;
    let Some(b) = store.read_ref(&journal_ref(&s.id))? else {
        return Ok(());
    };
    let j: Journal = serde_json::from_slice(&b).context("the swap journal is damaged")?;
    let to: Files = replay(&store.chain(Some(&j.to))?)?;
    if on_disk(&j.path) == Some((to.updates, to.chat)) && s.head.as_ref() != Some(&j.to) {
        log::warn!(
            "horse: finishing the swap of session {} to {} that a crash interrupted",
            s.id,
            j.to.short()
        );
        move_head(locked, s, native, &j.to)?;
    }
    store.remove_ref(&journal_ref(&s.id))
}

/// Swap the session folder of a session that no run holds for the
/// projection of the branch that ends at `to`, under the same session id.
/// `home` is the Grok home that holds `sessions/`. The caller saved the
/// verdict of the guard before.
pub fn swap_file(store: &Store, session: &str, to: &Hash, home: &Path) -> Result<FileSwap> {
    let locked = store.lock()?;
    let s = store.session(session)?;
    let native = check_session(&s)?;
    recover(&locked, &s, &native)?;
    let s = store.session(session)?;
    if s.head.as_ref() == Some(to) {
        bail!("session {session} is at node {} already", to.short());
    }
    let files = replay(&branch(store, &s, to)?)?;
    let summary = stored_summary(store, &s)?;
    let path = super::session_dir(home, &summary.cwd, &native);
    if !path.is_dir() {
        bail!(
            "no Grok session {native} at {}: project the session first",
            path.display()
        );
    }
    let j = Journal {
        from: s.head.clone(),
        to: to.clone(),
        path: path.clone(),
    };
    store.write_ref(&journal_ref(session), &serde_json::to_vec(&j)?)?;
    let raw = super::project::counted_summary(&summary.raw, &files)?;
    let backup = match super::project::publish(&path, &summary.cwd, &files, &raw, true) {
        Ok(b) => b,
        Err(e) => {
            store.remove_ref(&journal_ref(session))?;
            return Err(e);
        }
    };
    let kept = move_head(&locked, &s, &native, to)?;
    store.remove_ref(&journal_ref(session))?;
    drop(locked);
    if let Err(e) = store.refresh_guard(session) {
        log::warn!("horse guard {session}: {e:#}");
    }
    Ok(FileSwap {
        session: session.into(),
        native,
        from: s.head,
        to: to.clone(),
        path,
        backup,
        kept,
    })
}

/// The run event of a swap.
pub fn record(store: &Store, s: &Session, f: &FileSwap) -> Result<Hash> {
    let rh = RunHorse {
        horse: s.horse.clone(),
        session: s.id.clone(),
        harness: HARNESS.into(),
    };
    append_event(
        store,
        &rh,
        &json!({
            "type": "swap",
            "session": f.session,
            "from": f.from,
            "to": f.to,
            "thread": f.native,
            "kept": f.kept,
            "at_ms": crate::horse::now_ms(),
        }),
    )
}
