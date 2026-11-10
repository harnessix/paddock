//! The prefix guard for Grok Build sessions.
//!
//! Grok keys its prompt cache by the session id, as Codex keys it by the
//! thread id. So a fork under a new session id starts cold, also when its
//! items are the same bytes, and a swap under the same id keeps the
//! prompt up to the fork point warm. The model sees the items of
//! `chat_history.jsonl`, about four bytes per token.

use super::{Op, Verdict};
use crate::horse::grok::{replay::replay, CHAT, HARNESS};
use crate::horse::store::{Hash, Node, Store};
use crate::horse::Session;
use anyhow::{bail, Context as _, Result};

/// The verdict for a fork or a swap of a Grok session, or `None` when the
/// common rules apply.
pub fn check(store: &Store, s: &Session, op: &Op, now_ms: u64) -> Result<Option<Verdict>> {
    if s.binding.harness != HARNESS {
        return Ok(None);
    }
    let nodes = store.session_nodes(s)?;
    let v = match op {
        Op::Swap { to } => swap(store, s, &nodes, to, now_ms)?,
        Op::Fork { at } => fork(s, &nodes, at, now_ms)?,
        _ => return Ok(None),
    };
    Ok(Some(v))
}

/// The bytes of the chat items that a chain gives the model.
fn seen(nodes: &[Node]) -> u64 {
    replay(nodes)
        .map(|f| f.chat.iter().map(|c| c.len() as u64).sum())
        .unwrap_or_else(|_| {
            nodes
                .iter()
                .filter(|n| n.meta.container.as_deref() == Some(CHAT))
                .map(|n| n.raw.len() as u64)
                .sum()
        })
}

fn native(s: &Session) -> String {
    s.binding
        .native_id
        .clone()
        .unwrap_or_else(|| "(not projected)".into())
}

fn verdict(s: &Session, op: String, now_ms: u64) -> Verdict {
    Verdict {
        session: s.id.clone(),
        op,
        keeps: true,
        prefix_bytes: 0,
        lost_bytes: 0,
        lost_tokens: 0,
        reasons: Vec::new(),
        suggestion: None,
        notes: Vec::new(),
        at_ms: now_ms,
    }
}

fn swap(store: &Store, s: &Session, cur: &[Node], to: &Hash, now_ms: u64) -> Result<Verdict> {
    let branch = store.chain(Some(to))?;
    match (cur.first(), branch.first()) {
        (Some(a), Some(b)) if a.hash == b.hash => {}
        (None, _) => bail!("session {} has no rows to swap", s.id),
        _ => bail!(
            "node {} does not continue Grok session {}: its first row is another one",
            to.short(),
            native(s)
        ),
    }
    let k = cur
        .iter()
        .zip(&branch)
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    let mut v = verdict(s, format!("a swap to {}", to.short()), now_ms);
    v.prefix_bytes = seen(cur);
    if s.head.as_ref() == Some(to) {
        v.op = "a swap to the head".into();
        return Ok(v);
    }
    let kept = seen(&cur[..k]);
    let new = seen(&branch).saturating_sub(kept);
    v.lost_bytes = new;
    v.lost_tokens = new.div_ceil(4);
    v.keeps = new == 0;
    v.notes.push(format!(
        "Grok session {} keeps its id, so its cache key stays: the prompt up to the fork point {} (about {} tokens) stays warm",
        native(s),
        branch[k - 1].hash.short(),
        kept.div_ceil(4)
    ));
    if new > 0 {
        v.reasons.push(format!(
            "the branch has {} rows after the fork point; the next request reads about {} tokens of them without the cache",
            branch.len() - k,
            v.lost_tokens
        ));
    }
    Ok(v)
}

fn fork(s: &Session, cur: &[Node], at: &Hash, now_ms: u64) -> Result<Verdict> {
    let k = cur
        .iter()
        .position(|n| &n.hash == at)
        .with_context(|| format!("node {} is not in the session", at.short()))?;
    let lost = seen(&cur[..=k]);
    let mut v = verdict(s, format!("a fork at {}", at.short()), now_ms);
    v.prefix_bytes = seen(cur);
    v.lost_bytes = lost;
    v.lost_tokens = lost.div_ceil(4);
    v.keeps = lost == 0;
    v.reasons.push(
        "a Grok fork runs under a new session id, and Grok keys its prompt cache by the session id, so the fork starts cold".into(),
    );
    v.suggestion = Some(format!(
        "paddock horse swap {} {} (the same Grok session {} continues at that node, warm up to it; one branch runs under an id at a time)",
        s.id,
        at.short(),
        native(s)
    ));
    Ok(v)
}
