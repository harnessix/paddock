//! The request shape that Claude Code records for each API request.
//!
//! Claude Code 2.1.317 appends an `api-request` row to the transcript after
//! each main-thread request: `timestamp` (RFC 3339), `shapeHash` (the
//! sha256 of the system prompt and the tool list, without their
//! `cache_control`), `params` (the other request fields, `model` among
//! them) and `cacheControl` (one `{path, value}` per cache breakpoint, the
//! path from the request root, for example `["messages", 4, "content", 0]`).
//! A subagent request has an `agentId` and goes to the file of the agent;
//! a row with an `agentId` in the main file is not a main-thread request.
//! `api-request-shape` and `api-request-blob` rows carry the content and
//! are not requests.

use super::super::store::Node;
use serde_json::Value;

/// The row type of a request record.
pub const API_REQUEST: &str = "api-request";

/// One recorded request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    /// `shapeHash`: the system prompt and the tools, as Claude Code hashes them.
    pub hash: String,
    /// The request time, in ms since the Unix epoch.
    pub at_ms: u64,
    /// The message indexes of the cache breakpoints in `messages`, in
    /// order. Breakpoints in the system prompt or the tools are not in it.
    pub breakpoints: Vec<u64>,
    /// `params.model`, when the row has it.
    pub model: Option<String>,
}

/// The row is an `api-request` row (not `api-request-shape` or `-blob`).
pub fn row_is_api_request(raw: &[u8]) -> bool {
    serde_json::from_slice::<Value>(raw)
        .is_ok_and(|v| v.get("type").and_then(Value::as_str) == Some(API_REQUEST))
}

/// The shape of a main-thread `api-request` row. A row without a
/// `shapeHash` or with a bad `timestamp` gives `None`.
pub fn parse(raw: &[u8]) -> Option<Shape> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    if v.get("type").and_then(Value::as_str) != Some(API_REQUEST) || v.get("agentId").is_some() {
        return None;
    }
    let hash = v.get("shapeHash").and_then(Value::as_str)?.to_string();
    if hash.is_empty() {
        return None;
    }
    let at_ms = time_ms(v.get("timestamp").and_then(Value::as_str)?)?;
    let mut breakpoints: Vec<u64> = v
        .get("cacheControl")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let p = b.get("path")?.as_array()?;
            (p.first()?.as_str()? == "messages").then(|| p.get(1)?.as_u64())?
        })
        .collect();
    breakpoints.sort_unstable();
    breakpoints.dedup();
    let model = v
        .pointer("/params/model")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(Shape {
        hash,
        at_ms,
        breakpoints,
        model,
    })
}

/// The last main-thread `api-request` of a chain (root first).
pub fn latest(nodes: &[Node]) -> Option<Shape> {
    nodes.iter().rev().find_map(|n| parse(&n.raw))
}

/// An RFC 3339 time in ms. Claude Code writes `toISOString()`, for example
/// `2026-11-04T10:00:00.250Z`.
fn time_ms(s: &str) -> Option<u64> {
    let secs = crate::waiting::parse_rfc3339(s).ok()?;
    let frac = s
        .get(19..)
        .and_then(|r| r.strip_prefix('.'))
        .map(|r| r.bytes().take_while(u8::is_ascii_digit).collect::<Vec<_>>())
        .unwrap_or_default();
    let ms = frac
        .iter()
        .chain(b"000")
        .take(3)
        .fold(0u64, |a, d| a * 10 + u64::from(d - b'0'));
    secs.checked_mul(1000)?.checked_add(ms)
}

#[cfg(test)]
#[path = "shape_tests.rs"]
pub(crate) mod tests;
