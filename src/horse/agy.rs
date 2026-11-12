//! agy (Antigravity CLI) 1.4.12 conversation codec.
//!
//! An agy conversation is one SQLite database
//! `conversations/<conversation id>.db` with protobuf step payloads, plus
//! its row in `conversation_summaries.db`. `transcript.jsonl` is derived
//! from the database, so paddock does not read it.
//!
//! agy rewrites rows in place: a running step is updated at the same
//! index, and a rewind truncates the steps from an index. So the identity
//! of a row is its table and its step index, and the store keeps every
//! version that it saw:
//! - a row of `steps`, `gen_metadata`, `executor_metadata` or
//!   `parent_references` is a node keyed `<table>:<idx>`, with every
//!   column and every blob byte exact (row.rs). The rows of one index come
//!   before its step row;
//! - a row at a known index with other bytes is a new node after the old
//!   one. The old node stays in the chain;
//! - rows that the database does not have any more (a truncation) are
//!   one drop node keyed `drop:<hash>` that lists them. Their nodes stay.
//!
//! `trajectory_meta`, `trajectory_metadata_blob`, `battle_mode_infos` and
//! the summaries row are the header blob of the session. Other agy
//! versions and other schemas are refused (fail closed).

pub(crate) mod db;
#[cfg(test)]
pub(crate) mod fixture;
mod project;
pub mod proto;
pub mod request;
pub mod row;

use super::store::{Hash, Lock, Node, Origin};
use super::{Binding, Store};
use anyhow::{bail, Context as _, Result};
use db::{Rows, IDX_TABLES, STEPS};
pub use project::{project, Projection};
use row::{decode_tables, encode_tables, Cell, Row, Tables};
use std::collections::BTreeMap;
use std::path::Path;

pub const VERSION: &str = "1.4.12";
pub const HARNESS: &str = "agy";
/// A conversation database with its summaries row.
pub const CONTAINER: &str = "agy-conversation-db";
/// A row of an idx-keyed table.
pub const ROW: &str = "agy-sqlite-row";
/// The rows that a database does not have any more.
pub const DROP: &str = "agy-sqlite-drop";
const HEADER_MEDIA: &str = "agy-header";
/// The first table of a header blob: the conversation id.
const ID_TABLE: &str = "paddock_conversation";

/// Step types of agy (`CORTEX_STEP_TYPE_*`) that paddock names.
pub const USER_INPUT: i64 = 14;
pub const PLANNER_RESPONSE: i64 = 15;
pub const CHECKPOINT: i64 = 23;
/// Step status DONE and ERROR.
pub const DONE: i64 = 3;
pub const ERROR: i64 = 7;

pub(crate) const fn origin(container: &'static str) -> Origin<'static> {
    Origin {
        harness: HARNESS,
        container,
        version: VERSION,
    }
}

pub fn check_version(v: &str) -> Result<()> {
    if v != VERSION {
        bail!("agy {v} is not supported; expected {VERSION}");
    }
    Ok(())
}

pub(crate) fn binding(native_id: Option<String>) -> Binding {
    Binding {
        harness: HARNESS.into(),
        native_id,
        version: VERSION.into(),
        container: CONTAINER.into(),
    }
}

/// The conversation header: the header tables and the summaries row.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Header {
    /// The conversation id of the database.
    pub id: String,
    pub tables: Tables,
    pub summary: Option<Row>,
}

impl Header {
    fn encode(&self) -> Vec<u8> {
        let mut t = vec![(
            ID_TABLE.to_string(),
            vec![Row(vec![(
                "id".into(),
                Cell::Text(self.id.clone().into_bytes()),
            )])],
        )];
        t.extend(self.tables.iter().cloned());
        t.push((
            db::SUMMARIES_TABLE.into(),
            self.summary.iter().cloned().collect(),
        ));
        encode_tables(&t)
    }

