//! The prefix guard for Codex sessions.
//!
//! Codex keys its prompt cache by the root thread id (`prompt_cache_key`),
//! not by content alone. So:
//! - a swap continues another branch under the same thread id: the key
//!   stays, the prompt up to the fork point stays warm, and what the
//!   branch has after the fork point is a break of that size;
//! - a fork under a new thread id gets a new key and starts cold, also
//!   when its rows are the same bytes;
//! - an ephemeral `thread/fork` of a live thread (a warm side branch,
//!   live/codex/side.rs) keeps the key of its parent: Codex writes no
//!   rollout for it, and its prompt up to the fork point stays warm.
//!
//! The model sees the `response_item` rows of a rollout. Their size in
//! tokens comes from the last `token_count` event that gives the input
//! tokens of a request, or else from four bytes per token.

use super::{Op, Verdict};
use crate::horse::store::{Hash, Node, Store};
use crate::horse::Session;
use anyhow::{bail, Context as _, Result};
use serde_json::Value;

/// The verdict for an operation on a Codex session, or `None` when the
/// common rules apply (another harness, or an operation that Codex treats
/// as the other harnesses do).
pub fn check(store: &Store, s: &Session, op: &Op, now_ms: u64) -> Result<Option<Verdict>> {
    if s.binding.harness != crate::horse::codex::HARNESS {
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

fn parse(n: &Node) -> Value {
    serde_json::from_slice(&n.raw).unwrap_or(Value::Null)
}

/// The bytes of the rows that the model sees.
fn seen_bytes(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .filter(|n| parse(n)["type"] == "response_item")
        .map(|n| n.raw.len() as u64)
        .sum()
}

/// Tokens per byte from the last request whose input size Codex wrote.
fn scale(nodes: &[Node]) -> Option<f64> {
    let mut best = None;
    let mut before = 0u64;
    for n in nodes {
        let v = parse(n);
        if v["type"] == "response_item" {
            before += n.raw.len() as u64;
        }
        let input = v["payload"]["info"]["last_token_usage"]["input_tokens"].as_u64();
        if v["type"] == "event_msg" && v["payload"]["type"] == "token_count" {
            if let Some(t) = input.filter(|t| *t > 0 && before > 0) {
                best = Some(t as f64 / before as f64);
            }
        }
    }
    best
}

fn tokens(bytes: u64, scale: Option<f64>) -> u64 {
    match scale {
        Some(r) => (bytes as f64 * r).round() as u64,
        None => bytes.div_ceil(4),
    }
}

fn thread_id(s: &Session) -> String {
    s.binding
        .native_id
        .clone()
        .unwrap_or_else(|| "(not projected)".into())
}

/// A turn ends with `task_complete` or `turn_aborted`.
fn at_turn_end(n: &Node) -> bool {
    let v = parse(n);
    v["type"] == "session_meta"
        || (v["type"] == "event_msg"
            && matches!(
                v["payload"]["type"].as_str(),
                Some("task_complete" | "turn_aborted")
            ))
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
            "node {} does not continue thread {}: its first row is another session_meta",
            to.short(),
            thread_id(s)
        ),
    }
    let k = cur
        .iter()
        .zip(&branch)
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    let r = scale(cur).or_else(|| scale(&branch));
    let mut v = verdict(s, format!("a swap to {}", to.short()), now_ms);
    v.prefix_bytes = seen_bytes(cur);
    if s.head.as_ref() == Some(to) {
        v.op = "a swap to the head".into();
        return Ok(v);
    }
    let fork = &branch[k - 1].hash;
    let kept = seen_bytes(&cur[..k]);
    let new = seen_bytes(&branch[k..]);
    let dropped = seen_bytes(&cur[k..]);
    v.lost_bytes = new;
    v.lost_tokens = tokens(new, r);
    v.keeps = new == 0;
    v.notes.push(format!(
        "thread {} keeps its id, so its cache key stays: the prompt up to the fork point {} (about {} tokens) stays warm",
        thread_id(s),
        fork.short(),
        tokens(kept, r)
    ));
    if new > 0 {
        v.reasons.push(format!(
            "the branch has {} rows after the fork point; the next request reads about {} tokens of them without the cache",
            branch.len() - k,
            v.lost_tokens
        ));
    }
    if dropped > 0 {
        v.notes.push(format!(
            "the rows of the current branch after the fork point (about {} tokens) stay in the store, not in the rollout",
            tokens(dropped, r)
        ));
    }
    if branch.last().is_some_and(|n| !at_turn_end(n)) {
        v.reasons.push(format!(
            "node {} is not the end of a turn (task_complete or turn_aborted); Codex resumes the branch inside an unfinished turn",
            to.short()
        ));
    }
    Ok(v)
}

fn fork(s: &Session, cur: &[Node], at: &Hash, now_ms: u64) -> Result<Verdict> {
    let k = cur
        .iter()
        .position(|n| &n.hash == at)
        .with_context(|| format!("node {} is not in the session", at.short()))?;
    let r = scale(cur);
    let lost = seen_bytes(&cur[..=k]);
    let mut v = verdict(s, format!("a fork at {}", at.short()), now_ms);
    v.prefix_bytes = seen_bytes(cur);
    v.lost_bytes = lost;
    v.lost_tokens = tokens(lost, r);
    v.keeps = lost == 0;
    v.reasons.push(
        "a Codex fork runs under a new thread id, and Codex keys its prompt cache by the root thread id, so the fork starts cold".into(),
    );
    v.suggestion = Some(format!(
        "paddock horse swap {} {} (the same thread {} continues at that node, warm up to it; one branch runs under an id at a time), or paddock horse fork {} --warm while a run holds the session at its head (a side branch under the same cache key that is never projected)",
        s.id,
        at.short(),
        thread_id(s),
        s.id
    ));
    Ok(v)
}

/// The verdict for a warm side branch: an ephemeral `thread/fork` of the
/// live thread of `s` at its head `at`. Codex derives the cache key of an
/// ephemeral root fork from its parent, so the whole prompt stays warm.
/// Only the head: an ephemeral fork copies the thread as it is.
pub fn warm_fork(store: &Store, s: &Session, at: &Hash, now_ms: u64) -> Result<Verdict> {
    if s.binding.harness != crate::horse::codex::HARNESS {
        bail!("session {} is not a Codex session", s.id);
    }
    if s.head.as_ref() != Some(at) {
        bail!(
            "a warm side branch starts at the head of session {}, not at node {}",
            s.id,
            at.short()
        );
    }
    let nodes = store.session_nodes(s)?;
    let r = scale(&nodes);
    let mut v = verdict(s, format!("a warm side branch at {}", at.short()), now_ms);
    v.prefix_bytes = seen_bytes(&nodes);
    v.notes.push(format!(
        "an ephemeral fork of thread {} keeps its cache key: the prompt up to {} (about {} tokens) stays warm",
        thread_id(s),
        at.short(),
        tokens(v.prefix_bytes, r)
    ));
    v.notes.push(
        "Codex writes no rollout for the side branch: its turns stay in the store only, marked live, and no codec projects them".into(),
    );
    Ok(v)
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
