//! User-configured soft memory boundary for systemd scopes.
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};

pub const DEFAULT_HIGH: u64 = 2 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// `None` means no memory.high boundary.
    pub high: Option<u64>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            high: Some(DEFAULT_HIGH),
        }
    }
}
impl Config {
    pub fn load(env: &HostEnv) -> Result<Self> {
        let file = env.paddock_config().join("config.toml");
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
        };
        Self::parse(&text).with_context(|| format!("[memory] in {} is invalid", file.display()))
    }
    pub fn parse(text: &str) -> Result<Self> {
        let doc = crate::manifest::user_config_doc(text)?;
        let Some(value) = doc.get("memory") else {
            return Ok(Self::default());
        };
        let table = value.as_table().context("[memory] must be a table")?;
        for key in table.keys() {
            if key != "high" {
                bail!("unknown [memory] setting {key}");
            }
        }
        let Some(value) = table.get("high") else {
            return Ok(Self::default());
        };
        let high = match value {
            toml::Value::String(s) if s == "max" => None,
            toml::Value::String(s) => Some(crate::spawn::parse_bytes(s).context("[memory] high")?),
            toml::Value::Integer(n) if *n > 0 => {
                Some(crate::spawn::parse_bytes(&n.to_string()).context("[memory] high")?)
            }
            _ => bail!("[memory] high must be a size such as '2G' or 'max'"),
        };
        Ok(Self { high })
    }
}

/// Apply the shared soft limit and an optional hard limit to a child scope.
pub fn apply_spawn(
    command: &mut std::process::Command,
    env: &HostEnv,
    max: Option<u64>,
) -> Result<()> {
    if let Some(high) = Config::load(env)?.high {
        command.arg("-p").arg(format!("MemoryHigh={high}"));
    }
    if let Some(max) = max {
        command.arg("-p").arg(format!("MemoryMax={max}"));
        command.arg("-p").arg("MemorySwapMax=0");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_override_and_opt_out() {
        assert_eq!(Config::parse("").unwrap().high, Some(2 << 30));
        assert_eq!(
            Config::parse("[memory]\nhigh = '512M'\n").unwrap().high,
            Some(512 << 20)
        );
        assert_eq!(
            Config::parse("[memory]\nhigh = 'max'\n").unwrap().high,
            None
        );
        assert!(Config::parse("[memory]\nhigh = '1M'\n").is_err());
        assert!(Config::parse("[memory]\nhigh = 'bad'\n").is_err());
        assert!(Config::parse("[memory]\nother = true\n").is_err());
    }
}
