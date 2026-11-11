//! The `credentials` of a device profile, in the device sandbox.
//!
//! Each name is a `[credentials.NAME]` table of the user configuration. Its
//! `via` decides what the instance gets: an `agent` credential of kind
//! `ssh` gives a filtering ssh agent of its own (credentials/sshagent.rs)
//! at `SSH_AUTH_SOCK`, and a `deliver` credential with `as = "file"` gives
//! the value as a FIFO (credentials/secret/fifo.rs) at
//! `/run/paddock/device/secret/NAME`, named by `PADDOCK_SECRET_NAME`. The
//! other ways are refused with a message that names the fix: a device
//! sandbox has no gateway, and a secret never becomes a variable there.
//!
//! No card: the user named the credential in the device table on the host.

use crate::credentials::{self, Credential, DeliverAs, Kind, Via};
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Where the secret FIFOs appear in the device sandbox.
pub const SECRET_DIR_IN: &str = "/run/paddock/device/secret";
/// The name of the ssh agent socket of an instance.
pub const AGENT_SOCKET: &str = "ssh-agent.sock";

/// What the profile asks for, checked against the user configuration.
#[derive(Debug, Clone, Default)]
pub struct CredPlan {
    /// ssh agent credentials.
    pub agents: Vec<(String, Credential)>,
    /// `deliver` credentials with `as = "file"`.
    pub files: Vec<(String, Credential)>,
}

/// `PADDOCK_SECRET_NAME`: upper case, `-` as `_`.
pub fn secret_var(name: &str) -> String {
    let tail: String = name
        .chars()
        .map(|c| {
            if c == '-' {
                '_'
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    format!("PADDOCK_SECRET_{tail}")
}

/// The path of the secret file `name` in the device sandbox.
pub fn secret_path_in(name: &str) -> String {
    format!("{SECRET_DIR_IN}/{name}")
}

/// Check each name of `credentials` against the user configuration.
pub fn plan(env: &HostEnv, device: &str, names: &[String]) -> Result<CredPlan> {
    let mut p = CredPlan::default();
    if names.is_empty() {
        return Ok(p);
    }
    let at = format!("[devices.{device}.sandbox] credentials");
    let known = credentials::load(env)?;
    for name in names {
        let Some(c) = known.get(name) else {
            bail!(
                "{at}: {name} is not a [credentials.{name}] table of {}",
                env.paddock_config().join("config.toml").display()
            );
        };
        match c.via {
            Via::Agent => match c.kind() {
                Kind::Ssh => {
                    if c.keys.is_empty() {
                        bail!("{at}: {name} is an ssh agent credential without keys; list the public key files in keys = [...]");
                    }
                    p.agents.push((name.clone(), c.clone()));
                }
                other => bail!(
                    "{at}: {name} is an agent credential of kind \"{}\": a device sandbox gets an ssh agent only. The docker and aws channels need the host passthrough of an agent sandbox",
                    other.name()
                ),
            },
            Via::Deliver => match c.as_ {
                DeliverAs::File => p.files.push((name.clone(), c.clone())),
                DeliverAs::Env => bail!(
                    "{at}: {name}: a device gets a secret as a file, not as a variable: as = \"file\""
                ),
            },
            Via::Gateway => bail!(
                "{at}: {name} uses via = \"gateway\": a device sandbox has no gateway. Use via = \"deliver\" with as = \"file\", and read the value from $PADDOCK_SECRET_{}",
                secret_var(name).trim_start_matches("PADDOCK_SECRET_")
            ),
            Via::Intercept => bail!(
                "{at}: {name} uses via = \"intercept\": a device sandbox has no gateway. Use via = \"deliver\" with as = \"file\""
            ),
        }
    }
    Ok(p)
}

/// The agents and files of one instance. Dropping it stops them.
pub struct Creds {
    agent_stop: Option<Arc<AtomicBool>>,
    /// The ssh agent socket on the host, when the plan has agents.
    pub agent_socket: Option<PathBuf>,
    fifos: Option<credentials::secret::fifo::SecretFifos>,
}

impl Drop for Creds {
    fn drop(&mut self) {
        if let Some(s) = &self.agent_stop {
            s.store(true, Ordering::SeqCst);
        }
        self.fifos.take();
        if let Some(p) = &self.agent_socket {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Start the agents and the FIFOs of `plan`: the FIFOs in the instance
/// folder `dir`, the ssh agent socket in `agent_dir` (the instance folder
/// on Linux; on macOS the short socket folder of the instance, see
/// backend/seatbelt/device.rs). `spec` is the spec of the device sandbox,
/// for the cards and the audit.
pub fn start(
    env: &HostEnv,
    plan: &CredPlan,
    dir: &Path,
    agent_dir: &Path,
    caller: &str,
    spec: &crate::spec::SandboxSpec,
) -> Result<Creds> {
    let mut out = Creds {
        agent_stop: None,
        agent_socket: None,
        fifos: None,
    };
    if !plan.agents.is_empty() {
        use credentials::sshagent;
        let upstream = env
            .var("SSH_AUTH_SOCK")
            .map(PathBuf::from)
            .context("SSH_AUTH_SOCK is not set where paddock started, so there is no ssh agent for the device")?;
        let keys = sshagent::load_keys(env, &plan.agents)?;
        sshagent::check_host(Some(&upstream), &keys)?;
        let sock = agent_dir.join(AGENT_SOCKET);
        crate::sockpath::check(&sock)?;
        let stop = Arc::new(AtomicBool::new(false));
        let approver: Arc<dyn crate::broker::Approver> = Arc::new(crate::broker::HostApprover);
        let (e1, s1) = (env.clone(), spec.clone());
        sshagent::Proxy::new(
            keys,
            upstream,
            credentials::revoked_file(env, caller),
            sshagent::approver_ask(
                env.clone(),
                spec.clone(),
                approver,
                Duration::from_secs(crate::devices::user::APPROVAL_WAIT),
            ),
            Arc::new(move |k: &sshagent::Key, result: &str| {
                sshagent::audit(&e1, &s1, k, result);
            }),
        )
        .serve_until(&sock, stop.clone())?;
        for (name, _) in &plan.agents {
            credentials::audit(env, spec, name, "ssh agent ready (device)");
        }
        out.agent_stop = Some(stop);
        out.agent_socket = Some(sock);
    }
    if !plan.files.is_empty() {
        let read_plan = credentials::Plan {
            deliver: plan.files.clone(),
            notices: Vec::new(),
        };
        let (values, notices) = credentials::read(env, &read_plan);
        if let Some(n) = notices.first() {
            bail!("{n}");
        }
        let (e2, s2) = (env.clone(), spec.clone());
        let secrets = credentials::secret::SecretFiles::new(
            Some(credentials::revoked_file(env, caller)),
            Some(Arc::new(move |name: &str| {
                credentials::audit_read(&e2, &s2, name)
            })),
        );
        for (name, _, value) in values {
            secrets.insert(&name, value);
        }
        secrets.watch();
        out.fifos = Some(credentials::secret::fifo::start(
            &dir.join("secret"),
            secrets,
            &spec.id,
        )?);
    }
    Ok(out)
}
