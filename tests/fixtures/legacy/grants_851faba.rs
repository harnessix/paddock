//! Compatibility fixture: the relevant closed-enum schema and startup and
//! writer behavior from 851faba:src/macvm/grants.rs and host.rs. Kept
//! independent of the current grants types so enum drift cannot hide a break.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    #[serde(default = "version_one")]
    pub v: u32,
    #[serde(default)]
    pub shares: Vec<FolderGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default)]
    pub clipboard: Clipboard,
    #[serde(default)]
    pub changes: Vec<Change>,
}

impl Default for Grants {
    fn default() -> Self {
        Self {
            v: 1,
            shares: Vec::new(),
            network: None,
            clipboard: Clipboard::default(),
            changes: Vec::new(),
        }
    }
}
fn version_one() -> u32 {
    1
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderGrant {
    pub path: PathBuf,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unprotected: bool,
    #[serde(default)]
    pub added: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clipboard {
    #[serde(default = "yes")]
    pub to_app: bool,
    #[serde(default = "yes")]
    pub from_app: bool,
}
impl Default for Clipboard {
    fn default() -> Self {
        Self {
            to_app: true,
            from_app: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum What {
    Folder,
    Network,
    Clipboard,
    Rule,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applies {
    Live,
    ReopenFiles,
    NextStart,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    You,
    QuestionPanel,
    Approver,
    Profile,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub t: u64,
    pub what: What,
    pub action: String,
    pub target: String,
    pub value: String,
    pub applies: Applies,
    pub source: Source,
}

/// Pure equivalent of the legacy `load` for an existing file.
pub fn load(bytes: &[u8]) -> Result<Grants, String> {
    let grants: Grants = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if grants.v > 1 {
        return Err("newer grants".into());
    }
    Ok(grants)
}

/// Legacy `load_for_start`: malformed policy is discarded, clipboard is
/// disabled, and the caller receives a warning.
pub fn load_for_start(bytes: &[u8]) -> (Grants, bool) {
    match load(bytes) {
        Ok(g) => (g, false),
        Err(_) => (
            Grants {
                clipboard: Clipboard {
                    to_app: false,
                    from_app: false,
                },
                ..Grants::default()
            },
            true,
        ),
    }
}

/// `host.rs` uses ask when no network override or saved word is present.
pub fn startup_network(g: &Grants) -> &str {
    g.network.as_deref().unwrap_or("ask")
}

/// Legacy `update` loads, changes, and serializes grants.json under its lock.
pub fn update(bytes: &[u8], f: impl FnOnce(&mut Grants)) -> Result<Vec<u8>, String> {
    let mut g = load(bytes)?;
    f(&mut g);
    g.v = 1;
    let start = g.changes.len().saturating_sub(50);
    g.changes.drain(..start);
    serde_json::to_vec_pretty(&g).map_err(|e| e.to_string())
}
