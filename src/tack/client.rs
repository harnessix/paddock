//! The client side of the tackd protocol: one request, one answer, with
//! errors that say what to do.

use super::proto::{self, Answer, Request};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long a client waits for tackd's answer.
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum Error {
    /// No socket: tackd is not installed, or not started.
    NoSocket,
    /// A socket without a tackd behind it.
    NotRunning,
    TimedOut,
    Io(io::Error),
    Protocol(String),
    /// tackd answered with an error.
    Refused(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoSocket => write!(f, "tackd is not running (no socket): sudo paddock tack setup installs and starts it"),
            Error::NotRunning => write!(f, "tackd does not answer on its socket: it stopped or crashed (see paddock tack status)"),
            Error::TimedOut => write!(f, "tackd did not answer within {}s", TIMEOUT.as_secs()),
            Error::Io(e) => write!(f, "cannot talk to tackd: {e}"),
            Error::Protocol(e) => write!(f, "{e}"),
            Error::Refused(e) => write!(f, "tackd refused the request: {e}"),
        }
    }
}

impl std::error::Error for Error {}

fn io_err(e: io::Error) -> Error {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => Error::TimedOut,
        io::ErrorKind::UnexpectedEof => {
            Error::Protocol("tackd closed the connection without an answer".into())
        }
        _ => Error::Io(e),
    }
}

/// Send one request and read the answer. An `Answer::Error` becomes
/// `Error::Refused`.
pub fn ask(socket: &Path, req: Request, timeout: Duration) -> Result<Answer, Error> {
    let s = UnixStream::connect(socket).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => Error::NoSocket,
        io::ErrorKind::ConnectionRefused => Error::NotRunning,
        _ => Error::Io(e),
    })?;
    s.set_read_timeout(Some(timeout)).map_err(Error::Io)?;
    s.set_write_timeout(Some(timeout)).map_err(Error::Io)?;
    let mut w = &s;
    // A busy server may send its refusal and close before our request is
    // fully written. Read that reply even when the write sees a broken pipe.
    let write_error = proto::write_frame(&mut w, &proto::envelope(req)).err();
    if let Some(ref e) = write_error {
        if e.kind() != io::ErrorKind::BrokenPipe {
            return Err(io_err(write_error.unwrap()));
        }
    }
    let mut r = &s;
    let body = match proto::read_frame(&mut r) {
        Ok(body) => body,
        Err(e) => return Err(io_err(write_error.unwrap_or(e))),
    };
    match proto::parse_answer(&body).map_err(Error::Protocol)? {
        Answer::Error { message, .. } => Err(Error::Refused(message)),
        a => Ok(a),
    }
}
