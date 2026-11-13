//! The two sandboxes of a gateway agent.
//!
//! - The gateway sandbox: the harness, its own home
//!   (`<data>/gateways/NAME/home`, never the host home), an empty working
//!   folder, the network mode of the user configuration (egress or host).
//!   In mode egress the sandbox has its own network, and paddock publishes
//!   the one port on `127.0.0.1` of the host (pasta `-t 127.0.0.1/PORT` on
//!   Linux). Seatbelt lets a sandbox in mode egress listen on localhost
//!   only.
//! - The client sandbox: the harness, the home of the clients, an empty
//!   working folder, outbound network, and of the host loopback only the
//!   port of the gateway. It never sees the gateway's home.
//!
//! policy.rs runs on both, as on every sandbox.

use super::shape::{Shape, Values};
use super::state;
use crate::env::HostEnv;
use crate::profile::{Harness, Profile};
use crate::spec::{Mount, Network, SandboxSpec};
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeMap;

/// The id of the gateway sandbox.
pub fn gateway_id(name: &str) -> String {
    format!("gw-{name}")
}

/// The id of a client sandbox.
pub fn client_id(name: &str, pid: u32) -> String {
    format!("gwc-{name}-{pid}")
}

/// The network mode of the gateway: `--network`, else `[network] mode` of
/// the user configuration, else egress. A gateway must listen for its
/// clients, so modes without a listening port are refused.
pub fn network(env: &HostEnv, wanted: Option<Network>) -> Result<Network> {
    let user = crate::manifest::Manifest::load_user(env)?;
    let (mode, from) = match wanted {
        Some(n) => (n, "--network"),
        None => match user.network.mode {
            Some(n) => (n, "[network] mode of the user configuration"),
            None => (Network::Egress, "the default"),
        },
    };
    match mode {
        Network::Egress | Network::Host => Ok(mode),
        other => bail!(
            "network mode {} ({from}) gives the gateway no port for its clients: use egress (outbound only, the default) or host",
            format!("{other:?}").to_lowercase()
        ),
    }
}

/// The variables that the gateway gets from the host: the profile's
/// `env_pass` and `[env] pass` of the user configuration. `start` stores
/// their values, so a restart by the user manager has them too.
pub fn secret_names(env: &HostEnv, profile: &Profile) -> Result<Vec<String>> {
    let user = crate::manifest::Manifest::load_user(env)?;
    let mut v: Vec<String> = profile.state.env_pass.clone();
    for n in user.env.pass {
        if !v.contains(&n) {
            v.push(n);
        }
    }
    Ok(v)
}

/// The mounts and the path of the harness, as an agent sandbox has them.
fn add_harness(spec: &mut SandboxSpec, harness: &Harness) {
    let origin = format!("profile {}: install", harness.profile.name);
    for p in &harness.install_mounts {
        spec.add_mount(Mount::same(p, false, &origin));
    }
    if let Some(d) = harness.exe.parent() {
        spec.inherit_path.push(d.to_path_buf());
    }
    for d in &harness.path_dirs {
        spec.add_path(d.clone());
    }
}

/// The host proxy, as an agent sandbox has it: traffic leaves the way the
/// traffic of the host does.
fn add_proxy(env: &HostEnv, spec: &mut SandboxSpec) {
    for k in crate::proxy::PROXY_VARS {
        if let Some(v) = env.var(k).filter(|v| !v.is_empty()) {
            spec.set_secret_env(k, v, "network: host proxy");
            if let Some((_, host, port)) = crate::proxy::parse_url(v) {
                if crate::proxy::is_loopback(&host) && !k.to_ascii_uppercase().starts_with("NO_") {
                    spec.add_host_port(port, &format!("network: host proxy in {k}"));
                }
            }
        }
    }
}

