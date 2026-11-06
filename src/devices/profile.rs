//! `[devices.NAME.sandbox]`: the profile of a device sandbox, and the
//! `connect` entries that reach one host and port through the broker.
//!
//! The baseline is the system read-only, an empty home, its own `/tmp`, no
//! network. Each key widens it. Nothing here starts a sandbox: sandbox.rs
//! turns the profile into a spec.

use super::config::var_name_ok;
use super::reader::{ParseEnv, Reader};
use crate::execbox::Home;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;

/// The most `connect` entries of one profile.
pub const MAX_CONNECT: usize = 16;
/// Where the caller's project appears in the device sandbox.
pub const CALLER_PROJECT_MOUNT: &str = "/run/paddock/caller/project";

/// The network of a device sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Net {
    None,
    Egress,
    Api,
}

impl Net {
    pub fn name(self) -> &'static str {
        match self {
            Net::None => "none",
            Net::Egress => "egress",
            Net::Api => "api",
        }
    }
}

/// `connect = ["2222:mac.local:22"]`: `127.0.0.1:2222` inside the device
/// sandbox reaches `mac.local:22` from the host. The sandbox side first,
/// the reverse of docker's `-p`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Connect {
    pub local_port: u16,
    pub host: String,
    pub port: u16,
}

impl Connect {
    /// `LOCAL:HOST:PORT` or `HOST:PORT` (the local port is the remote port).
    /// A loopback or LAN address is refused unless `allow_local` (tests).
    pub fn parse(s: &str, allow_local: bool) -> Result<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        let (local, host, port) = match parts.as_slice() {
            [host, port] => (None, *host, *port),
            [local, host, port] => (Some(*local), *host, *port),
            _ => bail!(
                "connect {s:?}: use LOCAL_PORT:HOST:PORT or HOST:PORT, for example 2222:mac.local:22"
            ),
        };
        let port: u16 = port
            .parse()
            .ok()
            .filter(|p| *p > 0)
            .with_context(|| format!("connect {s:?}: {port:?} is not a port (1 to 65535)"))?;
        let local_port: u16 = match local {
            Some(l) => l
                .parse()
                .ok()
                .filter(|p| *p > 0)
                .with_context(|| format!("connect {s:?}: {l:?} is not a port (1 to 65535)"))?,
            None => port,
        };
        if host.is_empty()
            || host.len() > 253
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            || host.starts_with('-')
        {
            bail!("connect {s:?}: {host:?} is not a host name or an address");
        }
        let local_literal = match host.parse::<IpAddr>() {
            Ok(IpAddr::V4(a)) => {
                a.is_loopback()
                    || a.is_private()
                    || a.is_link_local()
                    || a.is_unspecified()
                    || a.is_broadcast()
            }
            Ok(IpAddr::V6(a)) => a.is_loopback() || a.is_unspecified() || is_v6_local(&a),
            Err(_) => host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost"),
        };
        // The tests reach a server of their own on the loopback with the
        // flag; a user configuration never does.
        if local_literal && !allow_local {
            bail!(
                "connect {s:?}: connect reaches another machine. For a service on this host use an http device"
            );
        }
        if local_port < 1024 {
            bail!(
                "connect {s:?}: name a local port of 1024 or more: {}:{host}:{port}",
                suggest_port(local_port)
            );
        }
        Ok(Self {
            local_port,
            host: host.to_string(),
            port,
        })
    }

    pub fn render(&self) -> String {
        format!("{}:{}:{}", self.local_port, self.host, self.port)
    }
}

fn is_v6_local(a: &std::net::Ipv6Addr) -> bool {
    let s = a.segments();
    // fe80::/10 link-local, fc00::/7 unique local.
    (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xfe00) == 0xfc00
}

/// A local port of 1024 or more that looks like `port`: 22 gives 2222,
/// 80 gives 8080, 443 gives 8443.
fn suggest_port(port: u16) -> u16 {
    if port >= 1024 {
        return port;
    }
    if port >= 100 {
        return 8000 + port;
    }
    let digits = port.to_string();
    let repeated = digits.repeat(4 / digits.len().max(1));
    repeated.parse().unwrap_or(1024).max(1024)
}

/// What of the caller the device sandbox sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallerMount {
    /// The caller's project, read-only, at `CALLER_PROJECT_MOUNT`.
    ProjectRo,
}

impl CallerMount {
    pub fn name(self) -> &'static str {
        match self {
            CallerMount::ProjectRo => "project:ro",
        }
    }
}

