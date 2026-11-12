//! The device sandbox on macOS: `paddock device-serve` under Seatbelt
//! (`sandbox-exec`), for a device table with `run = "sandbox"`.
//!
//! The spec of the device sandbox (devices/sandbox/spec.rs) is built and
//! checked as on Linux, so policy.rs refuses the same mounts on both
//! platforms. Seatbelt cannot move a host path to another place, so the
//! supervisor and the program see host paths: the private home of the
//! instance is `HOME`, `TMPDIR` is a folder of the instance, the caller's
//! project is at its own path (`PADDOCK_CALLER_MOUNT` names it), and the
//! secret FIFOs are in the instance folder.
//!
//! The Unix sockets of an instance are not in the instance folder: that
//! path is longer than the 104 bytes a socket address allows on macOS
//! (sockpath.rs). They live in two short socket folders of the device
//! sandbox id: the supervisor creates `dev.sock` in the folder of the id,
//! and the host side of the instance (the filtering ssh agent, the
//! `connect` listeners) is in the folder of the id with `-host`. The
//! sandbox writes the first and only connects to the second, so the
//! program cannot replace the agent socket or a connect socket.
//!
//! The profile is a terminal profile ([`crate::seatbelt::Scope::Terminal`]):
//! the system read-only, the private home and `TMPDIR` read-write, `ro`
//! and `rw` of the table at their own paths, the caller's project
//! read-only, the real home denied unless `home` says else. The instance
//! folder itself is not writable: the program could replace a secret FIFO
//! in it. No IP network, or localhost only when the table has `connect`:
//! the supervisor binds `127.0.0.1:PORT` inside, and the profile allows
//! outbound connections to those ports only.
//!
//! No systemd: the instance is a plain child in its own process group, and
//! the supervisor ends itself when its parent pid changes (supervise.rs).
//! `mem` and `cpus` of the table are not available, and `paddock freeze`
//! does not reach a device instance on macOS.
//!
//! Plain code, tested on every platform. The `sandbox-exec` tests of
//! device_macos_tests.rs run only on macOS.