    fn decode(raw: &[u8]) -> Result<Header> {
        let mut tables = decode_tables(raw)?;
        if tables.first().map(|(t, _)| t.as_str()) != Some(ID_TABLE) {
            bail!("a bad agy conversation header");
        }
        let id = tables
            .remove(0)
            .1
            .first()
            .and_then(|r| r.get("id"))
            .and_then(Cell::text)
            .context("a bad agy conversation header")?
            .to_string();
        let summary = match tables.iter().position(|(t, _)| t == db::SUMMARIES_TABLE) {
            Some(i) => tables.remove(i).1.pop(),
            None => None,
        };
        Ok(Header {
            id,
            tables,
            summary,
        })
    }
}

pub(crate) fn stored_header(store: &Store, s: &super::Session) -> Result<Header> {
    let h = s
        .native_header
        .as_ref()
        .context("the session has no agy conversation header")?;
    Header::decode(&store.get(h)?.raw)
}

fn drop_list(raw: &[u8]) -> Result<BTreeMap<String, Vec<i64>>> {
    serde_json::from_slice(raw).context("a bad agy drop node")
}

/// The rows of a chain: the newest version of every row that no drop
/// node removed.
pub fn replay(nodes: &[Node]) -> Result<Rows> {
    let mut rows = Rows::new();
    for n in nodes {
        if n.meta.harness.as_deref() != Some(HARNESS) {
            bail!("node {} is not an agy row", n.hash.short());
        }
        match n.meta.container.as_deref() {
            Some(ROW) => {
                let key = n.meta.key.as_deref().unwrap_or_default();
                let table = key.split(':').next().unwrap_or_default();
                let r = Row::decode(&n.raw)?;
                rows.insert((r.idx()?, db::table_no(table)?), r);
            }
            Some(DROP) => {
                for (t, idxs) in drop_list(&n.raw)? {
                    let no = db::table_no(&t)?;
                    for i in idxs {
                        rows.remove(&(i, no));
                    }
                }
            }
            _ => bail!("node {} is not an agy row", n.hash.short()),
        }
    }
    Ok(rows)
}

/// The steps of a view, in index order.
pub fn steps(rows: &Rows) -> impl Iterator<Item = &Row> {
    rows.iter()
        .filter(|((_, no), _)| *no == STEPS)
        .map(|(_, r)| r)
}

#[derive(Debug)]
pub struct Ingest {
    pub session: String,
    /// New nodes: new rows and new versions of known rows.
    pub added: usize,
    pub kept: usize,
    /// Rows at a known index with other bytes. The old version stays.
    pub updated: usize,
    /// Rows that the database does not have any more. They stay in the store.
    pub dropped: usize,
}

/// Read the conversation database `path` into the store.
pub fn ingest(store: &Store, path: &Path, version: &str, horse: Option<&str>) -> Result<Ingest> {
    ingest_locked(&store.lock()?, path, version, horse)
}

pub fn ingest_locked(
    locked: &Lock<'_>,
    path: &Path,
    version: &str,
    horse: Option<&str>,
) -> Result<Ingest> {
    ingest_impl(locked, path, version, super::target::Target::Any(horse))
}

pub(crate) fn ingest_run_locked(
    locked: &Lock<'_>,
    path: &Path,
    version: &str,
    horse: &str,
) -> Result<Ingest> {
    ingest_impl(locked, path, version, super::target::Target::Run(horse))
}

/// Ingest a private copy of a host session file into its host session,
/// or into a new host session in `horse`.
pub(crate) fn ingest_host_locked(
    locked: &Lock<'_>,
    path: &Path,
    version: &str,
    horse: &str,
    origin: &super::HostOrigin,
) -> Result<Ingest> {
    ingest_impl(
        locked,
        path,
        version,
        super::target::Target::Host { horse, origin },
    )
}

