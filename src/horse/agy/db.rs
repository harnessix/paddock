//! The SQLite side of the agy codec: the pinned schema, a consistent read
//! of a conversation database, and the writes of a projection.
//!
//! The schema is the one of agy 1.4.12 (the same gorm column tags as the
//! 1.1.16 schema that session-migrate validated natively). A database with
//! another table set, other column declarations, other indexes, any other
//! schema object or another `PRAGMA user_version` is refused.

use super::proto;
use super::row::{Cell, Row, Tables};
use anyhow::{bail, Context as _, Result};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::collections::BTreeMap;
use std::path::Path;

/// `PRAGMA user_version` of the pinned schema. agy reads it, and paddock
/// accepts only this value. It is the value of the databases that agy
/// accepted in the native check of session-migrate; a database with
/// another value is refused until a native sample shows it. The value is
/// not confirmed against a database that a real agy 1.4.12 wrote, so a
/// wrong pin must fail closed: ingest and projection refuse, and nothing
/// is written.
pub const USER_VERSION: i64 = 0;
/// The page size that agy sets on a new conversation database.
pub const PAGE_SIZE: i64 = 32768;

/// The tables whose rows are keyed by the step index, in node order: the
/// rows of one index come before its step row, so a fork at a step keeps
/// the metadata of that step.
pub const IDX_TABLES: [&str; 4] = [
    "gen_metadata",
    "executor_metadata",
    "parent_references",
    "steps",
];
/// The tables of the conversation header. They go into the header blob.
pub const HEADER_TABLES: [&str; 3] = [
    "trajectory_meta",
    "trajectory_metadata_blob",
    "battle_mode_infos",
];
pub const SUMMARIES_TABLE: &str = "conversation_summaries";

