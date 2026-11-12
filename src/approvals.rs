//! Who approves the agent's steps: the agent itself (`auto`, the default)
//! or the user, in the agent's own interface (`manual`).
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
//! and last the default `auto`.

use crate::manifest::Manifest;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Approvals {
    /// The agent approves its own steps; the sandbox is the boundary.
    #[default]
    Auto,
    /// The user approves the agent's steps in the agent's interface.
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

/// The first mode that is set: flag, project, agent, user, then `auto`.
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
        (Approvals::Auto, "default".to_string())
    };
    Choice { mode, origin }
}

/// The mode of a harness from a merged manifest (user configuration plus
/// the project file) and the command line.
pub fn of(m: &Manifest, harness: &str, flag: Option<Approvals>) -> Choice {
    let agent = m
        .run
        .agents
        .get(harness)
        .and_then(|a| a.approvals)
        .map(|a| (harness, a));
    resolve(flag, m.run.project, agent, m.run.approvals)
}

/// The mode that a new run of `harness` in `project` gets without a flag.
/// Without a project, only the user configuration counts (the shims, the
/// start dialog of the TUI). An unreadable configuration counts as unset.
pub fn default_for(
    env: &crate::env::HostEnv,
    harness: &str,
    project: Option<&std::path::Path>,
) -> Choice {
    let m = match project {
        Some(p) => Manifest::load(env, p).map(|l| l.merged).ok(),
        None => Manifest::load_user(env).ok(),
    };
    of(&m.unwrap_or_default(), harness, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Approvals::{Auto, Manual};

    #[test]
    fn the_order_is_flag_project_agent_user_default() {
        let c = resolve(None, None, None, None);
        assert_eq!((c.mode, c.origin.as_str()), (Auto, "default"));
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
        assert_eq!(of(&m, "codex", None).mode, Manual);
        assert_eq!(of(&m, "claude", None).mode, Auto);
        assert_eq!(of(&m, "codex", Some(Auto)).mode, Auto);
    }
}
