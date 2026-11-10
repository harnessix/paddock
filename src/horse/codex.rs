//! Codex 0.166.0 and 0.167.0 legacy rollout codec. Complete rows stay byte exact.
pub mod notify;
mod project;
pub mod swap;
pub(crate) mod writer_lock;
use super::store::Origin;
use super::{Binding, Store};
use anyhow::{bail, Context as _, Result};
pub use project::{project, Projection};
use serde_json::Value;
use std::path::Path;

pub const VERSION: &str = "0.166.0";
pub const HARNESS: &str = "codex";
pub const CONTAINER: &str = "codex-rollout-jsonl";
const ORIGIN: Origin = Origin {
    harness: HARNESS,
    container: CONTAINER,
    version: VERSION,
};

pub fn check_version(v: &str) -> Result<()> {
    if !matches!(v, VERSION | "0.167.0") {
        bail!("Codex {v} is not supported; expected 0.166.0 or 0.167.0");
    }
    Ok(())
}
fn parse(raw: &[u8]) -> Result<Value> {
    let v: Value = serde_json::from_slice(raw).context("invalid Codex row")?;
    if !v.is_object() || !v.get("type").is_some_and(Value::is_string) {
        bail!("invalid Codex row type");
    }
    if v["type"] == "session_meta" {
        check_version(
            v["payload"]["cli_version"]
                .as_str()
                .context("missing Codex version")?,
        )?;
        if v["payload"]["history_mode"] == "paginated" || !v["payload"]["history_base"].is_null() {
            bail!("paginated rollouts are not supported");
        }
    }
    Ok(v)
}
mod mutation;
use mutation::check as check_mutation_row;
pub(crate) use mutation::{
    check_file as check_delivery_file, check_session as check_mutation_session,
};
fn key(v: &Value, index: usize) -> String {
    if let Some(n) = v.get("ordinal").and_then(Value::as_u64) {
        return format!("ordinal:{n}");
    }
    if v["type"] == "response_item" {
        if let Some(id) = v["payload"]["id"].as_str() {
            return format!("response:{id}");
        }
    }
    format!("row:{index}")
}
fn read(path: &Path) -> Result<(Vec<Vec<u8>>, bool)> {
    let bytes = std::fs::read(path)?;
    let (rows, torn) = super::jsonl::lines(&bytes);
    for r in &rows {
        parse(r)?;
    }
    Ok((rows, torn))
}
#[derive(Debug)]
pub struct Ingest {
    pub session: String,
    pub added: usize,
    pub kept: usize,
    pub torn: bool,
}
pub fn ingest(store: &Store, path: &Path, version: &str, horse: Option<&str>) -> Result<Ingest> {
    ingest_locked(&store.lock()?, path, version, horse)
}

/// Ingest while holding the store writer lock.
pub fn ingest_locked(
    locked: &super::store::Lock<'_>,
    path: &Path,
    version: &str,
    horse: Option<&str>,
) -> Result<Ingest> {
    ingest_impl(locked, path, version, horse, false)
}

pub(crate) fn ingest_run_locked(
    locked: &super::store::Lock<'_>,
    path: &Path,
    version: &str,
    horse: &str,
) -> Result<Ingest> {
    ingest_impl(locked, path, version, Some(horse), true)
}

fn ingest_impl(
    locked: &super::store::Lock<'_>,
    path: &Path,
    version: &str,
    horse: Option<&str>,
    scoped: bool,
) -> Result<Ingest> {
    let store = locked.store;
    check_version(version)?;
    let (rows, torn) = read(path)?;
    let first = parse(rows.first().context("no complete session metadata")?)?;
    if first["type"] != "session_meta" {
        bail!("first row is not session metadata");
    }
    let producer = first["payload"]["cli_version"]
        .as_str()
        .context("missing Codex producer version")?;
    let id = first["payload"]["id"]
        .as_str()
        .context("missing thread id")?;
    super::claude::check_session_id(id)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("bad rollout name")?;
    if !name.starts_with("rollout-") || !name.ends_with(&format!("{id}.jsonl")) {
        bail!("rollout name does not match thread id");
    }
    let relative = path.ancestors().take(5).collect::<Vec<_>>();
    if relative.len() < 5 {
        bail!("rollout path needs date directories");
    }
    let rel = path
        .strip_prefix(relative[4])?
        .to_str()
        .context("bad rollout path")?;
    let active = store
        .read_ref(&format!("codex/{id}.json"))?
        .map(String::from_utf8)
        .transpose()?;
    let active = if scoped { None } else { active };
    let session = match active {
        Some(s) => store.session(&s)?,
        None => match if scoped {
            store.sessions()?.into_iter().find(|s| {
                s.horse == horse.unwrap_or_default()
                    && s.binding.harness == HARNESS
                    && s.binding.native_id.as_deref() == Some(id)
            })
        } else {
            store.session_by_native(HARNESS, id)?
        } {
            Some(s) => s,
            None => {
                let h = match horse {
                    Some(h) => store.horse(h)?,
                    None => super::dna::new_default_horse_locked(locked)?,
                };
                locked.new_session(
                    &h.id,
                    Binding {
                        harness: HARNESS.into(),
                        native_id: Some(id.into()),
                        version: producer.into(),
                        container: CONTAINER.into(),
                    },
                )?
            }
        },
    };
    check_version(&session.binding.version)?;
    if session.binding.harness != HARNESS || session.binding.container != CONTAINER {
        bail!("invalid Codex binding");
    }
    let old = store.session_nodes(&session)?;
    let common = old
        .iter()
        .zip(&rows)
        .take_while(|(n, r)| n.raw == **r)
        .count();
    if common < old.len().min(rows.len()) {
        bail!("Codex rollout changed before its tail");
    }
    if rows.len() < old.len() {
        bail!("Codex rollout was truncated");
    }
    let mut head = session.head.clone();
    let scoped = store.for_horse(&session.horse)?;
    let origin = Origin { version, ..ORIGIN };
    for (i, raw) in rows.iter().enumerate().skip(common) {
        head = Some(scoped.put_native(&origin, head.as_ref(), &key(&parse(raw)?, i), raw)?);
    }
    let header = scoped.put_blob("codex-path", rel.as_bytes())?;
    locked.update_session(&session.id, |s| {
        s.head = head;
        s.binding.version = producer.into();
        s.codex_reader_version = Some(version.into());
        s.native_header = Some(header);
    })?;
    Ok(Ingest {
        session: session.id,
        added: rows.len() - common,
        kept: common,
        torn,
    })
}
#[cfg(test)]
mod oracle_tests;
#[cfg(test)]
mod rewrite_tests;
#[cfg(test)]
mod tests;
