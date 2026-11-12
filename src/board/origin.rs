//! The prompt of a run, recorded when the run starts, so the board maps a
//! run to its ticket. `<state>/board/<ID>.json`, which no sandbox sees:
//!
//! - `file`: the prompt file as the starter named it (`paddock run
//!   --detach --prompt-file`, `paddock-host spawn --prompt-file`). A
//!   relative spawn path is relative to the project of the parent.
//! - `sha256`: the prompt text, for a prompt that names no file.
//! - `model`: the model that paddock chose for a spawned run.

use crate::env::HostEnv;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Records older than this go when a new one is written.
const KEEP: u64 = 30 * 24 * 3600;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default)]
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub fn dir(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("board")
}

fn file(env: &HostEnv, id: &str) -> Option<PathBuf> {
    crate::view::valid_id(id).then(|| dir(env).join(format!("{id}.json")))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Record the prompt of run `id`.
pub fn record(
    env: &HostEnv,
    id: &str,
    prompt_file: Option<&Path>,
    prompt: &[u8],
    model: Option<&str>,
) -> Result<()> {
    let f = file(env, id).with_context(|| format!("{id:?} is not a sandbox id"))?;
    let o = Origin {
        file: prompt_file.map(|p| p.display().to_string()),
        sha256: sha256_hex(prompt),
        model: model.filter(|m| !m.is_empty()).map(str::to_string),
    };
    crate::privfs::dir(&dir(env))?;
    prune(env, crate::state::now());
    crate::privfs::write_atomic(&f, serde_json::to_vec(&o)?)
        .with_context(|| format!("cannot record the prompt of {id}"))
}

/// Record and only log a failure: the run starts anyway.
pub fn note(
    env: &HostEnv,
    id: &str,
    prompt_file: Option<&Path>,
    prompt: &[u8],
    model: Option<&str>,
) {
    if let Err(e) = record(env, id, prompt_file, prompt, model) {
        log::warn!("the board cannot map run {id} to its ticket: {e:#}");
    }
}

/// Remove the record of run `id`, for a run that failed to start.
pub fn forget(env: &HostEnv, id: &str) {
    if let Some(f) = file(env, id) {
        let _ = std::fs::remove_file(f);
    }
}

pub fn load(env: &HostEnv, id: &str) -> Option<Origin> {
    let bytes = std::fs::read(file(env, id)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn prune(env: &HostEnv, now: u64) {
    let Ok(rd) = std::fs::read_dir(dir(env)) else {
        return;
    };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|t| now.saturating_sub(t.as_secs()) > KEEP);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
