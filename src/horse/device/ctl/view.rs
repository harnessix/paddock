//! Derived views: `views/<horse>/<name>.json` in the store names a session
//! of the horse and a filter. The device shows each view as the read-only
//! file `self/views/<name>`: the message lines of the session that pass
//! the filter, in the format of `self/sessions/<id>/log`.

use crate::horse::device::Tree;
use crate::horse::{claude, dsh, Horse, Store};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Which messages a view shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Filter {
    All,
    /// The messages of one role.
    Role(String),
    /// The last N messages.
    Last(usize),
    /// The messages whose text has this text.
    Grep(String),
}

impl Filter {
    /// `all`, `role:ROLE`, `last:N` or `grep:TEXT`.
    pub fn parse(s: &str) -> Result<Filter> {
        let s = s.trim();
        if s == "all" {
            return Ok(Filter::All);
        }
        if let Some(r) = s.strip_prefix("role:").filter(|r| !r.is_empty()) {
            return Ok(Filter::Role(r.into()));
        }
        if let Some(n) = s.strip_prefix("last:").and_then(|n| n.parse().ok()) {
            return Ok(Filter::Last(n));
        }
        if let Some(t) = s.strip_prefix("grep:").filter(|t| !t.is_empty()) {
            return Ok(Filter::Grep(t.into()));
        }
        bail!("a view filter is all, role:ROLE, last:N or grep:TEXT")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub session: String,
    pub filter: Filter,
}

fn dir(horse: &str) -> String {
    format!("views/{horse}")
}

/// Save view `name` of `horse`. The caller checks the name and that the
/// session belongs to the horse.
pub(super) fn save(store: &Store, horse: &str, name: &str, v: &View) -> Result<()> {
    let _l = store.lock()?;
    store.write_ref(
        &format!("{}/{name}.json", dir(horse)),
        &serde_json::to_vec_pretty(v)?,
    )
}

/// Add `self/views/` and every view of `horse` whose session is still in
/// the horse.
pub(in crate::horse::device) fn add(store: &Store, h: &Horse, t: &mut Tree) -> Result<()> {
    t.dir("self/views");
    let d = dir(&h.id);
    if !store.root().join(&d).is_dir() {
        return Ok(());
    }
    for file in store.list_locked_refs(&d)? {
        let Some(name) = file.strip_suffix(".json") else {
            continue;
        };
        if !super::super::valid_name(name) {
            continue;
        }
        let Some(bytes) = store.read_ref(&format!("{d}/{file}"))? else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<View>(&bytes) else {
            continue;
        };
        // A session that moved away takes its content along.
        if !h.sessions.contains(&v.session) {
            continue;
        }
        let s = store.session(&v.session)?;
        if s.horse != h.id {
            continue;
        }
        let mut lines = Vec::new();
        for n in store.session_nodes(&s)? {
            let view = match s.binding.harness.as_str() {
                "dsh" => dsh::view(&n.raw),
                "claude" => claude::view(&n.raw),
                _ => None,
            };
            let Some(m) = view else { continue };
            let keep = match &v.filter {
                Filter::Role(r) => m.role == *r,
                Filter::Grep(x) => m.text.contains(x.as_str()),
                Filter::All | Filter::Last(_) => true,
            };
            if keep {
                let text: String = m.text.chars().take(120).collect();
                lines.push(format!(
                    "{} {} {}\n",
                    n.hash.short(),
                    m.role,
                    text.replace('\n', " ")
                ));
            }
        }
        if let Filter::Last(k) = v.filter {
            lines.drain(..lines.len().saturating_sub(k));
        }
        t.file(&format!("self/views/{name}"), lines.concat());
    }
    Ok(())
}
