//! `[gateway]` of a harness profile: the shape of a gateway agent.
//!
//! ```toml
//! [gateway]
//! args = ["gateway", "run", "--port", "{port}", "--bind", "loopback", "--auth", "token"]
//! env = { OPENCLAW_GATEWAY_TOKEN = "{token}" }
//! port = 19101
//! host_port = 19001
//! host_commands = ["gateway", "daemon"]
//! state = ".openclaw"
//!
//! [gateway.client]
//! commands = ["tui"]
//! args = ["--url", "{url}", "--token", "{token}"]
//! ```
//!
//! The values can hold `{port}`, `{token}`, `{url}` and `{host}`. paddock
//! fills them in at each start. Nothing here starts a process.

use anyhow::{bail, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The address that every gateway binds to.
pub const LOOPBACK: &str = "127.0.0.1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// The arguments of the harness command that run the gateway in the
    /// foreground.
    pub args: Vec<String>,
    /// More variables of the gateway process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The port that paddock asks for first. A free port when it is taken.
    pub port: u16,
    /// The port of a gateway that the user starts on the host without
    /// paddock. paddock looks there to tell that one apart.
    pub host_port: Option<u16>,
    /// The first arguments that start or install a gateway on the host
    /// (`openclaw gateway`, `openclaw daemon`). paddock never runs them
    /// in an agent sandbox and points to `paddock gateway NAME start`.
    #[serde(default)]
    pub host_commands: Vec<String>,
    /// The folder of the gateway's own state, relative to its home.
    pub state: Option<String>,
    /// The URL that a client uses. Default: `ws://{host}:{port}`.
    #[serde(default = "default_url")]
    pub url: String,
    pub client: Client,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    /// The first arguments of a client command, for example `tui`. Such a
    /// command attaches to the sandboxed gateway.
    pub commands: Vec<String>,
    /// The arguments that follow the client command, for example
    /// `--url {url} --token {token}`. The arguments of the user after the
    /// client command come after these.
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn default_url() -> String {
    "ws://{host}:{port}".to_string()
}

/// The values that fill the placeholders.
#[derive(Debug, Clone)]
pub struct Values<'a> {
    pub port: u16,
    pub token: &'a str,
}

impl Values<'_> {
    fn fill_raw(&self, s: &str) -> String {
        s.replace("{host}", LOOPBACK)
            .replace("{port}", &self.port.to_string())
            .replace("{token}", self.token)
    }
}

impl Shape {
    /// Refuse a shape that cannot work, with the key and the fix.
    pub fn check(&self, profile: &str) -> Result<()> {
        let at = format!("profile {profile}: [gateway]");
        if self.args.is_empty() {
            bail!("{at} args is empty: name the arguments that run the gateway in the foreground");
        }
        if self.port < 1024 {
            bail!("{at} port = {}: use a port of 1024 or more", self.port);
        }
        if !self.args.iter().any(|a| a.contains("{port}")) {
            bail!("{at} args must pass {{port}}: paddock chooses the port");
        }
        let token_given = self
            .args
            .iter()
            .chain(self.env.values())
            .any(|a| a.contains("{token}"));
        if !token_given {
            bail!(
                "{at} must pass {{token}} in args or env: the gateway needs the token of paddock"
            );
        }
        if self.client.commands.is_empty() {
            bail!("{at}.client needs commands: the first arguments of a client, for example tui");
        }
        Ok(())
    }

    /// The URL of the gateway for a client.
    pub fn url(&self, v: &Values) -> String {
        v.fill_raw(&self.url)
    }

    fn fill(&self, s: &str, v: &Values) -> String {
        v.fill_raw(&s.replace("{url}", &self.url(v)))
    }

    /// The arguments of the gateway process.
    pub fn gateway_args(&self, v: &Values) -> Vec<String> {
        self.args.iter().map(|a| self.fill(a, v)).collect()
    }

    pub fn gateway_env(&self, v: &Values) -> BTreeMap<String, String> {
        self.env
            .iter()
            .map(|(k, val)| (k.clone(), self.fill(val, v)))
            .collect()
    }

    /// The arguments of the client: the client command, the profile's
    /// arguments, then the user's `extra`.
    pub fn client_args(&self, v: &Values, command: &str, extra: &[String]) -> Vec<String> {
        let mut out = vec![command.to_string()];
        out.extend(self.client.args.iter().map(|a| self.fill(a, v)));
        out.extend(extra.iter().cloned());
        out
    }

    /// The first client command: what a launcher such as
    /// `omarchy-launch-openclaw --tui` starts.
    pub fn default_client(&self) -> &str {
        self.client
            .commands
            .first()
            .map(String::as_str)
            .unwrap_or("tui")
    }

    pub fn client_env(&self, v: &Values) -> BTreeMap<String, String> {
        self.client
            .env
            .iter()
            .map(|(k, val)| (k.clone(), self.fill(val, v)))
            .collect()
    }

    /// What a harness command with `args` is.
    pub fn classify(&self, args: &[String]) -> Form {
        match args.first() {
            Some(a) if self.client.commands.contains(a) => Form::Client,
            Some(a) if self.host_commands.contains(a) => Form::Host,
            _ => Form::Other,
        }
    }
}

/// The kinds of command of a gateway agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// A client of the gateway: it attaches to the sandboxed one.
    Client,
    /// A command that starts or installs a gateway on the host.
    Host,
    /// Anything else, for example the embedded runtime.
    Other,
}
