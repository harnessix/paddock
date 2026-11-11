//! Host-owned definitions for scheduled jobs.

use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod budget;
pub mod check;
pub mod git;
pub mod host_checks;
pub mod limits;
pub mod pause;
pub mod report;
pub mod units;
pub mod wake;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub harness: String,
    pub horse: Option<String>,
    #[serde(default = "default_mode")]
    pub mode: String,
    pub check: String,
    pub check_timeout: Option<String>,
    pub trigger: String,
    pub schedule: Option<String>,
    pub jitter: Option<String>,
    pub branch: Option<String>,
    pub project: Option<String>,
    pub poll_interval: Option<String>,
    pub quiet_hours: Option<String>,
    pub max_load: Option<f64>,
    pub max_runtime: Option<String>,
    pub max_mem: Option<String>,
    pub mailbox: Option<String>,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub suites: Vec<String>,
    pub suite_timeout: Option<String>,
}

fn default_mode() -> String {
    "fork".into()
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

impl Job {
    fn validate(&self, name: &str) -> Result<()> {
        if !valid_name(name) {
            bail!("job names may contain only letters, numbers, '-' and '_'");
        }
        if self.harness.trim().is_empty() || self.check.trim().is_empty() {
            bail!("harness and check must not be empty");
        }
        if self
            .mailbox
            .as_ref()
            .is_some_and(|name| !crate::view::valid_id(name))
        {
            bail!("mailbox must be a valid mailbox name");
        }
        if !matches!(self.mode.as_str(), "fork" | "resume") {
            bail!("mode must be fork or resume");
        }
        if !matches!(self.trigger.as_str(), "timer" | "git") {
            bail!("trigger must be timer or git");
        }
        if self.trigger == "timer" && self.schedule.as_deref().is_none_or(str::is_empty) {
            bail!("timer jobs need a schedule");
        }
        if self.trigger == "git"
            && (self.branch.as_deref().is_none_or(str::is_empty)
                || self.project.as_deref().is_none_or(str::is_empty))
        {
            bail!("git jobs need a project and branch");
        }
        if self.trigger == "git"
            && self
                .project
                .as_deref()
                .is_some_and(|path| !std::path::Path::new(path).is_absolute())
        {
            bail!("git job project must be an absolute path");
        }
        if name == "host-checks" {
            if self.harness != "none" || self.trigger != "git" || self.suites.is_empty() {
                bail!("host-checks requires harness = 'none', a git trigger, and suites");
            }
            if self.suites.iter().any(|suite| !valid_name(suite)) {
                bail!("suite names may contain only letters, numbers, '-' and '_'");
            }
        }
        for (field, value) in [
            ("poll_interval", &self.poll_interval),
            ("max_runtime", &self.max_runtime),
            ("check_timeout", &self.check_timeout),
            ("suite_timeout", &self.suite_timeout),
            ("jitter", &self.jitter),
        ] {
            if let Some(value) = value {
                let seconds = crate::timer::parse_duration(value)
                    .with_context(|| format!("invalid {field}"))?;
                if seconds == 0 || (field == "poll_interval" && seconds < 60) {
                    bail!("{field} is too short");
                }
            }
        }
        if let Some(quiet) = &self.quiet_hours {
            limits::quiet_range(quiet).context("invalid quiet_hours")?;
        }
        if let Some(mem) = &self.max_mem {
            let digits = mem.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let suffix = &mem[digits.len()..];
            if digits.is_empty()
                || !digits.bytes().all(|c| c.is_ascii_digit())
                || digits.parse::<u64>().ok().is_none_or(|n| n == 0)
                || !matches!(suffix, "" | "K" | "M" | "G" | "T")
            {
                bail!("max_mem must be a positive size such as 512M or 2G");
            }
        }
        if self.max_load.is_some_and(|n| !n.is_finite() || n <= 0.0) {
            bail!("max_load must be positive");
        }
        Ok(())
    }
}

/// Read only the host user's configuration; project files never supply jobs.
pub fn load(env: &HostEnv) -> Result<BTreeMap<String, Job>> {
    let file = env.paddock_config().join("config.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", file.display())),
    };
    parse(&text).with_context(|| format!("{} is invalid", file.display()))
}

pub fn parse(text: &str) -> Result<BTreeMap<String, Job>> {
    let value: toml::Value = toml::from_str(text)?;
    let jobs: BTreeMap<String, Job> = match value.get("turnout") {
        Some(value) => value.clone().try_into()?,
        None => BTreeMap::new(),
    };
    for (name, job) in &jobs {
        job.validate(name)
            .with_context(|| format!("turnout job {name}"))?;
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jobs_are_validated_and_project_definitions_refused() {
        let text = "[turnout.checks]\nharness = 'none'\ncheck = 'echo work'\ntrigger = 'git'\nproject = '/tmp/repo'\nbranch = 'main'\npoll_interval = '60s'\n";
        assert_eq!(parse(text).unwrap()["checks"].mode, "fork");
        assert!(parse(&text.replace("60s", "59s")).is_err());
        assert!(parse(&text.replace("/tmp/repo", "relative/repo")).is_err());
        assert!(parse(&text.replace("'git'", "'unknown'")).is_err());
        assert!(toml::from_str::<crate::manifest::Manifest>(text).is_err());
        assert!(crate::manifest::Manifest::parse_user(text).is_ok());
    }
}
