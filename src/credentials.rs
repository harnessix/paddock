//! Credentials that paddock keeps for the sandboxes (`[credentials.NAME]`
//! of the user configuration).
//!
//! A real credential does not enter a sandbox by default. The user names
//! each one and the way the sandbox may use it:
//!
//! - `deliver`: the value enters the sandbox after the user allows it on
//!   the start card. It cannot be taken back.
//! - `gateway`: a local gateway adds the value to requests for the hosts
//!   that the credential is bound to; the sandbox sees a stand-in.
//! - `agent`: a protocol agent (ssh-agent, a git credential helper) uses
//!   the value on the host.
//! - `intercept`: TLS interception for tools that can do neither.
//!
//! `deliver`, `gateway`, and `agent` for ssh (with `keys`, sshagent.rs),
//! docker registries and AWS profiles (`kind`, channels.rs) are built. The others are accepted in the configuration and named as not
//! available. A project `paddock.toml` can
//! never define a credential, and `from = "command"` runs on the host, like
//! `[exec]`. Values are read when they are needed, never written to disk.

pub mod channels;
mod command;
pub(crate) use command::run_command;
pub mod gateway;
pub mod keys;
pub mod secret;
pub mod sshagent;

use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Agent,
    Gateway,
    Deliver,
    Intercept,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ssh => "ssh",
            Kind::Docker => "docker",
            Kind::Aws => "aws",
        }
    }
}

