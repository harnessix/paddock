//! Who approves the agent's steps: the user, in the agent's own interface
//! (`manual`, the default of a new installation), or the agent itself
//! (`auto`, an opt-in).
//!
//! The sandbox is the same in both modes. Only the agent's auto-approval
//! flags (`[launch] yolo_args`) and the settings that skip the agent's own
//! questions change:
//!
//! - `auto`: paddock adds `yolo_args`, writes the `yolo_only` settings (for
//!   example Claude Code's bypass warning), the trust settings (`trust`
//!   entries of `[[state.settings]]` and `[[state.toml_set]]`), and answers
//!   the folder trust screens.
//! - `manual`: none of them. The agent asks as it does outside paddock.
//!
//! The mode comes from, in this order: the command line (`--manual`, its
//! alias `--no-yolo`, or `--auto`), the project's `paddock.toml` (`[run]
//! approvals`, under the one-time approval), `[run.agents.NAME] approvals`
//! of the user configuration, `[run] approvals` of the user configuration,
//! and last the default of the installation (MANUAL2):
//!
//! - A new installation (no user configuration and no earlier run records
//!   when paddock first starts) gets `manual`. A marker in the state folder
//!   keeps that choice, so later runs and configuration do not change it.
//! - A current installation keeps `auto`, the mode it used until now.
//!   `settle` writes `[run] approvals = "auto"` into its configuration once
//!   and returns one line that says how to switch.

use crate::env::HostEnv;
use crate::manifest::Manifest;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Approvals {
    /// The agent approves its own steps; the sandbox is the boundary.
    Auto,
    /// The user approves the agent's steps in the agent's interface.
    #[default]
    Manual,
}

impl Approvals {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

impl std::fmt::Display for Approvals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for Approvals {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "auto" => Ok(Self::Auto),
            "manual" => Ok(Self::Manual),
            _ => anyhow::bail!("unknown approvals mode '{s}': use auto or manual"),
        }
    }
}

/// The mode of a run and where it comes from, for `paddock explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub mode: Approvals,
    pub origin: String,
}

/// The name of a mode as runs report it: `auto`, `manual`, or `command`
/// for a run whose own command decides (`paddock wrap`, desktop apps):
/// paddock adds no approval flags of its own there.
pub fn shown(mode: Option<Approvals>) -> &'static str {
    mode.map_or("command", Approvals::name)
}

/// The origin of the mode when nothing sets it.
pub const DEFAULT: &str = "default";
/// The origin of `auto` for a current installation without the setting.
pub const CURRENT: &str = "current installation";

/// The first mode that is set: flag, project, agent, user, then the
/// default `manual`.
pub fn resolve(
    flag: Option<Approvals>,
    project: Option<Approvals>,
    agent: Option<(&str, Approvals)>,
    user: Option<Approvals>,
) -> Choice {
    let (mode, origin) = if let Some(m) = flag {
        (m, "command line".to_string())
    } else if let Some(m) = project {
        (m, "paddock.toml [run] approvals".to_string())
    } else if let Some((name, m)) = agent {
        (
            m,
            format!("user configuration [run.agents.{name}] approvals"),
        )
    } else if let Some(m) = user {
        (m, "user configuration [run] approvals".to_string())
    } else {
        (Approvals::Manual, DEFAULT.to_string())
    };
    Choice { mode, origin }
}

/// The mode of a harness from a merged manifest (user configuration plus
/// the project file) and the command line. Without a setting, the default
/// of the installation decides (`installation_default`).
pub fn of(env: &HostEnv, m: &Manifest, harness: &str, flag: Option<Approvals>) -> Choice {
    let agent = m
        .run
        .agents
        .get(harness)
        .and_then(|a| a.approvals)
        .map(|a| (harness, a));
    let c = resolve(flag, m.run.project, agent, m.run.approvals);
    if c.origin == DEFAULT && installation_default(env) == Approvals::Auto {
        return Choice {
            mode: Approvals::Auto,
            origin: CURRENT.into(),
        };
    }
    c
}

