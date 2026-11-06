//! The spec of a device sandbox: a paddock sandbox without a harness.
//!
//! The baseline is the `[exec]` view of the command device of 0.2.1: the
//! system read-only, an empty home at the host home path (so `~` in the
//! program's own files resolves), its own `/tmp`, the runtime folders of
//! the user hidden, no network. The profile `[devices.NAME.sandbox]` widens
//! it (profile.rs). The command is the supervisor `paddock device-serve`,
//! the paddock binary itself bound read-only at `/run/paddock/bin/paddock`,
//! with the flags of the table. The instance folder of the host is bound
//! read-write at `/run/paddock/device`, where the supervisor creates
//! `dev.sock`. policy.rs runs on the spec as for any sandbox.

use super::connect::{socket_in, CONNECT_DIR_IN};
use super::creds::{secret_path_in, secret_var, CredPlan};
use crate::devices::config::{DeviceDef, Net, Run, CALLER_PROJECT_MOUNT};
use crate::devices::profile::SandboxProfile;
use crate::env::HostEnv;
use crate::execbox::{ExecCfg, Home};
use crate::spec::{Mount, Network, SandboxSpec, TOOL_DIR};
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};

/// The instance folder in the device sandbox.
pub const DEVICE_DIR_IN: &str = "/run/paddock/device";
/// The socket of the supervisor in the device sandbox.
pub const SOCKET_IN: &str = "/run/paddock/device/dev.sock";
/// The name of the socket in the instance folder.
pub const SOCKET_NAME: &str = "dev.sock";
/// The harness name of a device sandbox.
pub const HARNESS: &str = "device";
/// The origin of the mounts that paddock adds for the instance itself.
/// policy.rs lets them pass under the run folder.
pub const INSTANCE_ORIGIN: &str = "device instance";
/// The default size of the `/tmp` of a device sandbox.
pub const DEFAULT_TMP: u64 = 1 << 30;

/// The caller of an instance and where the instance lives.
#[derive(Debug, Clone)]
pub struct InstanceCtx {
    /// The sandbox id of the caller.
    pub caller: String,
    pub project: PathBuf,
    pub harness: String,
    /// The sandboxes above the caller, root first.
    pub chain: Vec<String>,
    /// The instance folder on the host.
    pub dir: PathBuf,
}

/// Point the host side of the ssh agent socket and of the `connect`
/// folder of `spec` from the instance folder `dir` to the short socket
/// folders `agent_dir` and `connect_dir`, for an instance folder too deep
/// for a socket address. The paths inside the sandbox stay the same.
pub fn move_host_sockets(spec: &mut SandboxSpec, dir: &Path, agent_dir: &Path, connect_dir: &Path) {
    let old_connect = dir.join("connect");
    for m in &mut spec.mounts {
        if m.src.as_deref() == Some(old_connect.as_path()) {
            m.src = Some(connect_dir.to_path_buf());
        }
    }
    if let Some(sock) = &mut spec.ssh_agent_socket {
        if let Ok(name) = sock.strip_prefix(dir) {
            *sock = agent_dir.join(name);
        }
    }
}

/// The id of the device sandbox: `dev-<name>-<caller short id>`.
pub fn sandbox_id(name: &str, caller: &str) -> String {
    format!("dev-{name}-{}", crate::ids::short(caller))
}

/// The profile of the table, with the `[exec]` keys as the base for a table
/// of 0.2.1, and the view of its `rw` and `home` checked.
pub fn resolved_profile(env: &HostEnv, def: &DeviceDef) -> Result<SandboxProfile> {
    let exec = ExecCfg::load(env)?;
    let p = def.profile().with_exec_base(&exec);
    let at = format!("[devices.{}.sandbox]", def.name);
    for (k, list) in [("rw", &p.rw), ("ro", &p.ro)] {
        for path in list {
            if path == Path::new("/") || *path == env.home {
                bail!(
                    "{at}: {k} = {:?} would open the whole system or home; use home = \"full\"",
                    path.display()
                );
            }
        }
    }
    Ok(p)
}

/// The names of the variables that the supervisor passes to the program on
/// top of its base set: the profile's `env` and `env_pass`, the ssh agent
/// socket and the secret file paths.
pub fn program_env_names(profile: &SandboxProfile, plan: &CredPlan) -> Vec<String> {
    let mut v: Vec<String> = profile.env.keys().cloned().collect();
    for n in &profile.env_pass {
        if !v.contains(n) {
            v.push(n.clone());
        }
    }
    if !plan.agents.is_empty() {
        v.push("SSH_AUTH_SOCK".into());
    }
    for (name, _) in &plan.files {
        v.push(secret_var(name));
    }
    v
}

/// The command line of the supervisor inside the bwrap device sandbox:
/// the paths of the mounts.
pub fn supervisor_command(
    def: &DeviceDef,
    profile: &SandboxProfile,
    plan: &CredPlan,
) -> Vec<String> {
    supervisor_command_at(
        def,
        profile,
        plan,
        &format!("{TOOL_DIR}/paddock"),
        SOCKET_IN,
        &socket_in,
    )
}

