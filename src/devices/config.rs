//! `[devices.NAME]` tables of the user configuration, version 2.
//!
//! ```toml
//! [devices.mac]
//! shape = "exec"                  # exec | serve. kind = "command" of 0.2.1 means shape = "exec"
//! run = "sandbox"                 # sandbox | host | remote
//! command = ["~/.config/paddock/devices/mac/build.sh"]
//! enabled = true
//! grant = ["pasture:workers", "project:~/src/app", "harness:claude"]
//! mode = "ask"                    # ask | allow
//! scope = "run"                   # run | shared
//! start = "first-request"         # first-request | with-sandbox
//! stream = true
//! max_request = "4M"
//! max_response = "16M"
//! timeout = 900
//! concurrency = 1
//! [devices.mac.sandbox]           # the device sandbox: profile.rs
//! ```
//!
//! Every table of 0.2.1 (`kind = "http"`, `kind = "command"`, `max_body`)
//! parses to the same meaning. Every refusal names the table, the key, the
//! allowed values and the fix. Unknown keys are errors, so a typo cannot
//! widen or narrow a device without a word.
//!
//! This file parses. render.rs writes a table back. Nothing here runs a
//! device: user.rs holds the gate and the off files, outproc.rs and
//! http.rs the runtimes.

use super::grant::Grant;
use super::profile::parse_profile;
pub use super::profile::{CallerMount, Connect, Net, SandboxProfile, CALLER_PROJECT_MOUNT};
pub use super::reader::ParseEnv;
use super::reader::Reader;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The largest `max_request` or `max_response` a device can set.
pub const MAX_BODY_CAP: usize = 64 << 20;
/// The default `max_request` and `max_response` (`max_body` of 0.2.1).
pub const DEFAULT_MAX_BODY: usize = 1 << 20;
/// The default and the largest `timeout`, in seconds.
pub const DEFAULT_TIMEOUT: u64 = 30;
pub const MAX_TIMEOUT: u64 = 3600;
/// `concurrency`: requests of one instance at the same time.
pub const MAX_CONCURRENCY: u32 = 16;
/// `idle`: seconds without a request before a shared instance stops.
/// One second is the floor, so a test and a user who wants a cold start
/// per request can both have it; the cost of a small number is only the
/// start time of the next request.
pub const DEFAULT_IDLE: u64 = 300;
pub const MIN_IDLE: u64 = 1;
pub const MAX_IDLE: u64 = 86_400;
/// `ready`: seconds that the start waits for `/v1/health`.
pub const DEFAULT_READY: u64 = 10;
pub const MAX_READY: u64 = 300;

/// The shape of the device program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// The command runs once per request: stdin in, stdout out.
    Exec,
    /// The command runs once per instance and serves HTTP/1.1 on a socket.
    Serve,
    /// The loopback proxy of `kind = "http"`. paddock is the program.
    HttpProxy,
}

impl Shape {
    pub fn name(self) -> &'static str {
        match self {
            Shape::Exec => "exec",
            Shape::Serve => "serve",
            Shape::HttpProxy => "http",
        }
    }
}

/// Where the device program runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "run")]
pub enum Run {
    /// A paddock sandbox of its own, with the profile of the table.
    Sandbox,
    /// On the host, as the user, with the whole environment that paddock
    /// got.
    Host {
        /// The working folder. None: the home.
        cwd: Option<PathBuf>,
        /// Fixed variables on top of the host environment.
        env: BTreeMap<String, String>,
    },
    /// Another paddock, over a transport.
    Remote { target: String, device: String },
    /// The http proxy: the broker itself.
    Broker,
}

impl Run {
    pub fn name(&self) -> &'static str {
        match self {
            Run::Sandbox => "sandbox",
            Run::Host { .. } => "host",
            Run::Remote { .. } => "remote",
            Run::Broker => "broker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// One instance per agent sandbox.
    Run,
    /// One instance per user, for every sandbox.
    Shared,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Run => "run",
            Scope::Shared => "shared",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Start {
    FirstRequest,
    WithSandbox,
}

impl Start {
    pub fn name(self) -> &'static str {
        match self {
            Start::FirstRequest => "first-request",
            Start::WithSandbox => "with-sandbox",
        }
    }
}

/// Whether the first request of a sandbox puts a card on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Ask,
    Allow,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Allow => "allow",
        }
    }
}

/// The sizes and times of one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub max_request: usize,
    pub max_response: usize,
    /// Seconds per request.
    pub timeout: u64,
    /// Requests of one instance at the same time.
    pub concurrency: u32,
}

