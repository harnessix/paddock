//! How a request reaches the host: the gateway device or the host socket.

use super::device;
use crate::broker::{Request, Response, SOCKET_IN_SANDBOX};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

/// Send one request to the host: through the gateway device
/// `/dev/paddock/gw` when the sandbox has it, else through the host socket.
pub fn send(req: &Request) -> Result<Response, String> {
    // A unit test talks to its own broker. Run in a paddock sandbox, the
    // gateway would take the request to the real host instead.
    if cfg!(test) && std::env::var_os("PADDOCK_HOST_SOCKET").is_some() {
        return send_socket(req);
    }
    let gw = device::root().join("gw");
    if gw.exists() {
        return send_gateway(req);
    }
    send_socket(req)
}

/// Through the gateway. A read that reaches its time limit (still waiting)
/// sends the same request again, which attaches to the one that waits.
/// When the device fails, paddock-host does not fall back to the socket:
/// the request may have run.
fn send_gateway(req: &Request) -> Result<Response, String> {
    let body = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    loop {
        match device::call("gw", &body) {
            Ok(out) => {
                return serde_json::from_slice(&out)
                    .map_err(|e| format!("bad response from the host: {e}"))
            }
            Err(e) if e == crate::waiting::ERRNO_STILL_WAITING => continue,
            Err(e) => {
                return Err(format!(
                    "the gateway /dev/paddock/gw failed: {}",
                    device::describe(e)
                ))
            }
        }
    }
}

/// Through the host socket, for a sandbox without `/dev/paddock`.
fn send_socket(req: &Request) -> Result<Response, String> {
    let path =
        std::env::var("PADDOCK_HOST_SOCKET").unwrap_or_else(|_| SOCKET_IN_SANDBOX.to_string());
    let mut s =
        UnixStream::connect(&path).map_err(|e| format!("cannot reach the host at {path}: {e}"))?;
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(s);
    let mut out = String::new();
    reader.read_line(&mut out).map_err(|e| e.to_string())?;
    serde_json::from_str(out.trim()).map_err(|e| format!("bad response from the host: {e}"))
}
