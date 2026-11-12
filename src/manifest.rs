//! `paddock.toml`: explicit choices for one project, plus the same schema in
//! the user configuration file for defaults.
//!
//! The project file lives inside the writable project, so an agent can edit it.
//! Any field that widens access therefore needs a one-time approval on the host.

use crate::env::HostEnv;
use crate::spec::{GuiMode, Network};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "paddock.toml";

#[path = "manifest_net.rs"]
mod net;
pub use net::{check_max, net_name, net_rank, Base};

/// Tables of `~/.config/paddock/config.toml` that other modules read
/// themselves: `[kv]` (kv.rs), `[pastures]`
/// (pasture.rs), `[credentials]` (credentials.rs) and `[exec]`
/// (gatewayd.rs), `[storage]` (storage/dedupe.rs), `[snapshot]`
/// (snapshot.rs), `[profiles]` (shadow.rs), `[detach]` (detach.rs),
/// `[compaction]` (compaction.rs), `[hooks]` (hooks.rs), `[speckit]`
/// (speckit.rs), `[loft]` (loft.rs), `[horse]` (horse/live.rs), and
/// `[tack]` (tack/config.rs). A project
/// `paddock.toml` with one of them is invalid, except `[loft]` with only
/// `mode` (loft/sync.rs). `[devices]` is not in the
/// list: the user configuration defines devices there (devices.rs), and a
/// project only requests them with `use` (`check_project_devices`).
pub const USER_ONLY: &[&str] = &[
    "memory",
    "kv",
    "exec",
    "pastures",
    "credentials",
    "logins",
    "status",
    "monitor",
    "storage",
    "cache",
    "snapshot",
    "view",
    "profiles",
    "detach",
    "compaction",
    "macos",
    "hooks",
    "speckit",
    "loft",
    "horse",
    "turnout",
    "exclave",
    "tack",
];

