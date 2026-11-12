//! The registry part of a store file: which sandboxes paddock started,
//! where they sit in a spawn tree, and the session grants of each.

use super::{Record, Store};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use std::path::{Path, PathBuf};

/// How many sandboxes above one another `ancestors` follows at most.
const MAX_CHAIN: usize = 32;

impl Store {
    /// Record a sandbox at its start. The parent is the sandbox that started
    /// it, when there is one.
    pub fn register(
        &self,
        id: &str,
        harness: &str,
        project: &Path,
        parent: Option<&str>,
        pasture: Option<&str>,
        now: u64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sandboxes (id, harness, project, parent, pasture, started, ended)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![id, harness, project.display().to_string(), parent, pasture, now],
        )?;
        Ok(())
    }

    /// Record the end of a sandbox. Its session grants end with it.
    pub fn finish(&self, id: &str, now: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE sandboxes SET ended = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        self.conn
            .execute("DELETE FROM grants WHERE sandbox = ?1", params![id])?;
        Ok(())
    }

    /// Whether a sandbox ended. None when paddock has no record of it.
    pub fn ended(&self, id: &str) -> Result<Option<bool>> {
        Ok(self
            .conn
            .query_row(
                "SELECT ended IS NOT NULL FROM sandboxes WHERE id = ?1",
                params![id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?)
    }

    /// Record that the user allowed `name` for the session of a sandbox.
    /// Only the host process of that sandbox writes it; children read it.
    pub fn grant(&self, sandbox: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO grants (sandbox, name) VALUES (?1, ?2)",
            params![sandbox, name],
        )?;
        Ok(())
    }

    pub fn granted(&self, sandbox: &str, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM grants g JOIN sandboxes s ON s.id = g.sandbox
                 WHERE g.sandbox = ?1 AND g.name = ?2 AND s.ended IS NULL",
                params![sandbox, name],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The sandboxes above `id`, as paddock recorded them: the parent first.
    pub fn ancestors(&self, id: &str) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        let mut cur = id.to_string();
        while out.len() < MAX_CHAIN {
            match self.parent_of(&cur)? {
                Some(p) if p != id && !out.contains(&p) => {
                    out.push(p.clone());
                    cur = p;
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// The sandbox that started `id`, as paddock recorded it.
    pub fn parent_of(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT parent FROM sandboxes WHERE id = ?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Registry: forget the records of sandboxes that ended before
    /// `cutoff` and have no files left. The sandboxes above a running one
    /// stay, because the running one reads them.
    pub fn forget_ended(&mut self, cutoff: u64, running: &[String]) -> Result<u64> {
        let mut keep: Vec<String> = running.to_vec();
        for id in running {
            keep.extend(self.ancestors(id)?);
        }
        let old: Vec<String> = {
            let mut st = self
                .conn
                .prepare("SELECT id FROM sandboxes WHERE ended IS NOT NULL AND ended < ?1")?;
            let rows = st.query_map(params![cutoff], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut n = 0;
        for id in old.iter().filter(|i| !keep.contains(i)) {
            self.forget_sandbox(id)?;
            n += 1;
        }
        Ok(n)
    }

    /// Registry: the harness and the project of a sandbox. None without a
    /// record.
    pub fn harness_and_project(&self, id: &str) -> Result<Option<(String, PathBuf)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT harness, project FROM sandboxes WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        PathBuf::from(r.get::<_, String>(1)?),
                    ))
                },
            )
            .optional()?)
    }

    /// Registry: the sandboxes recorded for a project, oldest first.
    pub fn sandboxes_of(&self, project: &Path) -> Result<Vec<Record>> {
        let mut st = self.conn.prepare(
            "SELECT id, harness, parent, pasture, started, ended FROM sandboxes
             WHERE project = ?1 ORDER BY started, id",
        )?;
        let rows = st.query_map(params![project.display().to_string()], |r| {
            Ok(Record {
                id: r.get(0)?,
                harness: r.get(1)?,
                parent: r.get(2)?,
                pasture: r.get(3)?,
                started: r.get(4)?,
                ended: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Registry: when a sandbox started and ended. None without a record.
    pub fn times(&self, id: &str) -> Result<Option<(u64, Option<u64>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT started, ended FROM sandboxes WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Registry: every project that a sandbox was recorded for.
    pub fn projects(&self) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT project FROM sandboxes ORDER BY project")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn forget_sandbox(&mut self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM sandboxes WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM grants WHERE sandbox = ?1", params![id])?;
        Ok(())
    }
}
