//! Small helpers of the kv service: the JSON of an entry, the response of a
//! write error, the peer check and request ids.

use super::{store, Response, Result, Status, WriteError};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Instant;

pub(super) fn entry_json(e: &store::Entry) -> serde_json::Value {
    serde_json::json!({
        "key": e.key, "value": e.value, "version": e.version,
        "writer": e.writer, "writer_harness": e.writer_harness,
        "updated": e.updated, "expires": e.expires,
    })
}

pub(super) fn write_error(e: WriteError) -> Result<Response> {
    Ok(match e {
        WriteError::Conflict { key, current } => Response {
            status: Status::Conflict,
            message: format!("{key:?} has version {current}"),
            result: serde_json::json!({ "key": key, "version": current }),
        },
        WriteError::Quota(m) => Response::new(Status::TooLarge, m),
        WriteError::Missing => Response::new(Status::NotFound, "no such key"),
        WriteError::Db(e) => return Err(e),
    })
}

/// True when the other end closed its side.
pub(super) fn peer_gone(conn: &UnixStream) -> bool {
    crate::platform::peer_gone(conn.as_raw_fd())
}

pub(crate) fn request_id() -> String {
    use sha2::{Digest, Sha256};
    let seed = format!("{:?}-{}-kv", Instant::now(), std::process::id());
    let h = crate::manifest::hex(&Sha256::digest(seed.as_bytes()));
    format!("r-{}-{}", crate::state::now(), &h[..8])
}
