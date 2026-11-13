//! The environment of a run, in four groups (ENV1):
//!
//! 1. `agent`: needed by the agent. The basics (terminal, locale, time
//!    zone, proxy), the profile's `env`, and its login variables
//!    (`env_pass`), which pass from the host with a label.
//! 2. `yours`: `[env] pass` and `set`, for all agents, one agent
//!    (`[env.agent.NAME]`) or one project (`[env.project."PATH"]`).
//! 3. `keys`: `[credentials.NAME]`, with their grants and uses.
//! 4. `host`: every other host variable. In `listed` mode it stays on this
//!    computer; in `inherit` mode it passes unseen.
//!
//! Some names never pass (policy::never_pass), and names that look like
//! secrets or cloud credentials pass only as keys. They are "held".
//!
//! `paddock env show --json` gives this view to the launcher and the TUI
//! (`API`). Each run writes a record of names and origins, never values
//! (`RUN_API`), and `paddock explain` shows the same groups.

use crate::env::HostEnv;
use crate::manifest::{Manifest, ScopeCfg};
use crate::policy::{self, Never};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The version of `paddock env show --json`.
pub const API: &str = "paddock.env/v1";
/// The version of the record of a run (`env.json`, `paddock env show --run`).
pub const RUN_API: &str = "paddock.env.run/v1";
/// The version of `paddock env migrate --json`, the review of current users.
pub const REVIEW_API: &str = "paddock.env.review/v1";

/// The words under every list that a user changes.
pub const NEXT_START: &str = "Applies at the next start.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Only the four lists pass.
    Listed,
    /// The host environment passes, without the held names.
    Inherit,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Listed => "listed",
            Mode::Inherit => "inherit",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "listed" | "clean" => Some(Mode::Listed),
            "inherit" => Some(Mode::Inherit),
            _ => None,
        }
    }
}

/// The mode of a run and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolved {
    pub mode: Mode,
    /// "command line", "configuration", "forced", "current installation",
    /// "new installation".
    pub origin: &'static str,
    /// The review of current users is open: the version that wrote
    /// `mode = "inherit"`, and the version that forces `listed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<Review>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Review {
    pub migrated: String,
    pub forced_from: String,
}

/// The version of this paddock.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// The version two releases after `v`: the patch number plus two.
pub fn forced_from(v: &str) -> String {
    match parse_version(v) {
        Some((a, b, c)) => format!("{a}.{b}.{}", c + 2),
        None => v.to_string(),
    }
}

/// Whether the review period of a migration in version `migrated` is over
/// in version `now`: two releases later. A newer minor or major version
/// counts as one release, and each of its patches as one more.
pub fn forced(migrated: &str, now: &str) -> bool {
    let (Some(m), Some(n)) = (parse_version(migrated), parse_version(now)) else {
        return false;
    };
    let releases = if (n.0, n.1) == (m.0, m.1) {
        n.2.saturating_sub(m.2)
    } else if (n.0, n.1) > (m.0, m.1) {
        let minors = if n.0 == m.0 { n.1 - m.1 } else { 2 };
        n.2 + minors
    } else {
        0
    };
    releases >= 2
}

/// The marker of an installation that started with `listed`.
fn marker(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("env-default.json")
}

/// Whether this installation ran sandboxes before ENV1, or used `paddock
/// env hide`, which only `inherit` reads.
pub fn current_installation(env: &HostEnv, user: &Manifest) -> bool {
    if marker(env).is_file() {
        return false;
    }
    if env.paddock_state().join("recent.json").is_file() {
        return true;
    }
    let file = env.paddock_config().join("config.toml");
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("env").and_then(|e| e.get("hide")).cloned())
        .is_some()
        || !user.env.hide.is_empty()
}

/// The mode of the configuration `m` (merged user and project files),
/// without the command line. Pure: it writes nothing.
pub fn resolve(env: &HostEnv, m: &Manifest) -> Resolved {
    resolve_at(env, m, version())
}