/// The loopback proxy of `kind = "http"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HttpProxy {
    pub target: SocketAddr,
    pub methods: Vec<String>,
    pub paths: Vec<String>,
}

/// One `[devices.NAME]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceDef {
    pub name: String,
    pub enabled: bool,
    pub shape: Shape,
    pub run: Run,
    pub scope: Scope,
    pub start: Start,
    pub stream: bool,
    /// None: no `grant` key, every sandbox sees the device.
    pub grant: Option<Vec<Grant>>,
    pub mode: Mode,
    pub limits: Limits,
    pub idle: u64,
    pub ready: u64,
    /// The declared cost of one request, in micro units (ledger.rs). 0:
    /// no declared cost.
    pub cost: u64,
    /// The response header where a service reports the cost of each
    /// response.
    pub cost_header: Option<String>,
    /// The budget per run from the table, in micro units.
    pub budget: Option<u64>,
    /// The program and its fixed arguments. Empty for the http proxy.
    pub command: Vec<String>,
    pub sandbox: Option<SandboxProfile>,
    pub http: Option<HttpProxy>,
    /// Keys of 0.2.1 that the table used (`kind`, `max_body`), for the
    /// lines of `paddock doctor`.
    pub old_keys: Vec<String>,
}

impl DeviceDef {
    /// `max_body` of 0.2.1: the larger of the two sizes.
    pub fn max_body(&self) -> usize {
        self.limits.max_request.max(self.limits.max_response)
    }

    pub fn timeout(&self) -> u64 {
        self.limits.timeout
    }

    /// `kind` of 0.2.1, for the catalog of readers of version 1.
    pub fn kind_name(&self) -> &'static str {
        match self.shape {
            Shape::HttpProxy => "http",
            Shape::Exec => "command",
            Shape::Serve => "serve",
        }
    }

    /// True when a request of this device costs money: it needs a budget.
    pub fn paid(&self) -> bool {
        self.cost > 0 || self.cost_header.is_some()
    }

    /// The profile of the device sandbox, the default for a table without
    /// one.
    pub fn profile(&self) -> SandboxProfile {
        self.sandbox.clone().unwrap_or_default()
    }

    /// The reason why this paddock cannot run the device yet, or None
    /// when a runtime of today (the http proxy, the device sandbox, the
    /// host, a forward to another paddock) serves it. Every value of
    /// this version is served; the method stays for the next one.
    pub fn not_yet(&self) -> Option<String> {
        None
    }
}

/// A size: a number of bytes, or a number with K, M or G (1024 based).
pub fn parse_size(s: &str) -> Result<usize> {
    let t = s.trim();
    let (num, mult) = match t.char_indices().last() {
        Some((i, 'K' | 'k')) => (&t[..i], 1usize << 10),
        Some((i, 'M' | 'm')) => (&t[..i], 1 << 20),
        Some((i, 'G' | 'g')) => (&t[..i], 1 << 30),
        _ => (t, 1),
    };
    let n: usize = num
        .trim()
        .parse()
        .with_context(|| format!("{s:?} is not a size like 512, 256K or 1M"))?;
    let v = n
        .checked_mul(mult)
        .with_context(|| format!("{s:?} is too large"))?;
    if v == 0 || v > MAX_BODY_CAP {
        bail!("{s:?}: a size from 1 byte to 64M");
    }
    Ok(v)
}

/// A size as the table writes it: 256K, 1M, or bytes.
pub fn render_size(n: usize) -> String {
    if n >= 1 << 20 && n.is_multiple_of(1 << 20) {
        format!("{}M", n >> 20)
    } else if n >= 1 << 10 && n.is_multiple_of(1 << 10) {
        format!("{}K", n >> 10)
    } else {
        n.to_string()
    }
}

/// A name for `/dev/paddock/NAME`: lower case letters, digits, `-` and
/// `_`, not the name of a built-in file.
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && name.starts_with(|c: char| c.is_ascii_lowercase());
    if !ok {
        bail!("{name:?}: a device name has 1 to 32 lower case letters, digits, - or _, and starts with a letter");
    }
    if super::BUILTIN.contains(&name) || ["mount", "gw", "secret", "use"].contains(&name) {
        bail!("{name:?} is a built-in name");
    }
    Ok(())
}

pub(crate) fn methods_ok(m: &str) -> bool {
    (1..=16).contains(&m.len()) && m.chars().all(|c| c.is_ascii_uppercase())
}

pub(crate) fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// A variable name: letters, digits and `_`.
pub(crate) fn var_name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !n.starts_with(|c: char| c.is_ascii_digit())
}