/// (name, declared type, not null, default, primary key) per column, as
/// `PRAGMA table_info` reports them.
type Col = (&'static str, &'static str, bool, Option<&'static str>, bool);

const SCHEMA: [(&str, &[Col]); 7] = [
    (
        "trajectory_meta",
        &[
            ("trajectory_id", "TEXT", false, None, true),
            ("cascade_id", "TEXT", false, None, false),
            ("trajectory_type", "INTEGER", false, None, false),
            ("source", "INTEGER", false, None, false),
        ],
    ),
    (
        "steps",
        &[
            ("idx", "INTEGER", false, None, true),
            ("step_type", "INTEGER", true, Some("0"), false),
            ("status", "INTEGER", true, Some("0"), false),
            ("has_subtrajectory", "numeric", true, Some("false"), false),
            ("metadata", "BLOB", false, None, false),
            ("error_details", "BLOB", false, None, false),
            ("permissions", "BLOB", false, None, false),
            ("task_details", "BLOB", false, None, false),
            ("render_info", "BLOB", false, None, false),
            ("step_payload", "BLOB", false, None, false),
            ("step_format", "INTEGER", true, Some("0"), false),
        ],
    ),
    (
        "gen_metadata",
        &[
            ("idx", "INTEGER", false, None, true),
            ("data", "BLOB", false, None, false),
            ("size", "INTEGER", true, Some("0"), false),
        ],
    ),
    (
        "executor_metadata",
        &[
            ("idx", "INTEGER", false, None, true),
            ("data", "BLOB", false, None, false),
        ],
    ),
    (
        "parent_references",
        &[
            ("idx", "INTEGER", false, None, true),
            ("data", "BLOB", false, None, false),
        ],
    ),
    (
        "trajectory_metadata_blob",
        &[
            ("id", "TEXT", false, Some("\"main\""), true),
            ("data", "BLOB", false, None, false),
        ],
    ),
    (
        "battle_mode_infos",
        &[
            ("idx", "INTEGER", false, None, true),
            ("data", "BLOB", false, None, false),
        ],
    ),
];

const INDEXES: [(&str, &str, &str); 2] = [
    ("idx_steps_status", "steps", "status"),
    ("idx_steps_step_type", "steps", "step_type"),
];

/// The tables of a new conversation database, with the declarations of
/// [`SCHEMA`].
const DDL: &str = r#"
CREATE TABLE trajectory_meta (
  trajectory_id text, cascade_id text, trajectory_type integer, source integer,
  PRIMARY KEY (trajectory_id)
);
CREATE TABLE steps (
  idx integer, step_type integer NOT NULL DEFAULT 0,
  status integer NOT NULL DEFAULT 0,
  has_subtrajectory numeric NOT NULL DEFAULT false,
  metadata blob, error_details blob, permissions blob, task_details blob,
  render_info blob, step_payload blob,
  step_format integer NOT NULL DEFAULT 0,
  PRIMARY KEY (idx)
);
CREATE INDEX idx_steps_status ON steps(status);
CREATE INDEX idx_steps_step_type ON steps(step_type);
CREATE TABLE gen_metadata (
  idx integer, data blob, size integer NOT NULL DEFAULT 0, PRIMARY KEY (idx)
);
CREATE TABLE executor_metadata (idx integer, data blob, PRIMARY KEY (idx));
CREATE TABLE parent_references (idx integer, data blob, PRIMARY KEY (idx));
CREATE TABLE trajectory_metadata_blob (
  id text DEFAULT "main", data blob, PRIMARY KEY (id)
);
CREATE TABLE battle_mode_infos (idx integer, data blob, PRIMARY KEY (idx));
"#;

/// The columns of `conversation_summaries` in agy 1.4.12: the 1.1.16
/// columns, then `group_id` and `raw_summary`, which 1.4.12 adds.
pub const SUMMARY_COLUMNS: [&str; 21] = [
    "conversation_id",
    "title",
    "preview",
    "step_count",
    "last_modified_time",
    "workspace_uris",
    "status",
    "source",
    "project_id",
    "agent_name",
    "parent_conversation_id",
    "nesting_depth",
    "battle_id",
    "winning_conversation_id",
    "not_fully_idle",
    "killed",
    "last_user_input_time",
    "last_user_input_step_index",
    "app_data_dir",
    "group_id",
    "raw_summary",
];

const SUMMARY_DDL: &str = r#"
CREATE TABLE conversation_summaries (
  conversation_id text,
  title text NOT NULL DEFAULT "",
  preview text NOT NULL DEFAULT "",
  step_count integer NOT NULL DEFAULT 0,
  last_modified_time datetime NOT NULL,
  workspace_uris text NOT NULL,
  status text NOT NULL DEFAULT "",
  source text NOT NULL DEFAULT "",
  project_id text NOT NULL DEFAULT "",
  agent_name text NOT NULL DEFAULT "",
  parent_conversation_id text NOT NULL DEFAULT "",
  nesting_depth integer NOT NULL DEFAULT 0,
  battle_id text NOT NULL DEFAULT "",
  winning_conversation_id text NOT NULL DEFAULT "",
  not_fully_idle numeric NOT NULL DEFAULT false,
  killed numeric NOT NULL DEFAULT false,
  last_user_input_time datetime NOT NULL,
  last_user_input_step_index integer NOT NULL DEFAULT -1,
  app_data_dir text NOT NULL DEFAULT "",
  PRIMARY KEY (conversation_id)
);
CREATE INDEX idx_conversation_summaries_last_user_input_time
  ON conversation_summaries(last_user_input_time);
CREATE INDEX idx_conversation_summaries_last_modified_time
  ON conversation_summaries(last_modified_time);
ALTER TABLE conversation_summaries ADD COLUMN group_id text;
ALTER TABLE conversation_summaries ADD COLUMN raw_summary blob;
"#;

/// The rows keyed by step index: `(idx, table)` sorts in node order.
pub type Rows = BTreeMap<(i64, usize), Row>;

/// The position of an idx-keyed table in [`IDX_TABLES`].
pub fn table_no(name: &str) -> Result<usize> {
    IDX_TABLES
        .iter()
        .position(|t| *t == name)
        .with_context(|| format!("{name} is not an agy step table"))
}

pub const STEPS: usize = 3;

/// A conversation database, read in one snapshot.
pub struct Conversation {
    pub id: String,
    pub header: Tables,
    pub rows: Rows,
    /// Its row in `conversation_summaries.db`, when that file has one.
    pub summary: Option<Row>,
}

fn open_ro(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open {}", path.display()))?;
    c.execute_batch("PRAGMA trusted_schema=OFF")?;
    Ok(c)
}

/// Check the schema of a conversation database against the pin.
pub fn check_schema(c: &Connection) -> Result<()> {
    let uv: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if uv != USER_VERSION {
        bail!(
            "agy database user_version {uv} is not supported: paddock reads only user_version {USER_VERSION} (agy 1.4.12, not yet confirmed against a real database), so it changes nothing"
        );
    }
    let mut objects: Vec<(String, String)> = c
        .prepare("SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    objects.sort();
    let mut want: Vec<(String, String)> = SCHEMA
        .iter()
        .map(|(t, _)| ("table".to_string(), t.to_string()))
        .chain(
            INDEXES
                .iter()
                .map(|(i, _, _)| ("index".into(), i.to_string())),
        )
        .collect();
    want.sort();
    if objects != want {
        bail!(
            "the agy database schema is not the one of agy {}: its tables and indexes differ",
            super::VERSION
        );
    }
    for (table, cols) in SCHEMA {
        let got = table_info(c, table)?;
        let want: Vec<(String, String, bool, Option<String>, bool)> = cols
            .iter()
            .map(|(n, t, nn, d, pk)| (n.to_string(), t.to_string(), *nn, d.map(String::from), *pk))
            .collect();
        if got != want {
            bail!(
                "the agy table {table} is not the one of agy {}: its columns differ",
                super::VERSION
            );
        }
    }
    for (index, table, col) in INDEXES {
        let on: String = c.query_row(
            "SELECT tbl_name FROM sqlite_master WHERE type='index' AND name=?1",
            [index],
            |r| r.get(0),
        )?;
        let cols: Vec<String> = c
            .prepare(&format!("PRAGMA index_info('{index}')"))?
            .query_map([], |r| r.get(2))?
            .collect::<rusqlite::Result<_>>()?;
        if on != table || cols != [col] {
            bail!(
                "the agy index {index} is not the one of agy {}",
                super::VERSION
            );
        }
    }
    Ok(())
}

type ColInfo = (String, String, bool, Option<String>, bool);

fn table_info(c: &Connection, table: &str) -> Result<Vec<ColInfo>> {
    Ok(c.prepare(&format!("PRAGMA table_info('{table}')"))?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn rows_of(c: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Row>> {
    let mut st = c.prepare(sql)?;
    let names: Vec<String> = st.column_names().map(|s| s.to_string()).collect();
    let mut q = st.query(args)?;
    let mut out = Vec::new();
    while let Some(r) = q.next()? {
        let mut row = Vec::with_capacity(names.len());
        for (i, n) in names.iter().enumerate() {
            row.push((n.clone(), Cell::from_ref(r.get_ref(i)?)));
        }
        out.push(Row(row));
    }
    Ok(out)
}

/// The step type and status of a step row, checked against its payload:
/// the columns and fields 1 and 4 of the serialized step must agree.
pub fn step_kind(row: &Row) -> Result<(i64, i64)> {
    let idx = row.idx()?;
    let ty = row.get("step_type").and_then(Cell::int);
    let st = row.get("status").and_then(Cell::int);
    let (Some(ty), Some(st)) = (ty, st) else {
        bail!("agy step {idx} has no integer type or status");
    };
    let Some(Cell::Blob(payload)) = row.get("step_payload") else {
        bail!("agy step {idx} has no step payload");
    };
    let pty = proto::varint(payload, 1)
        .with_context(|| format!("agy step {idx} has a bad payload"))?
        .unwrap_or(0);
    let pst = proto::varint(payload, 4)?.unwrap_or(0);
    if pty as i64 != ty || pst as i64 != st {
        bail!("agy step {idx}: the type and status of the row and of its payload differ");
    }
    Ok((ty, st))
}

/// Read a conversation database in one snapshot and check it.
pub fn read(path: &Path) -> Result<Conversation> {
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|_| path.extension().is_some_and(|e| e == "db"))
        .with_context(|| format!("{} is not an agy conversation file <id>.db", path.display()))?
        .to_string();
    crate::horse::claude::check_session_id(&id)
        .with_context(|| format!("{} is not named by a conversation id", path.display()))?;
    let c = open_ro(path)?;
    c.execute_batch("BEGIN")?;
    check_schema(&c)?;
    let mut header = Vec::new();
    for t in HEADER_TABLES {
        header.push((
            t.to_string(),
            rows_of(&c, &format!("SELECT * FROM {t} ORDER BY 1"), &[])?,
        ));
    }
    let mut rows = Rows::new();
    for (no, t) in IDX_TABLES.iter().enumerate() {
        for r in rows_of(&c, &format!("SELECT * FROM {t} ORDER BY idx"), &[])? {
            rows.insert((r.idx()?, no), r);
        }
    }
    c.execute_batch("COMMIT")?;
    let conv = Conversation {
        summary: read_summary(path, &id)?,
        id,
        header,
        rows,
    };
    check(&conv.id, &conv.header, &conv.rows)?;
    Ok(conv)
}

/// The rows of `table` in a header.
pub fn header_rows<'a>(h: &'a Tables, table: &str) -> &'a [Row] {
    h.iter()
        .find(|(t, _)| t == table)
        .map(|(_, r)| r.as_slice())
        .unwrap_or(&[])
}

/// A conversation must name itself, and its steps must be contiguous from
/// 0 with a type and a status that agree with the payload.
pub fn check(id: &str, header: &Tables, rows: &Rows) -> Result<()> {
    let own = header_rows(header, "trajectory_meta")
        .iter()
        .any(|r| r.get("cascade_id").and_then(Cell::text) == Some(id));
    if !own {
        bail!("the agy database has no trajectory of conversation {id}");
    }
    for r in header_rows(header, "trajectory_metadata_blob") {
        if let Some(Cell::Blob(b)) = r.get("data") {
            if let Some(root) = proto::bytes(b, 6)? {
                if root != id.as_bytes() {
                    bail!("the agy trajectory metadata names another conversation");
                }
            }
        }
    }
    let mut next = 0;
    for ((idx, no), r) in rows {
        if *no != STEPS {
            continue;
        }
        if *idx != next {
            bail!("agy steps are not contiguous: step {next} is missing");
        }
        step_kind(r)?;
        next += 1;
    }
    Ok(())
}

/// The summaries database next to the `conversations/` folder of `db`.
pub fn summaries_path(db: &Path) -> Option<std::path::PathBuf> {
    let dir = db.parent()?;
    if dir.file_name()? != "conversations" {
        return None;
    }
    Some(dir.parent()?.join("conversation_summaries.db"))
}

fn check_summary_schema(c: &Connection) -> Result<()> {
    let cols: Vec<String> = table_info(c, SUMMARIES_TABLE)?
        .into_iter()
        .map(|c| c.0)
        .collect();
    if cols != SUMMARY_COLUMNS {
        bail!(
            "conversation_summaries.db is not the one of agy {}: its columns differ",
            super::VERSION
        );
    }
    Ok(())
}

fn read_summary(db: &Path, id: &str) -> Result<Option<Row>> {
    let Some(p) = summaries_path(db) else {
        return Ok(None);
    };
    if !p.is_file() {
        return Ok(None);
    }
    let c = open_ro(&p)?;
    check_summary_schema(&c)?;
    let mut rows = rows_of(
        &c,
        "SELECT * FROM conversation_summaries WHERE conversation_id = ?1",
        &[&id],
    )?;
    Ok(rows.pop())
}

fn insert(c: &Connection, table: &str, row: &Row) -> Result<()> {
    let names: Vec<&str> = row.0.iter().map(|(n, _)| n.as_str()).collect();
    let marks = vec!["?"; names.len()].join(",");
    let vals = row
        .0
        .iter()
        .map(|(_, v)| v.to_value())
        .collect::<Result<Vec<_>>>()?;
    c.execute(
        &format!("INSERT INTO {table} ({}) VALUES ({marks})", names.join(",")),
        rusqlite::params_from_iter(vals),
    )
    .with_context(|| format!("cannot write a row of {table}"))?;
    Ok(())
}

/// Write a new conversation database at `path` (a temporary name; the
/// caller renames it). Every value keeps its type and its bytes.
pub fn write(path: &Path, header: &Tables, rows: &Rows) -> Result<()> {
    let mut c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    c.execute_batch(&format!(
        "PRAGMA page_size={PAGE_SIZE}; PRAGMA journal_mode=DELETE; PRAGMA user_version={USER_VERSION};"
    ))?;
    let tx = c.transaction()?;
    tx.execute_batch(DDL)?;
    for (t, rs) in header {
        if !HEADER_TABLES.contains(&t.as_str()) {
            bail!("{t} is not an agy header table");
        }
        for r in rs {
            insert(&tx, t, r)?;
        }
    }
    for ((_, no), r) in rows {
        insert(&tx, IDX_TABLES[*no], r)?;
    }
    tx.commit()?;
    let ok: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if ok != "ok" {
        bail!("the projected agy database fails its integrity check");
    }
    check_schema(&c)?;
    c.close().map_err(|(_, e)| e)?;
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

/// Open `conversation_summaries.db` for a write, and create it with the
/// pinned schema when it is missing (a temporary file and a rename).
pub fn open_summaries(path: &Path) -> Result<Connection> {
    if !path.exists() {
        let dir = path.parent().context("the summaries file has no folder")?;
        let tmp = dir.join(format!(".paddock-{}.db", crate::horse::uuid_v4()?));
        let made = (|| -> Result<()> {
            let c = Connection::open(&tmp)?;
            c.execute_batch(SUMMARY_DDL)?;
            c.close().map_err(|(_, e)| e)?;
            std::fs::File::open(&tmp)?.sync_all()?;
            set_private(&tmp)?;
            match std::fs::hard_link(&tmp, path) {
                Ok(()) => Ok(()),
                // Another writer made it first: use that file.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(e) => Err(e.into()),
            }
        })();
        let _ = std::fs::remove_file(&tmp);
        made?;
    }
    let c = Connection::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    check_summary_schema(&c)?;
    Ok(c)
}

/// Insert the summaries row of a new conversation inside `BEGIN
/// IMMEDIATE`, then run `publish` (the rename of the database) before the
/// commit. A row for that id refuses.
pub fn insert_summary(
    c: &mut Connection,
    row: &Row,
    publish: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let id = row
        .get("conversation_id")
        .and_then(Cell::text)
        .context("a summaries row has no conversation id")?;
    let n: i64 = tx.query_row(
        "SELECT count(*) FROM conversation_summaries WHERE conversation_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    if n > 0 {
        bail!("conversation_summaries.db has a row for conversation {id} already");
    }
    insert(&tx, SUMMARIES_TABLE, row)?;
    publish()?;
    tx.commit()?;
    Ok(())
}

pub fn set_private(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}
