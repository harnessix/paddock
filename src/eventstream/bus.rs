//! The socket of the stream. Each follower (`paddock events --follow`)
//! listens on `<pid>.sock` in one folder under paddock's run directory
//! (`sockpath::socket_dir(run, uid, "events")`). A publisher (a session,
//! the firewall, the setup, a grant command) connects to each socket of
//! the folder, writes its line and closes. No daemon owns the stream:
//! with no follower, a publish costs one `read_dir`.
//!
//! The rules:
//! - A publish never fails its caller and never waits long: a connection
//!   that does not take the line in [`WRITE_WAIT`] is dropped.
//! - A line that a live follower did not get marks it lost: the publisher
//!   makes `<pid>.lost`, and the follower prints a `stream` line of state
//!   `lost`, so the reader queries the state again.
//! - A socket whose follower is dead goes. A live follower whose socket
//!   refuses (a full backlog) is marked lost, never removed.

use super::schema::{self, Event, Line};
use crate::env::HostEnv;
use anyhow::{Context, Result};
use std::io::{BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The id of the socket folder in the run directory.
pub const SOCKET_ID: &str = "events";
/// The longest wait of a publisher on one follower.
pub const WRITE_WAIT: Duration = Duration::from_millis(500);
/// The longest wait of a follower on one publisher.
pub const READ_WAIT: Duration = Duration::from_secs(2);
/// The lines that wait for the reader of a follower at most.
pub const QUEUE: usize = 4096;
/// The most bytes that a follower reads from one connection.
const CONN_MAX: u64 = (schema::MAX_LINE as u64) * 16;

/// The folder of the follower sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bus {
    dir: PathBuf,
    /// The run directory and the uid of [`Bus::of`]: the follower makes
    /// its socket through `sockpath::host_socket`, which checks each
    /// level of the short folders of macOS.
    run: Option<(PathBuf, u32)>,
}

impl Bus {
    /// The bus of the user of `env`.
    pub fn of(env: &HostEnv) -> Self {
        let run = env.paddock_run();
        Self {
            dir: crate::sockpath::socket_dir(&run, env.uid, SOCKET_ID),
            run: Some((run, env.uid)),
        }
    }

    /// A bus in one folder. Tests use a temporary one.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            run: None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Publish one event now. Never fails: a follower that misses it is
    /// marked lost.
    pub fn publish(&self, event: Event) {
        self.send(&Line::now(event).to_text());
    }

    /// Send one line to every follower: the count that took it.
    pub fn send(&self, text: &str) -> usize {
        let mut sent = 0;
        for (pid, path) in self.followers() {
            match deliver(&path, text) {
                Ok(()) => sent += 1,
                Err(e) if gone(&e) && !crate::state::alive(pid) => {
                    remove_follower(&path);
                }
                Err(e) => {
                    log::debug!("events: {} missed a line: {e}", path.display());
                    mark_lost(&path);
                }
            }
        }
        sent
    }

    /// The sockets of the folder: `(pid, path)`, sorted.
    pub fn followers(&self) -> Vec<(u32, PathBuf)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<(u32, PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let pid = name.to_str()?.strip_suffix(".sock")?.parse::<u32>().ok()?;
                (pid > 0).then(|| (pid, e.path()))
            })
            .collect();
        out.sort();
        out
    }
}

/// Write `text` to the follower at `path`.
fn deliver(path: &Path, text: &str) -> std::io::Result<()> {
    let mut s = connect_timeout(path)?;
    s.set_write_timeout(Some(WRITE_WAIT))?;
    s.write_all(text.as_bytes())?;
    s.shutdown(std::net::Shutdown::Write)
}

