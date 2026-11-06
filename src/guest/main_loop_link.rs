//! The links and the login callbacks in the main loop (`link.rs`): the
//! writer of the open connection for the link socket, and the
//! `login.forward` request, which a thread of its own answers.

use super::{Session, Writer};
use crate::guest::link;
use crate::guest::platform::Platform;
use crate::macvm::proto::{ErrorCode, Event, Reply};
use crate::macvm::text;
use std::sync::Mutex;

/// The writer of the open control connection, for the link socket.
static LIVE: Mutex<Option<Writer>> = Mutex::new(None);

/// A connection opened (`Some`) or ended (`None`).
pub(super) fn set_live(w: Option<Writer>) {
    *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = w;
}

/// Send `link.open` for a link of the helper on the open connection.
pub fn send_link(url: &str) -> Result<(), String> {
    link::check_url(url)?;
    let w = LIVE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match w {
        Some(w) => w.send(&Event::LinkOpen {
            url: url.to_string(),
        }),
        None => Err(text::link_no_host_connection()),
    }
}

impl<P: Platform> Session<'_, P> {
    /// `login.forward`: connect to the guest port and dial the host in a
    /// thread, which replies, then copies the bytes. None: the thread
    /// replies.
    pub(super) fn login_forward(&mut self, req: u64, port: u16, ticket: String) -> Option<Reply> {
        let token = self.st.token.clone();
        if port == 0 || !link::valid_ticket(&ticket) {
            return Some(Reply::error(
                req,
                ErrorCode::BadRequest,
                text::login_forward_bad(),
            ));
        }
        let writer = self.writer.clone();
        let spawned = std::thread::Builder::new()
            .name("login-forward".into())
            .spawn(move || {
                let started = link::forward_start(
                    port,
                    &ticket,
                    token.as_deref(),
                    |p| link::connect_local(p, link::LOCAL_CONNECT),
                    dial,
                );
                match started {
                    Ok((local, up)) => {
                        let _ = writer.send(&Event::Reply(Reply::ok(req)));
                        let (a, b) = crate::proxy::splice(local, up, link::FORWARD_IDLE);
                        log::info!("login callback of port {port}: {b} bytes in, {a} bytes back");
                    }
                    Err(e) => {
                        log::info!("login callback of port {port}: {e}");
                        let _ = writer.send(&Event::Reply(Reply::error(req, ErrorCode::Io, e)));
                    }
                }
            });
        match spawned {
            Ok(_) => None,
            Err(e) => Some(Reply::error(req, ErrorCode::Internal, e)),
        }
    }
}

/// The `guest-login` dial to the host.
#[cfg(target_os = "macos")]
fn dial() -> std::io::Result<std::os::unix::net::UnixStream> {
    super::super::vsock::connect(
        super::super::HOST_CID,
        crate::vm::ports::GUEST_LOGIN,
        link::LOCAL_CONNECT,
    )
}

/// No vsock outside the guest.
#[cfg(not(target_os = "macos"))]
fn dial() -> std::io::Result<std::os::unix::net::UnixStream> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
