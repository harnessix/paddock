//! Run events written while the caller holds the store lock.

use super::*;
use serde_json::json;

pub fn record_event_locked(
    locked: &super::super::store::Lock<'_>,
    rh: &RunHorse,
    kind: &str,
    sandbox: &str,
    extra: Value,
) -> Result<Hash> {
    let session = locked.store.session(&rh.session)?;
    let mut body = json!({"type": kind, "horse": rh.horse, "session": rh.session,
        "sandbox": sandbox, "harness": rh.harness, "version": session.binding.version,
        "at_ms": super::super::now_ms()});
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            body[key] = value.clone();
        }
    }
    append_event_locked(locked, rh, &body)
}

/// Reconcile a bound dsh session and record its end under one writer lock.
/// `sessions_root` is the dsh sessions directory in the sandbox home.
pub fn end_dsh(
    store: &Store,
    rh: &RunHorse,
    sandbox: &str,
    sessions_root: &std::path::Path,
    custody: bool,
) -> Result<()> {
    anyhow::ensure!(rh.harness == dsh::HARNESS, "expected a dsh session");
    let locked = store.lock()?;
    let session = store.session(&rh.session)?;
    anyhow::ensure!(session.horse == rh.horse, "session is not owned by horse");
    let mut nodes = 0;
    if let Some(native) = &session.binding.native_id {
        if let Some(dir) = super::super::live::custody::find_session_dir(sessions_root, native) {
            let result =
                dsh::ingest_locked(&locked, &dir, &session.binding.version, Some(&rh.horse))?;
            anyhow::ensure!(
                result.session == rh.session,
                "native session belongs to another session"
            );
            nodes = store.session_nodes(&store.session(&rh.session)?)?.len();
            if nodes > 0 {
                store.refresh_guard(&rh.session)?;
            }
        }
    }
    let end = record_event_locked(
        &locked,
        rh,
        "run.end",
        sandbox,
        json!({
            "nodes": nodes,
            "ok": true,
            "custody": if custody { "live" } else { "final-only" }
        }),
    )?;
    super::adapt_after(&locked, rh, true, &end);
    Ok(())
}
