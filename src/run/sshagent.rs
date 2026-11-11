//! The filtering ssh agent of a run (credentials/sshagent.rs), started
//! before the sandbox.

use super::Built;
use crate::env::HostEnv;
use anyhow::{Context as _, Result};
use std::time::Duration;

/// The filtering ssh agent of the run (credentials/sshagent.rs), alive as
/// long as this process. SSH_AUTH_SOCK in the sandbox points at it.
pub(super) fn start(
    env: &HostEnv,
    built: &mut Built,
    approver: &std::sync::Arc<dyn crate::broker::Approver>,
    wait: Duration,
    agents: &[(String, crate::credentials::Credential)],
) -> Result<()> {
    use crate::credentials::sshagent;
    let upstream = env
        .var("SSH_AUTH_SOCK")
        .map(std::path::PathBuf::from)
        .context("SSH_AUTH_SOCK is not set where paddock started")?;
    let keys = sshagent::load_keys(env, agents)?;
    sshagent::check_host(Some(&upstream), &keys)?;
    let run_dir = env.paddock_run().join(&built.spec.id);
    crate::privfs::dir(&run_dir)?;
    let sock = crate::sockpath::named(env, &built.spec.id, "ssh-agent.sock")?;
    let (e1, spec) = (env.clone(), built.spec.clone());
    sshagent::Proxy::new(
        keys,
        upstream,
        crate::credentials::revoked_file(env, &built.spec.id),
        sshagent::approver_ask(env.clone(), built.spec.clone(), approver.clone(), wait),
        std::sync::Arc::new(move |k: &sshagent::Key, result: &str| {
            sshagent::audit(&e1, &spec, k, result);
        }),
    )
    .serve(&sock)?;
    for (name, _) in agents {
        crate::credentials::audit(env, &built.spec, name, "ssh agent ready");
    }
    built.spec.ssh_agent_socket = Some(sock);
    Ok(())
}
