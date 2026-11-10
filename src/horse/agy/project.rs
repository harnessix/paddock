//! Write a session or a paddock fork back as an agy conversation.
//!
//! A session with a native id keeps it. A paddock fork gets the shape of a
//! native fork: a new conversation id and a new trajectory id, field 6 of
//! the trajectory metadata set to the new id, the steps up to the fork
//! point with contiguous indexes and every blob byte exact, no metadata
//! rows after the last step, and a summaries row of its own. agy has no
//! documented fork provenance fields (their wire numbers are unknown), so
//! the provenance stays in paddock's DAG (`fork_of`).

use super::db::{self, Rows, IDX_TABLES, STEPS, SUMMARY_COLUMNS};
use super::row::{Cell, Row};
use super::{
    check_version, origin, replay, steps, stored_header, Header, CONTAINER, DONE, DROP, ERROR,
    HARNESS, HEADER_MEDIA, USER_INPUT,
};
use crate::horse::store::{sync_dir, Store};
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Projection {
    /// The conversation database.
    pub path: PathBuf,
    pub native_id: String,
    pub forked: bool,
}

/// Write session `session` as `conversations/<id>.db` with its row in
/// `conversation_summaries.db` under the agy home `home` (for example the
/// `~/.gemini/antigravity-cli` of a sandbox). The database must be new.
pub fn project(store: &Store, session: &str, home: &Path) -> Result<Projection> {
    let s = store.session(session)?;
    if s.binding.harness != HARNESS || s.binding.container != CONTAINER {
        bail!("not an agy session");
    }
    check_version(&s.binding.version)?;
    let nodes = store
        .native_chain(s.head.as_ref())
        .with_context(|| format!("session {} cannot be projected", s.id))?;
    let mut rows = replay(&nodes)?;
    let mut header = stored_header(store, &s)?;
    let forked = s.binding.native_id.is_none();
    let mut gone: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    if forked {
        fork_point(&rows, &header)?;
        let n = steps(&rows).count() as i64;
        rows.retain(|(idx, no), _| {
            let keep = *no == STEPS || *idx < n;
            if !keep {
                gone.entry(IDX_TABLES[*no].into()).or_default().push(*idx);
            }
            keep
        });
        header = fork_header(&header, &crate::horse::uuid_v4()?, &rows)?;
    } else {
        if s.binding.native_id.as_deref() != Some(header.id.as_str()) {
            bail!(
                "the agy header does not belong to conversation {}",
                header.id
            );
        }
        if header.summary.is_none() {
            header.summary = Some(summary(None, &header.id, &rows));
        }
    }
    db::check(&header.id, &header.tables, &rows)?;
    let path = publish(home, &header, &rows)?;
    if forked {
        let scoped = store.for_horse(&s.horse)?;
        let mut head = s.head.clone();
        if !gone.is_empty() {
            let raw = serde_json::to_vec(&gone)?;
            let key = format!("drop:{}", &crate::horse::Hash::of(&raw).as_str()[..16]);
            head = Some(scoped.put_native(&origin(DROP), head.as_ref(), &key, &raw)?);
        }
        let blob = scoped.put_blob(HEADER_MEDIA, &header.encode())?;
        let id = header.id.clone();
        store.update_session(session, |s| {
            s.head = head;
            s.native_header = Some(blob);
            s.binding.native_id = Some(id);
        })?;
    }
    Ok(Projection {
        path,
        native_id: header.id,
        forked,
    })
}

/// A native fork keeps whole turns: its last step is a finished step that
/// is no user input, and the conversation has no battle mode arm.
fn fork_point(rows: &Rows, header: &Header) -> Result<()> {
    let last = steps(rows)
        .last()
        .context("an agy fork needs at least one step")?;
    let (ty, st) = db::step_kind(last)?;
    let idx = last.idx()?;
    if ty == USER_INPUT {
        bail!("cannot fork an agy conversation at step {idx}: that step is a user message");
    }
    if st != DONE && st != ERROR {
        bail!("cannot fork an agy conversation at step {idx}: that step has not finished");
    }
    if !db::header_rows(&header.tables, "battle_mode_infos").is_empty() {
        bail!("cannot fork an agy conversation in battle mode");
    }
    Ok(())
}