/// `~` and `~/x` against `home`; everything else as written.
pub(crate) fn expand_home(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    }
}

/// The keys of a table per shape, for the message of an unknown key.
const COMMON_KEYS: &[&str] = &[
    "enabled",
    "shape",
    "run",
    "scope",
    "start",
    "stream",
    "grant",
    "mode",
    "max_request",
    "max_response",
    "max_body",
    "timeout",
    "concurrency",
    "idle",
    "ready",
    "cost",
    "budget",
    "command",
    "sandbox",
    "cwd",
    "env",
    "env_pass",
    "target",
    "device",
];
const SERVE_KEYS: &[&str] = &["cost_header"];
const HTTP_KEYS: &[&str] = &[
    "kind",
    "enabled",
    "target",
    "methods",
    "paths",
    "max_body",
    "max_request",
    "max_response",
    "timeout",
    "cost",
    "cost_header",
    "budget",
    "grant",
    "mode",
];
const OLD_COMMAND_KEYS: &[&str] = &["kind", "network"];

impl DeviceDef {
    /// Parse `[devices.NAME]` without a host: `~` stays as written.
    pub fn parse(name: &str, t: &toml::Table) -> Result<Self> {
        Self::parse_with(name, t, ParseEnv::default())
    }

    /// Parse `[devices.NAME]`; `~` expands to the home of `env`.
    pub fn parse_with(name: &str, t: &toml::Table, env: ParseEnv<'_>) -> Result<Self> {
        check_name(name)?;
        let at = format!("[devices.{name}]");
        let r = Reader {
            at: at.clone(),
            t,
            env,
        };
        let mut old_keys = Vec::new();
        let kind = match t.get("kind") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .with_context(|| format!("{at}: kind must be \"http\" or \"command\""))?,
            ),
        };
        if kind.is_some() && t.contains_key("shape") {
            bail!("{at}: use shape or kind, not both");
        }
        if kind.is_some() {
            old_keys.push("kind".to_string());
        }
        if t.contains_key("max_body") {
            old_keys.push("max_body".to_string());
        }
        // A remote table has no program here: its shape is the other
        // side's, which the forward reads from the catalog there.
        let remote = t.get("run").and_then(|v| v.as_str()) == Some("remote");
        let shape = match kind {
            Some("http") => Shape::HttpProxy,
            Some("command") => Shape::Exec,
            Some(other) => bail!("{at}: unknown kind {other:?}; use http or command"),
            None => match r.word("shape", &["exec", "serve"])? {
                Some("exec") => Shape::Exec,
                Some("serve") => Shape::Serve,
                None if remote => Shape::Exec,
                _ => bail!(
                    "{at}: shape must be \"exec\" or \"serve\" (or kind = \"http\" for a loopback proxy)"
                ),
            },
        };
        let mut allowed: Vec<&str> = match shape {
            Shape::HttpProxy => HTTP_KEYS.to_vec(),
            Shape::Exec => COMMON_KEYS.to_vec(),
            Shape::Serve => [COMMON_KEYS, SERVE_KEYS].concat(),
        };
        if kind == Some("command") {
            allowed.extend(OLD_COMMAND_KEYS);
        }
        r.unknown_keys(&allowed, shape.name())?;
        let enabled = r.flag("enabled")?;
        let timeout = r.int("timeout", 1, MAX_TIMEOUT)?.unwrap_or(DEFAULT_TIMEOUT);
        let sizes = (
            r.size("max_body")?,
            r.size("max_request")?,
            r.size("max_response")?,
        );
        let (max_request, max_response) = match sizes {
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => bail!(
                "{at}: max_body sets both max_request and max_response; use max_body alone, or the two keys"
            ),
            (Some(b), None, None) => (b, b),
            (None, q, p) => (
                q.unwrap_or(DEFAULT_MAX_BODY),
                p.unwrap_or(DEFAULT_MAX_BODY),
            ),
        };
        let concurrency = match r.int("concurrency", 1, MAX_CONCURRENCY as u64)? {
            Some(n) => n as u32,
            None if shape == Shape::Serve => 4,
            None => 1,
        };
        let cost = r.money("cost")?.unwrap_or(0);
        let budget = r.money("budget")?;
        let cost_header = match t.get("cost_header") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .filter(|h| !h.is_empty() && h.len() <= 64 && h.chars().all(token_char))
                    .with_context(|| {
                        format!("{at}: cost_header must be a header name like \"X-Cost\"")
                    })?
                    .to_string(),
            ),
        };
        let mode = match r.word("mode", &["ask", "allow"])? {
            Some("allow") => Mode::Allow,
            _ => Mode::Ask,
        };
        let grant = match t.get("grant") {
            None => None,
            Some(_) => {
                let list = r.str_list("grant")?.unwrap_or_default();
                if list.is_empty() {
                    bail!(
                        "{at}: grant = [] grants nothing. Remove the key so that every sandbox sees the device, or list grants: pasture:NAME, project:PATH, harness:NAME, all"
                    );
                }
                Some(
                    list.iter()
                        .map(|g| Grant::parse_with(g, env).with_context(|| at.clone()))
                        .collect::<Result<Vec<_>>>()?,
                )
            }
        };
        let limits = Limits {
            max_request,
            max_response,
            timeout,
            concurrency,
        };
        if shape == Shape::HttpProxy {
            return Ok(Self {
                name: name.to_string(),
                enabled,
                shape,
                run: Run::Broker,
                scope: Scope::Run,
                start: Start::FirstRequest,
                stream: false,
                grant,
                mode,
                limits,
                idle: DEFAULT_IDLE,
                ready: DEFAULT_READY,
                cost,
                cost_header,
                budget,
                command: Vec::new(),
                sandbox: None,
                http: Some(parse_http_proxy(&r)?),
                old_keys,
            });
        }
        let mut command = r.str_list("command")?.unwrap_or_default();
        if !remote && (command.is_empty() || command[0].is_empty()) {
            bail!(
                "{at}: command must be a list with the program first, for example [\"jq\", \"-c\", \".\"]"
            );
        }
        if command.iter().any(|a| a.contains('\0')) {
            bail!("{at}: command holds a NUL byte");
        }
        // `~/bin/x` as the program: the supervisor runs the path as given,
        // and no shell expands it, so the parser does, against the home of
        // the host (the device sandbox binds host paths at their own place).
        if command.first().is_some_and(|c| c.starts_with("~/")) {
            command[0] = r.env.expand(&command[0]).display().to_string();
        }
        let run = parse_run(&r, name)?;
        let host = matches!(run, Run::Host { .. });
        if host && mode == Mode::Allow {
            bail!(
                "{at}: mode = \"allow\" is refused for run = \"host\": a program that runs on the host as you shows the first-request card every session"
            );
        }
        let scope = match r.word("scope", &["run", "shared"])? {
            Some("shared") => Scope::Shared,
            _ => Scope::Run,
        };
        if scope == Scope::Shared && host {
            bail!(
                "{at}: scope = \"shared\" needs run = \"sandbox\": a shared instance serves every sandbox of this user, and a program with run = \"host\" runs as you, with your files and network, so it shows the first-request card per sandbox instead"
            );
        }
        let start = match r.word("start", &["first-request", "with-sandbox"])? {
            Some("with-sandbox") => Start::WithSandbox,
            _ => Start::FirstRequest,
        };
        let idle = r.int("idle", MIN_IDLE, MAX_IDLE)?.unwrap_or(DEFAULT_IDLE);
        let ready = r.int("ready", 1, MAX_READY)?.unwrap_or(DEFAULT_READY);
        let stream = r.flag("stream")?;
        let sandbox = match t.get("sandbox") {
            Some(_) if host => bail!(
                "{at}: a [devices.{name}.sandbox] table is not allowed with run = \"host\": the command runs as you, with your files and network. Remove the table, or use run = \"sandbox\""
            ),
            Some(v) => {
                if t.contains_key("network") {
                    bail!("{at}: network is the spelling of 0.2.1; with a [devices.{name}.sandbox] table set network there");
                }
                let pt = v.as_table().with_context(|| {
                    format!("{at}: sandbox must be a table, [devices.{name}.sandbox]")
                })?;
                Some(parse_profile(name, pt, env)?)
            }
            None if kind == Some("command") => Some(SandboxProfile {
                network: if r.flag("network")? {
                    Net::Egress
                } else {
                    Net::None
                },
                exec_base: true,
                ..Default::default()
            }),
            None => None,
        };
        if let Some(p) = &sandbox {
            if !p.caller.is_empty() && scope == Scope::Shared {
                bail!(
                    "{at}: a shared device serves many sandboxes and cannot mount the caller's project. Use scope = \"run\""
                );
            }
        }
        super::forward::check_table(
            &at,
            &run,
            mode,
            scope,
            start,
            !command.is_empty(),
            sandbox.is_some(),
        )?;
        Ok(Self {
            name: name.to_string(),
            enabled,
            shape,
            run,
            scope,
            start,
            stream,
            grant,
            mode,
            limits,
            idle,
            ready,
            cost,
            cost_header,
            budget,
            command,
            sandbox,
            http: None,
            old_keys,
        })
    }
}