/// `[devices.NAME.sandbox]`: the device sandbox. The baseline is the
/// system read-only, an empty home, its own `/tmp`, no network. Each key
/// widens it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxProfile {
    pub network: Net,
    /// Domains for `api`, as `--allow`.
    pub allow: Vec<String>,
    pub connect: Vec<Connect>,
    /// Host paths, read-only, at the same path inside.
    pub ro: Vec<PathBuf>,
    /// Host paths, read-write.
    pub rw: Vec<PathBuf>,
    pub home: Home,
    pub env: BTreeMap<String, String>,
    /// Host variables by name, on top of the base set.
    pub env_pass: Vec<String>,
    /// `[credentials.NAME]` of the user configuration.
    pub credentials: Vec<String>,
    pub caller: Vec<CallerMount>,
    /// Bytes, `MemoryMax`.
    pub mem: Option<u64>,
    /// Percent of one CPU, `CPUQuota`.
    pub cpu_percent: Option<u32>,
    /// The size of the `/tmp` of the device sandbox, in bytes. None: 1 GiB.
    pub tmp: Option<u64>,
    /// A `kind = "command"` table of 0.2.1: the `[exec]` keys of the user
    /// configuration are the base of the profile, as before.
    pub exec_base: bool,
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self {
            network: Net::None,
            allow: Vec::new(),
            connect: Vec::new(),
            ro: Vec::new(),
            rw: Vec::new(),
            home: Home::Hidden,
            env: BTreeMap::new(),
            env_pass: Vec::new(),
            credentials: Vec::new(),
            caller: Vec::new(),
            mem: None,
            cpu_percent: None,
            tmp: None,
            exec_base: false,
        }
    }
}

impl SandboxProfile {
    /// True when the profile reaches something beyond the baseline that a
    /// grant should guard: a credential, a `connect`, a writable host
    /// folder, or the home.
    pub fn reaches_something(&self) -> bool {
        !self.connect.is_empty()
            || !self.credentials.is_empty()
            || !self.rw.is_empty()
            || self.home != Home::Hidden
    }

    /// True when the profile adds nothing that the command runtime of
    /// 0.2.1 cannot give: the network only.
    pub fn is_plain(&self) -> bool {
        let base = Self {
            network: self.network,
            exec_base: self.exec_base,
            ..Default::default()
        };
        *self == base && self.network != Net::Api
    }

    /// The `[exec]` keys of the user configuration on top of this profile,
    /// for a table of 0.2.1 (`exec_base`). The result is what the device
    /// sandbox would get; `show` and `doctor` print it.
    pub fn with_exec_base(&self, exec: &crate::execbox::ExecCfg) -> Self {
        if !self.exec_base {
            return self.clone();
        }
        let mut p = self.clone();
        p.home = exec.home;
        p.rw = exec.paths.clone();
        p.ro = exec.ro_paths.clone();
        p.env_pass = exec.env.clone();
        p
    }

    /// One line per thing the device sandbox reaches, for `show` and the
    /// card. Empty for the baseline.
    pub fn reach_lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        match self.network {
            Net::None => {}
            Net::Egress => v.push("network: egress (every host)".into()),
            Net::Api => v.push(format!("network: api ({})", self.allow.join(", "))),
        }
        for c in &self.connect {
            v.push(format!(
                "connect: 127.0.0.1:{} inside reaches {}:{} from the host",
                c.local_port, c.host, c.port
            ));
        }
        for p in &self.ro {
            v.push(format!("read-only: {}", p.display()));
        }
        for p in &self.rw {
            v.push(format!("read-write: {}", p.display()));
        }
        match self.home {
            Home::Hidden => {}
            Home::ReadOnly => v.push("home: read-only".into()),
            Home::Full => v.push("home: full (read-write)".into()),
        }
        for c in &self.credentials {
            v.push(format!("credential: {c}"));
        }
        for c in &self.caller {
            v.push(format!(
                "caller: the project of the calling sandbox, read-only, at {CALLER_PROJECT_MOUNT} ({})",
                c.name()
            ));
        }
        v
    }
}

const PROFILE_KEYS: &[&str] = &[
    "network",
    "allow",
    "connect",
    "ro",
    "rw",
    "home",
    "env",
    "env_pass",
    "credentials",
    "caller",
    "mem",
    "cpus",
    "tmp",
];

