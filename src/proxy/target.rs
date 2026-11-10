//! DNS lookup for a proxy target.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

/// Resolve `host`. A name gets a trailing dot so a DNS search domain cannot
/// turn it into a different host. An address is used as written.
pub(super) fn resolve_target(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    if host.parse::<IpAddr>().is_ok() || host.ends_with('.') {
        return Ok((host, port).to_socket_addrs()?.collect());
    }
    let fqdn = format!("{host}.");
    Ok((fqdn.as_str(), port).to_socket_addrs()?.collect())
}

/// The host and port of a CONNECT target `host:port`. The port must be
/// written out; `Url::port` alone hides port 80 as the default of http.
pub(super) fn connect_target(target: &str) -> Option<(String, u16)> {
    let (_, port) = target.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    let parsed = url::Url::parse(&format!("http://{target}")).ok()?;
    Some((super::host_of(&parsed)?, parsed.port_or_known_default()?))
}

#[cfg(not(test))]
pub(super) fn registry_loopback_allowed(cfg: &super::HostProxy, host: &str, port: u16) -> bool {
    cfg.registry_http.is_none()
        || cfg!(feature = "exclave-fake-registry-test")
            && host == "localhost"
            && port == 38473
            && cfg
                .registry_http
                .as_ref()
                .is_some_and(|p| p.check(host, port).is_ok())
}

pub(super) fn check_resolved(
    cfg: &super::HostProxy,
    host: &str,
    port: u16,
    resolved: &[std::net::SocketAddr],
    local: &[IpAddr],
) -> anyhow::Result<()> {
    for a in resolved {
        let ip = super::canonical(a.ip());
        if ip.is_loopback() {
            let allowed = cfg.allow_loopback.contains(&port);
            #[cfg(not(test))]
            let allowed = allowed && registry_loopback_allowed(cfg, host, port);
            if !allowed {
                cfg.refused(host, port, "host loopback port not in [network] host_ports");
                anyhow::bail!("refused: {host}:{port} is a host loopback port that is not allowed");
            }
        } else if ip.is_unspecified() || local.contains(&ip) {
            cfg.refused(host, port, "an address of this host");
            anyhow::bail!("refused: {host}:{port} is an address of this host");
        } else if (!cfg.allow_lan || cfg.registry_http.is_some())
            && super::is_lan(&ip)
            && !cfg
                .registry_private
                .as_ref()
                .is_some_and(|p| p.check(host, port).is_ok())
        {
            cfg.refused(host, port, "private network ([network] lan = false)");
            anyhow::bail!("refused: {host}:{port} is on a private network; set [network] lan = true to allow it");
        }
    }
    Ok(())
}
