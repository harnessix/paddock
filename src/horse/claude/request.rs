//! What Claude Code loads from a transcript, rebuilt offline.
//!
//! Claude Code is closed source, so this is a model of the loader and not a
//! port. From the docs and the 2.1.317 binary: the loader takes the last
//! `last-prompt.leafUuid` as the active leaf (else the last chain row),
//! follows `parentUuid` back to a row whose `parentUuid` is null (the root
//! or a `compact_boundary`), and sends the `message` of the rows on that
//! path in order. `content-replacement` rows replace large tool results in
//! the request, and `memory-mode`, `relocated`, `atis-latch` and
//! `tools-used` carry state. The system prompt and the tool list come from
//! the client: a fork without `api-request*` rows gets them as a native
//! `/branch` fork does. Two transcripts with the same `Loaded` give the
//! same message prefix when the same client resumes them in the same cwd.

use super::{info, json, Info};
use anyhow::{bail, Context as _, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// State rows that the loader keeps. `content-replacement` rows add up,
/// the others count by their last row.
pub const ACCUMULATE: [&str; 1] = ["content-replacement"];
pub const LAST_WINS: [&str; 4] = ["memory-mode", "relocated", "atis-latch", "tools-used"];

/// One message on the active path: the row type, the uuid and the raw
/// bytes of its `message` value (or of the whole row without session
/// fields when it has no message, for example a compact boundary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Msg {
    pub ty: String,
    pub uuid: String,
    pub message: Vec<u8>,
}

/// What the loader sees in a transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub leaf: String,
    pub path: Vec<Msg>,
    /// The state rows, each without `sessionId`, in the order they count.
    pub state: Vec<Vec<u8>>,
}

/// The fields that name the session or the file, not the conversation.
const SESSION_FIELDS: [&str; 6] = [
    "sessionId",
    "parentUuid",
    "isSidechain",
    "sessionKind",
    "forkedFrom",
    "leafUuid",
];

/// The active leaf: the last `last-prompt.leafUuid` that names a chain
/// row, else the last main-thread chain row.
pub fn active_leaf(rows: &[(Info, &[u8])]) -> Option<String> {
    let uuids: HashSet<&str> = rows
        .iter()
        .filter(|(n, _)| n.is_chain())
        .filter_map(|(n, _)| n.uuid.as_deref())
        .collect();
    rows.iter()
        .rev()
        .find_map(|(n, _)| {
            (n.ty == "last-prompt")
                .then_some(n.leaf.as_deref())
                .flatten()
                .filter(|l| uuids.contains(l))
        })
        .or_else(|| {
            rows.iter()
                .rev()
                .find(|(n, _)| n.is_chain() && !n.sidechain && n.ty != "progress")
                .and_then(|(n, _)| n.uuid.as_deref())
        })
        .map(str::to_string)
}

/// The indexes of the rows from the root (or the last boundary) to
/// `leaf`, root first. A broken chain is an error.
pub fn path_to(rows: &[(Info, &[u8])], leaf: &str) -> Result<Vec<usize>> {
    let by_uuid: HashMap<&str, usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| n.is_chain())
        .map(|(i, (n, _))| (n.uuid.as_deref().expect("chain row"), i))
        .collect();
    let mut out = Vec::new();
    let mut next = Some(leaf.to_string());
    while let Some(u) = next {
        let i = *by_uuid
            .get(u.as_str())
            .with_context(|| format!("the chain breaks at {u}: no row has this uuid"))?;
        if out.contains(&i) {
            bail!("the chain has a loop at {u}");
        }
        out.push(i);
        next = rows[i].0.parent.clone();
    }
    out.reverse();
    Ok(out)
}

/// The state rows of the main thread, without their session fields.
pub fn state(rows: &[(Info, &[u8])]) -> Result<Vec<Vec<u8>>> {
    let mut acc = Vec::new();
    let mut last: Vec<(&str, Vec<u8>)> = Vec::new();
    for (n, raw) in rows {
        if n.agent_id.is_some() {
            continue;
        }
        let clean = || json::edit(raw, &[], &SESSION_FIELDS);
        if ACCUMULATE.contains(&n.ty.as_str()) {
            acc.push(clean()?);
        } else if let Some(t) = LAST_WINS.iter().find(|t| **t == n.ty) {
            last.retain(|(k, _)| k != t);
            last.push((t, clean()?));
        }
    }
    last.sort_by_key(|(k, _)| LAST_WINS.iter().position(|t| t == k));
    acc.extend(last.into_iter().map(|(_, r)| r));
    Ok(acc)
}

/// Load a transcript as the client would.
pub fn load(rows: &[Vec<u8>]) -> Result<Loaded> {
    let parsed: Vec<(Info, &[u8])> = rows
        .iter()
        .map(|r| Ok((info(r)?, r.as_slice())))
        .collect::<Result<_>>()?;
    let leaf = active_leaf(&parsed).context("the transcript has no chain rows")?;
    let mut path = Vec::new();
    for i in path_to(&parsed, &leaf)? {
        let (n, raw) = &parsed[i];
        let message = match json::raw_value(raw, "message")? {
            Some(m) => m.to_vec(),
            None => json::edit(raw, &[], &SESSION_FIELDS)?,
        };
        path.push(Msg {
            ty: n.ty.clone(),
            uuid: n.uuid.clone().expect("chain row"),
            message,
        });
    }
    Ok(Loaded {
        leaf,
        path,
        state: state(&parsed)?,
    })
}

/// The record types of a transcript, for the check that a fork carries
/// what a native fork carries and nothing more.
pub fn types(rows: &[Vec<u8>]) -> Result<Vec<String>> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| Ok(info(r)?.ty))
        .collect::<Result<_>>()?;
    out.sort();
    out.dedup();
    Ok(out)
}

/// The token counts of the assistant rows. One API response can span rows
/// that share `message.id`: it counts once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub requests: u64,
    pub input: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub output: u64,
}

pub fn usage(rows: &[Vec<u8>]) -> Usage {
    let mut seen = HashSet::new();
    let mut u = Usage::default();
    for r in rows {
        let Ok(v) = serde_json::from_slice::<Value>(r) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(m) = v.get("message") else { continue };
        let id = m
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !id.is_empty() && !seen.insert(id) {
            continue;
        }
        let Some(x) = m.get("usage") else { continue };
        let n = |k: &str| x.get(k).and_then(Value::as_u64).unwrap_or(0);
        u.requests += 1;
        u.input += n("input_tokens");
        u.cache_creation += n("cache_creation_input_tokens");
        u.cache_read += n("cache_read_input_tokens");
        u.output += n("output_tokens");
    }
    u
}
