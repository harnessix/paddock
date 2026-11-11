//! Deliberate, scoped reads of session content through the host broker.

pub mod grant;
mod pattern;
pub use grant::{issue_own, revoke};

use super::Access;
#[cfg(any(target_os = "linux", test))]
use super::Tree;
use crate::broker::{Request, Response, Status, PROTOCOL};
use crate::horse::{store::IndexedNode, Session};
use anyhow::{bail, Context as _, Result};

const CHUNK: usize = 128 * 1024;
const MAX_RESULT: usize = 64 * 1024 * 1024;

/// Add trainer-grant files to the mount. Their independent keys remain in
/// the encrypted grant record on the host and are never returned to the run.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn add_mount(access: &Access, sandbox: &str, tree: &mut Tree) -> Result<()> {
    tree.dir("peek");
    for grant in grant::active(&access.store, sandbox)? {
        if grant.id == "own" {
            continue;
        }
        let session = access.store.session(&grant.session)?;
        if session.horse != grant.horse {
            continue;
        }
        let dir = format!("peek/{}", grant.id);
        tree.dir(&dir);
        let key = crate::horse::keys::Key::from_bytes(&grant.mount_key)?;
        let nodes = access
            .store
            .indexed_chain(session.head.as_ref(), u64::MAX, &mut 0)?;
        for (index, node) in nodes.iter().enumerate() {
            let path = format!("{dir}/{}", node_name(index, &node.kind, node.size));
            let context = format!("paddock trainer grant\0{sandbox}\0{}\0{path}", grant.id);
            let ciphertext = access.store.object_ciphertext(&node.hash)?;
            let bytes = crate::horse::crypto::seal(&key, context.as_bytes(), &ciphertext)?;
            tree.file(&path, bytes);
        }
    }
    Ok(())
}

pub(super) fn node_name(index: usize, kind: &str, size: u64) -> String {
    let kind = if kind.len() <= 32 && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
        kind
    } else {
        "other"
    };
    format!("{index:06}-{kind}-{size}")
}

fn select<'a>(
    nodes: &'a [IndexedNode],
    session: &str,
    grant_id: &str,
    selector: &str,
) -> Result<(usize, &'a IndexedNode)> {
    let name = if selector.contains('/') {
        let own = format!("/dev/paddock/horse/self/sessions/{session}/nodes/");
        let relative = format!("self/sessions/{session}/nodes/");
        let trainer = format!("/dev/paddock/horse/peek/{grant_id}/");
        let trainer_relative = format!("peek/{grant_id}/");
        selector
            .strip_prefix(&own)
            .or_else(|| selector.strip_prefix(&relative))
            .or_else(|| selector.strip_prefix(&trainer))
            .or_else(|| selector.strip_prefix(&trainer_relative))
            .context("node path is outside this grant")?
    } else {
        selector
    };
    if name.contains('/') || name.is_empty() {
        bail!("invalid node selector");
    }
    if let Ok(index) = name.parse::<usize>() {
        return nodes
            .get(index)
            .map(|node| (index, node))
            .context("node sequence is out of range");
    }
    let mut matches = nodes.iter().enumerate().filter(|(i, node)| {
        node_name(*i, &node.kind, node.size) == name
            || (name.len() >= 6 && node.hash.as_str().starts_with(name))
    });
    let first = matches
        .next()
        .context("no such node in the granted session")?;
    if matches.next().is_some() {
        bail!("node selector is ambiguous");
    }
    Ok(first)
}

fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    if out
        .len()
        .checked_add(bytes.len())
        .is_none_or(|n| n > MAX_RESULT)
    {
        bail!("peek result is too large; select fewer nodes");
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn selected<'a>(
    nodes: &'a [IndexedNode],
    session: &str,
    grant_id: &str,
    selectors: &[String],
) -> Result<Vec<(usize, &'a IndexedNode)>> {
    selectors
        .iter()
        .map(|s| select(nodes, session, grant_id, s))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render(
    access: &Access,
    target: &Session,
    grant_id: &str,
    operation: &str,
    args: &[String],
    budget: u64,
    inspected: &mut u64,
) -> Result<(Vec<u8>, bool)> {
    if operation == "dna" {
        if !args.is_empty() {
            bail!("dna takes no arguments");
        }
        let horse = access.store.horse(&target.horse)?;
        let size = access.store.object_stored_size(&horse.dna)?;
        if size > budget {
            bail!("peek grant byte limit reached");
        }
        *inspected = size;
        let bytes = crate::horse::dna::get(&access.store, &horse.dna)?
            .encode()
            .into_bytes();
        return Ok((bytes, true));
    }
    if operation == "inbox" {
        if !args.is_empty() || grant_id != "own" {
            bail!("inbox is available only for the run's own grant");
        }
        let dir = format!("inbox/{}", target.id);
        let mut out = Vec::new();
        if access.store.root().join(&dir).is_dir() {
            for name in access.store.list_refs(&dir)? {
                if !name.ends_with(".json") {
                    continue;
                }
                let rel = format!("{dir}/{name}");
                if let Some(bytes) = access.store.read_ref_bounded(&rel, budget, inspected)? {
                    append(&mut out, &bytes)?;
                    append(&mut out, b"\n")?;
                }
            }
        }
        return Ok((out, true));
    }
    if operation == "binding" {
        if !args.is_empty() {
            bail!("binding takes no arguments");
        }
        let mut bytes = serde_json::to_vec(&target.binding)?;
        bytes.push(b'\n');
        *inspected = u64::try_from(bytes.len())?;
        if *inspected > budget {
            *inspected = 0;
            bail!("peek grant byte limit reached");
        }
        return Ok((bytes, true));
    }
    let nodes = access
        .store
        .indexed_chain(target.head.as_ref(), budget, inspected)?;
    let mut out = Vec::new();
    match operation {
        "ls" => {
            if !args.is_empty() {
                bail!("ls takes no arguments");
            }
            for (i, node) in nodes.iter().enumerate() {
                let time = node.time.unwrap_or(target.created_ms);
                append(
                    &mut out,
                    format!(
                        "{}\t{}\t{}\t{}\t{}\n",
                        node_name(i, &node.kind, node.size),
                        node.kind,
                        node.size,
                        time,
                        node.hash
                    )
                    .as_bytes(),
                )?;
            }
            Ok((out, true))
        }
        "cat" => {
            if args.is_empty() {
                bail!("cat needs one or more node selectors");
            }
            for (_, node) in selected(&nodes, &target.id, grant_id, args)? {
                let content = access.store.get_bounded(&node.hash, budget, inspected)?;
                append(&mut out, &content.raw)?;
            }
            Ok((out, true))
        }
        "range" => {
            if args.len() != 2 {
                bail!("range needs FROM and TO sequence numbers");
            }
            let from: usize = args[0].parse().context("invalid range start")?;
            let to: usize = args[1].parse().context("invalid range end")?;
            if from > to || to >= nodes.len() {
                bail!("range is outside the granted session");
            }
            for node in &nodes[from..=to] {
                let content = access.store.get_bounded(&node.hash, budget, inspected)?;
                append(&mut out, &content.raw)?;
                if !content.raw.ends_with(b"\n") {
                    append(&mut out, b"\n")?;
                }
            }
            Ok((out, true))
        }
        "grep" => {
            let pattern = args.first().context("grep needs a pattern")?;
            if pattern.is_empty() || pattern.len() > 1024 {
                bail!("grep pattern must contain 1 to 1024 bytes");
            }
            let regex = pattern::Pattern::compile(pattern)?;
            let chosen = if args.len() == 1 {
                nodes.iter().enumerate().collect()
            } else {
                selected(&nodes, &target.id, grant_id, &args[1..])?
            };
            let mut found = false;
            for (index, node) in chosen {
                let content = access.store.get_bounded(&node.hash, budget, inspected)?;
                let text = String::from_utf8_lossy(&content.raw);
                for (line, value) in text.lines().enumerate() {
                    if regex.matches(value) {
                        found = true;
                        append(
                            &mut out,
                            format!(
                                "{}:{}:{value}\n",
                                node_name(index, &node.kind, node.size),
                                line + 1
                            )
                            .as_bytes(),
                        )?;
                    }
                }
            }
            Ok((out, found))
        }
        _ => bail!("unknown peek operation"),
    }
}

pub(crate) fn handle(access: Option<&Access>, sandbox: &str, req: &Request) -> Response {
    let result = (|| -> Result<serde_json::Value> {
        if req.v != PROTOCOL {
            bail!("unsupported protocol version");
        }
        let access = access.context("this run has no horse")?;
        let id = req
            .args
            .get("grant")
            .and_then(|v| v.as_str())
            .unwrap_or("own");
        let operation = req
            .args
            .get("command")
            .and_then(|v| v.as_str())
            .context("missing peek command")?;
        let args: Vec<String> = serde_json::from_value(
            req.args
                .get("args")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )?;
        if args.len() > 256
            || args.iter().any(|a| a.len() > 4096)
            || args.iter().map(String::len).sum::<usize>() > 16 * 1024
        {
            bail!("peek arguments are too large");
        }
        let offset = req.args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        let offset = usize::try_from(offset)?;
        if grant::get(&access.store, sandbox, id)?.is_none() {
            bail!("no peek grant for this run");
        }
        let guard = grant::lock(&access.store, sandbox, id)?;
        let grant =
            grant::get(&access.store, sandbox, id)?.context("no peek grant for this run")?;
        grant::check(&grant, sandbox, id)?;
        let budget = grant::remaining(&grant)?;
        if id == "own" && (grant.horse != access.horse || grant.session != access.session) {
            bail!("peek grant does not match this run");
        }
        let session = access.store.session(&grant.session)?;
        if session.horse != grant.horse {
            bail!("peek grant target moved to another horse");
        }
        let snapshot = session.head.as_ref().map_or("", |hash| hash.as_str());
        if req
            .args
            .get("snapshot")
            .and_then(|value| value.as_str())
            .is_some_and(|expected| expected != snapshot)
        {
            bail!("session changed during peek; retry the read");
        }
        let mut inspected = 0;
        let result = render(
            access,
            &session,
            id,
            operation,
            &args,
            budget,
            &mut inspected,
        )
        .and_then(|(out, found)| {
            if offset > out.len() {
                bail!("peek offset is out of range");
            }
            let charged = inspected.max(u64::try_from(out.len())?);
            if charged > budget {
                bail!("peek grant byte limit reached");
            }
            if access.store.session(&grant.session)?.horse != grant.horse {
                bail!("peek grant target moved to another horse");
            }
            let fingerprint = crate::horse::Hash::of(&out);
            if req
                .args
                .get("fingerprint")
                .and_then(|value| value.as_str())
                .is_some_and(|expected| expected != fingerprint.as_str())
            {
                bail!("peek result changed during read; retry it");
            }
            Ok((out, found, charged))
        });
        let (out, found, charged) = match result {
            Ok(result) => result,
            Err(error) => {
                grant::record(
                    &access.store,
                    &guard,
                    sandbox,
                    id,
                    &grant.horse,
                    &grant.session,
                    operation,
                    args,
                    inspected,
                    "failed",
                )?;
                return Err(error);
            }
        };
        let end = out.len().min(offset.saturating_add(CHUNK));
        let fingerprint = crate::horse::Hash::of(&out);
        let chunk = &out[offset..end];
        grant::record(
            &access.store,
            &guard,
            sandbox,
            id,
            &grant.horse,
            &grant.session,
            operation,
            args,
            charged,
            "done",
        )?;
        Ok(
            serde_json::json!({"bytes": chunk, "next": end, "done": end == out.len(), "found": found, "snapshot": snapshot, "fingerprint": fingerprint.as_str()}),
        )
    })();
    match result {
        Ok(value) => Response::done(value),
        Err(error) => Response::new(Status::Failed, format!("{error:#}")),
    }
}

#[cfg(test)]
#[path = "peek/tests.rs"]
mod tests;