/// `run`, with the keys of each mode.
fn parse_run(r: &Reader<'_>, name: &str) -> Result<Run> {
    let (at, t, env) = (&r.at, r.t, r.env);
    let run = match r.word("run", &["sandbox", "host", "remote"])? {
        None | Some("sandbox") => Run::Sandbox,
        Some("host") => Run::Host {
            cwd: match t.get("cwd") {
                None => None,
                Some(v) => Some(env.expand(v.as_str().with_context(|| {
                    format!("{at}: cwd must be a folder, for example \"~/notes\"")
                })?)),
            },
            env: parse_env(r)?,
        },
        Some("remote") => {
            let target = t
                .get("target")
                .and_then(|v| v.as_str())
                .filter(|s| s.starts_with("ssh://") || s.starts_with("unix:"))
                .with_context(|| {
                    format!(
                        "{at}: run = \"remote\" needs target = \"ssh://HOST\" or \"unix:/PATH\""
                    )
                })?;
            let device = t.get("device").and_then(|v| v.as_str()).with_context(|| {
                format!(
                    "{at}: run = \"remote\" needs device = \"NAME\", the device on the other side"
                )
            })?;
            check_name(device).with_context(|| format!("{at}: device"))?;
            Run::Remote {
                target: target.to_string(),
                device: device.to_string(),
            }
        }
        Some(_) => unreachable!(),
    };
    let host = matches!(run, Run::Host { .. });
    if t.contains_key("env_pass") {
        if host {
            bail!(
                "{at}: a host program gets your whole environment; env_pass is a key of the sandbox profile"
            );
        }
        bail!(
            "{at}: env_pass belongs in [devices.{name}.sandbox], the profile of the device sandbox"
        );
    }
    for k in ["cwd", "env"] {
        if t.contains_key(k) && !host {
            bail!(
                "{at}: {k} is for run = \"host\" only. A device sandbox sets {} in [devices.{name}.sandbox]",
                if k == "cwd" {
                    "its own working folder"
                } else {
                    "env"
                }
            );
        }
    }
    for k in ["target", "device"] {
        if t.contains_key(k) && !matches!(run, Run::Remote { .. }) {
            bail!("{at}: {k} is for run = \"remote\" only");
        }
    }
    Ok(run)
}