pub fn resolve_at(env: &HostEnv, m: &Manifest, now: &str) -> Resolved {
    let review = m.env.migrated.as_ref().map(|v| Review {
        migrated: v.clone(),
        forced_from: forced_from(v),
    });
    if let Some(mode) = m.env.mode {
        if mode == Mode::Inherit {
            if let Some(r) = &review {
                if forced(&r.migrated, now) {
                    return Resolved {
                        mode: Mode::Listed,
                        origin: "forced",
                        review: None,
                    };
                }
            }
        }
        return Resolved {
            mode,
            origin: "configuration",
            review: review.filter(|_| mode == Mode::Inherit),
        };
    }
    if let Some(i) = m.env.inherit {
        return Resolved {
            mode: if i { Mode::Inherit } else { Mode::Listed },
            origin: "configuration",
            review: None,
        };
    }
    if current_installation(env, m) {
        return Resolved {
            mode: Mode::Inherit,
            origin: "current installation",
            review: Some(Review {
                migrated: now.to_string(),
                forced_from: forced_from(now),
            }),
        };
    }
    Resolved {
        mode: Mode::Listed,
        origin: "new installation",
        review: None,
    }
}

/// The mode of a run: the command line (`--env`), then the configuration.
pub fn run_mode(env: &HostEnv, m: &Manifest, cli: Option<bool>) -> Resolved {
    match cli {
        Some(i) => Resolved {
            mode: if i { Mode::Inherit } else { Mode::Listed },
            origin: "command line",
            review: None,
        },
        None => resolve(env, m),
    }
}

/// At a start: write down the choice for this installation, once. A
/// current installation gets `mode = "inherit"` with the version, and the
/// review opens. A new one gets a marker in the state folder: `listed`.
/// Returns true when it wrote the configuration.
pub fn settle(env: &HostEnv) -> Result<bool> {
    let user = Manifest::load_user(env)?;
    if user.env.mode.is_some() || user.env.inherit.is_some() || marker(env).is_file() {
        return Ok(false);
    }
    if current_installation(env, &user) {
        edit(env, |t| {
            t.insert("mode", toml_edit::value("inherit"));
            t.insert("migrated", toml_edit::value(version()));
            Ok(())
        })?;
        return Ok(true);
    }
    crate::privfs::dir(&env.paddock_state())?;
    crate::privfs::write_atomic(
        &marker(env),
        serde_json::to_vec(&serde_json::json!({"default": "listed", "since": version()}))?,
    )?;
    Ok(false)
}

// ---------------------------------------------------------------------
// Editing the user configuration.

fn config_file(env: &HostEnv) -> PathBuf {
    env.paddock_config().join("config.toml")
}

/// Change `[env]` of the user configuration with toml_edit, keeping the
/// rest of the file as it is.
fn edit(env: &HostEnv, f: impl FnOnce(&mut toml_edit::Table) -> Result<()>) -> Result<()> {
    let file = config_file(env);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is invalid", file.display()))?;
    let t = doc
        .entry("env")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .context("[env] in the configuration is not a table")?;
    f(t)?;
    let out = doc.to_string();
    // The result must still be a valid configuration.
    Manifest::parse_user(&out).context("the change would make the configuration invalid")?;
    std::fs::create_dir_all(file.parent().context("no configuration folder")?)?;
    std::fs::write(&file, out)?;
    Ok(())
}

/// `paddock env mode`: an explicit choice. It closes the review.
pub fn set_mode(env: &HostEnv, mode: Mode) -> Result<()> {
    edit(env, |t| {
        t.insert("mode", toml_edit::value(mode.name()));
        t.remove("migrated");
        t.remove("inherit");
        Ok(())
    })
}

/// Where `paddock env add` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    User,
    Agent(String),
    Project(String),
}

impl Scope {
    pub fn describe(&self) -> String {
        match self {
            Scope::User => "all agents".into(),
            Scope::Agent(a) => format!("the agent {a}"),
            Scope::Project(p) => format!("the project {p}"),
        }
    }
}

/// The key of a project in `[env.project]`: `~/...` under the home, the
/// absolute path otherwise.
pub fn project_key(env: &HostEnv, project: &Path) -> String {
    let p = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    for h in policy::homes(env) {
        if let Ok(rest) = p.strip_prefix(&h) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    p.display().to_string()
}

fn scope_table<'a>(t: &'a mut toml_edit::Table, scope: &Scope) -> Result<&'a mut toml_edit::Table> {
    let (kind, key) = match scope {
        Scope::User => return Ok(t),
        Scope::Agent(a) => ("agent", a.as_str()),
        Scope::Project(p) => ("project", p.as_str()),
    };
    let outer = t
        .entry(kind)
        .or_insert(toml_edit::table())
        .as_table_mut()
        .with_context(|| format!("[env.{kind}] is not a table"))?;
    outer.set_implicit(true);
    outer
        .entry(key)
        .or_insert(toml_edit::table())
        .as_table_mut()
        .with_context(|| format!("[env.{kind}.{key}] is not a table"))
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && n.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Why a name cannot be added to `[env]`: the sentence, and the exit code
/// of the CLI (2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub name: String,
    pub rule: &'static str,
    pub reason: &'static str,
    pub sentence: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.sentence)
    }
}