/// The mode that a new run of `harness` in `project` gets without a flag.
/// Without a project, only the user configuration counts (the shims, the
/// start dialog of the TUI). An unreadable configuration counts as unset.
pub fn default_for(env: &HostEnv, harness: &str, project: Option<&std::path::Path>) -> Choice {
    let m = match project {
        Some(p) => Manifest::load(env, p).map(|l| l.merged).ok(),
        None => Manifest::load_user(env).ok(),
    };
    of(env, &m.unwrap_or_default(), harness, None)
}

// ---------------------------------------------------------------------
// New and current installations (MANUAL2).

/// The marker of the installation default: `{"default": "manual"}` for a
/// new installation, `{"default": "auto"}` after `settle` wrote the mode of
/// a current one.
fn marker(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("approvals-default.json")
}

fn config_file(env: &HostEnv) -> PathBuf {
    env.paddock_config().join("config.toml")
}

/// The default in the marker, if there is a marker.
fn marked(env: &HostEnv) -> Option<Approvals> {
    let text = std::fs::read_to_string(marker(env)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("default")?.as_str()?.parse().ok()
}

/// Whether this host used paddock before: a user configuration, or the
/// records of earlier runs (the recent projects, the environment records).
pub fn used_before(env: &HostEnv) -> bool {
    let state = env.paddock_state();
    config_file(env).exists()
        || state.join("recent.json").exists()
        || std::fs::read_dir(state.join("env-runs")).is_ok_and(|mut d| d.next().is_some())
}

/// The mode of a run that no setting decides: the marker, else `manual`
/// for a host that did not use paddock before, else `auto`.
pub fn installation_default(env: &HostEnv) -> Approvals {
    marked(env).unwrap_or(if used_before(env) {
        Approvals::Auto
    } else {
        Approvals::Manual
    })
}

fn write_marker(env: &HostEnv, mode: Approvals) -> Result<()> {
    crate::privfs::dir(&env.paddock_state())?;
    let v = serde_json::json!({"default": mode.name(), "since": env!("CARGO_PKG_VERSION")});
    crate::privfs::write_atomic(&marker(env), serde_json::to_vec(&v)?)?;
    Ok(())
}

/// The first start of any paddock command: a host that did not use paddock
/// before is a new installation. The marker keeps that, before other
/// commands write a configuration or run records. Errors are ignored: the
/// next command tries again.
pub fn note_new_installation(env: &HostEnv) {
    if marker(env).exists() || used_before(env) {
        return;
    }
    if let Err(e) = write_marker(env, Approvals::Manual) {
        log::debug!("cannot write {}: {e:#}", marker(env).display());
    }
}

/// The line that a current installation sees once, when `settle` writes
/// its mode.
pub const KEPT_AUTO: &str = "paddock: you keep auto approvals, as before ([run] approvals = \"auto\" in ~/.config/paddock/config.toml). To approve each step yourself, set it to \"manual\" or start with --manual.";

/// Before a run: a current installation without `[run] approvals` gets
/// `approvals = "auto"` in its user configuration, once. Returns the line
/// to show, or None when nothing changed. The rest of the file stays as it
/// is (toml_edit).
pub fn settle(env: &HostEnv) -> Result<Option<&'static str>> {
    if marker(env).exists() {
        return Ok(None);
    }
    if !used_before(env) {
        write_marker(env, Approvals::Manual)?;
        return Ok(None);
    }
    if Manifest::load_user(env)?.run.approvals.is_some() {
        write_marker(env, Approvals::Auto)?;
        return Ok(None);
    }
    let file = config_file(env);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("{} is invalid", file.display()))?;
    doc.entry("run")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .context("[run] in the configuration is not a table")?
        .insert("approvals", toml_edit::value(Approvals::Auto.name()));
    let out = doc.to_string();
    Manifest::parse_user(&out).context("the change would make the configuration invalid")?;
    std::fs::create_dir_all(file.parent().context("no configuration folder")?)?;
    std::fs::write(&file, out)?;
    write_marker(env, Approvals::Auto)?;
    Ok(Some(KEPT_AUTO))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Approvals::{Auto, Manual};

    #[test]
    fn the_order_is_flag_project_agent_user_default() {
        let c = resolve(None, None, None, None);
        assert_eq!((c.mode, c.origin.as_str()), (Manual, "default"));
        let c = resolve(None, None, None, Some(Auto));
        assert_eq!(c.mode, Auto);
        let c = resolve(None, None, None, Some(Manual));
        assert_eq!(c.mode, Manual);
        assert_eq!(c.origin, "user configuration [run] approvals");
        let c = resolve(None, None, Some(("codex", Auto)), Some(Manual));
        assert_eq!(c.mode, Auto);
        assert_eq!(c.origin, "user configuration [run.agents.codex] approvals");
        let c = resolve(None, Some(Manual), Some(("codex", Auto)), Some(Auto));
        assert_eq!(c.mode, Manual);
        assert_eq!(c.origin, "paddock.toml [run] approvals");
        let c = resolve(
            Some(Auto),
            Some(Manual),
            Some(("codex", Manual)),
            Some(Manual),
        );
        assert_eq!((c.mode, c.origin.as_str()), (Auto, "command line"));
        let c = resolve(Some(Manual), None, None, None);
        assert_eq!((c.mode, c.origin.as_str()), (Manual, "command line"));
    }

    #[test]
    fn names_parse_and_print() {
        assert_eq!("auto".parse::<Approvals>().unwrap(), Auto);
        assert_eq!("manual".parse::<Approvals>().unwrap(), Manual);
        assert!("yolo".parse::<Approvals>().is_err());
        assert_eq!(Manual.to_string(), "manual");
        assert_eq!(shown(Some(Auto)), "auto");
        assert_eq!(shown(None), "command");
    }

    /// `paddock ps --json` has `approvals`; a record of an older paddock
    /// reads as `auto`, which was the only mode it started with flags.
    #[test]
    fn a_run_record_names_its_mode() {
        let old: crate::state::Running =
            serde_json::from_str(r#"{"id":"a","harness":"codex","project":"/p","backend":"bwrap","network":"egress","gui":"off","pid":1,"started":1}"#).unwrap();
        assert_eq!(old.approvals, "auto");
        let r = crate::state::Running {
            approvals: shown(Some(Manual)).into(),
            ..old
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["approvals"], "manual");
    }

    #[test]
    fn the_agent_table_applies_only_to_its_agent() {
        let m = Manifest::parse_user(
            "[run]\napprovals = \"auto\"\n[run.agents.codex]\napprovals = \"manual\"\n",
        )
        .unwrap();
        let (_t, env) = host();
        assert_eq!(of(&env, &m, "codex", None).mode, Manual);
        assert_eq!(of(&env, &m, "claude", None).mode, Auto);
        assert_eq!(of(&env, &m, "codex", Some(Auto)).mode, Auto);
    }

    /// A host with a home and nothing of paddock in it.
    fn host() -> (tempfile::TempDir, HostEnv) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HostEnv::from_parts(home, "t".into(), 1000, 1000, Default::default());
        (t, env)
    }

    fn cfg(env: &HostEnv) -> String {
        std::fs::read_to_string(config_file(env)).unwrap_or_default()
    }

    fn write_cfg(env: &HostEnv, text: &str) {
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(config_file(env), text).unwrap();
    }

    fn mode(env: &HostEnv) -> Choice {
        of(env, &Manifest::load_user(env).unwrap(), "claude", None)
    }

    #[test]
    fn a_new_installation_runs_in_manual_mode_and_stays_there() {
        let (_t, env) = host();
        assert!(!used_before(&env));
        assert_eq!(
            (mode(&env).mode, mode(&env).origin.as_str()),
            (Manual, DEFAULT)
        );
        // The first command marks the installation as new.
        note_new_installation(&env);
        assert_eq!(marked(&env), Some(Manual));
        // Configuration and runs later do not make it a current one.
        write_cfg(&env, "# mine\n[network]\nmode = \"egress\"\n");
        std::fs::write(env.paddock_state().join("recent.json"), "[]").unwrap();
        assert!(used_before(&env));
        assert_eq!(settle(&env).unwrap(), None);
        assert_eq!(cfg(&env), "# mine\n[network]\nmode = \"egress\"\n");
        assert_eq!(
            (mode(&env).mode, mode(&env).origin.as_str()),
            (Manual, DEFAULT)
        );
        // The flags work as before.
        let m = Manifest::load_user(&env).unwrap();
        assert_eq!(of(&env, &m, "claude", Some(Auto)).mode, Auto);
        assert_eq!(of(&env, &m, "claude", Some(Auto)).origin, "command line");
    }

    #[test]
    fn a_current_installation_keeps_auto_written_once() {
        // Earlier runs and no configuration.
        let (_t, env) = host();
        std::fs::create_dir_all(env.paddock_state()).unwrap();
        std::fs::write(env.paddock_state().join("recent.json"), "[]").unwrap();
        note_new_installation(&env);
        assert_eq!(marked(&env), None);
        assert_eq!(
            (mode(&env).mode, mode(&env).origin.as_str()),
            (Auto, CURRENT)
        );
        assert_eq!(settle(&env).unwrap(), Some(KEPT_AUTO));
        assert!(
            cfg(&env).contains("[run]\napprovals = \"auto\""),
            "{}",
            cfg(&env)
        );
        let c = mode(&env);
        assert_eq!(
            (c.mode, c.origin.as_str()),
            (Auto, "user configuration [run] approvals")
        );
        // Once: the line does not come again, and the file stays.
        let text = cfg(&env);
        assert_eq!(settle(&env).unwrap(), None);
        assert_eq!(cfg(&env), text);
        // --manual works as before.
        let m = Manifest::load_user(&env).unwrap();
        assert_eq!(of(&env, &m, "claude", Some(Manual)).mode, Manual);

        // A configuration without runs: the rest of the file stays.
        let (_t, env) = host();
        write_cfg(&env, "# notes\n[network]\nmode = \"egress\"\n");
        assert_eq!(settle(&env).unwrap(), Some(KEPT_AUTO));
        let text = cfg(&env);
        assert!(text.starts_with("# notes\n[network]\n"), "{text}");
        assert!(text.contains("approvals = \"auto\""), "{text}");

        // An environment record of an earlier run counts too.
        let (_t, env) = host();
        std::fs::create_dir_all(env.paddock_state().join("env-runs")).unwrap();
        assert!(!used_before(&env));
        std::fs::write(env.paddock_state().join("env-runs/r1.json"), "{}").unwrap();
        assert!(used_before(&env));
    }

    #[test]
    fn a_users_own_setting_is_never_rewritten() {
        let (_t, env) = host();
        write_cfg(&env, "[run]\napprovals = \"manual\"\n");
        assert_eq!(settle(&env).unwrap(), None);
        assert_eq!(cfg(&env), "[run]\napprovals = \"manual\"\n");
        assert_eq!(mode(&env).mode, Manual);
        // An agent table alone is not the mode of the other agents.
        let (_t, env) = host();
        write_cfg(&env, "[run.agents.codex]\napprovals = \"manual\"\n");
        assert_eq!(settle(&env).unwrap(), Some(KEPT_AUTO));
        let m = Manifest::load_user(&env).unwrap();
        assert_eq!(of(&env, &m, "codex", None).mode, Manual);
        assert_eq!(of(&env, &m, "claude", None).mode, Auto);
    }
}