/// The header of a fork under the new conversation id `id`.
fn fork_header(h: &Header, id: &str, rows: &Rows) -> Result<Header> {
    let old = h.id.as_str();
    let mut tables = h.tables.clone();
    for (t, rs) in &mut tables {
        for r in rs.iter_mut() {
            match t.as_str() {
                "trajectory_meta" if r.get("cascade_id").and_then(Cell::text) == Some(old) => {
                    r.set("cascade_id", text(id));
                    r.set("trajectory_id", text(&crate::horse::uuid_v4()?));
                }
                "trajectory_metadata_blob" => {
                    if let Some(Cell::Blob(b)) = r.get("data") {
                        if super::proto::bytes(b, 6)? == Some(old.as_bytes()) {
                            let b = super::proto::replace_bytes(b, 6, id.as_bytes())?;
                            r.set("data", Cell::Blob(b));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(Header {
        id: id.into(),
        tables,
        summary: Some(summary(h.summary.as_ref(), id, rows)),
    })
}

fn text(s: &str) -> Cell {
    Cell::Text(s.as_bytes().to_vec())
}

/// The time format of the summaries rows: `2026-11-08 12:00:00+00:00`.
fn now() -> String {
    crate::waiting::format_rfc3339(crate::horse::now_ms() / 1000)
        .replace('T', " ")
        .replace('Z', "+00:00")
}

/// The summaries row of a new conversation `id` with the steps `rows`:
/// the row of the parent with the new id, the counts of the fork and new
/// times. The preview and the raw summary describe turns after the fork
/// point, so they are empty; agy rebuilds them. `parent_conversation_id`
/// stays empty, because agy uses it for subagent nesting.
fn summary(parent: Option<&Row>, id: &str, rows: &Rows) -> Row {
    let now = text(&now());
    let n = steps(rows).count() as i64;
    let last_user = steps(rows)
        .filter(|r| r.get("step_type").and_then(Cell::int) == Some(USER_INPUT))
        .filter_map(|r| r.idx().ok())
        .last()
        .unwrap_or(-1);
    let mut row = Row(SUMMARY_COLUMNS
        .iter()
        .map(|c| {
            let v = match *c {
                "step_count" | "nesting_depth" | "not_fully_idle" | "killed" => Cell::Int(0),
                "raw_summary" | "group_id" => Cell::Null,
                "workspace_uris" => text("[]"),
                _ => text(""),
            };
            (c.to_string(), v)
        })
        .collect());
    if let Some(p) = parent {
        for (c, v) in &p.0 {
            row.set(c, v.clone());
        }
    }
    let fresh: [(&str, Cell); 12] = [
        ("conversation_id", text(id)),
        ("preview", text("")),
        ("step_count", Cell::Int(n)),
        ("last_modified_time", now.clone()),
        ("last_user_input_time", now),
        ("last_user_input_step_index", Cell::Int(last_user)),
        ("parent_conversation_id", text("")),
        ("battle_id", text("")),
        ("winning_conversation_id", text("")),
        ("not_fully_idle", Cell::Int(0)),
        ("killed", Cell::Int(0)),
        ("raw_summary", Cell::Null),
    ];
    for (c, v) in fresh {
        row.set(c, v);
    }
    row
}

fn mkdir(dir: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot make {}", dir.display()))
}

/// Write the database as a temporary file, insert the summaries row and
/// rename the file into place before the row commits. A failure leaves
/// neither the file nor the row.
fn publish(home: &Path, header: &Header, rows: &Rows) -> Result<PathBuf> {
    let dir = home.join("conversations");
    mkdir(&dir)?;
    let path = dir.join(format!("{}.db", header.id));
    if path.exists() {
        bail!("the agy conversation {} exists already", path.display());
    }
    let tmp = dir.join(format!(".paddock-{}.db", crate::horse::uuid_v4()?));
    let linked = std::cell::Cell::new(false);
    let r = (|| -> Result<()> {
        db::write(&tmp, &header.tables, rows)?;
        db::set_private(&tmp)?;
        let summary = header.summary.as_ref().context("no summaries row")?;
        let mut c = db::open_summaries(&home.join("conversation_summaries.db"))?;
        db::insert_summary(&mut c, summary, || {
            // A hard link fails when the name exists: never overwrite.
            std::fs::hard_link(&tmp, &path).context("cannot publish the conversation")?;
            linked.set(true);
            sync_dir(&dir)
        })
    })();
    let _ = std::fs::remove_file(&tmp);
    if let Err(e) = r {
        if linked.get() && !summary_has(home, &header.id) {
            let _ = std::fs::remove_file(&path);
        }
        return Err(e);
    }
    Ok(path)
}

fn summary_has(home: &Path, id: &str) -> bool {
    rusqlite::Connection::open(home.join("conversation_summaries.db"))
        .and_then(|c| {
            c.query_row(
                "SELECT count(*) FROM conversation_summaries WHERE conversation_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
        })
        .is_ok_and(|n| n > 0)
}
