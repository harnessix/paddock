//! The network settings of a project file over those of the user
//! configuration: what widens, and the hard ceiling `network.max`.

use super::Manifest;
use crate::spec::Network;
use anyhow::{bail, Result};

/// The network of the user configuration, which a project file can
/// narrow without approval but not widen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Base {
    /// The effective network mode of the user configuration.
    pub mode: Network,
    /// The domains of the user configuration's `network.allow`.
    pub allow: Vec<String>,
    /// The command line names the network mode, so the mode of the
    /// project file does not apply.
    pub cli_network: bool,
}

impl Base {
    pub fn of_user(user: &Manifest) -> Self {
        let mut mode = user.network.mode.unwrap_or_default();
        if let Some(max) = user.network.max.filter(|m| net_rank(mode) > net_rank(*m)) {
            mode = max;
        }
        Self {
            mode,
            allow: user.network.allow.clone(),
            cli_network: false,
        }
    }
}

/// none < api < allowlist < egress < host.
pub fn net_rank(n: Network) -> u8 {
    crate::spawn::network_rank(n)
}

pub fn net_name(n: Network) -> String {
    serde_json::to_value(n)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// Refuse a network mode above `network.max` of the user configuration.
pub fn check_max(max: Option<Network>, mode: Network, what: &str) -> Result<()> {
    if let Some(max) = max.filter(|m| net_rank(mode) > net_rank(*m)) {
        bail!(
            "{what} \"{}\" is wider than network.max = \"{}\" in the user configuration (~/.config/paddock/config.toml). No project file, approval or flag can exceed network.max",
            net_name(mode),
            net_name(max)
        );
    }
    Ok(())
}
