//! The files of a gateway on the host. The sandboxes never see them.
//!
//! `<paddock state>/gateways/NAME/`:
//!
//! - `token`: the token of the gateway, mode 0600. `start` makes it once
//!   and keeps it, so a client that attached before a restart attaches
//!   again with the same token.
//! - `start.json`: what `start` chose (port, network, backend). The
//!   supervisor reads it, also after a restart by systemd or launchd.
//! - `secrets.json`, mode 0600: the values of the profile's `env_pass` at
//!   `start`. A unit of the user manager does not have the shell's
//!   variables, and a unit file or `systemctl show` must not show them.
//!   `stop` deletes it.
//! - `state.json`: what the supervisor does now.
//! - `gateway.log`: the output of the gateway.
//! - `clients/PID.json`: one file per attached client.
//!
//! `<paddock data>/gateways/NAME/` holds the homes: `home` is the home of
//! the gateway, `client/home` the home of its clients, and `project` and
//! `client/project` are their empty working folders.

use crate::env::HostEnv;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The version of `state.json` and of the JSON of `ps`, `board` and
/// `gateway status --json`.
pub const VERSION: u32 = 1;

/// The folder of the state files of gateway `name`.
pub fn dir(env: &HostEnv, name: &str) -> PathBuf {
    env.paddock_state().join("gateways").join(name)
}

/// The folder of the homes of gateway `name`.
pub fn data_dir(env: &HostEnv, name: &str) -> PathBuf {
    env.paddock_data().join("gateways").join(name)
}

/// The home of the gateway: its `~/.openclaw` lives here.
pub fn home(env: &HostEnv, name: &str) -> PathBuf {
    data_dir(env, name).join("home")
}

pub fn project(env: &HostEnv, name: &str) -> PathBuf {
    data_dir(env, name).join("project")
}

/// The home of the clients. It is not the gateway's home.
pub fn client_home(env: &HostEnv, name: &str) -> PathBuf {
    data_dir(env, name).join("client").join("home")
}

pub fn client_project(env: &HostEnv, name: &str) -> PathBuf {
    data_dir(env, name).join("client").join("project")
}

pub fn log_file(env: &HostEnv, name: &str) -> PathBuf {
    dir(env, name).join("gateway.log")
}

fn token_file(env: &HostEnv, name: &str) -> PathBuf {
    dir(env, name).join("token")
}

/// The names of the gateways that have a state folder.
pub fn names(env: &HostEnv) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(env.paddock_state().join("gateways"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// The token of gateway `name`, made on the first call: 32 random bytes as
/// hex, in a file of mode 0600.
pub fn token(env: &HostEnv, name: &str) -> Result<String> {
    let f = token_file(env, name);
    if let Ok(t) = std::fs::read_to_string(&f) {
        let t = t.trim().to_string();
        if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(t);
        }
    }
    crate::privfs::dir(&dir(env, name))?;
    let mut b = [0u8; 32];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut b)
        .context("cannot read /dev/urandom")?;
    let t = crate::manifest::hex(&b);
    crate::privfs::write(&f, &t)?;
    Ok(t)
}

/// The token when one exists. Never makes one.
pub fn existing_token(env: &HostEnv, name: &str) -> Option<String> {
    std::fs::read_to_string(token_file(env, name))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// What `start` chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartCfg {
    pub port: u16,
    /// The network mode: egress or host.
    pub network: String,
    /// auto, or the backend that the user named.
    pub backend: String,
    /// How the supervisor runs: `systemd-run`, `launch agent` or `owned`.
    pub how: String,
    /// The unit or the label of the user manager.
    pub unit: Option<String>,
}

pub fn read_start(env: &HostEnv, name: &str) -> Option<StartCfg> {
    let t = std::fs::read_to_string(dir(env, name).join("start.json")).ok()?;
    serde_json::from_str(&t).ok()
}

pub fn write_start(env: &HostEnv, name: &str, c: &StartCfg) -> Result<()> {
    crate::privfs::dir(&dir(env, name))?;
    crate::privfs::write(
        &dir(env, name).join("start.json"),
        serde_json::to_string_pretty(c)?,
    )?;
    Ok(())
}

