use super::*;

/// At sandbox start: serve the store on a socket in the run folder and
/// record it in the spec, for the backend. A problem is a notice: the
/// sandbox starts without the store.
/// `project` names the project scope: the project folder, or for a paddock
/// worktree the repository it belongs to, so that all its worktrees share it.
pub fn start(
    env: &HostEnv,
    spec: &mut crate::spec::SandboxSpec,
    project: PathBuf,
    pasture: Option<String>,
    approver: Arc<dyn Approver>,
    lineage: Lineage,
    bridge: Option<crate::pasture::bridge::Command>,
) -> Option<Served> {
    let cfg = match KvCfg::load(env) {
        Ok(c) => c,
        Err(e) => {
            spec.notices.push(format!("paddock kv is off: {e:#}"));
            return None;
        }
    };
    if !cfg.enabled {
        return None;
    }
    let wait = crate::broker::HostPolicy::load(env)
        .map(|p| p.wait)
        .unwrap_or(120);
    let id = Identity {
        tree: lineage.tree_root(&spec.id),
        sandbox: spec.id.clone(),
        harness: spec.harness.clone(),
        project,
        pasture,
        parent: lineage.parent,
        chain: lineage.chain,
    };
    watch_timers(env.clone(), id.clone(), kv_root(env), cfg.clone());
    let monitor = crate::monitor::watch(env.clone(), id.clone());
    let svc = Service::new(env.clone(), id, cfg, approver, Duration::from_secs(wait))
        .with_bridge(bridge)
        .with_live_namespaces();
    let sock = match crate::sockpath::named(env, &spec.id, "kv.sock") {
        Ok(p) => p,
        Err(e) => {
            spec.notices.push(format!("paddock kv is off: {e:#}"));
            return None;
        }
    };
    match serve(&sock, svc) {
        Ok(mut s) => {
            s.monitor = Some(monitor);
            spec.kv_socket = Some(sock);
            Some(s)
        }
        Err(e) => {
            spec.notices.push(format!("paddock kv is off: {e:#}"));
            None
        }
    }
}
