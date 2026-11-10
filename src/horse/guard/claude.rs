//! The prefix guard for Claude Code sessions.
//!
//! Claude Code records each main-thread request as an `api-request` row
//! (claude/shape.rs). Its `shapeHash` covers the system prompt and the tool
//! list, and its `timestamp` is the request time. The guard takes them as
//! the facts for the system-prompt and tool-set fields of the tuple and for
//! the cache time, instead of an estimate. The rules come from the Claude
//! prompt-caching docs:
//! - the cache is a prefix in the order tools, system, messages, so a
//!   change of the shape, the tools, the system prompt or the model misses
//!   from token 0,
//! - an unused cache entry lives 5 min; Claude Code on a subscription asks
//!   for 1 h, so a session idle under 1 h can still be warm,
//! - time never blocks an operation: it changes the text and the estimate.

use super::{Op, Tuple, Verdict};
use crate::horse::claude::shape::{self, Shape};
use crate::horse::claude::HARNESS;
use crate::horse::store::{Hash, Node};
use serde_json::Value;

/// The default cache lifetime of the Claude API.
pub const WARM_MS: u64 = 5 * 60_000;
/// The 1 h cache lifetime that Claude Code asks for on a subscription.
pub const MAYBE_WARM_MS: u64 = 60 * 60_000;

/// The cache-write price, in thousandths of the base input-token price:
/// 1.25 times for a 5 min write (Claude prompt-caching pricing, 2026-11-06).
pub const CACHE_WRITE_PERMILLE: u64 = 1250;
/// The cache-read price, in thousandths of the base input-token price:
/// 0.1 times (Claude prompt-caching pricing, 2026-11-06).
pub const CACHE_READ_PERMILLE: u64 = 100;

/// What the cached prefix costs, in base input tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    /// A break writes the prefix to the cache again.
    pub on_break: u64,
    /// A hit reads the prefix from the cache.
    pub on_hit: u64,
}

/// The Claude cost table for a prefix of `prefix_tokens`.
pub fn cost(prefix_tokens: u64) -> Cost {
    let at = |permille: u64| prefix_tokens.saturating_mul(permille).div_ceil(1000);
    Cost {
        on_break: at(CACHE_WRITE_PERMILLE),
        on_hit: at(CACHE_READ_PERMILLE),
    }
}

/// How warm the cache is after an idle time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ttl {
    /// Under 5 min: the cache holds on every plan.
    Warm,
    /// Under 1 h: the cache holds when the request asked for 1 h.
    MaybeWarm,
    Cold,
}

/// The cache state `now_ms` for a last request at `last_ms`.
pub fn ttl_verdict(now_ms: u64, last_ms: u64) -> Ttl {
    match now_ms.saturating_sub(last_ms) {
        i if i < WARM_MS => Ttl::Warm,
        i if i < MAYBE_WARM_MS => Ttl::MaybeWarm,
        _ => Ttl::Cold,
    }
}

/// The TTL line of a verdict.
pub fn ttl_line(now_ms: u64, last_ms: u64) -> String {
    let state = match ttl_verdict(now_ms, last_ms) {
        Ttl::Warm => "warm",
        Ttl::MaybeWarm => "maybe warm (1 h on a subscription)",
        Ttl::Cold => "cold",
    };
    format!(
        "last request {} min ago: {state}",
        now_ms.saturating_sub(last_ms) / 60_000
    )
}

/// What the guard reads from a Claude Code session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    pub shape: Option<Shape>,
    /// The prompt tokens of the last reply: input, cache write and cache read.
    pub prefix_tokens: u64,
}

/// The facts of a session of harness `harness`, and the tuple filled from
/// them. `None` for a session of another harness.
pub fn read(harness: &str, nodes: &[Node], t: &mut Tuple) -> Option<Facts> {
    if harness != HARNESS {
        return None;
    }
    let f = Facts {
        shape: shape::latest(nodes),
        prefix_tokens: prefix_tokens(nodes),
    };
    if let Some(s) = &f.shape {
        fill(t, s);
    }
    Some(f)
}

/// The tuple fields that the shape gives: the system-prompt and tool-set
/// hashes (one hash covers both) and the time of the last request.
pub fn fill(t: &mut Tuple, shape: &Shape) {
    let h = Hash::parse(&shape.hash).unwrap_or_else(|_| Hash::of(shape.hash.as_bytes()));
    t.system_hash = Some(h.clone());
    t.tools_hash = Some(h);
    t.last_request_ms = Some(shape.at_ms);
    if t.model.is_none() {
        t.model.clone_from(&shape.model);
    }
}

/// The prompt tokens of the last assistant row with a usage.
fn prefix_tokens(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .rev()
        .find_map(|n| {
            let v: Value = serde_json::from_slice(&n.raw).ok()?;
            if v.get("type").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            let u = v.pointer("/message/usage")?;
            let k = |x: &str| u.get(x).and_then(Value::as_u64).unwrap_or(0);
            Some(
                k("input_tokens") + k("cache_creation_input_tokens") + k("cache_read_input_tokens"),
            )
        })
        .unwrap_or(0)
}

/// The shape changed from the saved tuple to the new one.
pub fn shape_changed(harness: &str, saved: &Tuple, new: &Tuple) -> bool {
    harness == HARNESS
        && matches!((&saved.system_hash, &new.system_hash), (Some(a), Some(b)) if a != b)
}

/// The Claude part of a verdict: a shape change since the saved tuple, an
/// operation that breaks from token 0, and the cache time.
pub fn judge(v: &mut Verdict, op: &Op, f: &Facts, saved: &Tuple, now: &Tuple, now_ms: u64) {
    let c = cost(f.prefix_tokens);
    let price = format!(
        "a break pays {} tokens at the cache-write price (about {} input tokens) instead of {} at the cache-read price",
        f.prefix_tokens, c.on_break, c.on_hit
    );
    let mut from_zero = false;
    if let (true, Some(a), Some(b)) = (
        shape_changed(HARNESS, saved, now),
        &saved.system_hash,
        &now.system_hash,
    ) {
        v.reasons.push(format!(
            "the system prompt or the tools changed (shape {} to {}): they come first, so the cache misses from token 0; {price}",
            a.short(),
            b.short()
        ));
        from_zero = true;
    }
    let breaks = match op {
        Op::Model(m) => now.model.as_deref().is_some_and(|x| x != m),
        Op::System | Op::Tools { .. } => true,
        _ => false,
    };
    if breaks {
        v.reasons
            .push(format!("Claude caches tools, system and messages in this order, so this misses from token 0; {price}"));
        from_zero = true;
    }
    if let Some(s) = &f.shape {
        v.notes
            .push(format!("shape {}", &s.hash[..s.hash.len().min(12)]));
        let line = ttl_line(now_ms, s.at_ms);
        match ttl_verdict(now_ms, s.at_ms) {
            Ttl::Warm => v.notes.push(line),
            Ttl::MaybeWarm => v.reasons.push(line),
            Ttl::Cold => {
                v.reasons.push(format!("{line}; {price}"));
                from_zero = true;
            }
        }
    }
    if from_zero {
        v.keeps = false;
        v.lost_tokens = v.lost_tokens.max(f.prefix_tokens);
    }
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
