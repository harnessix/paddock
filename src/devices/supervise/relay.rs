//! `--connect PORT:SOCKET`: `127.0.0.1:PORT` in the device sandbox reaches
//! a socket that the broker serves on the host, one connection at a time.
//! The relay copies bytes both ways and logs the counts. At most 32
//! connections per entry.

use super::Log;
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Connections at the same time, per entry.
pub const MAX_CONNECTIONS: usize = 32;

pub struct Relay {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Relay {
    pub fn start(port: u16, socket: &Path, log: Arc<Log>, stop: Arc<AtomicBool>) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .with_context(|| format!("--connect {port}: cannot listen on 127.0.0.1:{port}"))?;
        listener.set_nonblocking(true)?;
        let socket = socket.to_path_buf();
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name(format!("device-serve-relay-{port}"))
            .spawn(move || accept_loop(listener, socket, log, s, port))
            .context("cannot start the relay thread")?;
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop accepting. Open connections end on their own.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    socket: PathBuf,
    log: Arc<Log>,
    stop: Arc<AtomicBool>,
    port: u16,
) {
    let open = Arc::new(AtomicUsize::new(0));
    let fd = listener.as_raw_fd();
    while !stop.load(Ordering::SeqCst) {
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll on our own listener.
        let n = unsafe { libc::poll(&mut p, 1, 200) };
        if n <= 0 {
            continue;
        }
        let (tcp, _) = match listener.accept() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => {
                log.line(&format!("connect {port}: accept failed: {e}"));
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            log.line(&format!(
                "connect {port}: refused, {MAX_CONNECTIONS} connections are open"
            ));
            drop(tcp);
            continue;
        }
        let _ = tcp.set_nonblocking(false);
        let unix = match UnixStream::connect(&socket) {
            Ok(u) => u,
            Err(e) => {
                log.line(&format!(
                    "connect {port}: cannot reach {}: {e}",
                    socket.display()
                ));
                continue;
            }
        };
        open.fetch_add(1, Ordering::SeqCst);
        let open2 = open.clone();
        let log2 = log.clone();
        let _ = std::thread::Builder::new()
            .name(format!("device-serve-relay-{port}-conn"))
            .spawn(move || {
                let (up, down) = pair(tcp, unix);
                log2.line(&format!(
                    "connect {port}: closed, {up} bytes up, {down} bytes down"
                ));
                open2.fetch_sub(1, Ordering::SeqCst);
            });
    }
}

/// Copy both ways until both sides end. Bytes from the TCP side (up) and
/// from the socket (down).
fn pair(tcp: TcpStream, unix: UnixStream) -> (usize, usize) {
    let (Ok(tcp2), Ok(unix2)) = (tcp.try_clone(), unix.try_clone()) else {
        return (0, 0);
    };
    let up = std::thread::spawn(move || {
        copy(tcp, &unix, |u| {
            let _ = u.shutdown(Shutdown::Write);
        })
    });
    let down = copy(unix2, &tcp2, |t| {
        let _ = t.shutdown(Shutdown::Write);
    });
    (up.join().unwrap_or(0), down)
}

fn copy<R: Read, W: Write>(mut from: R, mut to: W, end: impl Fn(&mut W)) -> usize {
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0;
    loop {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                total += n;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    end(&mut to);
    total
}