fn new_spec(
    env: &HostEnv,
    id: String,
    profile: &Profile,
    project: &std::path::Path,
    home: std::path::PathBuf,
) -> Result<SandboxSpec> {
    state::make(project)?;
    state::make(&home)?;
    Ok(SandboxSpec::new(
        id,
        &profile.name,
        project,
        home,
        &env.home,
        env.uid,
        env.gid,
        &env.user,
    ))
}

/// The spec of the gateway sandbox.
pub fn gateway(
    env: &HostEnv,
    harness: &Harness,
    shape: &Shape,
    net: Network,
    port: u16,
    token: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<SandboxSpec> {
    let profile = &harness.profile;
    let name = &profile.name;
    let mut s = new_spec(
        env,
        gateway_id(name),
        profile,
        &state::project(env, name),
        state::home(env, name),
    )?;
    s.network = net;
    if net == Network::Egress {
        s.publish = vec![port];
    }
    add_harness(&mut s, harness);
    let porigin = format!("profile {name}");
    for rel in &profile.state.import_ro {
        let host = env.home.join(rel);
        if host.exists() {
            s.add_mount(Mount::same(&host, false, &format!("{porigin} import_ro")));
        }
    }
    for (k, v) in &profile.launch.env {
        s.set_env(k, v.clone(), &porigin);
    }
    add_proxy(env, &mut s);
    for (k, v) in secrets {
        if let Some(why) = crate::policy::never_pass(k, Some(v)) {
            s.deny(crate::boundary::ENV, k, why.name());
            continue;
        }
        s.set_secret_env(k, v.clone(), &format!("{porigin} env_pass"));
    }
    let values = Values { port, token };
    for (k, v) in shape.gateway_env(&values) {
        s.set_secret_env(&k, v, &format!("{porigin} [gateway] env"));
    }
    s.set_env("TERM", "dumb", "gateway sandbox");
    let user = crate::manifest::Manifest::load_user(env)?;
    crate::privacy::apply(profile, crate::privacy::mode(&user), &mut s)?;
    let mut cmd = vec![harness.exe.display().to_string()];
    cmd.extend(shape.gateway_args(&values));
    s.command = cmd;
    crate::policy::enforce(env, &s).with_context(|| format!("the {name} gateway"))?;
    Ok(s)
}

/// The variables of the terminal that a client interface needs.
const TERMINAL_VARS: &[&str] = &[
    "TERM",
    "COLORTERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "NO_COLOR",
];

/// The spec of a client sandbox for the gateway on `port`. `command` is the
/// client command (`tui`), `extra` the arguments of the user after it.
pub fn client(
    env: &HostEnv,
    harness: &Harness,
    shape: &Shape,
    port: u16,
    token: &str,
    command: &str,
    extra: &[String],
) -> Result<SandboxSpec> {
    let profile = &harness.profile;
    let name = &profile.name;
    let mut s = new_spec(
        env,
        client_id(name, std::process::id()),
        profile,
        &state::client_project(env, name),
        state::client_home(env, name),
    )?;
    s.network = Network::Egress;
    s.add_host_port(port, &format!("the sandboxed {name} gateway"));
    add_harness(&mut s, harness);
    let porigin = format!("profile {name}");
    for (k, v) in &profile.launch.env {
        s.set_env(k, v.clone(), &porigin);
    }
    for k in TERMINAL_VARS {
        if let Some(v) = env.var(k) {
            s.set_env(k, v, "the terminal");
        }
    }
    let values = Values { port, token };
    for (k, v) in shape.client_env(&values) {
        s.set_secret_env(&k, v, &format!("{porigin} [gateway.client] env"));
    }
    let user = crate::manifest::Manifest::load_user(env)?;
    crate::privacy::apply(profile, crate::privacy::mode(&user), &mut s)?;
    let mut cmd = vec![harness.exe.display().to_string()];
    cmd.extend(shape.client_args(&values, command, extra));
    s.command = cmd;
    crate::policy::enforce(env, &s).with_context(|| format!("the {name} gateway client"))?;
    Ok(s)
}