use super::{real, LINUX_ONLY, PROXY_KEYS, SANDBOX_EXEC};
use crate::backend::{base_env, Launch};
use crate::devices::config::DeviceDef;
use crate::devices::sandbox::creds::{secret_var, CredPlan, AGENT_SOCKET};
use crate::devices::sandbox::spec::{self as sspec, InstanceCtx, SOCKET_NAME};
use crate::env::HostEnv;
use crate::execbox::Home;
use crate::seatbelt::{Access, Net, Policy};
use crate::spec::SandboxSpec;
use anyhow::{Context as _, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The PATH of a device program on macOS: the system, Homebrew and
/// MacPorts, which the terminal profile reads.
pub const PATH: &str =
    "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/local/bin";

/// The line of `paddock doctor` and of the start for a table with `mem` or
/// `cpus`.
pub fn limits_notice(name: &str) -> String {
    format!(
        "[devices.{name}.sandbox] mem and cpus are ignored: memory and cpu limits of devices need systemd. Not on macOS"
    )
}

/// The id of the socket folder of the host side of the device sandbox
/// `id`: the ssh agent socket and the `connect` sockets.
pub fn host_id(id: &str) -> String {
    format!("{id}-host")
}

/// The sockets of an instance on macOS.
#[derive(Debug, Clone)]
pub struct Sockets {
    /// `dev.sock` of the supervisor, in the socket folder of the device
    /// sandbox id (the sandbox writes that folder).
    pub supervisor: PathBuf,
    /// The folder of the host side: `ssh-agent.sock` and `connect/N.sock`
    /// (the sandbox connects to them and writes nothing there).
    pub host: PathBuf,
}

impl Sockets {
    /// The sockets of the instance of `def` for `ctx`. Creates the two
    /// folders (mode 0700). Both paths are short enough for a socket
    /// address, else this is the error that names the path.
    pub fn of(env: &HostEnv, def: &DeviceDef, ctx: &InstanceCtx) -> Result<Self> {
        let id = sspec::sandbox_id(&def.name, &ctx.caller);
        let supervisor = crate::sockpath::named(env, &id, SOCKET_NAME)?;
        let agent = crate::sockpath::named(env, &host_id(&id), AGENT_SOCKET)?;
        let host = agent
            .parent()
            .with_context(|| format!("{} has no folder", agent.display()))?
            .to_path_buf();
        Ok(Self { supervisor, host })
    }

    /// The ssh agent socket of the instance.
    pub fn agent(&self) -> PathBuf {
        self.host.join(AGENT_SOCKET)
    }

    /// The folder of the `connect` sockets (`N.sock`).
    pub fn connect(&self) -> PathBuf {
        self.host.join("connect")
    }
}

/// Remove the two socket folders of the device sandbox `id` at the end of
/// the instance. On Linux they exist only when the thread or bwrap
/// launcher needed them (see [`Sockets::of`]).
pub fn remove_sockets(env: &HostEnv, id: &str) {
    let run = env.paddock_run();
    for i in [id.to_string(), host_id(id)] {
        if !i.is_empty() {
            let _ = std::fs::remove_dir_all(crate::sockpath::socket_dir(&run, env.uid, &i));
        }
    }
}

/// The folders and files of an instance that the sandbox uses on macOS.
pub struct Places {
    /// `HOME` of the supervisor and the program.
    pub home: PathBuf,
    /// `TMPDIR`: the work folders of exec requests.
    pub tmp: PathBuf,
    /// The secret FIFOs of `deliver` credentials.
    pub secret: PathBuf,
    /// The rendered profile.
    pub profile: PathBuf,
    /// The filtering ssh agent of the instance.
    pub agent: PathBuf,
    /// The host side of the `connect` entries (`N.sock`).
    pub connect: PathBuf,
}

impl Places {
    pub fn of(ctx: &InstanceCtx, sockets: &Sockets) -> Self {
        Self {
            home: ctx.dir.join("home"),
            tmp: ctx.dir.join("tmp"),
            secret: ctx.dir.join("secret"),
            profile: ctx.dir.join("seatbelt.sb"),
            agent: sockets.agent(),
            connect: sockets.connect(),
        }
    }
}

/// The Seatbelt policy of the device sandbox of `def` for `ctx`. `plan`
/// says whether the instance has an ssh agent and secret files; `sockets`
/// are the sockets of the instance on the host; `paddock` the binary.
pub fn policy(
    env: &HostEnv,
    def: &DeviceDef,
    ctx: &InstanceCtx,
    plan: &CredPlan,
    sockets: &Sockets,
    paddock: &Path,
) -> Result<Policy> {
    let profile = sspec::resolved_profile(env, def)?;
    let at = format!("[devices.{}.sandbox]", def.name);
    let places = Places::of(ctx, sockets);
    let ports: Vec<u16> = profile.connect.iter().map(|c| c.local_port).collect();
    let net = if ports.is_empty() {
        Net::None
    } else {
        Net::Localhost
    };
    let mut p = Policy::terminal(net, ports);
    p.add(
        real(&places.home),
        Access::ReadWrite,
        "the private home of the instance (HOME)",
    );
    p.add(
        real(&places.tmp),
        Access::ReadWrite,
        "the private temporary folder (TMPDIR): the work folder of each request",
    );
    let sock_dir = sockets
        .supervisor
        .parent()
        .with_context(|| format!("{} has no folder", sockets.supervisor.display()))?;
    p.add(
        real(sock_dir),
        Access::ReadWrite,
        "the socket folder of the instance: the supervisor creates dev.sock",
    );
    p.add_file(
        real(paddock),
        Access::ReadOnly,
        "paddock device-serve (paddock)",
    );
    for path in &profile.ro {
        let why = format!("{at} ro");
        if path.is_file() {
            p.add_file(real(path), Access::ReadOnly, &why);
        } else {
            p.add(real(path), Access::ReadOnly, &why);
        }
    }
    for path in &profile.rw {
        p.add(real(path), Access::ReadWrite, &format!("{at} rw"));
    }
    if !profile.caller.is_empty() {
        p.add(
            real(&ctx.project),
            Access::ReadOnly,
            &format!("{at} caller project:ro (PADDOCK_CALLER_MOUNT)"),
        );
    }
    match profile.home {
        Home::Hidden => p.add(
            real(&env.home),
            Access::Deny,
            "the real home is denied, not absent: names stay visible",
        ),
        Home::ReadOnly => p.add(real(&env.home), Access::ReadOnly, &format!("{at} home")),
        Home::Full => p.add(real(&env.home), Access::ReadWrite, &format!("{at} home")),
    }
    if !profile.connect.is_empty() {
        p.add(
            real(&places.connect),
            Access::Connect,
            "connect: the host side of each entry (the supervisor relays 127.0.0.1:PORT to it)",
        );
    }
    if !plan.agents.is_empty() {
        p.add_file(
            real(&places.agent),
            Access::Connect,
            "the filtering ssh agent of the instance (SSH_AUTH_SOCK)",
        );
    }
    if !plan.files.is_empty() {
        p.add(
            real(&places.secret),
            Access::ReadOnly,
            "the secret FIFOs of the credentials (PADDOCK_SECRET_NAME)",
        );
    }
    Ok(p)
}

/// The environment of the supervisor: the base set of every backend with
/// the Linux variables and the proxy variables removed, the private home
/// and temporary folder, the host paths of the secret files and of the
/// ssh agent socket.
pub fn run_env(
    env: &HostEnv,
    spec: &SandboxSpec,
    ctx: &InstanceCtx,
    plan: &CredPlan,
    sockets: &Sockets,
) -> BTreeMap<String, String> {
    let places = Places::of(ctx, sockets);
    let mut vars = base_env(env, spec);
    for k in LINUX_ONLY.iter().chain(PROXY_KEYS) {
        vars.remove(*k);
    }
    for k in ["NO_PROXY", "no_proxy"] {
        vars.remove(k);
    }
    let home = places.home.display().to_string();
    vars.insert("HOME".into(), home.clone());
    vars.insert("CFFIXED_USER_HOME".into(), home);
    vars.insert("TMPDIR".into(), places.tmp.display().to_string());
    vars.insert("PATH".into(), PATH.into());
    for (name, _) in &plan.files {
        vars.insert(
            secret_var(name),
            places.secret.join(name).display().to_string(),
        );
    }
    if !plan.agents.is_empty() {
        vars.insert("SSH_AUTH_SOCK".into(), places.agent.display().to_string());
    }
    vars
}

/// The launch of the instance: `sandbox-exec -f PROFILE paddock
/// device-serve ...` with host paths, and the profile to write first.
pub fn launch(
    env: &HostEnv,
    def: &DeviceDef,
    ctx: &InstanceCtx,
    plan: &CredPlan,
    spec: &SandboxSpec,
    sockets: &Sockets,
    paddock: &Path,
) -> Result<Launch> {
    let profile = sspec::resolved_profile(env, def)?;
    let places = Places::of(ctx, sockets);
    let pol = policy(env, def, ctx, plan, sockets, paddock)?;
    let connect_dir = places.connect.clone();
    let connect_sock = move |n: usize| connect_dir.join(format!("{n}.sock")).display().to_string();
    let mut args = vec!["-f".to_string(), places.profile.display().to_string()];
    args.extend(sspec::supervisor_command_at(
        def,
        &profile,
        plan,
        &paddock.display().to_string(),
        &sockets.supervisor.display().to_string(),
        &connect_sock,
    ));
    Ok(Launch {
        program: PathBuf::from(SANDBOX_EXEC),
        args,
        env: run_env(env, spec, ctx, plan, sockets),
        files: vec![(places.profile, crate::seatbelt::render(&pol)?, 0o600)],
        cwd: None,
    })
}

#[cfg(test)]
#[path = "device_tests.rs"]
mod tests;

/// Device sandboxes with `sandbox-exec`. They run only on macOS.
#[cfg(all(test, target_os = "macos"))]
#[path = "device_macos_tests.rs"]
mod macos_tests;
