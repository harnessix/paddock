//! A chain of Grok nodes back to the two files, and the turn of each row.

use super::{parse_update, CHAT, HARNESS, UPDATES};
use crate::horse::store::{Hash, Node};
use anyhow::{bail, Context as _, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// The files that a chain holds.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Files {
    /// The rows of `updates.jsonl`, without their newline.
    pub updates: Vec<Vec<u8>>,
    /// The items of `chat_history.jsonl`, without their newline.
    pub chat: Vec<Vec<u8>>,
    /// The key of each chat item.
    pub chat_keys: Vec<String>,
}

/// What paddock reads of a chat item.
#[derive(Debug, Default, Clone)]
pub(crate) struct Chat {
    pub prompt_index: Option<u64>,
}

pub(crate) fn parse_chat(raw: &[u8]) -> Result<Chat> {
    let v: Value = serde_json::from_slice(raw).context("invalid Grok chat item")?;
    let Some(kind) = v["type"].as_str() else {
        bail!("a Grok chat item has no type");
    };
    Ok(Chat {
        prompt_index: (kind == "user")
            .then(|| v["prompt_index"].as_u64())
            .flatten(),
    })
}

/// The index node of a chat rewrite.
pub(crate) fn is_rewrite(n: &Node) -> bool {
    n.meta.container.as_deref() == Some(CHAT)
        && n.meta
            .key
            .as_deref()
            .is_some_and(|k| k.starts_with("rewrite:"))
}

/// The key of each chat item: its prompt index, the hash of its bytes, and
/// the number of equal items before it.
pub(crate) fn chat_keys(rows: &[Vec<u8>]) -> Result<Vec<String>> {
    let mut count: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let c = parse_chat(r)?;
        let pi = c.prompt_index.map_or("-".to_string(), |p| p.to_string());
        let base = format!("chat:{pi}:{}", &Hash::of(r).as_str()[..16]);
        let n = count.entry(base.clone()).or_default();
        out.push(format!("{base}:{n}"));
        *n += 1;
    }
    Ok(out)
}

/// The turn of each chat item: the `prompt_index` of the last user item
/// with one, at or before it, or -1.
pub(crate) fn chat_turns(rows: &[Vec<u8>]) -> Result<Vec<i64>> {
    let mut cur = -1;
    rows.iter()
        .map(|r| {
            if let Some(p) = parse_chat(r)?.prompt_index {
                cur = p as i64;
            }
            Ok(cur)
        })
        .collect()
}

/// For each updates row that opens a user prompt, the prompt index: its
/// `_meta.promptIndex`, or else the number of prompts before it.
pub(crate) fn update_opens(rows: &[Vec<u8>]) -> Result<Vec<Option<u64>>> {
    let mut out = Vec::with_capacity(rows.len());
    let mut in_user = false;
    let mut count = 0u64;
    for r in rows {
        let u = parse_update(r)?;
        if u.user && !in_user {
            out.push(Some(u.prompt_index.unwrap_or(count)));
            count += 1;
        } else {
            out.push(None);
        }
        in_user = u.user;
    }
    Ok(out)
}

/// The two files that a chain of Grok nodes holds.
pub fn replay(nodes: &[Node]) -> Result<Files> {
    let mut f = Files::default();
    let mut items: HashMap<String, Vec<u8>> = HashMap::new();
    let mut listed: HashSet<String> = HashSet::new();
    for n in nodes {
        if n.meta.harness.as_deref() != Some(HARNESS) {
            bail!("node {} is not a Grok row", n.hash.short());
        }
        let key = n.meta.key.clone().unwrap_or_default();
        match n.meta.container.as_deref() {
            Some(UPDATES) => f.updates.push(n.raw.clone()),
            Some(CHAT) if is_rewrite(n) => {
                f.chat_keys = serde_json::from_slice(&n.raw)
                    .with_context(|| format!("node {} is a bad chat index", n.hash.short()))?;
                listed = f.chat_keys.iter().cloned().collect();
            }
            Some(CHAT) => {
                items.insert(key.clone(), n.raw.clone());
                if listed.insert(key.clone()) {
                    f.chat_keys.push(key);
                }
            }
            _ => bail!("node {} is not a Grok row", n.hash.short()),
        }
    }
    for k in &f.chat_keys {
        let raw = items
            .get(k)
            .with_context(|| format!("the chat history of this chain misses item {k}"))?;
        f.chat.push(raw.clone());
    }
    Ok(f)
}

/// The bytes of a JSONL file of these rows.
pub fn jsonl(rows: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        out.extend_from_slice(r);
        out.push(b'\n');
    }
    out
}

/// One line about a Grok node for `paddock horse log`.
pub fn describe(n: &Node) -> String {
    if is_rewrite(n) {
        return "chat history rewritten".into();
    }
    let v: Value = serde_json::from_slice(&n.raw).unwrap_or(Value::Null);
    let text = |c: &Value| -> String {
        match c {
            Value::String(s) => s.clone(),
            Value::Array(a) => a
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join(" "),
            Value::Object(_) => c["text"].as_str().unwrap_or_default().into(),
            _ => String::new(),
        }
    };
    if n.meta.container.as_deref() == Some(UPDATES) {
        let u = &v["params"]["update"];
        let tag = u["sessionUpdate"].as_str().unwrap_or("?");
        return format!("update {tag}: {}", text(&u["content"]));
    }
    let ty = v["type"].as_str().unwrap_or("?");
    format!("chat {ty}: {}", text(&v["content"]))
}
