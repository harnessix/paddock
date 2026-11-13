use super::*;

/// Network mode `api` on bwrap, docker and podman: the sandbox has no
/// network. The paddock proxy (proxy.rs) on a Unix socket lets through TLS to
/// the allowed hosts, and `paddock relay` offers it on 127.0.0.1:3131 in the
/// sandbox. srt enforces the list with its own proxy, and seatbelt starts
/// its proxy with `backend::start_host_proxy`.
pub(super) fn start_api_proxy(env: &HostEnv, built: &mut Built, backend: &str) -> Result<()> {
    if built.spec.network != Network::Api || matches!(backend, "srt" | "seatbelt" | "vm") {
        return Ok(());
    }
    use crate::proxy::{allow, API_PORT, API_SOCKET_IN_SANDBOX};
    let run_dir = env.paddock_run().join(&built.spec.id);
    crate::privfs::dir(&run_dir)?;
    let sock = crate::sockpath::named(env, &built.spec.id, "api-proxy.sock")?;
    let (list, _) = allow::Allowlist::parse(&built.spec.allow_domains);
    let refusals = backend::api_refusals(env, &built.spec);
    let mut cfg = backend::host_proxy_cfg(env, &built.spec);
    // The api proxy reaches no host loopback port.
    cfg.allow_loopback = Vec::new();
    cfg.api = Some(allow::ApiPolicy::api(list, Some(refusals)));
    let sock = crate::vm::agent::api_proxy(sock, cfg)?;
    built.spec.add_mount(crate::spec::Mount::bind(
        &sock,
        API_SOCKET_IN_SANDBOX,
        false,
        "network: api proxy",
    ));
    let me = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("cannot find the paddock executable")?;
    built
        .spec
        .add_tool("paddock", me, "network: api proxy relay");
    built.spec.setup.insert(
        0,
        format!(
            "paddock relay --listen 127.0.0.1:{API_PORT} --socket {API_SOCKET_IN_SANDBOX} --ready-file /tmp/.paddock-api >/dev/null 2>&1 &\ni=0; while [ ! -e /tmp/.paddock-api ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i+1)); done"
        ),
    );
    Ok(())
}
