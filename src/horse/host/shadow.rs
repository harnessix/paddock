//! `--profile default` in horse mode: the run continues a host session.
//!
//! A shadow run works on a private snapshot of the host data
//! (crate::shadow). When the run writes to a session file that was in the
//! snapshot, the run continues a host session. At the final ingest:
//!
//! - The host session comes from the base: the snapshot as it was before
//!   the run. On Linux that is the `lower` folder of the overlay, which
//!   the run cannot change. On macOS the run changes its clone in place,
//!   so paddock copies the transcript folder of the clone at the start.
//!   When the store has the host session already (import or adoption),
//!   paddock uses that session and leaves it as it is.
//! - The run's rows go into the run's own session (target.rs `Run`). That
//!   session is a fork of the host session at the last row that both have,
//!   and its lineage starts with the lineage of the host session.
//! - When the host session lives in a horse of its own (an import or an
//!   adoption), the run's session joins that horse at the end of the run:
//!   the host session and its continuation are then in one horse. The
//!   session keeps the run as its `origin_run`: the rows after its fork
//!   node are the run's.
//! - The host session never gets the run's rows, and the host file never
//!   changes, so after the run the store and the host history agree: the
//!   host has the rows up to the fork node, and only the run's session has
//!   the rows after it.
//!
//! None of this may stop a run or its final ingest. A base that paddock
//! cannot make, a host session that it cannot read, and a lineage that it
//! cannot record are logged, and the run goes on without the lineage.

use super::{Found, HostOrigin};
use crate::horse::store::Lock;
use crate::horse::{run, ForkOf, Session};
use anyhow::{Context as _, Result};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The snapshot of a shadow run as it was before the run.
#[derive(Clone, Debug)]
pub struct Base {
    /// A home-like folder: the transcript root of the harness is below it.
    pub dir: PathBuf,
    /// The real host home that the snapshot came from.
    pub host_home: PathBuf,
}

impl Base {
    /// The base of a shadow run, before the sandbox starts. An overlay
    /// keeps its `lower` folder; a clone gets a copy of its transcript
    /// folder in the run folder (removed with the run folder), with
    /// reflinks or APFS clones where the file system has them. None: no
    /// base (logged); the run goes on without the lineage of host sessions.
    pub fn prepare(plan: &crate::shadow::Plan, harness: &str, host_home: &Path) -> Option<Self> {
        match Self::try_prepare(plan, harness, host_home) {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("horse: no host lineage for this run: {e:#}");
                None
            }
        }
    }

    fn try_prepare(plan: &crate::shadow::Plan, harness: &str, host_home: &Path) -> Result<Self> {
        let dir = match &plan.lower {
            Some(lower) => lower.clone(),
            None => {
                let dir = plan.dir.join("horse-base");
                crate::privfs::dir(&dir)?;
                for found in super::list(&plan.home, harness, None)? {
                    copy_found(&plan.home, &found, &dir)?;
                }
                dir
            }
        };
        Ok(Self {
            dir,
            host_home: super::canon_home(host_home),
        })
    }

    /// The base copy of the session at `rel` (relative to the home), if the
    /// snapshot had it.
    pub fn session(&self, rel: &Path) -> Option<PathBuf> {
        let p = self.dir.join(rel);
        std::fs::symlink_metadata(&p)
            .is_ok_and(|m| m.is_file() || m.is_dir())
            .then_some(p)
    }
}

fn copy_found(home: &Path, found: &Found, to: &Path) -> Result<()> {
    for f in &found.files {
        let dst = to.join(f);
        std::fs::create_dir_all(dst.parent().context("a session file has a folder")?)?;
        super::copy_file(&home.join(f), &dst)?;
    }
    Ok(())
}

/// The host session that the base copy `path` (at `rel` of the home)
/// belongs to: the one in the store, else a new host session in `horse`.
pub(crate) fn host_parent(
    locked: &Lock<'_>,
    harness: &str,
    version: &str,
    horse: &str,
    base: &Base,
    rel: &Path,
    path: &Path,
) -> Result<Session> {
    let store = locked.store;
    let native = super::native_id(harness, path)?;
    if let Some(s) = store.host_session_at(harness, &native, &base.host_home)? {
        return Ok(s);
    }
    let origin = HostOrigin {
        via: "shadow".into(),
        home: base.host_home.clone(),
        path: rel.to_path_buf(),
        read_ms: crate::horse::now_ms(),
    };
    match super::ingest_locked(locked, harness, path, version, horse, &origin) {
        Ok(id) => store.session(&id),
        Err(e) => {
            // A read that failed half way leaves no half-made host session.
            if let Ok(Some(s)) = store.host_session_at(harness, &native, &base.host_home) {
                if let Err(d) = locked.discard_host_session(&s.id) {
                    log::warn!(
                        "horse: cannot take out the half-read host session {}: {d:#}",
                        s.id
                    );
                }
            }
            Err(e)
        }
    }
}

