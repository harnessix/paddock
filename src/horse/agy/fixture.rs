//! Synthetic agy conversations for tests, built from the schema and the
//! protobuf field numbers of agy 1.1.16 (the same column tags as 1.4.12). No real agy file is read.

use super::proto::put_varint;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub(crate) const ID: &str = "4b0c2a1e-7d3f-4a5b-9c6d-0e1f2a3b4c5d";
const TRAJECTORY: &str = "9a8b7c6d-5e4f-4321-8fed-cba987654321";

const DDL: &str = r#"
CREATE TABLE trajectory_meta (trajectory_id text, cascade_id text, trajectory_type integer, source integer, PRIMARY KEY (trajectory_id));
CREATE TABLE steps (idx integer, step_type integer NOT NULL DEFAULT 0, status integer NOT NULL DEFAULT 0, has_subtrajectory numeric NOT NULL DEFAULT false, metadata blob, error_details blob, permissions blob, task_details blob, render_info blob, step_payload blob, step_format integer NOT NULL DEFAULT 0, PRIMARY KEY (idx));
CREATE INDEX idx_steps_status ON steps(status);
CREATE INDEX idx_steps_step_type ON steps(step_type);
CREATE TABLE gen_metadata (idx integer, data blob, size integer NOT NULL DEFAULT 0, PRIMARY KEY (idx));
CREATE TABLE executor_metadata (idx integer, data blob, PRIMARY KEY (idx));
CREATE TABLE parent_references (idx integer, data blob, PRIMARY KEY (idx));
CREATE TABLE trajectory_metadata_blob (id text DEFAULT "main", data blob, PRIMARY KEY (id));
CREATE TABLE battle_mode_infos (idx integer, data blob, PRIMARY KEY (idx));
"#;

const SUMMARY_DDL: &str = r#"
CREATE TABLE conversation_summaries (conversation_id text, title text NOT NULL DEFAULT "", preview text NOT NULL DEFAULT "", step_count integer NOT NULL DEFAULT 0, last_modified_time datetime NOT NULL, workspace_uris text NOT NULL, status text NOT NULL DEFAULT "", source text NOT NULL DEFAULT "", project_id text NOT NULL DEFAULT "", agent_name text NOT NULL DEFAULT "", parent_conversation_id text NOT NULL DEFAULT "", nesting_depth integer NOT NULL DEFAULT 0, battle_id text NOT NULL DEFAULT "", winning_conversation_id text NOT NULL DEFAULT "", not_fully_idle numeric NOT NULL DEFAULT false, killed numeric NOT NULL DEFAULT false, last_user_input_time datetime NOT NULL, last_user_input_step_index integer NOT NULL DEFAULT -1, app_data_dir text NOT NULL DEFAULT "", PRIMARY KEY (conversation_id));
CREATE INDEX idx_conversation_summaries_last_user_input_time ON conversation_summaries(last_user_input_time);
CREATE INDEX idx_conversation_summaries_last_modified_time ON conversation_summaries(last_modified_time);
ALTER TABLE conversation_summaries ADD COLUMN group_id text;
ALTER TABLE conversation_summaries ADD COLUMN raw_summary blob;
"#;

pub(crate) fn field_varint(out: &mut Vec<u8>, n: u64, v: u64) {
    put_varint(out, n << 3);
    put_varint(out, v);
}

