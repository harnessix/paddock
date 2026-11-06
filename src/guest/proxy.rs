//! The proxy of the guest agent: the listener on `127.0.0.1:3128` in the
//! guest, the system proxy of the guest and the proxy of the app's
//! environment. It speaks HTTP CONNECT and plain proxy requests. For each
//! client it finds the guest process behind the loopback connection, opens
//! one vsock connection to the host's `guest-proxy` port (1031), writes the
//! token line, the header line and the request, and copies bytes both ways.
//! The `200 Connection established` of a CONNECT comes from the host proxy
//! through the copy.
//!
//! The core, [`serve_one`], is pure over two streams and has Linux tests
//! over socket pairs. The listener and the vsock dial are in [`start`].

use crate::macvm::proto::{Identity, ProxyHeader};
use crate::proxy::{connect_target, read_head, respond, splice, Duplex, HostProxy};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The loopback port of the proxy in the guest.
pub const PORT: u16 = crate::proxy::RELAY_PORT;
/// Open tunnels at most. The next client gets `503`.
pub const MAX_TUNNELS: usize = 512;
/// The vsock dial to the host gives up after this: the app sees `502`
/// within a second, not a hang.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(1);
/// A client that sends no complete request head within this time is
/// closed.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// After a failed bind of the port, try again after this.
pub const BIND_RETRY: Duration = Duration::from_secs(5);

/// The session token, set by the main loop at each `token` request.
static TOKEN: Mutex<Option<String>> = Mutex::new(None);

/// Remember the session token for the header lines.
pub fn set_token(token: &str) {
    if let Ok(mut t) = TOKEN.lock() {
        *t = Some(token.to_string());
    }
}

fn token() -> Option<String> {
    TOKEN.lock().ok().and_then(|t| t.clone())
}

/// What one client got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The tunnel ran and ended.
    Tunneled,
    /// The client got this status and nothing else.
    Refused(u16),
}

/// The open tunnels, for the cap.
#[derive(Default)]
pub struct Tunnels(AtomicUsize);

impl Tunnels {
    /// One more tunnel, or `None` at the cap.
    pub fn take(&self) -> Option<Slot<'_>> {
        let n = self.0.fetch_add(1, Ordering::SeqCst);
        if n >= MAX_TUNNELS {
            self.0.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(self))
    }

    pub fn open(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// One counted tunnel.
pub struct Slot<'a>(&'a Tunnels);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The request of a client: a CONNECT, or a plain request in absolute
/// form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Connect,
    Plain,
}

/// Parse the head: the target host and port, and the kind. `None` for a
/// request the proxy does not take.
pub fn parse_request(head: &[u8]) -> Option<(Kind, String, u16)> {
    let mut headers = [httparse::EMPTY_HEADER; 100];
    let mut req = httparse::Request::new(&mut headers);
    if !req.parse(head).ok()?.is_complete() {
        return None;
    }
    let method = req.method?;
    let target = req.path?;
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = connect_target(target)?;
        return Some((Kind::Connect, host, port));
    }
    let url = url::Url::parse(target).ok()?;
    if url.scheme() != "http" || !url.has_host() {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    Some((Kind::Plain, host, url.port_or_known_default().unwrap_or(80)))
}

