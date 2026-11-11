//! Reading one `[devices.NAME]` or `[devices.NAME.sandbox]` table: the
//! helpers that keep every message in one shape, `[devices.NAME]: KEY ...`.

use super::config::{expand_home, parse_size, var_name_ok};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// How a configuration text is parsed.
#[derive(Clone, Copy, Default)]
pub struct ParseEnv<'a> {
    /// Where `~` expands. None: the text is parsed without a host (tests),
    /// and `~` stays as written.
    pub home: Option<&'a Path>,
    /// A `connect` entry may name a loopback or LAN address. Only tests
    /// set it, for a server of their own on the loopback. A user
    /// configuration never does: `connect` reaches another machine.
    pub allow_local_connect: bool,
}

/// The variable that lets the integration tests reach their own loopback
/// server through `connect`. Only a debug build reads it.
pub const TEST_CONNECT_LOCAL: &str = "PADDOCK_TEST_CONNECT_LOCAL";

/// The value of [`ParseEnv::allow_local_connect`] for the binary: the test
/// variable counts only in a debug build. A release build ignores it, so
/// no environment turns the check off for a user.
pub fn local_connect_from_env(release: bool, var: Option<&std::ffi::OsStr>) -> bool {
    !release && var.is_some()
}

impl<'a> ParseEnv<'a> {
    /// The parse of the user configuration on this host: `~` expands to
    /// `home`. The one place that reads the test variable.
    pub fn host(home: &'a Path) -> Self {
        Self {
            home: Some(home),
            allow_local_connect: local_connect_from_env(
                !cfg!(debug_assertions),
                std::env::var_os(TEST_CONNECT_LOCAL).as_deref(),
            ),
        }
    }

    pub(crate) fn expand(&self, raw: &str) -> PathBuf {
        match self.home {
            Some(h) => expand_home(raw, h),
            None => PathBuf::from(raw),
        }
    }
}

/// Reads one table. The methods keep the messages in one shape:
/// `[devices.NAME]: KEY ...`.
pub(crate) struct Reader<'a> {
    pub(crate) at: String,
    pub(crate) t: &'a toml::Table,
    pub(crate) env: ParseEnv<'a>,
}

impl Reader<'_> {
    pub(crate) fn str_list(&self, k: &str) -> Result<Option<Vec<String>>> {
        let Some(v) = self.t.get(k) else {
            return Ok(None);
        };
        let a = v
            .as_array()
            .with_context(|| format!("{}: {k} must be a list of strings", self.at))?;
        a.iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .with_context(|| format!("{}: {k} must be a list of strings", self.at))
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    pub(crate) fn flag(&self, k: &str) -> Result<bool> {
        match self.t.get(k) {
            None => Ok(false),
            Some(v) => v
                .as_bool()
                .with_context(|| format!("{}: {k} must be true or false", self.at)),
        }
    }

    pub(crate) fn word<'v>(&self, k: &str, allowed: &[&'v str]) -> Result<Option<&'v str>> {
        let Some(v) = self.t.get(k) else {
            return Ok(None);
        };
        let s = v.as_str().unwrap_or("");
        match allowed.iter().find(|a| **a == s) {
            Some(a) => Ok(Some(a)),
            None => bail!(
                "{}: {k} must be {}",
                self.at,
                allowed
                    .iter()
                    .map(|a| format!("{a:?}"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        }
    }

    pub(crate) fn size(&self, k: &str) -> Result<Option<usize>> {
        match self.t.get(k) {
            None => Ok(None),
            Some(toml::Value::String(s)) => parse_size(s)
                .map(Some)
                .with_context(|| format!("{}: {k}", self.at)),
            Some(toml::Value::Integer(n)) => parse_size(&n.to_string())
                .map(Some)
                .with_context(|| format!("{}: {k}", self.at)),
            Some(_) => bail!("{}: {k} must be a size like \"256K\"", self.at),
        }
    }

    pub(crate) fn int(&self, k: &str, lo: u64, hi: u64) -> Result<Option<u64>> {
        match self.t.get(k) {
            None => Ok(None),
            Some(v) => v
                .as_integer()
                .filter(|n| (lo as i64..=hi as i64).contains(n))
                .map(|n| Some(n as u64))
                .with_context(|| format!("{}: {k} is {lo} to {hi}", self.at)),
        }
    }

    pub(crate) fn money(&self, k: &str) -> Result<Option<u64>> {
        let v = match self.t.get(k) {
            None => return Ok(None),
            Some(toml::Value::Float(f)) => Some(*f),
            Some(toml::Value::Integer(n)) => Some(*n as f64),
            Some(_) => None,
        };
        v.and_then(super::ledger::amount)
            .map(Some)
            .with_context(|| {
                format!(
                    "{}: {k} must be a number from 0 to {}, for example 0.002",
                    self.at,
                    super::ledger::MAX_AMOUNT
                )
            })
    }

    pub(crate) fn paths(&self, k: &str, home: Option<&Path>) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for raw in self.str_list(k)?.unwrap_or_default() {
            let p = self.env.expand(&raw);
            let is_home = raw == "~" || home.is_some_and(|h| p == h);
            if p == Path::new("/") || is_home {
                bail!(
                    "{}: {k}: {raw} would open the whole system or home; use home = \"full\"",
                    self.at
                );
            }
            if !p.is_absolute() && !raw.starts_with('~') {
                bail!(
                    "{}: {k}: {raw} must be an absolute path or start with ~/",
                    self.at
                );
            }
            out.push(p);
        }
        Ok(out)
    }

    pub(crate) fn var_names(&self, k: &str) -> Result<Vec<String>> {
        let v = self.str_list(k)?.unwrap_or_default();
        if let Some(bad) = v.iter().find(|n| !var_name_ok(n)) {
            bail!(
                "{}: {k}: {bad:?} is not a variable name (letters, digits and _)",
                self.at
            );
        }
        Ok(v)
    }

    pub(crate) fn unknown_keys(&self, allowed: &[&str], shape: &str) -> Result<()> {
        if self.t.contains_key("isolation") {
            bail!(
                "{}: isolation was never released. Use run = \"host\" or run = \"sandbox\"",
                self.at
            );
        }
        let mut known: Vec<&str> = allowed.to_vec();
        known.sort_unstable();
        known.dedup();
        for k in self.t.keys() {
            if !allowed.contains(&k.as_str()) {
                bail!(
                    "{}: unknown key {k} for {shape}. Known: {}",
                    self.at,
                    known.join(", ")
                );
            }
        }
        Ok(())
    }
}
