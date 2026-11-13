//! `[copy]` of the user configuration.
//!
//! ```toml
//! [copy]
//! default = "auto"      # auto, always or never
//! apply = "ask"         # ask or auto
//! max = "1G"            # the most bytes of a plain copy
//! store_dir = "/path"   # the folder of the stores
//! ```
//!
//! A project `paddock.toml` cannot set the table: the agent can edit the
//! project (manifest.rs `USER_ONLY`).

use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::path::PathBuf;

/// `[copy] max` when the configuration does not set it.
pub const DEFAULT_MAX: u64 = 1_000_000_000;

/// `[copy] default`: which runs work on a copy without `--copy`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Default_ {
    /// Launcher runs and folders without git.
    #[default]
    Auto,
    /// Every run that can.
    Always,
    /// Only runs with `--copy`.
    Never,
}

/// `[copy] apply`: when changes go back to the original.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Apply {
    /// Only `paddock apply`.
    #[default]
    Ask,
    /// At the end of a run with auto approvals, when no file has a
    /// conflict.
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub default: Default_,
    pub apply: Apply,
    pub max: u64,
    /// The folder of the stores. None: `<paddock data>/copies`.
    pub store_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default: Default_::Auto,
            apply: Apply::Ask,
            max: DEFAULT_MAX,
            store_dir: None,
        }
    }
}

impl Config {
    pub fn load(env: &HostEnv) -> Result<Self> {
        let f = env.paddock_config().join("config.toml");
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        Self::parse(&text).with_context(|| format!("{} is invalid", f.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut c = Config::default();
        let v: toml::Value = crate::manifest::user_config_doc(text)?;
        let Some(t) = v.get("copy") else {
            return Ok(c);
        };
        let Some(t) = t.as_table() else {
            bail!("[copy] is not a table");
        };
        for (k, v) in t {
            match (k.as_str(), v) {
                ("default", toml::Value::String(s)) => {
                    c.default = match s.as_str() {
                        "auto" => Default_::Auto,
                        "always" => Default_::Always,
                        "never" => Default_::Never,
                        _ => bail!("[copy] default = {s:?} is not auto, always or never"),
                    }
                }
                ("apply", toml::Value::String(s)) => {
                    c.apply = match s.as_str() {
                        "ask" => Apply::Ask,
                        "auto" => Apply::Auto,
                        _ => bail!("[copy] apply = {s:?} is not ask or auto"),
                    }
                }
                ("max", toml::Value::String(s)) => {
                    c.max = crate::snapshot::parse_size(s)
                        .with_context(|| format!("[copy] max = {s:?} is not a size"))?
                }
                ("max", toml::Value::Integer(n)) if *n >= 0 => c.max = *n as u64,
                ("store_dir", toml::Value::String(s)) => {
                    let p = PathBuf::from(s);
                    if !p.is_absolute() {
                        bail!("[copy] store_dir = {s:?} is not an absolute path");
                    }
                    c.store_dir = Some(p);
                }
                (k, _) => bail!("[copy] {k} is unknown or has the wrong type"),
            }
        }
        Ok(c)
    }

    /// The folder of the stores.
    pub fn root(&self, env: &HostEnv) -> PathBuf {
        self.store_dir.clone().unwrap_or_else(|| default_root(env))
    }

    /// Every folder that can hold stores: the configured one first, then
    /// the default, so a store from before a change of `store_dir` is
    /// still found.
    pub fn roots(&self, env: &HostEnv) -> Vec<PathBuf> {
        let mut v = vec![self.root(env)];
        let d = default_root(env);
        if !v.contains(&d) {
            v.push(d);
        }
        v
    }
}

pub fn default_root(env: &HostEnv) -> PathBuf {
    env.paddock_data().join("copies")
}