impl std::error::Error for Refusal {}

/// The exit code of a refused name on the host.
pub const REFUSED_EXIT: i32 = 2;
/// The exit code of a refused name in a sandbox (`paddock-host env request`).
pub const REFUSED_EXIT_SANDBOX: i32 = 77;

/// The refusal of a name, if it has one: the never-pass list, then the
/// names that pass only as keys. `value`: the host value, if any.
pub fn refusal(name: &str, value: Option<&str>) -> Option<Refusal> {
    if let Some(why) = policy::never_pass(name, value) {
        return Some(Refusal {
            name: name.into(),
            rule: "never",
            reason: why.name(),
            sentence: policy::never_sentence(name, why),
        });
    }
    if policy::is_cloud_secret(name) {
        return Some(Refusal {
            name: name.into(),
            rule: "key",
            reason: "cloud",
            sentence: format!(
                "{name} stays on this computer: it is a cloud or forge credential. It passes only as a key: paddock credentials add {name}. For AWS, use short-lived credentials with kind = \"aws\"."
            ),
        });
    }
    if policy::looks_secret(name) {
        return Some(Refusal {
            name: name.into(),
            rule: "key",
            reason: "secret",
            sentence: format!(
                "{name} stays on this computer: its name looks like a secret. It passes only as a key: paddock credentials add {name}"
            ),
        });
    }
    None
}

/// `paddock env add NAME` (`value` None: pass the host value) or
/// `NAME=VALUE` (a fixed value).
pub fn add(env: &HostEnv, scope: &Scope, name: &str, value: Option<&str>) -> Result<()> {
    if !valid_name(name) {
        bail!("'{name}' is not an environment variable name: use letters, digits and '_'");
    }
    if let Some(r) = refusal(name, env.var(name)) {
        return Err(r.into());
    }
    if crate::credentials::load(env)?
        .iter()
        .any(|(n, c)| c.sandbox_var(n) == name || c.host_var(n) == name)
    {
        bail!("{name} is a key in [credentials]: it passes through its grant, not through [env]");
    }
    edit(env, |t| {
        let t = scope_table(t, scope)?;
        match value {
            None => {
                let arr = t
                    .entry("pass")
                    .or_insert(toml_edit::value(toml_edit::Array::new()))
                    .as_array_mut()
                    .context("pass is not a list")?;
                if !arr.iter().any(|v| v.as_str() == Some(name)) {
                    arr.push(name);
                }
                if let Some(s) = t.get_mut("set").and_then(|s| s.as_table_like_mut()) {
                    s.remove(name);
                }
            }
            Some(v) => {
                let s = t
                    .entry("set")
                    .or_insert(toml_edit::value(toml_edit::InlineTable::new()))
                    .as_table_like_mut()
                    .context("set is not a table")?;
                s.insert(name, toml_edit::value(v));
                if let Some(arr) = t.get_mut("pass").and_then(|p| p.as_array_mut()) {
                    arr.retain(|v| v.as_str() != Some(name));
                }
            }
        }
        Ok(())
    })
}

/// `paddock env remove NAME`: out of `pass` and `set` of the scope.
/// Returns false when the scope did not have it.
pub fn remove(env: &HostEnv, scope: &Scope, name: &str) -> Result<bool> {
    let mut found = false;
    edit(env, |t| {
        let t = scope_table(t, scope)?;
        if let Some(arr) = t.get_mut("pass").and_then(|p| p.as_array_mut()) {
            let before = arr.len();
            arr.retain(|v| v.as_str() != Some(name));
            found |= arr.len() != before;
        }
        if let Some(s) = t.get_mut("set").and_then(|s| s.as_table_like_mut()) {
            found |= s.remove(name).is_some();
        }
        Ok(())
    })?;
    Ok(found)
}

mod groups;
mod lines;
mod record;
mod review;
pub use groups::*;
pub use lines::*;
pub use record::*;
pub use review::*;

#[cfg(test)]
#[path = "envset_tests.rs"]
mod tests;
