//! Final attribution of every changed native transcript to this run's horse.
use super::setup::Custody;
use super::{
    agy, claude, codex, dsh, grok, record_event_locked, snapshot::Snapshot, RunHorse, Store,
};
use crate::horse::host::shadow::{self, Base, Linked};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// `run_ok`: the harness exited with code 0. `live`: the custody of a
/// live or channel run and its report, `None` for a final-only run. The
/// report's fields go into the `run.end` event with `added`,
/// the nodes that this final ingest added (0 when the custody ingested
/// every turn).
/// The adapt rules of the DNA run after the `run.end` event.
pub fn end(
    store: &Store,
    run: &RunHorse,
    sandbox: &str,
    home: &Path,
    before: &Snapshot,
    run_ok: bool,
    live: Option<(Custody, Value)>,
) -> Result<()> {
    end_with(store, run, sandbox, home, before, run_ok, live, None).map(|_| ())
}

/// [`end`] for a run that may continue host sessions: `base` is the
/// snapshot of a shadow run before the run (host/shadow.rs). Returns the
/// host sessions that the run continued.
#[allow(clippy::too_many_arguments)]
pub fn end_with(
    store: &Store,
    run: &RunHorse,
    sandbox: &str,
    home: &Path,
    before: &Snapshot,
    run_ok: bool,
    live: Option<(Custody, Value)>,
    base: Option<&Base>,
) -> Result<Vec<Linked>> {
    let mut linked = Vec::new();
    let changed = before.changed(home, &run.harness)?;
    let locked = store.lock()?;
    let main = store.session(&run.session)?;
    anyhow::ensure!(main.horse == run.horse, "session is not owned by horse");
    let count = || -> Result<usize> {
        let mut n = 0;
        for id in store.horse(&run.horse)?.sessions {
            n += store.session_nodes(&store.session(&id)?)?.len();
        }
        Ok(n)
    };
    let had = count()?;
    let mut changed = changed;
    if run.harness == grok::HARNESS {
        // One ingest per session folder, whichever of its files changed.
        changed = changed
            .into_iter()
            .filter_map(|p| p.parent().map(std::path::Path::to_path_buf))
            .collect();
        changed.sort();
        changed.dedup();
    }
    if run.harness == agy::HARNESS {
        // One ingest per database, whether it or its WAL changed.
        changed = changed
            .into_iter()
            .map(|p| match p.to_str().and_then(|s| s.strip_suffix("-wal")) {
                Some(db) => std::path::PathBuf::from(db),
                None => p,
            })
            .collect();
        changed.sort();
        changed.dedup();
    }
    for path in changed {
        let native = crate::horse::host::native_id(&run.harness, &path)?;
        // The host session that this file continues, from the snapshot
        // before the run. A file that the base cannot read gets no lineage.
        let reader = match run.harness.as_str() {
            codex::HARNESS => main
                .codex_reader_version
                .clone()
                .context("Codex reader version is unavailable for final ingest")?,
            _ => main.binding.version.clone(),
        };
        let host = match base.zip(path.strip_prefix(home).ok()) {
            Some((b, rel)) => match b.session(rel) {
                Some(copy) => match shadow::host_parent(
                    &locked,
                    &run.harness,
                    &reader,
                    &run.horse,
                    b,
                    rel,
                    &copy,
                ) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        log::warn!("horse: no host session for {}: {e:#}", rel.display());
                        None
                    }
                },
                None => None,
            },
            None => None,
        };
        let matches: Vec<_> = store
            .sessions()?
            .into_iter()
            .filter(|s| {
                s.host.is_none()
                    && s.binding.harness == run.harness
                    && s.binding.native_id.as_deref() == Some(&native)
            })
            .collect();
        let own = matches.iter().any(|s| s.horse == run.horse);
        let main_bound = store.session(&run.session)?.binding.native_id.is_some();
        if !own && !matches.is_empty() && main_bound {
            continue;
        }
        if !own && !main_bound {
            locked.update_session(&run.session, |s| s.binding.native_id = Some(native.clone()))?;
        }
        let actual = match run.harness.as_str() {
            claude::HARNESS => {
                claude::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?
                    .session
            }
            codex::HARNESS => {
                let reader = main
                    .codex_reader_version
                    .as_deref()
                    .context("Codex reader version is unavailable for final ingest")?;
                codex::ingest_run_locked(&locked, &path, reader, &run.horse)?.session
            }
            dsh::HARNESS => {
                dsh::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?.session
            }
            grok::HARNESS => {
                grok::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?.session
            }
            agy::HARNESS => {
                agy::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?.session
            }
            _ => unreachable!(),
        };
        anyhow::ensure!(
            store.session(&actual)?.horse == run.horse,
            "native transcript belongs to another horse"
        );
        store.refresh_guard(&actual)?;
        if let Some(h) = &host {
            // A lineage that cannot be recorded never stops the final
            // ingest: the rows are in, and `run.end` must follow.
            match shadow::link(&locked, run, sandbox, h, &actual) {
                Ok(l) => linked.extend(l),
                Err(e) => log::warn!(
                    "horse: session {actual} has no lineage to the host session {}: {e:#}",
                    h.id
                ),
            }
        }
    }
    let nodes = count()?;
    let mut extra = json!({"nodes": nodes, "ok": run_ok,
        "custody": live.as_ref().map_or(Custody::FinalOnly, |l| l.0).label()});
    if let Some((_, Value::Object(report))) = live {
        extra["added"] = json!(nodes.saturating_sub(had));
        for (k, v) in report {
            extra[k.as_str()] = v;
        }
    }
    let end = record_event_locked(&locked, run, "run.end", sandbox, extra)?;
    super::adapt_after(&locked, run, run_ok, &end);
    // The run is over: a session that continues a host session of another
    // horse (an import or an adoption) joins that horse.
    for l in &mut linked {
        if let Err(e) = shadow::join(&locked, l) {
            log::warn!(
                "horse: session {} stays in horse {}: {e:#}",
                l.session,
                run.horse
            );
            l.joins = None;
        }
    }
    Ok(linked)
}

#[cfg(test)]
mod ownership_tests;