/// Connect without letting a full listener backlog stall the publisher.
fn connect_timeout(path: &Path) -> std::io::Result<UnixStream> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: zero is a valid initial value for sockaddr_un.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid event socket path",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    #[cfg(target_os = "macos")]
    {
        addr.sun_len = std::mem::size_of::<libc::sockaddr_un>() as u8;
    }
    // SAFETY: socket has no pointer arguments; ownership transfers to UnixStream.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_nonblocking(true)?;
    // SAFETY: addr is initialized and the supplied length matches its type.
    let rc = unsafe {
        libc::connect(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&addr) as libc::socklen_t,
        )
    };
    if rc < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
        let mut p = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: p is valid for one pollfd.
        let n = unsafe { libc::poll(&mut p, 1, WRITE_WAIT.as_millis() as libc::c_int) };
        if n < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "event connection timed out",
            ));
        }
        if let Some(e) = stream.take_error()? {
            return Err(e);
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// The errors of a socket without a listener.
fn gone(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
    )
}

/// The lost marker of a follower socket: `<pid>.lost`.
pub fn lost_marker(socket: &Path) -> PathBuf {
    socket.with_extension("lost")
}

fn mark_lost(socket: &Path) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(lost_marker(socket));
}

fn remove_follower(socket: &Path) {
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_file(lost_marker(socket));
}

/// The listening side: one socket, removed on drop.
pub struct Follower {
    path: PathBuf,
    listener: UnixListener,
}

impl Follower {
    /// Listen as `pid` on `bus`. The folder is made private; the sockets
    /// of dead followers go first.
    pub fn bind(bus: &Bus, pid: u32) -> Result<Self> {
        let name = format!("{pid}.sock");
        let path = match &bus.run {
            Some((run, uid)) => crate::sockpath::host_socket(run, *uid, SOCKET_ID, &name)?,
            None => bus.dir().join(&name),
        };
        crate::privfs::dir(bus.dir())
            .with_context(|| format!("cannot make {}", bus.dir().display()))?;
        crate::sockpath::check(&path)?;
        for (other, old) in bus.followers() {
            if other == pid || !crate::state::alive(other) {
                remove_follower(&old);
            }
        }
        if let Ok(entries) = std::fs::read_dir(bus.dir()) {
            for entry in entries.flatten() {
                let marker = entry.path();
                if marker.extension().is_some_and(|e| e == "lost")
                    && !marker.with_extension("sock").exists()
                {
                    let _ = std::fs::remove_file(marker);
                }
            }
        }
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("cannot listen on {}", path.display()))?;
        Ok(Self { path, listener })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True once after a publisher marked this follower lost.
    pub fn take_lost(&self) -> bool {
        std::fs::remove_file(lost_marker(&self.path)).is_ok()
    }

    /// Accept the publishers in a thread: each valid line goes to `tx`.
    /// When the reader of the follower lags [`QUEUE`] lines behind, the
    /// line is dropped and the follower marked lost. The thread ends when
    /// the receiver is gone.
    pub fn serve(&self, tx: std::sync::mpsc::SyncSender<String>) -> Result<()> {
        let listener = self.listener.try_clone()?;
        let path = self.path.clone();
        std::thread::Builder::new()
            .name("events-accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    for line in read_lines(conn) {
                        match tx.try_send(line) {
                            Ok(()) => {}
                            Err(std::sync::mpsc::TrySendError::Full(_)) => mark_lost(&path),
                            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
                        }
                    }
                }
            })?;
        Ok(())
    }
}

/// A channel for [`Follower::serve`].
pub fn queue() -> (
    std::sync::mpsc::SyncSender<String>,
    std::sync::mpsc::Receiver<String>,
) {
    std::sync::mpsc::sync_channel(QUEUE)
}

impl Drop for Follower {
    fn drop(&mut self) {
        remove_follower(&self.path);
    }
}

/// The valid lines of one connection, at most [`CONN_MAX`] bytes, each at
/// most [`schema::MAX_LINE`]. A line that is not a stream line is logged
/// and dropped.
pub fn read_lines(conn: UnixStream) -> Vec<String> {
    let _ = conn.set_read_timeout(Some(READ_WAIT));
    let mut reader = std::io::BufReader::new(conn.take(CONN_MAX));
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&buf);
        let text = text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        if schema::valid_line(text) {
            out.push(text.to_string());
        } else {
            log::warn!(
                "events: dropped a line that is not an event ({} bytes)",
                text.len()
            );
        }
    }
    out
}

#[cfg(test)]
#[path = "bus_tests.rs"]
mod tests;
