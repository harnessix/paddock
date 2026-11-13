//! What `paddock ps`, `paddock board --json`, `paddock gateway NAME status`
//! and `paddock doctor` say about the gateways.
//!
//! [`Gateway`] is the one shape for every JSON reader: the Omarchy panel,
//! the launcher and the TUI. A gateway that does not run has `state =
//! "stopped"` (or `"failed"`) and no pid; its port is the one of the last
//! start.

use super::state::{self, Phase};
use crate::env::HostEnv;
use crate::profile::HarnessFactory;
use serde::{Deserialize, Serialize};

/// One gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gateway {
    pub name: String,
    /// `starting`, `running`, `restarting`, `failed` or `stopped`.
    pub state: String,
    pub port: Option<u16>,
    pub url: Option<String>,
    /// Seconds since the gateway that runs now started.
    pub uptime: Option<u64>,
    /// When the supervisor started, RFC 3339 in UTC.
    pub started: Option<String>,
    pub restarts: u32,
    pub last_exit: Option<String>,
    /// Client sandboxes attached now.
    pub clients: u32,
    /// The supervisor.
    pub pid: Option<u32>,
    pub backend: Option<String>,
    pub network: Option<String>,
    /// `systemd-run`, `launch agent` or `detached`.
    pub how: Option<String>,
    pub unit: Option<String>,
    pub reason: Option<String>,
    /// The port of a gateway on the host that paddock did not start.
    pub host_gateway: Option<u16>,
}

/// The gateway `name` now.
pub fn of(env: &HostEnv, name: &str, host_port: Option<u16>) -> Gateway {
    let st = state::read(env, name);
    let alive = st
        .as_ref()
        .is_some_and(|s| s.pid > 0 && crate::state::alive(s.pid));
    let cfg = state::read_start(env, name);
    let now = crate::state::now();
    let phase = match (&st, alive) {
        (Some(s), true) => s.phase.name(),
        (Some(s), false) if s.phase == Phase::Failed => "failed",
        _ => "stopped",
    };
    let live = st.as_ref().filter(|_| alive);
    let host_gateway =
        host_port.filter(|p| live.map(|s| s.port) != Some(*p) && super::launch::answers(*p));
    Gateway {
        name: name.to_string(),
        state: phase.to_string(),
        port: live.map(|s| s.port).or(cfg.as_ref().map(|c| c.port)),
        url: live.map(|s| s.url.clone()),
        uptime: live
            .filter(|s| s.phase == Phase::Running)
            .map(|s| now.saturating_sub(s.since)),
        started: live.map(|s| crate::waiting::format_rfc3339(s.started)),
        restarts: st.as_ref().map(|s| s.restarts).unwrap_or(0),
        last_exit: st.as_ref().and_then(|s| s.last_exit.clone()),
        clients: if alive {
            state::clients(env, name).len() as u32
        } else {
            0
        },
        pid: live.map(|s| s.pid),
        backend: st
            .as_ref()
            .map(|s| s.backend.clone())
            .filter(|b| !b.is_empty()),
        network: st
            .as_ref()
            .map(|s| s.network.clone())
            .or(cfg.as_ref().map(|c| c.network.clone())),
        how: cfg.as_ref().map(|c| c.how.clone()),
        unit: cfg.as_ref().and_then(|c| c.unit.clone()),
        reason: st.as_ref().and_then(|s| s.reason.clone()),
        host_gateway,
    }
}

/// The profiles with a `[gateway]` table, with their host port.
pub fn profiles(env: &HostEnv) -> Vec<(String, Option<u16>)> {
    let Ok(f) = HarnessFactory::new(env) else {
        return Vec::new();
    };
    f.names()
        .into_iter()
        .filter_map(|n| {
            let p = f.profile(&n)?;
            let g = p.gateway.as_ref()?;
            Some((n.clone(), g.host_port))
        })
        .collect()
}

/// Every gateway that paddock started once, for `ps` and `board`. A
/// profile without a start yet is left out, so a host without gateway
/// agents shows nothing.
pub fn all(env: &HostEnv) -> Vec<Gateway> {
    let known = profiles(env);
    state::names(env)
        .into_iter()
        .map(|n| {
            let hp = known.iter().find(|(k, _)| *k == n).and_then(|(_, p)| *p);
            of(env, &n, hp)
        })
        .collect()
}

/// One line of `paddock ps` and of `paddock gateway NAME status`.
pub fn line(g: &Gateway) -> String {
    let mut s = format!("gateway {:<15} {}", g.name, g.state);
    if let Some(u) = &g.url {
        s.push_str(&format!(" {u}"));
    } else if let Some(p) = g.port {
        s.push_str(&format!(", port {p}"));
    }
    if let Some(up) = g.uptime {
        s.push_str(&format!(", up {}m", up / 60));
    }
    if g.state != "stopped" {
        s.push_str(&format!(
            ", {} client{}",
            g.clients,
            if g.clients == 1 { "" } else { "s" }
        ));
    }
    if g.restarts > 0 {
        s.push_str(&format!(", {} restarts", g.restarts));
    }
    if let (Some(b), Some(n)) = (&g.backend, &g.network) {
        if g.state != "stopped" {
            s.push_str(&format!(", {b}, network {n}"));
        }
    }
    if let Some(p) = g.pid {
        s.push_str(&format!(", pid {p}"));
    }
    if let Some(u) = &g.unit {
        if g.pid.is_some() {
            s.push_str(&format!(", unit {u}"));
        }
    }
    s
}

/// The gateway part of `paddock doctor`: the line of the executor of host
/// requests (gatewayd.rs), then the gateway agents.
pub fn print_doctor(env: &HostEnv) {
    println!("{}", crate::gatewayd::status_line(env));
    for l in doctor_lines(env) {
        println!("{l}");
    }
}

/// The lines of `paddock doctor` about the gateway agents, with their
/// heading. None when no profile has a gateway.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    let mut out = vec!["\nGateway agents (the gateway runs in a sandbox of its own):".to_string()];
    for (name, hp) in profiles(env) {
        let g = of(env, &name, hp);
        let tag = match g.state.as_str() {
            "running" => "ok",
            "failed" => "failed",
            "stopped" => "off",
            _ => "wait",
        };
        let what = match g.state.as_str() {
            "stopped" => format!(
                "no sandboxed gateway runs. paddock gateway {name} start runs one ({})",
                super::control::how(env)
            ),
            _ => line(&g),
        };
        out.push(format!("  {tag:<8} {name:<10} {what}"));
        if let Some(r) = &g.reason {
            out.push(format!("           {:<10} {r}", ""));
        }
        if let Some(p) = g.host_gateway {
            out.push(format!(
                "           {:<10} a gateway that paddock did not start answers on 127.0.0.1:{p}: it runs on the host without a sandbox. Stop it, then run paddock gateway {name} start",
                ""
            ));
        }
    }
    if out.len() == 1 {
        out.clear();
    }
    out
}