/// What the final ingest of a shadow run found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linked {
    pub session: String,
    pub host_session: String,
    pub harness: String,
    pub native_id: String,
    /// The rows of the host session that the run's session keeps.
    pub kept: usize,
    /// The rows that the run added after them.
    pub added: usize,
    /// The horse of the host session, when the run's session joins it at
    /// the end of the run ([`join`]).
    pub joins: Option<String>,
}

impl Linked {
    pub fn line(&self) -> String {
        let mut l = format!(
            "horse: {} continues the host session {} ({} {}) after row {}; its {} new row(s) are in the store only, the host history has none of them",
            self.session, self.host_session, self.harness, self.native_id, self.kept, self.added
        );
        if let Some(h) = &self.joins {
            l.push_str(&format!(
                "; it joins horse {h}, the horse of the host session"
            ));
        }
        l
    }
}

/// Make the run's session `session` a fork of the host session `host` at
/// the last row that both have. `host` is the host session as it was
/// before the run's ingest: its head must not have moved.
pub(crate) fn link(
    locked: &Lock<'_>,
    rh: &run::RunHorse,
    run_id: &str,
    host: &Session,
    session: &str,
) -> Result<Option<Linked>> {
    let store = locked.store;
    let now = store.session(&host.id)?;
    anyhow::ensure!(
        now.head == host.head,
        "the host session {} changed during the final ingest of a run",
        host.id
    );
    let s = store.session(session)?;
    anyhow::ensure!(
        s.host.is_none(),
        "the rows of the run went into the host session {}",
        s.id
    );
    if s.fork_of.as_ref().is_some_and(|f| f.session != host.id) {
        return Ok(None);
    }
    let kept = super::common_rows(store, host, &s)?;
    if kept == 0 {
        log::warn!(
            "horse: session {} has no row of the host session {}; no lineage",
            s.id,
            host.id
        );
        return Ok(None);
    }
    let host_nodes = store.session_nodes(host)?;
    let total = store.session_nodes(&s)?.len();
    let node = host_nodes[kept - 1].hash.clone();
    let lineage = if host.horse == s.horse {
        s.lineage.clone()
    } else {
        let mut steps = host.lineage.clone();
        if let Some(last) = steps.last_mut() {
            last.until_ms.get_or_insert(s.created_ms);
        }
        if s.lineage.starts_with(&steps) {
            s.lineage.clone()
        } else {
            steps.extend(s.lineage.iter().cloned());
            steps
        }
    };
    locked.update_session(session, |x| {
        x.fork_of = Some(ForkOf {
            session: host.id.clone(),
            node: node.clone(),
        });
        x.lineage = lineage;
        x.origin_run = Some(run_id.to_string());
    })?;
    let joins = (host.horse != rh.horse).then(|| host.horse.clone());
    let native = host.binding.native_id.clone().unwrap_or_default();
    let body = json!({"type": "host.continue", "session": session, "host_session": host.id,
        "host_horse": host.horse, "run": run_id, "node": node, "kept": kept, "joins": joins,
        "added": total - kept, "host_rows": host_nodes.len(), "at_ms": crate::horse::now_ms()});
    run::append_horse_event_locked(locked, &rh.horse, Some(&rh.harness), &body)?;
    if host.horse != rh.horse {
        run::append_horse_event_locked(locked, &host.horse, Some(&rh.harness), &body)?;
    }
    Ok(Some(Linked {
        session: session.into(),
        host_session: host.id.clone(),
        harness: rh.harness.clone(),
        native_id: native,
        kept,
        added: total - kept,
        joins,
    }))
}

/// The run's session of `l` joins the horse of its host session: a
/// lineage step, and a `session.move` event in both horses. The run is
/// over, so nothing of the run moves with it; the DNA of the run's horse
/// does not graft into the host horse.
///
/// The checks of every move come first (lineage.rs): a session that runs
/// again, or that has hitch edges (the run hitched a child, or was
/// hitched), stays in the run's horse, with its edges. Its lineage to the
/// host session stays too; a `host.stay` event in the run's horse says why.
pub(crate) fn join(locked: &Lock<'_>, l: &Linked) -> Result<()> {
    let Some(to) = &l.joins else {
        return Ok(());
    };
    let store = locked.store;
    let from = store.session(&l.session)?.horse;
    if let Err(e) = crate::horse::lineage::check_movable(store, &store.horse(&from)?, &l.session) {
        let body = json!({"type": "host.stay", "session": l.session, "host_horse": to,
            "why": format!("{e:#}"), "at_ms": crate::horse::now_ms()});
        run::append_horse_event_locked(locked, &from, None, &body)?;
        return Err(e);
    }
    locked.move_session(&l.session, to)?;
    let body = json!({"type": "session.move", "session": l.session, "from_horse": from,
        "to_horse": to, "why": "host.continue", "at_ms": crate::horse::now_ms()});
    run::append_horse_event_locked(locked, &from, None, &body)?;
    run::append_horse_event_locked(locked, to, None, &body)?;
    Ok(())
}
