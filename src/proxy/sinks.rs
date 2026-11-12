//! The sinks and the gate of the proxy: the egress monitor, the boundary
//! log, and the firewall gate of the macOS guest route, each a closure
//! the route installs.

use super::{authority, HostProxy, Identity};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

/// The sink of the egress monitor: the target host of an open connection,
/// and the guest process behind it on the macOS guest route (`None`
/// elsewhere).
#[derive(Clone)]
pub struct Egress(pub EgressFn);

pub type EgressFn = Arc<dyn Fn(&str, Option<&Identity>) + Send + Sync>;

impl std::fmt::Debug for Egress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Egress")
    }
}

/// The sink of the boundary log: target, rule and the guest process on
/// the macOS guest route (`None` elsewhere), never content.
#[derive(Clone)]
pub struct Boundary(pub BoundaryFn);

pub type BoundaryFn = Arc<dyn Fn(&str, &str, Option<&Identity>) + Send + Sync>;

impl std::fmt::Debug for Boundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Boundary")
    }
}

/// The firewall gate of the macOS guest route: asked for each connection
/// after the host list of the mode, before the upstream connect. It gets
/// the host, the port and the guest process, and answers with what to
/// call at the close, or with the rule word of the refusal. It may hold
/// the connection while it asks the user.
#[derive(Clone)]
pub struct Gate(pub GateFn);

pub type GateFn = Arc<dyn Fn(&str, u16, Option<&Identity>) -> Result<Opened, String> + Send + Sync>;

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Gate")
    }
}

/// An allowed connection: what the gate wants to know at its close, the
/// bytes up (from the app) and down (to the app), and what it does with
/// the client stream while the connection lives: the firewall of the
/// macOS guest keeps a clone, so a switch to network mode `none` can shut
/// the open connections down.
pub struct Opened {
    close: Box<dyn FnOnce(u64, u64) + Send>,
    hold: Option<HoldFn>,
}

pub type HoldFn = Box<dyn FnOnce(&UnixStream) -> Result<(), String> + Send>;

impl Opened {
    pub fn new(close: impl FnOnce(u64, u64) + Send + 'static) -> Self {
        Self {
            close: Box::new(close),
            hold: None,
        }
    }

    /// Give the client stream to `hold` once the gate allowed it.
    pub fn holding(
        mut self,
        hold: impl FnOnce(&UnixStream) -> Result<(), String> + Send + 'static,
    ) -> Self {
        self.hold = Some(Box::new(hold));
        self
    }

    /// Nothing to do at the close.
    pub fn silent() -> Self {
        Self::new(|_, _| {})
    }

    pub fn close(self, up: u64, down: u64) {
        (self.close)(up, down)
    }

    pub fn hold(&mut self, client: &UnixStream) -> Result<(), String> {
        if let Some(h) = self.hold.take() {
            h(client)
        } else {
            Ok(())
        }
    }
}

impl HostProxy {
    /// Note a refused target in the boundary log.
    pub(super) fn refused(&self, host: &str, port: u16, rule: &str, id: Option<&Identity>) {
        if let Some(b) = &self.boundary {
            (b.0)(&authority(host, port), rule, id);
        }
    }

    /// Note a host that the proxy connected.
    pub(super) fn reached(&self, host: &str, id: Option<&Identity>) {
        if let Some(e) = &self.egress {
            (e.0)(host, id);
        }
    }

    /// Ask the gate about a connection. Without a gate every connection
    /// passes. A refusal is the rule word, for the response and the log.
    pub(super) fn gated(
        &self,
        host: &str,
        port: u16,
        id: Option<&Identity>,
        client: &UnixStream,
    ) -> Result<Option<Opened>, String> {
        match &self.gate {
            None => Ok(None),
            Some(g) => {
                let mut o = (g.0)(host, port, id)?;
                o.hold(client)?;
                Ok(Some(o))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_refusal_stops_before_the_upstream_connection() {
        let cfg = HostProxy {
            gate: Some(Gate(Arc::new(|_, _, _| {
                Ok(Opened::silent().holding(|_| Err("stale".into())))
            }))),
            ..Default::default()
        };
        let (client, _peer) = UnixStream::pair().unwrap();
        assert!(matches!(
            cfg.gated("example.test", 443, None, &client),
            Err(ref e) if e == "stale"
        ));
    }
}
