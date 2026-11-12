//! The `ctl` grant of one sandbox: `ctl/grants/<sandbox>.json` in the
//! store, written only by the user on the host (`paddock horse grant`).
//!
//! A grant names the commands, the horses that a `move` may go to, a
//! budget of commands and a budget of tokens for cache breaks. The store
//! is never mounted into a sandbox, so a sandbox cannot change its grant.

use crate::horse::Store;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};

/// The commands of `ctl`, in the order of the help.
pub const COMMANDS: [&str; 5] = ["fork", "append", "move", "view", "disconnect"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub sandbox: String,
    /// The granted commands, a subset of [`COMMANDS`].
    pub commands: Vec<String>,
    /// The horses that `move` may go to.
    #[serde(default)]
    pub horses: Vec<String>,
    /// How many commands may run.
    pub budget: u64,
    /// How many tokens of cached prefix the approved breaks may lose.
    pub tokens: u64,
    #[serde(default)]
    pub used: u64,
    #[serde(default)]
    pub spent_tokens: u64,
    pub granted_ms: u64,
}

impl Grant {
    pub fn allows(&self, command: &str) -> bool {
        self.commands.iter().any(|c| c == command)
    }
    pub fn commands_left(&self) -> u64 {
        self.budget.saturating_sub(self.used)
    }
    pub fn tokens_left(&self) -> u64 {
        self.tokens.saturating_sub(self.spent_tokens)
    }
}

/// A sandbox id is one plain name.
pub fn valid_sandbox(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn path(sandbox: &str) -> String {
    format!("ctl/grants/{sandbox}.json")
}

/// The grant of `sandbox`, if the user gave one.
pub fn get(store: &Store, sandbox: &str) -> Result<Option<Grant>> {
    if !valid_sandbox(sandbox) {
        return Ok(None);
    }
    match store.read_ref(&path(sandbox))? {
        None => Ok(None),
        Some(b) => Ok(Some(serde_json::from_slice(&b).with_context(|| {
            format!("the ctl grant of sandbox {sandbox} is damaged")
        })?)),
    }
}

/// Write a new grant. It replaces an older one and its counters.
pub fn set(store: &Store, g: &Grant) -> Result<()> {
    if !valid_sandbox(&g.sandbox) {
        bail!("{:?} is not a sandbox id", g.sandbox);
    }
    if g.commands.is_empty() {
        bail!(
            "a grant needs at least one command: {}",
            COMMANDS.join(", ")
        );
    }
    for c in &g.commands {
        if !COMMANDS.contains(&c.as_str()) {
            bail!("{c} is not a ctl command: use {}", COMMANDS.join(", "));
        }
    }
    if g.allows("move") && g.horses.is_empty() {
        bail!("a grant of move needs at least one target horse (--horse)");
    }
    let _l = store.lock()?;
    for h in &g.horses {
        store.horse(h)?;
    }
    store.write_ref(&path(&g.sandbox), &serde_json::to_vec_pretty(g)?)
}

/// Remove the grant of `sandbox`. False when it had none.
pub fn revoke(store: &Store, sandbox: &str) -> Result<bool> {
    let _l = store.lock()?;
    if get(store, sandbox)?.is_none() {
        return Ok(false);
    }
    store.remove_ref(&path(sandbox))?;
    Ok(true)
}

/// Every grant of the store.
pub fn list(store: &Store) -> Result<Vec<Grant>> {
    let dir = "ctl/grants";
    if !store.root().join(dir).is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for name in store.list_locked_refs(dir)? {
        if let Some(id) = name.strip_suffix(".json") {
            if let Some(g) = get(store, id)? {
                out.push(g);
            }
        }
    }
    Ok(out)
}

/// Take one command and `tokens` from the budget, under the store lock.
/// False when the budget has no room: nothing changed then.
pub(super) fn spend(store: &Store, sandbox: &str, tokens: u64) -> Result<bool> {
    let _l = store.lock()?;
    let Some(mut g) = get(store, sandbox)? else {
        return Ok(false);
    };
    if g.commands_left() == 0 || g.tokens_left() < tokens {
        return Ok(false);
    }
    g.used += 1;
    g.spent_tokens += tokens;
    store.write_ref(&path(sandbox), &serde_json::to_vec_pretty(&g)?)?;
    Ok(true)
}

/// Give back what [`spend`] took, for a command that failed.
pub(super) fn refund(store: &Store, sandbox: &str, tokens: u64) -> Result<()> {
    let _l = store.lock()?;
    if let Some(mut g) = get(store, sandbox)? {
        g.used = g.used.saturating_sub(1);
        g.spent_tokens = g.spent_tokens.saturating_sub(tokens);
        store.write_ref(&path(sandbox), &serde_json::to_vec_pretty(&g)?)?;
    }
    Ok(())
}