pub fn read_secrets(env: &HostEnv, name: &str) -> BTreeMap<String, String> {
    std::fs::read_to_string(dir(env, name).join("secrets.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn write_secrets(env: &HostEnv, name: &str, v: &BTreeMap<String, String>) -> Result<()> {
    crate::privfs::dir(&dir(env, name))?;
    crate::privfs::write(
        &dir(env, name).join("secrets.json"),
        serde_json::to_string(v)?,
    )?;
    Ok(())
}

pub fn remove_secrets(env: &HostEnv, name: &str) {
    let _ = std::fs::remove_file(dir(env, name).join("secrets.json"));
}

/// The phase of the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// The gateway started and its port does not answer yet.
    Starting,
    Running,
    /// The gateway ended without a stop. The supervisor starts it again.
    Restarting,
    /// It ended too often: the supervisor gave up.
    Failed,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Running => "running",
            Phase::Restarting => "restarting",
            Phase::Failed => "failed",
        }
    }
}

/// `state.json`: what the supervisor does now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub v: u32,
    pub name: String,
    pub phase: Phase,
    /// The supervisor (`paddock gateway-serve NAME`).
    pub pid: u32,
    /// The sandbox process of the gateway, 0 when none runs.
    pub child: u32,
    pub port: u16,
    pub url: String,
    /// When the supervisor started, in seconds since the epoch.
    pub started: u64,
    /// When the gateway that runs now started.
    pub since: u64,
    pub restarts: u32,
    /// How the last gateway ended, for example `exit status 3`.
    pub last_exit: Option<String>,
    pub backend: String,
    pub network: String,
    pub how: String,
    pub unit: Option<String>,
    /// Why the backend is not the first choice, or why it failed.
    pub reason: Option<String>,
}

fn state_file(env: &HostEnv, name: &str) -> PathBuf {
    dir(env, name).join("state.json")
}

pub fn read(env: &HostEnv, name: &str) -> Option<State> {
    let t = std::fs::read_to_string(state_file(env, name)).ok()?;
    serde_json::from_str(&t).ok()
}

pub fn write(env: &HostEnv, s: &State) -> Result<()> {
    crate::privfs::dir(&dir(env, &s.name))?;
    crate::privfs::write_atomic(&state_file(env, &s.name), serde_json::to_string_pretty(s)?)?;
    Ok(())
}

pub fn remove(env: &HostEnv, name: &str) {
    let _ = std::fs::remove_file(state_file(env, name));
}

/// The state of a supervisor that still runs. A file of a supervisor that
/// died (`kill -9`, a reboot) counts as no gateway.
pub fn live(env: &HostEnv, name: &str) -> Option<State> {
    read(env, name).filter(|s| s.pid > 0 && crate::state::alive(s.pid))
}

/// One attached client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRec {
    pub pid: u32,
    pub started: u64,
}

fn clients_dir(env: &HostEnv, name: &str) -> PathBuf {
    dir(env, name).join("clients")
}

/// The clients that run now. The files of clients that ended go.
pub fn clients(env: &HostEnv, name: &str) -> Vec<ClientRec> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(clients_dir(env, name))
        .into_iter()
        .flatten()
        .flatten()
    {
        let rec: Option<ClientRec> = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok());
        match rec {
            Some(r) if crate::state::alive(r.pid) => out.push(r),
            _ => {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    out.sort_by_key(|r| r.started);
    out
}

/// The record of a client while it runs. Dropping it removes the file.
pub struct ClientGuard(PathBuf);

impl ClientGuard {
    pub fn new(env: &HostEnv, name: &str, pid: u32) -> Result<Self> {
        let d = clients_dir(env, name);
        crate::privfs::dir(&d)?;
        let f = d.join(format!("{pid}.json"));
        let rec = ClientRec {
            pid,
            started: crate::state::now(),
        };
        crate::privfs::write(&f, serde_json::to_string(&rec)?)?;
        Ok(Self(f))
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Make a folder of the gateway, mode 0700.
pub fn make(p: &Path) -> Result<()> {
    crate::privfs::dir(p).with_context(|| format!("cannot create {}", p.display()))
}