/// Serve one client: read its request, find the identity of `peer_port`,
/// dial the host, write the token line, the header line and the request,
/// then copy both ways. `dial` opens the vsock connection; a failure
/// answers `502`. `slot` is `None` at the tunnel cap: `503`.
pub fn serve_one<C: Duplex, U: Duplex>(
    mut client: C,
    peer_port: u16,
    slot: Option<Slot<'_>>,
    token: Option<&str>,
    lookup: impl FnOnce(u16) -> Identity,
    dial: impl FnOnce() -> io::Result<U>,
) -> Outcome {
    let Some(_slot) = slot else {
        respond(&mut client, 503, "Service Unavailable");
        return Outcome::Refused(503);
    };
    let _ = client.set_tick(HEAD_TIMEOUT);
    let (head, rest) = match read_head(&mut client) {
        Ok(x) => x,
        Err(_) => {
            respond(&mut client, 400, "Bad Request");
            return Outcome::Refused(400);
        }
    };
    let Some((kind, host, port)) = parse_request(&head) else {
        respond(&mut client, 400, "Bad Request");
        return Outcome::Refused(400);
    };
    let Some(token) = token else {
        log::warn!("proxy: a connection to {host}:{port} before the session token");
        respond(&mut client, 502, "Bad Gateway");
        return Outcome::Refused(502);
    };
    let id = lookup(peer_port);
    let mut up = match dial() {
        Ok(u) => u,
        Err(e) => {
            log::warn!("proxy: cannot reach the host for {host}:{port}: {e}");
            respond(&mut client, 502, "Bad Gateway");
            return Outcome::Refused(502);
        }
    };
    let header = ProxyHeader::new(token, &host, port, id);
    let mut front = format!("{token}\n{}", header.line()).into_bytes();
    match kind {
        Kind::Connect => {
            let target = crate::proxy::authority(&host, port);
            front.extend_from_slice(crate::macvm::text::connect_line(&target).as_bytes());
        }
        Kind::Plain => front.extend_from_slice(&head),
    }
    front.extend_from_slice(&rest);
    if up.write_all(&front).is_err() {
        respond(&mut client, 502, "Bad Gateway");
        return Outcome::Refused(502);
    }
    splice(client, up, HostProxy::default().idle);
    Outcome::Tunneled
}

/// The listener: accept on the loopback port, one thread per client.
/// `lookup` finds the identity of a peer port, `dial` opens the vsock
/// connection. A failed bind is logged and tried again every
/// [`BIND_RETRY`], so a port held by an old agent for a moment does not
/// end the proxy.
pub fn serve(
    lookup: Arc<dyn Fn(u16) -> Identity + Send + Sync>,
    dial: Arc<dyn Fn() -> io::Result<UnixStream> + Send + Sync>,
) -> ! {
    static TUNNELS: OnceLock<Tunnels> = OnceLock::new();
    let tunnels = TUNNELS.get_or_init(Tunnels::default);
    let mut said = false;
    loop {
        let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
            Ok(l) => l,
            Err(e) => {
                if !said {
                    log::error!("proxy: cannot listen on 127.0.0.1:{PORT}: {e}; trying again");
                    said = true;
                }
                std::thread::sleep(BIND_RETRY);
                continue;
            }
        };
        log::info!("proxy listens on 127.0.0.1:{PORT}");
        said = false;
        for conn in listener.incoming() {
            let conn: TcpStream = match conn {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("proxy: accept failed: {e}");
                    continue;
                }
            };
            let peer_port = conn.peer_addr().map(|a| a.port()).unwrap_or(0);
            let (lookup, dial) = (lookup.clone(), dial.clone());
            let slot = tunnels.take();
            if slot.is_none() {
                log::warn!("proxy: {MAX_TUNNELS} tunnels are open, refusing a new one");
            }
            std::thread::spawn(move || {
                let token = token();
                serve_one(
                    conn,
                    peer_port,
                    slot,
                    token.as_deref(),
                    |p| lookup(p),
                    || dial(),
                );
            });
        }
    }
}

/// Start the proxy in a thread, with the vsock dial to the host and the
/// identity lookup of the guest.
#[cfg(target_os = "macos")]
pub fn start() {
    let dial: Arc<dyn Fn() -> io::Result<UnixStream> + Send + Sync> = Arc::new(|| {
        super::vsock::connect(super::HOST_CID, crate::vm::ports::GUEST_PROXY, DIAL_TIMEOUT)
    });
    let lookup: Arc<dyn Fn(u16) -> Identity + Send + Sync> = Arc::new(super::identity::lookup);
    std::thread::Builder::new()
        .name("proxy".into())
        .spawn(move || serve(lookup, dial))
        .expect("a thread for the proxy");
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