fn ingest_impl(
    locked: &Lock<'_>,
    path: &Path,
    version: &str,
    target: super::target::Target<'_>,
) -> Result<Ingest> {
    let store = locked.store;
    check_version(version)?;
    let conv = db::read(path)?;
    let session = target.session(locked, HARNESS, &conv.id, || binding(Some(conv.id.clone())))?;
    check_version(&session.binding.version)?;
    if session.binding.harness != HARNESS || session.binding.container != CONTAINER {
        bail!("invalid agy binding");
    }
    let view = replay(&store.session_nodes(&session)?)?;

    let mut gone: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (idx, no) in view.keys() {
        if !conv.rows.contains_key(&(*idx, *no)) {
            gone.entry(IDX_TABLES[*no].into()).or_default().push(*idx);
        }
    }
    let mut head = session.head.clone();
    let scoped = store.for_horse(&session.horse)?;
    let dropped = gone.values().map(Vec::len).sum();
    if dropped > 0 {
        let raw = serde_json::to_vec(&gone)?;
        let key = format!("drop:{}", &Hash::of(&raw).as_str()[..16]);
        head = Some(scoped.put_native(&origin(DROP), head.as_ref(), &key, &raw)?);
    }
    let (mut added, mut kept, mut updated) = (0, 0, 0);
    for ((idx, no), r) in &conv.rows {
        match view.get(&(*idx, *no)) {
            Some(old) if old == r => {
                kept += 1;
                continue;
            }
            Some(_) => updated += 1,
            None => {}
        }
        let key = format!("{}:{idx}", IDX_TABLES[*no]);
        head = Some(scoped.put_native(&origin(ROW), head.as_ref(), &key, &r.encode())?);
        added += 1;
    }
    let header = Header {
        id: conv.id,
        tables: conv.header,
        summary: conv.summary,
    };
    let blob = scoped.put_blob(HEADER_MEDIA, &header.encode())?;
    locked.update_session(&session.id, |s| {
        s.head = head;
        s.native_header = Some(blob);
    })?;
    Ok(Ingest {
        session: session.id,
        added,
        kept,
        updated,
        dropped,
    })
}

/// The conversation id of a conversation database: its file name, checked
/// against its trajectory.
pub fn read_id(path: &Path) -> Result<String> {
    Ok(db::read(path)?.id)
}

/// The name of a step type for the log.
fn type_name(ty: i64) -> String {
    match ty {
        USER_INPUT => "user_input".into(),
        PLANNER_RESPONSE => "planner_response".into(),
        CHECKPOINT => "checkpoint".into(),
        8 => "view_file".into(),
        9 => "list_directory".into(),
        21 => "run_command".into(),
        38 => "mcp_tool".into(),
        86 => "file_change".into(),
        98 => "conversation_history".into(),
        112 => "shell_exec".into(),
        128 => "write_blob".into(),
        132 => "generic".into(),
        t => format!("type {t}"),
    }
}

/// One line about a node for `paddock horse log`.
pub fn describe(n: &Node) -> String {
    let key = n.meta.key.as_deref().unwrap_or_default();
    if n.meta.container.as_deref() == Some(DROP) {
        let count: usize = drop_list(&n.raw)
            .map(|d| d.values().map(Vec::len).sum())
            .unwrap_or(0);
        return format!("{count} rows left the database");
    }
    match Row::decode(&n.raw) {
        Ok(r) if key.starts_with("steps:") => {
            let ty = r.get("step_type").and_then(Cell::int).unwrap_or(-1);
            let st = match r.get("status").and_then(Cell::int) {
                Some(DONE) => "done".to_string(),
                Some(ERROR) => "error".into(),
                Some(s) => format!("status {s}"),
                None => "?".into(),
            };
            format!("step {}, {st}", type_name(ty))
        }
        Ok(r) => format!(
            "{}, {} bytes",
            key.split(':').next().unwrap_or("?"),
            r.get("data").and_then(Cell::bytes).map_or(0, <[u8]>::len)
        ),
        Err(_) => "?".into(),
    }
}

#[cfg(test)]
mod rewrite_tests;
#[cfg(test)]
mod tests;