/// `env = { NAME = "value" }` of a host device: fixed values on top of
/// the host environment.
fn parse_env(r: &Reader<'_>) -> Result<BTreeMap<String, String>> {
    let at = &r.at;
    let mut out = BTreeMap::new();
    let Some(v) = r.t.get("env") else {
        return Ok(out);
    };
    let table = v
        .as_table()
        .with_context(|| format!("{at}: env must be a table of NAME = \"value\""))?;
    for (k, v) in table {
        let s = v
            .as_str()
            .with_context(|| format!("{at}: env {k} must be a string"))?;
        if !var_name_ok(k) {
            bail!("{at}: env: {k:?} is not a variable name (letters, digits and _)");
        }
        out.insert(k.clone(), s.to_string());
    }
    Ok(out)
}

fn parse_http_proxy(r: &Reader<'_>) -> Result<HttpProxy> {
    let at = &r.at;
    let raw =
        r.t.get("target")
            .and_then(|v| v.as_str())
            .with_context(|| format!("{at}: target must be ADDRESS:PORT"))?;
    let target: SocketAddr = raw.parse().with_context(|| {
        format!(
            "{at}: target {raw:?} must be an IP address and a port, for example 127.0.0.1:11434"
        )
    })?;
    if !target.ip().is_loopback() {
        bail!(
            "{at}: target {raw} is not a loopback address; an http device reaches 127.0.0.0/8 or ::1 only"
        );
    }
    if target.port() == 0 {
        bail!("{at}: target needs a port");
    }
    let methods = r.str_list("methods")?.unwrap_or_else(|| vec!["GET".into()]);
    if methods.is_empty() || !methods.iter().all(|m| methods_ok(m)) {
        bail!("{at}: methods must be upper case HTTP methods, for example [\"GET\", \"POST\"]");
    }
    let paths = r.str_list("paths")?.unwrap_or_else(|| vec!["/".into()]);
    if paths.is_empty()
        || !paths
            .iter()
            .all(|p| p.starts_with('/') && super::http::path_ok(p))
    {
        bail!("{at}: paths must be prefixes that start with /, for example [\"/api/\"]");
    }
    Ok(HttpProxy {
        target,
        methods,
        paths,
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