/// `[devices.NAME.sandbox]`.
pub(super) fn parse_profile(
    name: &str,
    t: &toml::Table,
    env: ParseEnv<'_>,
) -> Result<SandboxProfile> {
    let at = format!("[devices.{name}.sandbox]");
    let r = Reader {
        at: at.clone(),
        t,
        env,
    };
    r.unknown_keys(PROFILE_KEYS, "the sandbox profile")?;
    let network = match r.word("network", &["none", "egress", "api"])? {
        None | Some("none") => Net::None,
        Some("egress") => Net::Egress,
        Some("api") => Net::Api,
        Some(_) => unreachable!(),
    };
    let allow = r.str_list("allow")?.unwrap_or_default();
    if !allow.is_empty() && network != Net::Api {
        bail!("{at}: allow lists domains for network = \"api\"; set network = \"api\" or remove allow");
    }
    for d in &allow {
        if d.is_empty()
            || !d
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-.*:".contains(c))
        {
            bail!("{at}: allow: {d:?} is not a domain (HOST, HOST:PORT or *.DOMAIN)");
        }
    }
    let connect_raw = r.str_list("connect")?.unwrap_or_default();
    if connect_raw.len() > MAX_CONNECT {
        bail!(
            "{at}: connect has {} entries; at most {MAX_CONNECT}",
            connect_raw.len()
        );
    }
    let mut connect = Vec::new();
    for c in &connect_raw {
        let c = Connect::parse(c, env.allow_local_connect).with_context(|| at.clone())?;
        if connect
            .iter()
            .any(|o: &Connect| o.local_port == c.local_port)
        {
            bail!("{at}: connect uses the local port {} twice", c.local_port);
        }
        connect.push(c);
    }
    let ro = r.paths("ro", env.home)?;
    let rw = r.paths("rw", env.home)?;
    let home = match r.word("home", &["hidden", "read-only", "full"])? {
        None | Some("hidden") => Home::Hidden,
        Some("read-only") => Home::ReadOnly,
        Some("full") => Home::Full,
        Some(_) => unreachable!(),
    };
    let mut env_vars = BTreeMap::new();
    if let Some(v) = t.get("env") {
        let tbl = v
            .as_table()
            .with_context(|| format!("{at}: env must be a table of NAME = \"value\""))?;
        for (k, v) in tbl {
            let s = v
                .as_str()
                .with_context(|| format!("{at}: env {k} must be a string"))?;
            if !var_name_ok(k) {
                bail!("{at}: env: {k:?} is not a variable name (letters, digits and _)");
            }
            env_vars.insert(k.clone(), s.to_string());
        }
    }
    let env_pass = r.var_names("env_pass")?;
    let credentials = r.str_list("credentials")?.unwrap_or_default();
    for c in &credentials {
        if c.is_empty()
            || !c
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("{at}: credentials: {c:?} is not a [credentials.NAME] name");
        }
    }
    let mut caller = Vec::new();
    for c in r.str_list("caller")?.unwrap_or_default() {
        match c.as_str() {
            "project:ro" => caller.push(CallerMount::ProjectRo),
            "project:rw" => bail!(
                "{at}: caller project:rw is refused: a device never writes into the caller's tree. Use project:ro"
            ),
            other => bail!("{at}: caller {other:?}: the only value is \"project:ro\""),
        }
    }
    let mem = match t.get("mem") {
        None => None,
        Some(toml::Value::String(s)) => {
            Some(crate::spawn::parse_bytes(s).with_context(|| format!("{at}: mem"))?)
        }
        Some(toml::Value::Integer(n)) if *n > 0 => {
            Some(crate::spawn::parse_bytes(&n.to_string()).with_context(|| format!("{at}: mem"))?)
        }
        Some(_) => bail!("{at}: mem must be a size such as \"512M\""),
    };
    let cpu_percent = match t.get("cpus") {
        None => None,
        Some(toml::Value::Integer(n)) => {
            Some(crate::spawn::parse_cpus(&n.to_string()).with_context(|| format!("{at}: cpus"))?)
        }
        Some(toml::Value::Float(f)) => {
            Some(crate::spawn::parse_cpus(&f.to_string()).with_context(|| format!("{at}: cpus"))?)
        }
        Some(_) => bail!("{at}: cpus must be a number such as 1 or 0.5"),
    };
    let tmp = match t.get("tmp") {
        None => None,
        Some(toml::Value::String(s)) => {
            Some(crate::spawn::parse_bytes(s).with_context(|| format!("{at}: tmp"))?)
        }
        Some(_) => bail!(
            "{at}: tmp must be a size such as \"4G\", the size of the /tmp of the device sandbox"
        ),
    };
    Ok(SandboxProfile {
        network,
        allow,
        connect,
        ro,
        rw,
        home,
        env: env_vars,
        env_pass,
        credentials,
        caller,
        mem,
        cpu_percent,
        tmp,
        exec_base: false,
    })
}
