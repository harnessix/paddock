//! The host side of `connect = ["2222:mac.local:22"]`: for each entry a
//! Unix listener at `<instance>/connect/N.sock`, which the device sandbox
//! sees read-only at `/run/paddock/connect/N.sock`. The supervisor inside
//! listens on `127.0.0.1:2222` and copies each connection to that socket
//! (supervise/relay.rs). Here, one thread per connection resolves the host
//! on the host at connect time, connects to `host:port` with a 10 s
//! timeout, copies bytes both ways and counts them. One audit line per
//! connection: the device, the caller, the host, the port, the bytes in and
//! out and the duration, never content. At most 32 connections per entry.
//!
//! The host resolves the name and uses its own routes, so the device
//! sandbox never has a route to the LAN. A host that resolves to a loopback
//! or LAN address is still connected (the user named it on the host), and
//! the audit line says `lan`.

use crate::devices::config::Connect;
use crate::env::HostEnv;
use anyhow::{Context as _, Result};
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the sockets appear in the device sandbox.
pub const CONNECT_DIR_IN: &str = "/run/paddock/connect";
/// Connections at the same time, per entry.
pub const MAX_CONNECTIONS: usize = 32;
/// The connect timeout to the host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The audit capability of one relayed connection.
pub const AUDIT_CAPABILITY: &str = "device-connect";

/// The socket of entry `n` in the device sandbox.
pub fn socket_in(n: usize) -> String {
    format!("{CONNECT_DIR_IN}/{n}.sock")
}

/// The listeners of one instance. Dropping them stops the accept loops;
/// open connections end on their own.
pub struct Listeners {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub dir: PathBuf,
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Who the connections are for, in the audit line.
#[derive(Clone)]
pub struct Who {
    pub env: HostEnv,
    pub device: String,
    pub caller: String,
}

/// Start one listener per entry in `dir` (created here, mode 0700).
pub fn start(who: Who, dir: &Path, entries: &[Connect]) -> Result<Listeners> {
    crate::privfs::dir(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    for (n, c) in entries.iter().enumerate() {
        let path = dir.join(format!("{n}.sock"));
        crate::sockpath::check(&path)?;
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).with_context(|| {
            format!(
                "connect {}: cannot listen on {}",
                c.render(),
                path.display()
            )
        })?;
        listener.set_nonblocking(true)?;
        let (s, w, c) = (stop.clone(), who.clone(), c.clone());
        threads.push(
            std::thread::Builder::new()
                .name(format!("device-connect-{n}"))
                .spawn(move || accept_loop(listener, s, w, c))
                .context("cannot start the connect thread")?,
        );
    }
    Ok(Listeners {
        stop,
        threads,
        dir: dir.to_path_buf(),
    })
}

fn accept_loop(listener: UnixListener, stop: Arc<AtomicBool>, who: Who, c: Connect) {
    let open = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        let mut p = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll on our own listener.
        if unsafe { libc::poll(&mut p, 1, 200) } <= 0 {
            continue;
        }
        let Ok((conn, _)) = listener.accept() else {
            continue;
        };
        let _ = conn.set_nonblocking(false);
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            audit(
                &who,
                &c,
                0,
                0,
                Duration::ZERO,
                false,
                "refused: too many connections",
            );
            continue;
        }
        let (o, w, c) = (open.clone(), who.clone(), c.clone());
        std::thread::spawn(move || {
            relay(conn, &w, &c);
            o.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// True for an address of this machine or its LAN.
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified()
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            a.is_loopback()
                || a.is_unspecified()
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xfe00) == 0xfc00
        }
    }
}

/// Connect to `host:port` on the host: the first address that answers.
fn dial(c: &Connect) -> Result<(TcpStream, SocketAddr), String> {
    let addrs: Vec<SocketAddr> = (c.host.as_str(), c.port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {e}", c.host))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("{} resolves to no address", c.host));
    }
    let mut last = String::new();
    for a in &addrs {
        match TcpStream::connect_timeout(a, CONNECT_TIMEOUT) {
            Ok(s) => return Ok((s, *a)),
            Err(e) => last = format!("{a}: {e}"),
        }
    }
    Err(format!("cannot connect to {}:{}: {last}", c.host, c.port))
}

fn relay(inside: UnixStream, who: &Who, c: &Connect) {
    let start = Instant::now();
    let (host, addr) = match dial(c) {
        Ok(x) => x,
        Err(e) => {
            audit(who, c, 0, 0, start.elapsed(), false, &format!("error: {e}"));
            let _ = inside.shutdown(Shutdown::Both);
            return;
        }
    };
    let lan = is_lan(addr.ip());
    let bytes_in = Arc::new(AtomicU64::new(0));
    let bytes_out = Arc::new(AtomicU64::new(0));
    let (Ok(mut in_r), Ok(mut host_r)) = (inside.try_clone(), host.try_clone()) else {
        audit(
            who,
            c,
            0,
            0,
            start.elapsed(),
            lan,
            "error: cannot clone the sockets",
        );
        return;
    };
    let (mut in_w, mut host_w) = (inside, host);
    let out = bytes_out.clone();
    // Inside to the host: the device program's bytes.
    let to_host = std::thread::spawn(move || {
        let n = pump(&mut in_r, &mut host_w);
        out.store(n, Ordering::SeqCst);
        let _ = host_w.shutdown(Shutdown::Write);
    });
    let n = pump(&mut host_r, &mut in_w);
    bytes_in.store(n, Ordering::SeqCst);
    let _ = in_w.shutdown(Shutdown::Write);
    let _ = to_host.join();
    audit(
        who,
        c,
        bytes_in.load(Ordering::SeqCst),
        bytes_out.load(Ordering::SeqCst),
        start.elapsed(),
        lan,
        "ok",
    );
}

/// Copy until end of file or an error; the number of bytes.
fn pump(r: &mut dyn Read, w: &mut dyn Write) -> u64 {
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if w.write_all(&buf[..n]).is_err() {
                    break;
                }
                total += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    total
}

/// One audit line per connection. `bytes_in`: from the host to the device,
/// `bytes_out`: from the device to the host.
fn audit(
    who: &Who,
    c: &Connect,
    bytes_in: u64,
    bytes_out: u64,
    took: Duration,
    lan: bool,
    status: &str,
) {
    crate::broker::audit_line(
        &who.env,
        &serde_json::json!({
            "time": crate::state::now(),
            "sandbox": who.caller,
            "capability": AUDIT_CAPABILITY,
            "device": who.device,
            "host": c.host,
            "port": c.port,
            "local_port": c.local_port,
            "bytes_in": bytes_in,
            "bytes_out": bytes_out,
            "duration_ms": took.as_millis() as u64,
            "lan": lan,
            "status": status,
        }),
    );
}
