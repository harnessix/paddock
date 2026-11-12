//! Named namespaces: `--scope ns:NAME`, shared by the projects and the
//! harnesses that the user names in `[kv.namespaces.NAME]` of the user
//! configuration, and by nothing else.
//!
//! ```toml
//! [kv.namespaces.host-build]
//! projects = ["~/src/paddock", "~/src/loft"]
//! harnesses = ["claude", "codex"]   # optional; without it, every harness
//! ```
//!
//! The table is the approval: a member never sees a card, and a sandbox that
//! is not a member gets a refusal that names the table to change. Only the
//! user configuration has it. A project `paddock.toml` cannot set `[kv]`
//! (manifest.rs `USER_ONLY`), because the agent can edit the project.
//!
//! A sandbox that another one started is a member only when every sandbox
//! above it is a member too, so `spawn` never reaches a namespace that its
//! parent cannot use.

use super::Identity;
use crate::env::HostEnv;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The longest namespace name.
pub const MAX_NAME: usize = 64;

/// `[kv.namespaces.NAME]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    /// The project folders whose sandboxes are members: absolute, or under
    /// `~`. A worktree of paddock belongs to the repository it came from.
    pub projects: Vec<String>,
    /// The harnesses that are members. None: all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harnesses: Option<Vec<String>>,
}

/// Namespace names become file names: 1 to 64 lower-case letters, digits,
/// `-` and `_`, starting with a letter or a digit.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        bail!(
            "'{name}' is not a namespace name: use 1 to {MAX_NAME} lower-case letters, digits, '-' and '_', starting with a letter or a digit"
        );
    }
    Ok(())
}

fn harness_ok(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 64
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Check `[kv.namespaces]` when the configuration loads.
pub(crate) fn check(all: BTreeMap<String, Namespace>) -> Result<BTreeMap<String, Namespace>> {
    for (name, ns) in &all {
        check_name(name).map_err(|e| anyhow::anyhow!("[kv.namespaces.{name}]: {e:#}"))?;
        if ns.projects.is_empty() {
            bail!("[kv.namespaces.{name}]: projects is empty; name at least one project folder");
        }
        for p in &ns.projects {
            if !(p == "~" || p.starts_with("~/") || Path::new(p).is_absolute()) {
                bail!(
                    "[kv.namespaces.{name}]: the project '{p}' is not an absolute path; write it as /path or ~/path"
                );
            }
        }
        if let Some(hs) = &ns.harnesses {
            if hs.is_empty() {
                bail!(
                    "[kv.namespaces.{name}]: harnesses is empty, so no sandbox could use it; leave the key out for all harnesses"
                );
            }
            if let Some(bad) = hs.iter().find(|h| !harness_ok(h)) {
                bail!("[kv.namespaces.{name}]: '{bad}' is not a harness name");
            }
        }
    }
    Ok(all)
}

/// A path as the user wrote it, made comparable: `~` expanded, and the real
/// path when the folder exists.
fn resolve(env: &HostEnv, raw: &str) -> PathBuf {
    let p = env.expand(raw, Path::new("/"));
    std::fs::canonicalize(&p).unwrap_or(p)
}

fn same_project(env: &HostEnv, ns: &Namespace, project: &Path) -> bool {
    let real = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    ns.projects.iter().any(|raw| {
        let p = resolve(env, raw);
        p == project || p == real
    })
}

fn harness_member(ns: &Namespace, harness: &str) -> bool {
    ns.harnesses
        .as_ref()
        .is_none_or(|hs| hs.iter().any(|h| h == harness))
}

/// The user configuration file, for a message.
fn config_file(env: &HostEnv) -> String {
    crate::freeze::tilde(env, &env.paddock_config().join("config.toml"))
}

/// A sandbox above the caller in its spawn tree, as the registry recorded it.
pub(crate) struct Above {
    pub sandbox: String,
    /// None: paddock has no record of it.
    pub record: Option<(String, PathBuf)>,
}

/// Whether the caller may use the namespace `name`. The error is the text
/// of the refusal, and whether the namespace exists at all.
pub(crate) fn admit(
    env: &HostEnv,
    all: &BTreeMap<String, Namespace>,
    name: &str,
    id: &Identity,
    above: &[Above],
) -> Result<(), (bool, String)> {
    let file = config_file(env);
    let Some(ns) = all.get(name) else {
        return Err((
            false,
            format!(
                "there is no kv namespace '{name}'; the user defines it in {file}: [kv.namespaces.{name}] projects = [\"{}\"]",
                crate::freeze::tilde(env, &id.project)
            ),
        ));
    };
    if !same_project(env, ns, &id.project) {
        return Err((
            true,
            format!(
                "this project ({}) is not a member of the kv namespace '{name}'; the user adds it to projects in [kv.namespaces.{name}] of {file}",
                crate::freeze::tilde(env, &id.project)
            ),
        ));
    }
    if !harness_member(ns, &id.harness) {
        return Err((
            true,
            format!(
                "the harness '{}' is not a member of the kv namespace '{name}'; the user adds it to harnesses in [kv.namespaces.{name}] of {file}",
                id.harness
            ),
        ));
    }
    for a in above {
        let ok = a
            .record
            .as_ref()
            .is_some_and(|(h, p)| harness_member(ns, h) && same_project(env, ns, p));
        if !ok {
            let what = match &a.record {
                Some((h, _)) => format!("sandbox {} ({h})", a.sandbox),
                None => format!("sandbox {}, of which paddock has no record,", a.sandbox),
            };
            return Err((
                true,
                format!(
                    "{what} is above this sandbox in its spawn tree and is not a member of the kv namespace '{name}'; a spawned sandbox uses a namespace only when every sandbox above it is a member ([kv.namespaces.{name}] of {file})"
                ),
            ));
        }
    }
    Ok(())
}

/// The namespaces that a caller is a member of, for `paddock-host kv info`.
pub(crate) fn of(
    env: &HostEnv,
    all: &BTreeMap<String, Namespace>,
    id: &Identity,
    above: &[Above],
) -> Vec<serde_json::Value> {
    all.iter()
        .filter(|(name, _)| admit(env, all, name, id, above).is_ok())
        .map(|(name, ns)| {
            serde_json::json!({
                "scope": format!("ns:{name}"),
                "access": "read-write; shared with the projects below; untrusted",
                "projects": ns.projects,
                "harnesses": ns.harnesses.clone().map_or(serde_json::json!("all"), |h| h.into()),
            })
        })
        .collect()
}
