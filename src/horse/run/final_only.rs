//! Final attribution of every changed native transcript to this run's horse.
use super::setup::Custody;
use super::{claude, codex, dsh, record_event_locked, snapshot::Snapshot, RunHorse, Store};
use anyhow::{bail, Context, Result};
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
    for path in changed {
        let native = match run.harness.as_str() {
            claude::HARNESS => claude::read_file(&path)?.session_id,
            codex::HARNESS => {
                let text = std::fs::read_to_string(&path)?;
                let first: Value =
                    serde_json::from_str(text.lines().next().context("empty Codex transcript")?)?;
                anyhow::ensure!(
                    first["type"] == "session_meta",
                    "missing Codex session metadata"
                );
                first["payload"]["id"]
                    .as_str()
                    .context("missing Codex session id")?
                    .to_owned()
            }
            dsh::HARNESS => dsh::read_file(&path)?.header.id,
            _ => bail!("no codec for {}", run.harness),
        };
        let existing = store.sessions()?.into_iter().find(|s| {
            s.horse == run.horse
                && s.binding.harness == run.harness
                && s.binding.native_id.as_deref() == Some(&native)
        });
        if existing.is_none() && store.session(&run.session)?.binding.native_id.is_none() {
            // A new run owns a new session even when it resumes the same native id.
            locked.update_session(&run.session, |s| s.binding.native_id = Some(native.clone()))?;
        }
        let actual = match run.harness.as_str() {
            claude::HARNESS => {
                claude::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?
                    .session
            }
            codex::HARNESS => {
                codex::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?.session
            }
            dsh::HARNESS => {
                dsh::ingest_run_locked(&locked, &path, &main.binding.version, &run.horse)?.session
            }
            _ => unreachable!(),
        };
        anyhow::ensure!(
            store.session(&actual)?.horse == run.horse,
            "native transcript belongs to another horse"
        );
        store.refresh_guard(&actual)?;
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
    Ok(())
}