/// Parse host user TOML without exposing private source lines in errors.
pub fn user_config_doc(text: &str) -> Result<toml::Value> {
    toml::from_str(text).map_err(|_| anyhow::anyhow!("invalid user configuration"))
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub default_harness: Option<String>,
    pub backend: Option<String>,
    #[serde(default)]
    pub mounts: Mounts,
    #[serde(default)]
    pub secrets: Secrets,
    #[serde(default)]
    pub network: NetworkCfg,
    #[serde(default)]
    pub gui: GuiCfg,
    #[serde(default)]
    pub env: EnvCfg,
    #[serde(default)]
    pub docker: DockerCfg,
    #[serde(default)]
    pub setup: SetupCfg,
    #[serde(default)]
    pub auth: AuthCfg,
    #[serde(default)]
    pub mcp: McpCfg,
    /// Host passthrough modes. Read by broker.rs from the user configuration
    /// only; a project file cannot set them.
    #[serde(default, skip_serializing)]
    pub host: Option<toml::Value>,
    /// Desktop apps (`paddock app`). Read from the user configuration only.
    #[serde(default)]
    pub apps: BTreeMap<String, AppCfg>,
    /// Recovery folders and ignore rules. Read by recover.rs from the user
    /// configuration only; a project file cannot set them.
    #[serde(default, skip_serializing)]
    pub recovery: Option<toml::Value>,
    /// Vendor telemetry inside the sandbox, and the training gate. Read
    /// from the user configuration only; a project file cannot change it.
    #[serde(default)]
    pub privacy: PrivacyCfg,
    /// Limits for `paddock-host spawn`. Read by spawn.rs from the user
    /// configuration only; a project file cannot set them.
    #[serde(default, skip_serializing)]
    pub spawn: Option<toml::Value>,
    /// `[devices] use = ["NAME"]` of a project file: a request for devices
    /// of the user configuration. It widens access, so it needs the
    /// one-time approval, and it never turns a device on. The user
    /// configuration's `[devices]` is read by devices.rs instead.
    #[serde(default)]
    pub devices: DevicesUse,
    /// The macOS guest of app mode (`paddock app run --backend macvm`).
    /// Read from the user configuration only.
    #[serde(default)]
    pub macvm: Option<MacvmCfg>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DevicesUse {
    #[serde(default)]
    pub r#use: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MacvmCfg {
    /// Save the machine state at quit, so the next start is warm. On by
    /// default. Off: every start is a cold boot.
    pub save: Option<bool>,
    /// When both VM slots are held and another session starts, save the
    /// least recently used session and free its slot. On by default.
    /// Off: a third session is refused.
    pub auto_suspend: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyCfg {
    /// off (default): paddock turns vendor telemetry off in every sandbox.
    /// vendor: the harness's own default applies.
    pub telemetry: Option<crate::privacy::Telemetry>,
    /// deny (default): paddock refuses the optional training opt-in in a
    /// harness's setup screens (training.rs). manual: paddock types
    /// nothing. vendor: paddock does not touch it.
    pub training: Option<crate::training::Policy>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppCfg {
    /// Workspace folders the app can open, read-write. The first one is the
    /// project for settings and the sandbox home.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Bind GPU render nodes.
    pub gpu: Option<bool>,
    /// Forward ports that the app listens on to 127.0.0.1 on the host, while
    /// it listens, so login callbacks from the host browser reach it. Any
    /// local process can connect to them meanwhile. Off by default.
    #[serde(default)]
    pub login_ports: bool,
    /// Open the first workspace folder in a new git worktree on each start.
    pub worktree: Option<bool>,
    /// The app's session bus. `private` (default): a bus inside the sandbox
    /// with a private keyring and paddock's link and notification services.
    /// `host-filtered`: the host bus through a filter that passes only the
    /// desktop portals, notifications and the tray, without a keyring.
    pub bus: Option<AppBus>,
    /// Flatpak apps only: start even when secret files in the workspace
    /// folders stay visible, because flatpak cannot hide a single file.
    /// Off by default; paddock warns on every start.
    #[serde(default)]
    pub unprotected_workspaces: bool,
    /// The memory of paddock's macOS guest for this app: `4G`, `6144M`.
    /// Default: 4 GiB. At most half the memory of the Mac.
    pub vm_memory: Option<String>,
    /// The app bundle on macOS. `paddock app install --drop` writes it for
    /// a bundle the user dropped on the drag target, and it wins over the
    /// `[app.macos] bundles` list of the profile.
    pub bundle: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppBus {
    #[default]
    Private,
    HostFiltered,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthCfg {
    /// shared (default), isolated, or isolated:NAME.
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpCfg {
    /// MCP servers whose host paths outside tool install directories may be
    /// mounted read-only.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Per-server startup policy, keyed by the harness's server name.
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerCfg>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerCfg {
    #[serde(default)]
    pub start: McpStart,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpStart {
    #[default]
    Auto,
    Always,
    Never,
    Lazy,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mounts {
    #[serde(default)]
    pub rw: Vec<String>,
    #[serde(default)]
    pub ro: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    /// Extra project paths to hide.
    #[serde(default)]
    pub hide: Vec<String>,
    /// Project paths that the secrets detector must not hide.
    #[serde(default)]
    pub reveal: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkCfg {
    pub mode: Option<Network>,
    #[serde(default)]
    pub publish: Vec<u16>,
    /// Host loopback ports the sandbox can reach, for example a local MCP server.
    #[serde(default)]
    pub host_ports: Vec<u16>,
    /// Extra domains for network modes `api` and `allowlist`, besides the harness's own.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Let the sandbox reach private and link-local networks (the LAN).
    /// Off by default: routers, NAS boxes and printers stay unreachable.
    pub lan: Option<bool>,
    /// The widest network mode of any sandbox: a hard ceiling that no
    /// project file, no approval and no `--network` flag can exceed. Read
    /// from the user configuration only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Network>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GuiCfg {
    /// off | headless | window | host. Unset lets the desktop detector decide.
    pub mode: Option<GuiMode>,
    /// Bind GPU render nodes.
    pub gpu: Option<bool>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvCfg {
    /// Pass the host environment (default true). False passes only the
    /// allowlist: locale, terminal, the profile's env_pass, and `pass`.
    pub inherit: Option<bool>,
    /// Host variables that are not inherited. `paddock env hide` edits this list.
    #[serde(default)]
    pub hide: Vec<String>,
    /// Host variables to pass through.
    #[serde(default)]
    pub pass: Vec<String>,
    /// Fixed variables.
    #[serde(default)]
    pub set: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DockerCfg {
    pub image: Option<String>,
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupCfg {
    /// Run `mise install` inside the sandbox when the project has a mise file.
    pub mise_install: Option<bool>,
    /// A `--worktree` run or a worker of a Rust project builds in its own
    /// copy of the repository's cargo build cache (storage/buildcache.rs).
    pub shared_build_cache: Option<bool>,
    /// Extra shell lines to run inside the sandbox before the harness.
    #[serde(default)]
    pub run: Vec<String>,
}

pub struct Loaded {
    pub merged: Manifest,
    pub project: Option<(PathBuf, Manifest)>,
    /// What the project file is compared with to find what it widens.
    pub base: Base,
}

impl Manifest {
    /// Read a project file. The sandbox can write the project, so a symlink
    /// at the file is refused, and a FIFO or a huge file never hangs paddock.
    pub fn read(path: &Path) -> Result<Self> {
        Self::read_with(path, false)
    }

    fn read_with(path: &Path, follow: bool) -> Result<Self> {
        let bytes = crate::safefs::read_path(path, follow, crate::safefs::MAX_READ)?
            .with_context(|| format!("{} does not exist", path.display()))?;
        let text = String::from_utf8(bytes)
            .with_context(|| format!("{} is not UTF-8 text", path.display()))?;
        if !follow {
            // A clear refusal of a table that only the user file may have,
            // before the schema names it an unknown field.
            let v: Option<toml::Value> = toml::from_str(&text).ok();
            if let Some(v) = &v {
                check_project_devices(path, v)?;
                if v.get("network").and_then(|n| n.get("max")).is_some() {
                    bail!(
                        "{}: network.max is only read from the user configuration (~/.config/paddock/config.toml); a project paddock.toml cannot set it",
                        path.display()
                    );
                }
            }
            // `[loft] mode` alone is a project setting (loft/sync.rs).
            let loft_mode_only = v
                .as_ref()
                .and_then(|v| v.get("loft"))
                .and_then(|l| l.as_table())
                .is_some_and(|t| !t.is_empty() && t.keys().all(|k| k == "mode"));
            if let Some(k) = USER_ONLY.iter().find(|k| {
                v.as_ref().is_some_and(|v| v.get(**k).is_some())
                    && !(**k == "loft" && loft_mode_only)
            }) {
                bail!(
                    "{}: [{k}] is only read from the user configuration (~/.config/paddock/config.toml); a project paddock.toml cannot set it, because an agent in the sandbox can edit the project",
                    path.display()
                );
            }
            if loft_mode_only {
                if let Some(mut v) = v {
                    if let Some(t) = v.as_table_mut() {
                        t.remove("loft");
                    }
                    return v
                        .try_into()
                        .with_context(|| format!("{} is invalid", path.display()));
                }
            }
            return toml::from_str(&text).with_context(|| format!("{} is invalid", path.display()));
        }
        Self::parse_user(&text).with_context(|| format!("{} is invalid", path.display()))
    }

    /// The user configuration. It has tables that only other parts of
    /// paddock read (`USER_ONLY`), and a project `paddock.toml` must never
    /// have them, so they are taken out here instead of in the schema: the
    /// project file still fails on them.
    pub fn parse_user(text: &str) -> Result<Self> {
        // User-only tables can contain private transport arguments. TOML
        // diagnostics include source lines, so do not expose them here.
        let mut v = user_config_doc(text)?;
        if let Some(t) = v.as_table_mut() {
            for k in USER_ONLY {
                t.remove(*k);
            }
            // The device definitions; devices.rs reads them.
            t.remove("devices");
        }
        Ok(v.try_into()?)
    }

    /// User configuration first, then the project file on top of it.
    pub fn load(env: &HostEnv, project: &Path) -> Result<Loaded> {
        let user_file = env.paddock_config().join("config.toml");
        let user = if user_file.is_file() {
            // The user configuration belongs to the host; it may be a link.
            Self::read_with(&user_file, true)?
        } else {
            Self::default()
        };
        let project_file = project.join(PROJECT_FILE);
        let proj = if project_file.is_file() {
            Some((project_file.clone(), Self::read(&project_file)?))
        } else {
            None
        };
        let max = user.network.max;
        if let Some(m) = user.network.mode {
            check_max(max, m, &format!("{}: network.mode =", user_file.display()))?;
        }
        if let Some((f, m)) = proj.as_ref().and_then(|(f, p)| Some((f, p.network.mode?))) {
            check_max(max, m, &format!("{}: network.mode =", f.display()))?;
        }
        let base = Base::of_user(&user);
        let mut merged = user;
        if let Some((_, p)) = &proj {
            merged.merge(p.clone());
        }
        // Without a mode anywhere, the default stays under the ceiling.
        if merged.network.mode.is_none() && max.is_some() {
            merged.network.mode = Some(base.mode);
        }
        Ok(Loaded {
            merged,
            project: proj,
            base,
        })
    }

    /// The warnings for the tables of a project file that only the user
    /// configuration can set.
    pub fn project_warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.host.as_ref().and_then(|h| h.get("tools")).is_some() {
            w.push(
                "[host.tools] in a project paddock.toml is ignored: named host tools come only from the user configuration".to_string(),
            );
        }
        if self.host.is_some()
            || !self.apps.is_empty()
            || self.recovery.is_some()
            || self.privacy.telemetry.is_some()
            || self.privacy.training.is_some()
            || self.spawn.is_some()
        {
            w.push(
                "[host], [apps], [recovery], [privacy] and [spawn] in a project paddock.toml are ignored".to_string(),
            );
        }
        w
    }

    fn merge(&mut self, o: Manifest) {
        // [host], [apps], [recovery], [privacy] and [spawn] are trusted only
        // from the user configuration.
        for w in o.project_warnings() {
            log::warn!("{w}");
        }
        if o.default_harness.is_some() {
            self.default_harness = o.default_harness;
        }
        if o.backend.is_some() {
            self.backend = o.backend;
        }
        self.mounts.rw.extend(o.mounts.rw);
        self.mounts.ro.extend(o.mounts.ro);
        self.secrets.hide.extend(o.secrets.hide);
        self.secrets.reveal.extend(o.secrets.reveal);
        if o.network.mode.is_some() {
            self.network.mode = o.network.mode;
        }
        self.network.publish.extend(o.network.publish);
        self.network.host_ports.extend(o.network.host_ports);
        self.network.allow.extend(o.network.allow);
        if o.network.lan.is_some() {
            self.network.lan = o.network.lan;
        }
        if o.auth.mode.is_some() {
            self.auth.mode = o.auth.mode;
        }
        self.mcp.allow.extend(o.mcp.allow);
        self.mcp.servers.extend(o.mcp.servers);
        if o.gui.mode.is_some() {
            self.gui.mode = o.gui.mode;
        }
        if o.gui.gpu.is_some() {
            self.gui.gpu = o.gui.gpu;
        }
        if o.gui.width.is_some() {
            self.gui.width = o.gui.width;
        }
        if o.gui.height.is_some() {
            self.gui.height = o.gui.height;
        }
        if o.env.inherit.is_some() {
            self.env.inherit = o.env.inherit;
        }
        self.env.pass.extend(o.env.pass);
        self.env.hide.extend(o.env.hide);
        self.env.set.extend(o.env.set);
        if o.docker.image.is_some() {
            self.docker.image = o.docker.image;
        }
        self.docker.packages.extend(o.docker.packages);
        if o.setup.mise_install.is_some() {
            self.setup.mise_install = o.setup.mise_install;
        }
        if o.setup.shared_build_cache.is_some() {
            self.setup.shared_build_cache = o.setup.shared_build_cache;
        }
        self.setup.run.extend(o.setup.run);
        self.devices.r#use.extend(o.devices.r#use);
    }

    /// A network mode of the project file wider than the user's, unless
    /// the command line names the mode. `host` has its own key.
    fn wider_mode(&self, base: &Base) -> Option<Network> {
        self.network.mode.filter(|m| {
            !base.cli_network && *m != Network::Host && net_rank(*m) > net_rank(base.mode)
        })
    }

    fn host_mode(&self, base: &Base) -> bool {
        !base.cli_network && self.network.mode == Some(Network::Host)
    }

    /// `network.allow` domains that the user configuration does not allow.
    fn extra_allow(&self, base: &Base) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for d in &self.network.allow {
            if !base.allow.contains(d) && !v.contains(d) {
                v.push(d.clone());
            }
        }
        v
    }

    /// The part of a project file that widens access over `base`. Approval
    /// covers exactly this.
    pub fn widening(&self, base: &Base) -> serde_json::Value {
        let mut widening = serde_json::json!({
            "mounts": { "rw": self.mounts.rw, "ro": self.mounts.ro },
            "secrets.reveal": self.secrets.reveal,
            "network.host": self.host_mode(base),
            "network.publish": self.network.publish,
            "network.host_ports": self.network.host_ports,
            "network.lan": self.network.lan == Some(true),
            "mcp.allow": self.mcp.allow,
            "auth.shared": self.auth.mode.as_deref() == Some("shared"),
            "env.inherit": self.env.inherit == Some(true),
            "gui.host": self.gui.mode == Some(GuiMode::Host),
            "gui.gpu": self.gui.gpu == Some(true),
            "env.pass": self.env.pass,
            "setup.run": self.setup.run,
            "docker": { "image": self.docker.image, "packages": self.docker.packages },
            "devices.use": self.devices.r#use,
        });
        let start: BTreeMap<_, _> = self
            .mcp
            .servers
            .iter()
            .filter(|(_, p)| p.start != McpStart::Never)
            .collect();
        if !start.is_empty() {
            widening["mcp.start"] = serde_json::json!(start);
        }
        // Only when present, so the approvals of other files stay valid.
        if let Some(m) = self.wider_mode(base) {
            widening["network.mode"] = serde_json::json!(m);
        }
        let allow = self.extra_allow(base);
        if !allow.is_empty() {
            widening["network.allow"] = serde_json::json!(allow);
        }
        widening
    }

    pub fn widens(&self, base: &Base) -> bool {
        !self.mounts.rw.is_empty()
            || !self.mounts.ro.is_empty()
            || !self.secrets.reveal.is_empty()
            || self.host_mode(base)
            || self.wider_mode(base).is_some()
            || !self.extra_allow(base).is_empty()
            || !self.network.publish.is_empty()
            || !self.network.host_ports.is_empty()
            || self.network.lan == Some(true)
            || !self.mcp.allow.is_empty()
            || self
                .mcp
                .servers
                .values()
                .any(|p| p.start != McpStart::Never)
            || self.auth.mode.as_deref() == Some("shared")
            || self.env.inherit == Some(true)
            || self.gui.mode == Some(GuiMode::Host)
            || self.gui.gpu == Some(true)
            || !self.env.pass.is_empty()
            || !self.setup.run.is_empty()
            || self.docker.image.is_some()
            || !self.docker.packages.is_empty()
            || !self.devices.r#use.is_empty()
    }
}

/// A project may request devices of the user configuration with
/// `[devices] use = [...]`. It may never define a device (`[devices.NAME]`)
/// or turn one on (`[devices] NAME = true`): the agent can edit the file.
fn check_project_devices(path: &Path, v: &toml::Value) -> Result<()> {
    let Some(t) = v.get("devices").and_then(|d| d.as_table()) else {
        return Ok(());
    };
    if let Some(k) = t.keys().find(|k| *k != "use") {
        let what = if t[k].is_table() {
            format!("[devices.{k}]")
        } else {
            format!("[devices] {k}")
        };
        bail!(
            "{}: {what} is only read from the user configuration (~/.config/paddock/config.toml); a project paddock.toml can only request devices of the user configuration with [devices] use = [\"NAME\"], because an agent in the sandbox can edit the project",
            path.display()
        );
    }
    Ok(())
}

fn approval_file(env: &HostEnv, project_file: &Path) -> PathBuf {
    let key = hex(&Sha256::digest(project_file.to_string_lossy().as_bytes()))[..16].to_string();
    env.paddock_state().join("approved").join(key)
}

fn widening_hash(m: &Manifest, base: &Base) -> String {
    hex(&Sha256::digest(m.widening(base).to_string().as_bytes()))
}

/// Bytes from lowercase or uppercase hex. None for odd lengths or other
/// characters.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// With `--network`, an approval that also covers the mode of the file
/// counts too.
pub fn is_approved(env: &HostEnv, project_file: &Path, m: &Manifest, base: &Base) -> bool {
    let Ok(s) = std::fs::read_to_string(approval_file(env, project_file)) else {
        return false;
    };
    let full = Base {
        cli_network: false,
        ..base.clone()
    };
    s.trim() == widening_hash(m, base) || s.trim() == widening_hash(m, &full)
}

pub fn approve(env: &HostEnv, project_file: &Path, m: &Manifest, base: &Base) -> Result<()> {
    let f = approval_file(env, project_file);
    crate::privfs::write(&f, widening_hash(m, base))?;
    Ok(())
}

/// Make sure that any widening in the project file is approved. Ask on a
/// terminal; refuse without one.
pub fn require_approval(env: &HostEnv, loaded: &Loaded, assume_yes: bool) -> Result<()> {
    let Some((file, m)) = &loaded.project else {
        return Ok(());
    };
    let base = &loaded.base;
    if !m.widens(base) || is_approved(env, file, m, base) {
        return Ok(());
    }
    let summary = serde_json::to_string_pretty(&m.widening(base))?;
    eprintln!(
        "paddock: {} asks for more access than the defaults:\n{summary}\n\
         The project directory is writable from inside the sandbox, so an agent can edit this file.",
        file.display()
    );
    if assume_yes {
        return approve(env, file, m, base);
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "not approved. Review the file, then run: paddock approve {}",
            file.parent().unwrap().display()
        );
    }
    eprint!("Approve this access? [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "y" | "Y" | "yes") {
        approve(env, file, m, base)
    } else {
        bail!("not approved")
    }
}

pub const TEMPLATE: &str = r#"# paddock.toml: sandbox settings for this project.
# Fields that widen access need a one-time approval on the host.

# default_harness = "claude"
# backend = "auto"            # auto | bwrap | docker

[mounts]
# rw = []                     # extra writable host paths
# ro = ["../shared-protos"]   # extra read-only host paths

[secrets]
# hide = ["secrets/"]         # hide more project paths
# reveal = [".env.test"]      # do not hide these

[network]
# mode = "egress"             # none | api | allowlist | egress | host
# publish = [5173]            # ports reachable from the host
# allow = ["crates.io"]       # extra domains for modes "api" and "allowlist"
# lan = true                # reach private networks: routers, NAS, other machines on the LAN
# host_ports = [8765]         # host loopback ports reachable from the sandbox (local MCP servers)

[auth]
# mode = "shared"             # shared: your login | isolated | isolated:NAME: a separate login

[mcp]
# allow = ["my-server"]       # MCP servers whose host files may be mounted read-only

[gui]
# mode = "headless"           # off | headless | window | host
#                             # headless: private display inside the sandbox (default for desktop projects)
#                             # window:   a nested compositor window on your desktop
#                             # host:     your real desktop; the agent can read the screen and type
# gpu = false                 # bind /dev/dri render nodes
# width = 1600
# height = 1000

[env]
# inherit = true              # pass the host environment; false passes only the allowlist
# pass = []                   # host variables to pass through when inherit = false
# set = { RUST_LOG = "info" }

[setup]
# mise_install = true
# shared_build_cache = true   # worktree runs build in a cargo cache outside the worktree
# run = []

[docker]
# image = "debian:stable"
# packages = ["libssl-dev"]

[devices]
# use = ["mac"]               # ask for devices of the user configuration (paddock device ls)
"#;

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