/// The command line of the supervisor with the paths it sees: `paddock`
/// the binary, `socket` where it listens, `connect_sock` the host side of
/// `connect` entry `n`. Seatbelt (backend/seatbelt/device.rs) gives host
/// paths, bwrap the paths of its mounts.
pub fn supervisor_command_at(
    def: &DeviceDef,
    profile: &SandboxProfile,
    plan: &CredPlan,
    paddock: &str,
    socket: &str,
    connect_sock: &dyn Fn(usize) -> String,
) -> Vec<String> {
    let mut v: Vec<String> = vec![
        paddock.to_string(),
        "device-serve".into(),
        "--socket".into(),
        socket.to_string(),
        "--shape".into(),
        def.shape.name().into(),
        "--timeout".into(),
        def.limits.timeout.to_string(),
        "--max-request".into(),
        def.limits.max_request.to_string(),
        "--max-response".into(),
        def.limits.max_response.to_string(),
        "--concurrency".into(),
        def.limits.concurrency.to_string(),
        "--ready".into(),
        def.ready.to_string(),
    ];
    if def.stream {
        v.push("--stream".into());
    }
    for (n, c) in profile.connect.iter().enumerate() {
        v.push("--connect".into());
        v.push(format!("{}:{}", c.local_port, connect_sock(n)));
    }
    for name in program_env_names(profile, plan) {
        v.push("--env-pass".into());
        v.push(name);
    }
    v.push("--".into());
    v.extend(def.command.iter().cloned());
    v
}

/// The spec of the device sandbox of `def` for the caller `ctx`. `plan` is
/// the credential plan of the profile (creds.rs); `paddock` the binary.
pub fn spec(
    env: &HostEnv,
    def: &DeviceDef,
    ctx: &InstanceCtx,
    plan: &CredPlan,
    paddock: &Path,
) -> Result<SandboxSpec> {
    if def.run != Run::Sandbox {
        bail!(
            "[devices.{}]: run = \"{}\" has no device sandbox",
            def.name,
            def.run.name()
        );
    }
    let profile = resolved_profile(env, def)?;
    let at = format!("[devices.{}.sandbox]", def.name);
    let home_src = ctx.dir.join("home");
    let empty = ctx.dir.join("empty");
    let mut s = SandboxSpec::new(
        sandbox_id(&def.name, &ctx.caller),
        HARNESS,
        &empty,
        home_src,
        &env.home,
        env.uid,
        env.gid,
        &env.user,
    );
    // The supervisor starts in the (empty) home; each exec request gets a
    // folder under /tmp.
    s.workdir = s.home.clone();
    s.network = match profile.network {
        Net::None => Network::None,
        Net::Egress => Network::Egress,
        Net::Api => Network::Api,
    };
    if profile.network == Net::Api {
        for d in &profile.allow {
            s.allow_domains.push(d.clone());
            s.allow_origins.insert(d.clone(), format!("{at} allow"));
        }
    }
    // The baseline of `[exec]`: the runtime folders of the user hidden.
    for d in ExecCfg::default().hide {
        s.add_mount(Mount::mask(
            d,
            true,
            "device sandbox: hidden runtime folder",
        ));
    }
    s.add_mount(Mount::tmpfs_sized(
        "/tmp",
        profile.tmp.unwrap_or(DEFAULT_TMP),
        &format!("{at} tmp"),
    ));
    for p in &profile.ro {
        s.add_mount(Mount::same(p, false, &format!("{at} ro")));
    }
    for p in &profile.rw {
        s.add_mount(Mount::same(p, true, &format!("{at} rw")));
    }
    match profile.home {
        Home::Hidden => {}
        Home::ReadOnly => {
            s.add_mount(Mount::bind(
                &env.home,
                &s.home,
                false,
                &format!("{at} home"),
            ));
        }
        Home::Full => {
            s.add_mount(Mount::bind(&env.home, &s.home, true, &format!("{at} home")));
        }
    }
    if !profile.caller.is_empty() {
        s.add_mount(Mount::bind(
            &ctx.project,
            CALLER_PROJECT_MOUNT,
            false,
            &format!("{at} caller project:ro"),
        ));
    }
    s.add_mount(Mount::bind(&ctx.dir, DEVICE_DIR_IN, true, INSTANCE_ORIGIN));
    if !profile.connect.is_empty() {
        s.add_mount(Mount::bind(
            ctx.dir.join("connect"),
            CONNECT_DIR_IN,
            false,
            INSTANCE_ORIGIN,
        ));
    }
    s.add_tool(
        "paddock",
        paddock.to_path_buf(),
        "device sandbox: the supervisor",
    );
    // The environment: the base set of the backend, the profile, the
    // device constants. Never a variable of the caller.
    for name in &profile.env_pass {
        if crate::policy::HOST_ONLY_ENV.contains(&name.as_str()) {
            s.warnings.push(format!(
                "{at} env_pass: {name} is set by the sandbox itself and is not passed"
            ));
            continue;
        }
        if let Some(v) = env.var(name) {
            s.set_env(name, v, &format!("profile {at} env_pass"));
        }
    }
    for (k, v) in &profile.env {
        s.set_env(k, v.clone(), &format!("profile {at} env"));
    }
    s.set_env("TERM", "dumb", "device sandbox");
    s.set_env("PADDOCK_DEVICE", def.name.clone(), "device sandbox");
    s.set_env("PADDOCK_DEVICE_PROTOCOL", "1", "device sandbox");
    for (name, _) in &plan.files {
        s.set_env(
            &secret_var(name),
            secret_path_in(name),
            "credentials: secret file",
        );
    }
    if !plan.agents.is_empty() {
        s.ssh_agent_socket = Some(ctx.dir.join(super::creds::AGENT_SOCKET));
    }
    s.command = supervisor_command(def, &profile, plan);
    crate::policy::enforce(env, &s).with_context(|| format!("[devices.{}]", def.name))?;
    Ok(s)
}