impl Via {
    pub fn name(self) -> &'static str {
        match self {
            Via::Agent => "agent",
            Via::Gateway => "gateway",
            Via::Deliver => "deliver",
            Via::Intercept => "intercept",
        }
    }
    /// Whether this paddock can use a credential this way. `agent` needs
    /// `keys` as well (Credential::available).
    pub fn available(self) -> bool {
        matches!(self, Via::Deliver | Via::Gateway | Via::Agent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The environment of the paddock process, under the credential's name
    /// or `var`.
    #[default]
    Env,
    /// The output of `command`, run on the host.
    Command,
    /// The keychain: the login keychain on macOS, the Secret Service on
    /// Linux (`paddock credentials add`, keys.rs).
    Store,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Confirm {
    /// Once for the run, on the start card.
    #[default]
    Session,
    /// At every use (ssh signatures).
    Each,
}

/// What an `agent` credential speaks to: the ssh agent (the default), a
/// docker credential helper, or the AWS `credential_process`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Ssh,
    Docker,
    Aws,
}

/// The shortest and longest life of AWS session credentials, in seconds.
pub const AWS_DURATION: (u64, u64) = (900, 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeliverAs {
    /// An environment variable of the sandbox.
    #[default]
    Env,
    /// The file `/dev/paddock/secret/NAME`, served by paddock.
    File,
}

/// Where a `deliver` credential appears in the sandbox, as a file.
pub fn secret_path(name: &str) -> String {
    secret::fifo::path_on(cfg!(target_os = "macos"), name)
}

/// One `[credentials.NAME]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub via: Via,
    #[serde(default)]
    pub from: Source,
    /// `from = "env"`: the host variable to read. Default: the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
    /// `from = "command"`: the program and its arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// gateway, intercept: the hosts that the credential is bound to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// gateway, intercept: path prefixes on those hosts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// gateway: the variable in the sandbox that names the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_env: Option<String>,
    /// gateway: a known API that fills hosts, header, prefix, base_path and
    /// base_url_env (`anthropic`, `openai`, `github`, `github-git`). Fields
    /// set here win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// gateway: the request header that carries the value. Default:
    /// `Authorization`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// gateway: text before the value in that header. Default: `Bearer `
    /// for Authorization, nothing for other headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// gateway: send the value as HTTP Basic auth with this user name:
    /// `Basic base64(USER:VALUE)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic_user: Option<String>,
    /// gateway: a query parameter that carries the value instead of a header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// gateway: the path that the sandbox's base URL ends with, such as
    /// `/v1`. Requests keep their own paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<String>,
    /// deliver: the variable in the sandbox that holds the value. Default:
    /// the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// deliver: an environment variable (`env`, the default) or the file
    /// `/dev/paddock/secret/NAME` (`file`).
    #[serde(default, rename = "as")]
    pub as_: DeliverAs,
    /// Only for these harnesses. Empty: every harness.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<String>,
    /// The projects where it passes without a question (`~/src/app`, an
    /// absolute path, or `*` for any project). Elsewhere the first start
    /// of an agent asks once (keys.rs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// agent (ssh): the public keys whose private keys the sandbox may use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// agent: `ssh` (the default), `docker` or `aws`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    /// agent, kind docker: the registries that the sandbox may log in to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registries: Vec<String>,
    /// agent, kind aws: a profile of the user's own AWS configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// agent, kind aws: the life of the session credentials, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
    #[serde(default)]
    pub confirm: Confirm,
}

impl Credential {
    /// The variable that holds it in the sandbox (deliver).
    pub fn sandbox_var<'a>(&'a self, name: &'a str) -> &'a str {
        self.env.as_deref().unwrap_or(name)
    }
    /// The host variable it comes from (`from = "env"`).
    pub fn host_var<'a>(&'a self, name: &'a str) -> &'a str {
        self.var.as_deref().unwrap_or(name)
    }
    /// The credential with its preset filled in.
    pub fn resolved(&self) -> Result<Credential> {
        let mut c = self.clone();
        let Some(p) = &self.preset else {
            return Ok(c);
        };
        if p == "github-git" {
            if c.hosts.is_empty() {
                c.hosts = vec!["github.com".to_string()];
            }
            if c.paths.is_empty() {
                c.paths = GIT_READ_PATHS.iter().map(|p| p.to_string()).collect();
            }
            c.header.get_or_insert_with(|| "Authorization".to_string());
            c.basic_user
                .get_or_insert_with(|| "x-access-token".to_string());
            c.prefix.get_or_insert_with(|| "Basic ".to_string());
            return Ok(c);
        }
        let (host, header, prefix, base_path, base_env) = match p.as_str() {
            "anthropic" => (
                "api.anthropic.com",
                "x-api-key",
                "",
                "",
                "ANTHROPIC_BASE_URL",
            ),
            "openai" => (
                "api.openai.com",
                "Authorization",
                "Bearer ",
                "/v1",
                "OPENAI_BASE_URL",
            ),
            "github" => (
                "api.github.com",
                "Authorization",
                "Bearer ",
                "",
                "GITHUB_API_URL",
            ),
            other => {
                bail!("unknown preset '{other}': use anthropic, openai, github or github-git")
            }
        };
        if c.hosts.is_empty() {
            c.hosts = vec![host.to_string()];
        }
        c.header.get_or_insert_with(|| header.to_string());
        c.prefix.get_or_insert_with(|| prefix.to_string());
        c.base_path.get_or_insert_with(|| base_path.to_string());
        c.base_url_env.get_or_insert_with(|| base_env.to_string());
        Ok(c)
    }

    /// The kind of an `agent` credential.
    pub fn kind(&self) -> Kind {
        self.kind.unwrap_or_default()
    }

    /// The life of AWS session credentials.
    pub fn aws_duration(&self) -> u64 {
        self.duration.unwrap_or(AWS_DURATION.0)
    }

    /// Whether this paddock can use it: `agent` for ssh needs keys.
    pub fn available(&self) -> bool {
        self.via.available()
            && (self.via != Via::Agent
                || match self.kind() {
                    Kind::Ssh => !self.keys.is_empty(),
                    Kind::Docker => !self.registries.is_empty(),
                    Kind::Aws => self.profile.is_some(),
                })
    }

    /// git smart HTTP through the gateway (`preset = "github-git"`): git in
    /// the sandbox reaches the host through a URL rewrite.
    pub fn is_git(&self) -> bool {
        self.preset.as_deref() == Some("github-git")
    }

    pub fn applies_to(&self, harness: &str) -> bool {
        self.harnesses.is_empty() || self.harnesses.iter().any(|h| h == harness)
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.starts_with(|c: char| c.is_ascii_alphabetic())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_var(n: &str) -> bool {
    !n.is_empty()
        && n.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The paths of `preset = "github-git"`: fetch and clone. Push has its own
/// path (`paddock-host git push`); add `/*/*/git-receive-pack` to allow it
/// here too.
pub const GIT_READ_PATHS: &[&str] = &["/*/*/info/refs", "/*/*/git-upload-pack"];

/// The environment that sends git in the sandbox to the gateway for the
/// given credentials (`(name, host)` pairs): `url.BASE/.insteadOf =
/// https://HOST/`, through `GIT_CONFIG_COUNT`, never in a repository file.
pub fn git_env(port: u16, routes: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = vec![("GIT_CONFIG_COUNT".to_string(), routes.len().to_string())];
    for (i, (name, host)) in routes.iter().enumerate() {
        let host = host.strip_suffix(":443").unwrap_or(host);
        out.push((
            format!("GIT_CONFIG_KEY_{i}"),
            format!("url.http://127.0.0.1:{port}/s/{name}/.insteadOf"),
        ));
        out.push((format!("GIT_CONFIG_VALUE_{i}"), format!("https://{host}/")));
    }
    out
}

/// `[credentials.*]` of the user configuration.
pub fn parse(text: &str) -> Result<BTreeMap<String, Credential>> {
    let v: toml::Value = crate::manifest::user_config_doc(text)?;
    let Some(t) = v.get("credentials") else {
        return Ok(BTreeMap::new());
    };
    let all: BTreeMap<String, Credential> = t.clone().try_into()?;
    for (name, c) in &all {
        if !valid_name(name) {
            bail!("'{name}' is not a credential name: use letters, digits, '_' and '-', starting with a letter");
        }
        match c.from {
            Source::Command if c.command.is_empty() => {
                bail!("[credentials.{name}] has from = \"command\" and no command")
            }
            Source::Env | Source::Store if !c.command.is_empty() => {
                bail!("[credentials.{name}] has a command; set from = \"command\" to use it")
            }
            Source::Store if c.var.is_some() => {
                bail!("[credentials.{name}]: from = \"store\" reads the keychain item {name}; var is only for from = \"env\"")
            }
            _ => {}
        }
        for v in [
            c.var.as_deref(),
            c.env.as_deref(),
            c.base_url_env.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !valid_var(v) {
                bail!("[credentials.{name}]: '{v}' is not an environment variable name");
            }
        }
        if !c.keys.is_empty() && (c.via != Via::Agent || c.kind() != Kind::Ssh) {
            bail!("[credentials.{name}]: keys are only for via = \"agent\" with kind = \"ssh\"");
        }
        if c.kind.is_some() && c.via != Via::Agent {
            bail!("[credentials.{name}]: kind is only for via = \"agent\"");
        }
        if !c.registries.is_empty() && c.kind() != Kind::Docker {
            bail!("[credentials.{name}]: registries are only for kind = \"docker\"");
        }
        if (c.profile.is_some() || c.duration.is_some()) && c.kind() != Kind::Aws {
            bail!("[credentials.{name}]: profile and duration are only for kind = \"aws\"");
        }
        if c.via == Via::Agent && c.kind() != Kind::Ssh {
            if c.from != Source::Env || c.var.is_some() || c.env.is_some() {
                bail!("[credentials.{name}]: kind = \"{}\" reads the login of the host tool; it takes no from, var, command or env", c.kind().name());
            }
            if c.confirm == Confirm::Each {
                bail!("[credentials.{name}]: kind = \"{}\" asks at the first use in each run; confirm = \"each\" is only for ssh", c.kind().name());
            }
        }
        if c.kind() == Kind::Docker {
            if c.registries.is_empty() {
                bail!("[credentials.{name}]: kind = \"docker\" needs registries, such as [\"ghcr.io\"]");
            }
            for r in &c.registries {
                if r.contains(['/', ' ']) || channels::registry_host(r).is_none() {
                    bail!("[credentials.{name}]: '{r}' is not a registry (HOST or HOST:PORT)");
                }
            }
        }
        if c.kind() == Kind::Aws {
            let Some(p) = &c.profile else {
                bail!("[credentials.{name}]: kind = \"aws\" needs profile, a profile of your ~/.aws/config");
            };
            if !channels::valid_profile(p) {
                bail!("[credentials.{name}]: '{p}' is not an AWS profile name");
            }
            let d = c.aws_duration();
            if d < AWS_DURATION.0 || d > AWS_DURATION.1 {
                bail!(
                    "[credentials.{name}]: duration is {d}; use {} to {} seconds",
                    AWS_DURATION.0,
                    AWS_DURATION.1
                );
            }
            if let Some((other, _)) = all.iter().find(|(n, o)| {
                n.as_str() < name.as_str() && o.kind == Some(Kind::Aws) && o.profile == c.profile
            }) {
                bail!("[credentials.{name}]: the profile {p} is in [credentials.{other}] already");
            }
        }
        if c.as_ == DeliverAs::File && c.via != Via::Deliver {
            bail!("[credentials.{name}]: as = \"file\" is only for via = \"deliver\"");
        }
        if c.as_ == DeliverAs::File && c.env.is_some() {
            bail!(
                "[credentials.{name}]: env is only for as = \"env\"; the file is {}",
                secret_path(name)
            );
        }
        if c.via == Via::Deliver && c.as_ == DeliverAs::Env && c.env.is_none() && !valid_var(name) {
            bail!("[credentials.{name}]: name the variable for the sandbox with env = \"...\"");
        }
        let c = c
            .resolved()
            .with_context(|| format!("[credentials.{name}]"))?;
        if matches!(c.via, Via::Gateway | Via::Intercept) && c.hosts.is_empty() {
            bail!(
                "[credentials.{name}]: via = \"{}\" needs hosts or a preset",
                c.via.name()
            );
        }
        if c.via == Via::Gateway {
            if c.base_url_env.is_none() && !c.is_git() {
                bail!("[credentials.{name}]: via = \"gateway\" needs base_url_env, the variable that points the tool at the gateway");
            }
            if c.env.is_none() && !valid_var(name) {
                bail!(
                    "[credentials.{name}]: name the variable for the stand-in with env = \"...\""
                );
            }
            for h in &c.hosts {
                let (host, port) = match h.split_once(':') {
                    Some((h, p)) => (h, Some(p)),
                    None => (h.as_str(), None),
                };
                if host.is_empty()
                    || host.contains(['/', ':', '@', ' '])
                    || port.is_some_and(|p| p.parse::<u16>().is_err())
                {
                    bail!("[credentials.{name}]: '{h}' is not a host name (HOST or HOST:PORT)");
                }
            }
            for p in c
                .paths
                .iter()
                .chain(c.base_path.iter().filter(|p| !p.is_empty()))
            {
                if !p.starts_with('/') {
                    bail!("[credentials.{name}]: the path '{p}' must start with /");
                }
            }
            if let Some(u) = &c.basic_user {
                if u.is_empty() || u.contains(':') || u.chars().any(|c| c.is_control()) {
                    bail!("[credentials.{name}]: '{u}' is not a user name for basic auth");
                }
            }
            if let Some(h) = &c.header {
                if h.is_empty() || !h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    bail!("[credentials.{name}]: '{h}' is not a header name");
                }
            }
        }
    }
    all.into_iter()
        .map(|(n, c)| Ok((n, c.resolved()?)))
        .collect()
}

pub fn load(env: &HostEnv) -> Result<BTreeMap<String, Credential>> {
    let file = env.paddock_config().join("config.toml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Ok(BTreeMap::new());
    };
    parse(&text).with_context(|| format!("[credentials] in {} is invalid", file.display()))
}

/// Whether the source of a credential is there, without reading a value
/// from a command: an env source must be set, a command must exist.
pub fn source_ready(env: &HostEnv, name: &str, c: &Credential) -> bool {
    if c.via == Via::Agent {
        return match c.kind() {
            Kind::Ssh => {
                !c.keys.is_empty() && c.keys.iter().all(|k| sshagent::key_path(env, k).is_file())
            }
            Kind::Docker => channels::docker_config_file(env).is_file(),
            Kind::Aws => env.which("aws").is_some(),
        };
    }
    match c.from {
        Source::Env => env.var(c.host_var(name)).is_some_and(|v| !v.is_empty()),
        Source::Command => c
            .command
            .first()
            .is_some_and(|p| p.contains('/') || env.which(p).is_some()),
        // Without reading: an unlock prompt could wait for the user.
        Source::Store => keys::keychain_tool(env),
    }
}

/// The host variables that registered credentials come from: they never
/// pass with the inherited environment.
pub fn host_vars(all: &BTreeMap<String, Credential>) -> Vec<String> {
    all.iter()
        .filter(|(_, c)| c.from == Source::Env && c.via != Via::Agent)
        .map(|(n, c)| c.host_var(n).to_string())
        .collect()
}

/// The capability name on the approval card.
pub const CAPABILITY: &str = "credentials";
/// The most bytes that a command source may print.
pub(crate) const MAX_VALUE: usize = 64 * 1024;
pub(crate) const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The credentials of a run: what to deliver, and what to say about the rest.
#[derive(Debug, Default)]
pub struct Plan {
    /// The credentials whose source is there, by name: `deliver`,
    /// `gateway`, and `agent` whose keys the host agent holds.
    pub deliver: Vec<(String, Credential)>,
    pub notices: Vec<String>,
}

/// The registered credentials for a harness. A spawned sandbox gets none:
/// it would ask the user again for each child.
pub fn plan(env: &HostEnv, harness: &str, spawned: bool) -> Result<Plan> {
    let mut p = Plan::default();
    for (name, c) in load(env)? {
        if !c.applies_to(harness) {
            continue;
        }
        if !c.available() {
            p.notices.push(if c.via == Via::Agent {
                format!("credential {name}: via = \"agent\" needs keys, the public ssh keys that the sandbox may use; the sandbox does not get it")
            } else {
                format!(
                    "credential {name}: via = \"{}\" is not available in this paddock yet; the sandbox does not get it",
                    c.via.name()
                )
            });
            continue;
        }
        if spawned {
            p.notices.push(format!(
                "credential {name}: a sandbox started by another one gets no credentials"
            ));
            continue;
        }
        if c.via == Via::Agent && c.kind() != Kind::Ssh {
            if !source_ready(env, &name, &c) {
                p.notices.push(match c.kind() {
                    Kind::Docker => format!(
                        "credential {name}: there is no docker configuration at {}; the sandbox does not get it",
                        channels::docker_config_file(env).display()
                    ),
                    _ => format!("credential {name}: the program aws is not there; the sandbox does not get it"),
                });
                continue;
            }
            p.deliver.push((name, c));
            continue;
        }
        if c.via == Via::Agent {
            let ready = sshagent::load_keys(env, &[(name.clone(), c.clone())]).and_then(|keys| {
                sshagent::check_host(env.var("SSH_AUTH_SOCK").map(std::path::Path::new), &keys)
            });
            if let Err(e) = ready {
                p.notices.push(format!(
                    "credential {name}: {e:#}; the sandbox starts without the ssh agent socket"
                ));
                continue;
            }
            p.deliver.push((name, c));
            continue;
        }
        if !source_ready(env, &name, &c) {
            p.notices.push(match c.from {
                Source::Env => format!(
                    "credential {name}: {} is not set where paddock started",
                    c.host_var(&name)
                ),
                Source::Command => format!(
                    "credential {name}: the program {} is not there",
                    c.command[0]
                ),
                Source::Store => format!("credential {name}: {}", keys::NO_KEYCHAIN),
            });
            continue;
        }
        p.deliver.push((name, c));
    }
    Ok(p)
}

pub(crate) fn describe(name: &str, c: &Credential) -> String {
    let from = match c.from {
        Source::Env => format!("from the host variable {}", c.host_var(name)),
        Source::Command => format!("from the command `{}`", c.command.join(" ")),
        Source::Store => "from the keychain of this computer".to_string(),
    };
    match c.via {
        Via::Agent if c.kind() == Kind::Docker => format!(
            "{name}: your docker login for {}. The sandbox's docker client asks paddock for it at each use, and you allow the first use of each registry in this run. The login stays in your docker credential store, but its value reaches the docker client in the sandbox for that request",
            c.registries.join(", ")
        ),
        Via::Agent if c.kind() == Kind::Aws => format!(
            "{name}: AWS credentials of your profile {}: {}. The long-lived keys stay on the host, and you allow the first use in this run. The sandbox gets a new short-lived set at each later use",
            c.profile.as_deref().unwrap_or(""),
            channels::aws_lifetime(c.aws_duration())
        ),
        Via::Agent => format!(
            "{name}: the ssh keys {} in your ssh agent. The private keys stay on the host: the sandbox gets an agent socket that lists only these keys and signs only with them{}",
            c.keys.join(", "),
            if c.confirm == Confirm::Each {
                ", and you allow each signature"
            } else {
                ", without a question for each signature during this run"
            }
        ),
        Via::Gateway => format!(
            "{name}: {from}. It stays on the host: the sandbox gets a stand-in, and paddock adds the real value to requests for {}{}",
            c.hosts.join(", "),
            if c.paths.is_empty() {
                String::new()
            } else {
                format!(" under {}", c.paths.join(", "))
            }
        ),
        _ if c.as_ == DeliverAs::File => format!(
            "{name}: {from}. The sandbox can read the value in the file {}; the agent can read it. Each open counts as a use, and paddock credentials revoke ends it",
            secret_path(name)
        ),
        _ => format!(
            "{name}: {from}. The value enters the sandbox as the variable {}; the agent can read it, and it cannot be taken back",
            c.sandbox_var(name)
        ),
    }
}

/// Ask the user twice: allow, then confirm. Only both answers deliver.
/// Every step goes to the audit log, never a value.
pub fn ask(
    env: &HostEnv,
    spec: &crate::spec::SandboxSpec,
    approver: &dyn crate::broker::Approver,
    wait: std::time::Duration,
    plan: &Plan,
) -> bool {
    if plan.deliver.is_empty() {
        return false;
    }
    let names: Vec<&str> = plan.deliver.iter().map(|(n, _)| n.as_str()).collect();
    let list: Vec<String> = plan.deliver.iter().map(|(n, c)| describe(n, c)).collect();
    let card = |step: &str, summary: String, detail: String| crate::broker::Pending {
        id: format!("cred-{}-{step}-{}", spec.id, crate::state::now()),
        sandbox: spec.id.clone(),
        harness: spec.harness.clone(),
        project: spec.project.clone(),
        capability: CAPABILITY.into(),
        summary,
        detail,
        created: crate::state::now(),
        expires: crate::state::now() + wait.as_secs(),
        session_allowed: false,
        ..Default::default()
    };
    let first = card(
        "allow",
        format!("give {} to this sandbox", names.join(", ")),
        format!(
            "{}\n\nThe harness starts after your answer.",
            list.join("\n")
        ),
    );
    let second = card(
        "confirm",
        format!("confirm: {} for this run", names.join(", ")),
        format!(
            "Second step. Yes gives this sandbox {} for this run, as the first card lists. No starts the sandbox without them.",
            names.join(", ")
        ),
    );
    for (step, p) in [("allowed", &first), ("confirmed", &second)] {
        let answer = approver.ask(env, p, wait);
        let decision = match answer {
            Some(a) if a.denies() => "denied",
            None => "timed out",
            Some(_) => step,
        };
        audit(env, spec, &names.join(", "), decision);
        if !matches!(
            answer,
            Some(crate::broker::Answer::Once | crate::broker::Answer::Session)
        ) {
            return false;
        }
    }
    true
}

/// One value that `read` got: the name, the variable in the sandbox, and
/// the value (empty for `agent`).
pub type Value = (String, String, secret::Secret);

/// Read the values. A source that fails is left out, with a notice.
pub fn read(env: &HostEnv, plan: &Plan) -> (Vec<Value>, Vec<String>) {
    let mut out = Vec::new();
    let mut notices = Vec::new();
    for (name, c) in &plan.deliver {
        if c.via == Via::Agent {
            // No value: the host agent keeps the keys.
            out.push((name.clone(), String::new(), String::new().into()));
            continue;
        }
        let value = match c.from {
            Source::Env => env
                .var(c.host_var(name))
                .map(secret::Secret::from)
                .ok_or_else(|| anyhow::anyhow!("{} is not set", c.host_var(name))),
            Source::Command => run_command(&c.command, &|_| {}),
            Source::Store => keys::read_value(env, name).map_err(anyhow::Error::from),
        };
        match value {
            Ok(v) => out.push((name.clone(), c.sandbox_var(name).to_string(), v)),
            Err(e) => notices.push(format!("credential {name} was not delivered: {e:#}")),
        }
    }
    (out, notices)
}

/// One line in the audit log of host requests. Names only.
pub fn audit(env: &HostEnv, spec: &crate::spec::SandboxSpec, names: &str, decision: &str) {
    crate::broker::audit_line(
        env,
        &serde_json::json!({
            "time": crate::state::now(),
            "sandbox": spec.id,
            "harness": spec.harness,
            "project": spec.project,
            "capability": CAPABILITY,
            "detail": format!("credentials {names}"),
            "decision": decision,
        }),
    );
}

/// One registered credential for `paddock credentials list`. No value.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub via: &'static str,
    pub available: bool,
    pub source: String,
    pub ready: bool,
    pub harnesses: Vec<String>,
    pub delivered: u64,
    pub last_delivered: Option<u64>,
    /// Requests that the gateway forwarded with it, signatures that the
    /// ssh agent channel allowed, or opens of its secret file.
    pub used: u64,
    /// deliver: `env` or `file`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliver_as: Option<&'static str>,
}

pub fn rows(env: &HostEnv) -> Result<Vec<Row>> {
    let all = load(env)?;
    // Deliveries from the audit log of host requests.
    let mut used: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut forwarded: BTreeMap<String, u64> = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(crate::broker::audit_file(env)) {
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v["capability"] != CAPABILITY {
                continue;
            }
            if v["decision"]
                .as_str()
                .is_some_and(|d| d.starts_with("forwarded"))
            {
                if let Some(name) = v["detail"]
                    .as_str()
                    .and_then(|d| d.strip_prefix("gateway "))
                    .and_then(|d| d.split(' ').next())
                {
                    *forwarded.entry(name.to_string()).or_default() += 1;
                }
                continue;
            }
            if v["decision"] == "read" {
                if let Some(name) = v["detail"].as_str().and_then(|d| d.strip_prefix("secret ")) {
                    *forwarded.entry(name.to_string()).or_default() += 1;
                }
                continue;
            }
            if v["decision"] == "allowed" {
                if let Some(name) = v["detail"]
                    .as_str()
                    .and_then(|d| d.strip_prefix("agent "))
                    .and_then(|d| d.split(' ').next())
                {
                    *forwarded.entry(name.to_string()).or_default() += 1;
                }
                continue;
            }
            if v["decision"] != "delivered" {
                continue;
            }
            if let Some(name) = v["detail"].as_str().and_then(|d| {
                // "deliver NAME" in logs of older versions.
                d.strip_prefix("credentials ")
                    .or_else(|| d.strip_prefix("deliver "))
            }) {
                let e = used.entry(name.to_string()).or_default();
                e.0 += 1;
                e.1 = e.1.max(v["time"].as_u64().unwrap_or(0));
            }
        }
    }
    Ok(all
        .iter()
        .map(|(name, c)| {
            let (n, last) = used.get(name).copied().unwrap_or_default();
            Row {
                name: name.clone(),
                via: c.via.name(),
                available: c.available(),
                source: match (c.via, c.from) {
                    (Via::Agent, _) if c.kind() == Kind::Docker => {
                        format!("docker login, registries {}", c.registries.join(" "))
                    }
                    (Via::Agent, _) if c.kind() == Kind::Aws => format!(
                        "aws profile {}, {} seconds",
                        c.profile.as_deref().unwrap_or(""),
                        c.aws_duration()
                    ),
                    (Via::Agent, _) => format!("ssh agent, keys {}", c.keys.join(" ")),
                    (_, Source::Env) => format!("env {}", c.host_var(name)),
                    (_, Source::Command) => format!("command {}", c.command[0]),
                    (_, Source::Store) => "keychain".to_string(),
                },
                ready: source_ready(env, name, c),
                harnesses: c.harnesses.clone(),
                delivered: n,
                last_delivered: (last > 0).then_some(last),
                used: forwarded.get(name).copied().unwrap_or(0),
                deliver_as: (c.via == Via::Deliver).then_some(match c.as_ {
                    DeliverAs::Env => "env",
                    DeliverAs::File => "file",
                }),
            }
        })
        .collect())
}

/// One open of a secret file, in the audit log of host requests. The name
/// only.
pub fn audit_read(env: &HostEnv, spec: &crate::spec::SandboxSpec, name: &str) {
    crate::broker::audit_line(
        env,
        &serde_json::json!({
            "time": crate::state::now(),
            "sandbox": spec.id,
            "harness": spec.harness,
            "project": spec.project,
            "capability": CAPABILITY,
            "detail": format!("secret {name}"),
            "decision": "read",
        }),
    );
}

/// One request through the gateway, in the audit log of host requests:
/// the name, the method, the host and the path without its query.
pub fn audit_use(
    env: &HostEnv,
    spec: &crate::spec::SandboxSpec,
    name: &str,
    method: &str,
    host: &str,
    path: &str,
    result: &str,
) {
    crate::broker::audit_line(
        env,
        &serde_json::json!({
            "time": crate::state::now(),
            "sandbox": spec.id,
            "harness": spec.harness,
            "project": spec.project,
            "capability": CAPABILITY,
            "detail": format!("gateway {name} {method} {host}{path}"),
            "decision": result,
        }),
    );
}

/// The file of a running sandbox whose lines name revoked credentials
/// (`*`: all). The gateway reads it at each request. It is in the run
/// folder, which the sandbox cannot see.
pub fn revoked_file(env: &HostEnv, sandbox: &str) -> std::path::PathBuf {
    env.paddock_run().join(sandbox).join("credentials-revoked")
}

/// Revoke a credential (or all, with None) of a running sandbox. The next
/// request through the gateway gets 403, the ssh agent channel signs no
/// more, the docker and aws channels give nothing, and a secret file fails
/// to open or read; the sandbox keeps running.
pub fn revoke(env: &HostEnv, sandbox: &str, name: Option<&str>) -> Result<Vec<String>> {
    let r = crate::state::Running::list(env)
        .into_iter()
        .find(|r| r.id == sandbox)
        .with_context(|| format!("no running sandbox with id {sandbox}"))?;
    let held: Vec<String> = r
        .gateway
        .iter()
        .chain(&r.agent)
        .chain(&r.secret_files)
        .cloned()
        .collect();
    let names: Vec<String> = match name {
        Some(n) if held.iter().any(|g| g == n) => vec![n.to_string()],
        Some(n) if r.credentials.iter().any(|c| c == n) => bail!(
            "{n} was delivered: its value is in the sandbox and cannot be taken back. Stop the sandbox to end it: paddock stop {}", crate::ids::short(sandbox)
        ),
        Some(n) => bail!("sandbox {sandbox} does not use the credential {n}"),
        None if held.is_empty() => {
            bail!("sandbox {sandbox} uses no credential through the gateway, an agent channel or a secret file")
        }
        None => held,
    };
    let mut f = crate::privfs::append(&revoked_file(env, sandbox))?;
    use std::io::Write as _;
    for n in &names {
        writeln!(f, "{n}")?;
    }
    for n in &names {
        crate::broker::audit_line(
            env,
            &serde_json::json!({
                "time": crate::state::now(),
                "sandbox": r.id,
                "harness": r.harness,
                "project": r.project,
                "capability": CAPABILITY,
                "detail": if r.agent.contains(n) {
                    format!("agent {n}")
                } else if r.secret_files.contains(n) {
                    format!("secret {n}")
                } else {
                    format!("gateway {n}")
                },
                "decision": "revoked by the user",
            }),
        );
    }
    Ok(names)
}

#[cfg(test)]
mod tests;
