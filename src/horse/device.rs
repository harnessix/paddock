//! A sandbox's explicitly granted view of one horse.
use super::{claude, dsh, Hash, Store};
use anyhow::{bail, Result};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Dir,
    File(Vec<u8>),
}
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub paths: BTreeMap<String, Entry>,
}
#[derive(Clone)]
pub struct Access {
    pub store: Store,
    pub horse: String,
    pub session: String,
}

impl Access {
    pub fn tree(&self) -> Result<Tree> {
        build(
            &self.store,
            &self.horse,
            &Grants {
                sessions: super::hitch::visible_sessions(&self.store, &self.horse, &self.session),
            },
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Grants {
    pub sessions: Vec<String>,
}

impl Tree {
    fn dir(&mut self, path: &str) {
        self.paths.insert(path.into(), Entry::Dir);
    }
    fn file(&mut self, path: &str, value: impl Into<Vec<u8>>) {
        self.paths.insert(path.into(), Entry::File(value.into()));
    }
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
}
fn memory_dir(horse: &str) -> String {
    format!("memory/{horse}")
}
/// Build only the granted sessions, checking ownership independently of grants.
pub fn build(store: &Store, horse: &str, grants: &Grants) -> Result<Tree> {
    let h = store.horse(horse)?;
    let mut t = Tree::default();
    for d in ["self", "self/sessions", "self/memory"] {
        t.dir(d);
    }
    t.file("self/id", format!("{}\n", h.id));
    t.file("self/dna", format!("{}\n", h.dna));
    // A horse from before DNA objects may have a DNA that does not read.
    if let Ok(dna) = super::dna::get(store, &h.dna) {
        t.file(DNA_PATH, dna.encode());
    }
    metadata::add(store, &h, &mut t)?;
    let mut lineage = String::new();
    for id in &h.sessions {
        if !grants.sessions.contains(id) {
            continue;
        }
        let s = store.session(id)?;
        if s.horse != h.id {
            continue;
        }
        let base = format!("self/sessions/{id}");
        t.dir(&base);
        t.file(
            &format!("{base}/head"),
            format!("{}\n", s.head.as_ref().map_or("", Hash::as_str)),
        );
        t.file(
            &format!("{base}/binding"),
            format!("{}\n", serde_json::to_string(&s.binding)?),
        );
        let inbox = store.inbox(id)?;
        let verdict = store
            .guard_state(id)?
            .last
            .map(|v| v.message())
            .unwrap_or_default();
        t.file(
            &format!("{base}/status"),
            format!(
                "guard: {verdict}\ninbox: {}\nnative id: {}\n",
                inbox.len(),
                s.binding.native_id.as_deref().unwrap_or("")
            ),
        );
        let mut log = String::new();
        let mut jsonl = String::new();
        for n in store.session_nodes(&s)? {
            let view = match s.binding.harness.as_str() {
                "dsh" => dsh::view(&n.raw),
                "claude" => claude::view(&n.raw),
                _ => None,
            };
            if let Some(v) = view {
                let text: String = v.text.chars().take(120).collect();
                log.push_str(&format!(
                    "{} {} {}\n",
                    n.hash.short(),
                    v.role,
                    text.replace('\n', " ")
                ));
                jsonl.push_str(&format!(
                    "{}\n",
                    serde_json::json!({"hash": n.hash.as_str(), "role": v.role, "text": v.text})
                ));
            }
        }
        t.file(&format!("{base}/log"), log);
        t.file(&format!("{base}/log.jsonl"), jsonl);
        let mut pending = String::new();
        for e in inbox {
            pending.push_str(&format!("{}\t{}\n", e.role, e.text));
        }
        t.file(&format!("{base}/inbox"), pending);
        for step in s.lineage {
            lineage.push_str(&format!(
                "{} {} {} {}\n",
                id, step.horse, step.dna, step.since_ms
            ));
        }
    }
    t.file("self/lineage", lineage);
    let dir = memory_dir(horse);
    if store.root().join(&dir).is_dir() {
        for name in store.list_refs(&dir)? {
            if !valid_name(&name) {
                continue;
            }
            if let Some(bytes) = store.read_ref(&format!("{dir}/{name}"))? {
                let hash = Hash::parse(std::str::from_utf8(&bytes)?.trim())?;
                t.file(&format!("self/memory/{name}"), store.get(&hash)?.raw);
            }
        }
    }
    Ok(t)
}
/// The writable DNA of the horse.
pub const DNA_PATH: &str = "self/dna.toml";

/// Whether a sandbox may write this path of the tree.
pub fn writable(path: &str) -> bool {
    path.starts_with("self/memory/") || path == DNA_PATH
}

/// A write of `self/dna.toml`: it hot-loads (`dna::hot_load`). The caller
/// checks the text with `dna::parse` first, so an invalid text stores
/// nothing.
pub fn write_dna(store: &Store, horse: &str, bytes: &[u8]) -> Result<Option<Hash>> {
    let text = std::str::from_utf8(bytes)?;
    super::dna::hot_load(store, horse, text)
}

/// Atomically replace a horse's named memory blob.
pub fn write_memory(store: &Store, horse: &str, name: &str, bytes: &[u8]) -> Result<()> {
    if !valid_name(name) {
        bail!("invalid memory name");
    }
    let _lock = store.lock()?;
    store.horse(horse)?;
    let hash = store.put_blob("text/plain", bytes)?;
    store.write_ref(
        &format!("{}/{name}", memory_dir(horse)),
        hash.as_str().as_bytes(),
    )
}

mod metadata;
pub mod registry;

#[cfg(test)]
mod tests;

/// Broker requests are bound to the setup belonging to this sandbox.
pub(crate) fn handle(
    setup: Option<&Access>,
    req: &crate::broker::Request,
) -> crate::broker::Response {
    use crate::broker::{Response, Status, PROTOCOL};
    let result = (|| -> Result<serde_json::Value> {
        if req.v != PROTOCOL {
            bail!("unsupported protocol version");
        }
        let setup = setup.ok_or_else(|| anyhow::anyhow!("this sandbox has no horse"))?;
        let op = req.args.get("op").and_then(|v| v.as_str()).unwrap_or("");
        if op == "dna_put" {
            let bytes: Vec<u8> = serde_json::from_value(
                req.args
                    .get("bytes")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing DNA bytes"))?,
            )?;
            let to = write_dna(&setup.store, &setup.horse, &bytes)?;
            let msg = match to {
                Some(h) => format!(
                    "the DNA of this horse is now {h}; it hot-loads at the next safe point\n"
                ),
                None => "the DNA did not change\n".into(),
            };
            return Ok(serde_json::json!({"bytes": msg.into_bytes()}));
        }
        if op == "put" {
            let name = req
                .args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing memory name"))?;
            let bytes: Vec<u8> = serde_json::from_value(
                req.args
                    .get("bytes")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("missing memory bytes"))?,
            )?;
            write_memory(&setup.store, &setup.horse, name, &bytes)?;
            return Ok(serde_json::json!({"bytes": []}));
        }
        let tree = setup.tree()?;
        let session = req
            .args
            .get("session")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let bytes = if op == "sessions" {
            let mut out = String::new();
            for (path, entry) in &tree.paths {
                if let Some(id) = path.strip_prefix("self/sessions/") {
                    if !id.contains('/') && *entry == Entry::Dir {
                        out.push_str(id);
                        out.push('\n');
                    }
                }
            }
            out.into_bytes()
        } else {
            let path = match op {
                "lineage" => "self/lineage".to_string(),
                "dna" => DNA_PATH.to_string(),
                "log" | "inbox" => format!("self/sessions/{session}/{op}"),
                _ => bail!("unknown horse operation"),
            };
            match tree.paths.get(&path) {
                Some(Entry::File(bytes)) => bytes.clone(),
                _ => bail!("no such horse path"),
            }
        };
        let bytes = if op == "log" {
            if let Some(n) = req.args.get("last").and_then(|v| v.as_u64()) {
                let lines: Vec<&[u8]> = bytes.split_inclusive(|b| *b == b'\n').collect();
                lines[lines.len().saturating_sub(n as usize)..].concat()
            } else {
                bytes
            }
        } else {
            bytes
        };
        Ok(serde_json::json!({"bytes": bytes}))
    })();
    match result {
        Ok(v) => Response::done(v),
        Err(e) => Response::new(Status::Failed, format!("{e:#}")),
    }
}