pub(crate) fn field_bytes(out: &mut Vec<u8>, n: u64, b: &[u8]) {
    put_varint(out, n << 3 | 2);
    put_varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

/// One step row.
#[derive(Clone)]
pub(crate) struct Step {
    pub ty: u64,
    pub status: u64,
    pub payload: Vec<u8>,
    pub metadata: Vec<u8>,
    pub render: Option<Vec<u8>>,
}

/// A conversation that a test builds step by step.
pub(crate) struct Fixture {
    pub id: String,
    pub steps: Vec<Step>,
    /// gen_metadata rows: idx and data.
    pub gen: Vec<(i64, Vec<u8>)>,
    pub executor: Vec<(i64, Vec<u8>)>,
    pub user_version: i64,
    pub summary: bool,
    time: u64,
}

impl Fixture {
    pub fn new(id: &str) -> Fixture {
        Fixture {
            id: id.into(),
            steps: Vec::new(),
            gen: Vec::new(),
            executor: Vec::new(),
            user_version: 0,
            summary: true,
            time: 1_793_750_400,
        }
    }

    fn push(&mut self, ty: u64, status: u64, oneof: u64, body: Vec<u8>, render: bool) {
        self.time += 1;
        let mut p = Vec::new();
        field_varint(&mut p, 1, ty);
        field_varint(&mut p, 4, status);
        field_bytes(&mut p, oneof, &body);
        // A field that paddock does not read, after the payload.
        field_bytes(&mut p, 5, &[0xff, 0x00, 0x80]);
        let mut ts = Vec::new();
        field_varint(&mut ts, 1, self.time);
        let mut m = Vec::new();
        field_bytes(&mut m, 1, &ts);
        let render = render.then(|| vec![0x0a, 0x02, 0xc3, 0xa9]);
        self.steps.push(Step {
            ty,
            status,
            payload: p,
            metadata: m,
            render,
        });
    }

    /// A user input step (type 14, field 19).
    pub fn user(&mut self, text: &str) -> &mut Self {
        let mut b = Vec::new();
        field_bytes(&mut b, 1, text.as_bytes());
        self.push(14, 3, 19, b, false);
        self
    }

    /// A planner response (type 15, field 20): text, visible thinking and
    /// a message id, with its gen_metadata row.
    pub fn reply(&mut self, text: &str) -> &mut Self {
        let mut b = Vec::new();
        field_bytes(&mut b, 1, text.as_bytes());
        field_bytes(&mut b, 3, b"thinking \xe2\x80\xa6");
        field_bytes(&mut b, 6, format!("msg-{}", self.steps.len()).as_bytes());
        self.push(15, 3, 20, b, true);
        let idx = self.steps.len() as i64 - 1;
        let mut usage = Vec::new();
        field_varint(&mut usage, 5, 30214);
        self.gen.push((idx, usage));
        self
    }

    /// A generic tool step (type 132, field 140) with a tool call, and its
    /// executor_metadata row.
    pub fn tool(&mut self, name: &str) -> &mut Self {
        let mut b = Vec::new();
        field_bytes(&mut b, 2, name.as_bytes());
        field_bytes(&mut b, 3, br#"{"path":"/home/demo/project"}"#);
        self.push(132, 3, 140, b, false);
        let idx = self.steps.len() as i64 - 1;
        self.executor.push((idx, vec![0x08, 0x01]));
        self
    }

    /// An ephemeral system step of a type that agy keeps out of the request.
    pub fn ephemeral(&mut self) -> &mut Self {
        self.push(77, 3, 90, b"\x0a\x03tip".to_vec(), false);
        self
    }

    /// Mark the last step as running (status 2, as agy writes it first).
    pub fn running(&mut self) -> &mut Self {
        let s = self.steps.last_mut().unwrap();
        s.status = 2;
        let body = s.payload.clone();
        let mut p = Vec::new();
        field_varint(&mut p, 1, s.ty);
        field_varint(&mut p, 4, 2);
        // The rest of the payload after fields 1 and 4.
        let skip = super::proto::fields(&body).unwrap()[..2]
            .iter()
            .map(|f| f.span.len())
            .sum::<usize>();
        p.extend_from_slice(&body[skip..]);
        s.payload = p;
        self
    }

    /// Two turns: a user message, a tool call and a reply, then a second
    /// user message and a reply.
    pub fn two_turns() -> Fixture {
        let mut f = Fixture::new(ID);
        f.user("Read the project")
            .tool("list_dir")
            .reply("It has one file.")
            .ephemeral()
            .user("Now fix the warning")
            .reply("Fixed.");
        f
    }

    /// Drop the steps from index `k` and their metadata rows, as a rewind.
    pub fn truncate(&mut self, k: usize) {
        self.steps.truncate(k);
        self.gen.retain(|(i, _)| (*i as usize) < k);
        self.executor.retain(|(i, _)| (*i as usize) < k);
    }

    /// The database path under the agy home `home`.
    pub fn path(&self, home: &Path) -> PathBuf {
        home.join("conversations").join(format!("{}.db", self.id))
    }

    /// Write the conversation under the agy home `home`, replacing an older
    /// copy, and its summaries row.
    pub fn write(&self, home: &Path) -> PathBuf {
        let path = self.path(home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "PRAGMA page_size=32768; PRAGMA journal_mode=WAL; PRAGMA user_version={};",
            self.user_version
        ))
        .unwrap();
        c.execute_batch(DDL).unwrap();
        c.execute(
            "INSERT INTO trajectory_meta VALUES (?1, ?2, 4, 17)",
            params![TRAJECTORY, self.id],
        )
        .unwrap();
        let mut ts = Vec::new();
        field_varint(&mut ts, 1, 1_793_750_400);
        let mut meta = Vec::new();
        field_bytes(&mut meta, 2, &ts);
        field_bytes(&mut meta, 6, self.id.as_bytes());
        field_bytes(&mut meta, 18, b"default-cli-project");
        c.execute(
            "INSERT INTO trajectory_metadata_blob (id, data) VALUES ('main', ?1)",
            [meta],
        )
        .unwrap();
        for (i, s) in self.steps.iter().enumerate() {
            c.execute(
                "INSERT INTO steps (idx, step_type, status, has_subtrajectory, metadata, render_info, step_payload, step_format) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, 0)",
                params![i as i64, s.ty as i64, s.status as i64, s.metadata, s.render, s.payload],
            )
            .unwrap();
        }
        for (i, d) in &self.gen {
            c.execute(
                "INSERT INTO gen_metadata VALUES (?1, ?2, ?3)",
                params![i, d, d.len() as i64],
            )
            .unwrap();
        }
        for (i, d) in &self.executor {
            c.execute(
                "INSERT INTO executor_metadata VALUES (?1, ?2)",
                params![i, d],
            )
            .unwrap();
        }
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        drop(c);
        if self.summary {
            self.write_summary(home);
        }
        path
    }

    fn write_summary(&self, home: &Path) {
        let p = home.join("conversation_summaries.db");
        let fresh = !p.exists();
        let c = Connection::open(&p).unwrap();
        if fresh {
            c.execute_batch(SUMMARY_DDL).unwrap();
        }
        let last_user = self
            .steps
            .iter()
            .rposition(|s| s.ty == 14)
            .map_or(-1, |i| i as i64);
        c.execute(
            "INSERT OR REPLACE INTO conversation_summaries (conversation_id, title, preview, step_count, last_modified_time, workspace_uris, source, project_id, last_user_input_time, last_user_input_step_index, app_data_dir, group_id, raw_summary) VALUES (?1, 'Fix the warning', 'Fixed.', ?2, '2026-11-01 10:00:00+00:00', '[\"file:///home/demo/project\"]', 'antigravity-cli', 'default-cli-project', '2026-11-01 09:59:00+00:00', ?3, 'antigravity-cli', NULL, x'0a0141')",
            params![self.id, self.steps.len() as i64, last_user],
        )
        .unwrap();
    }
}
