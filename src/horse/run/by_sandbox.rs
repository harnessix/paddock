//! The run of a sandbox, so a child's host process finds the horse of its
//! parent run. The parent's host process writes it at the run start. The
//! index lives in the store, which no sandbox sees.

use super::{RunHorse, Store};
use anyhow::{bail, Result};
use serde_json::{json, Value};

fn path(sandbox: &str) -> Result<String> {
    let ok = !sandbox.is_empty()
        && sandbox.len() <= 128
        && !sandbox.starts_with('.')
        && sandbox
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !ok {
        bail!("not a sandbox id: {sandbox}");
    }
    Ok(format!("runs/{sandbox}.json"))
}

/// Remember that this sandbox runs this horse session.
pub fn record(store: &Store, sandbox: &str, rh: &RunHorse) -> Result<()> {
    let body = json!({"horse": rh.horse, "session": rh.session, "harness": rh.harness});
    store.write_ref(&path(sandbox)?, &serde_json::to_vec(&body)?)
}

/// The run of a sandbox, when it ran in horse mode.
pub fn find(store: &Store, sandbox: &str) -> Result<Option<RunHorse>> {
    let Some(bytes) = store.read_ref(&path(sandbox)?)? else {
        return Ok(None);
    };
    let v: Value = serde_json::from_slice(&bytes)?;
    let field = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let rh = RunHorse {
        horse: field("horse"),
        session: field("session"),
        harness: field("harness"),
    };
    // A stale or damaged record names no horse.
    Ok(store.horse(&rh.horse).is_ok().then_some(rh))
}
