//! The devices of a sandbox over a Unix socket, for backends that cannot
//! show a FUSE mount (Seatbelt on macOS).
//!
//! The socket is a second front end to the same devices as `fuse.rs`: the
//! same `Device` trait, `Call`, catalog and secret store. There is one
//! socket per sandbox, so the socket names the caller, as the mount does on
//! Linux. No request carries an identity.
//!
//! One connection is one request. The client sends one JSON line, then the
//! request bytes:
//!
//! ```text
//! {"v":1,"op":"call","device":"wait","size":17}\n
//! <17 bytes>
//! ```
//!
//! The server answers one JSON line, then the response bytes:
//!
//! ```text
//! {"v":1,"ok":true,"size":42}\n
//! <42 bytes>
//! ```
//!
//! or `{"v":1,"ok":false,"errno":62,"error":"..."}` and closes. The errno
//! values are those of `DevError::errno` and of the FUSE server, so the
//! client maps them to the same exit codes. `op` is `call` (a device),
//! `catalog` (no body: the bytes of `catalog_with`) or `secret` with a
//! `name` (no body: the value of a secret file, one use counted).
//!
//! A `call` with `"stream":true` in its header gets the response as it
//! comes. The answer line is `{"v":1,"ok":true,"stream":true}` without a
//! `size`, then chunks: a 4-byte big-endian length and that many bytes. A
//! chunk of length 0 is the clean end. A connection that closes without
//! it is a failure after the first byte, as a read that fails with `EIO`
//! after bytes is on Linux. A failure before the first byte is the
//! `ok:false` line, as for a collected response. A client without
//! `stream` in its header gets the collected response with `size`, so an
//! older client keeps working, and a client that asks for a stream from an
//! older server gets `size` and reads that.
//!
//! The client keeps the connection open in both directions until it has
//! the answer: a closed connection cancels the request, as a close of the
//! device file does on Linux.
//!
//! The same framing carries a request from one paddock to another
//! (`run = "remote"`, forward.rs). The header then has `caller`,
//! `caller_host` and `caller_chain`: what the forwarding paddock says
//! about its caller, which a server made with `for_remote` keeps as text
//! for the card and the audit line (`Call::remote`). A per-sandbox server
//! ignores the fields, and a remote server refuses `op: secret`.

use super::{catalog_with, Call, DevError, Device, RemoteCaller, Sink};
use crate::credentials::secret::{FileError, SecretFiles};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Open connections at the same time, per sandbox. The same number as the
/// open device files of the FUSE server.
pub const MAX_OPEN: usize = 64;
/// Requests that run at the same time, per sandbox.
pub const MAX_RUNNING: usize = 16;
/// The longest header line, without its newline.
pub const MAX_HEADER: usize = 4096;
/// How long the server waits for the header and the body of a request.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// The pause after a failed accept.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(20);
/// How often a running request checks its connection.
const WATCH: Duration = Duration::from_millis(100);
/// The largest response that a client accepts.
const CLIENT_MAX_RESPONSE: usize = 64 << 20;
/// Chunks of a stream that wait for the connection before the device
/// waits too.
const STREAM_QUEUE: usize = 16;

/// The header line of a request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub v: u32,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub size: u64,
    /// `call`: the response as it comes, in chunks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
    /// From another paddock (forward.rs): its caller's sandbox id, its
    /// own name and the chain of sandboxes above the caller. Text only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_chain: Option<Vec<String>>,
}

impl Request {
    /// The request with the caller fields of a forwarded request.
    pub fn with_caller(mut self, c: &RemoteCaller) -> Self {
        self.caller = Some(c.sandbox.clone());
        self.caller_host = Some(c.host.clone());
        self.caller_chain = Some(c.chain.clone());
        self
    }

    /// The caller fields as a `RemoteCaller`, when any is set.
    pub fn remote_caller(&self) -> Option<RemoteCaller> {
        if self.caller.is_none() && self.caller_host.is_none() && self.caller_chain.is_none() {
            return None;
        }
        Some(RemoteCaller::new(
            self.caller.as_deref().unwrap_or(""),
            self.caller_host.as_deref().unwrap_or(""),
            self.caller_chain.as_deref().unwrap_or(&[]),
        ))
    }

    pub fn call(device: &str, size: usize) -> Self {
        Self {
            v: 1,
            op: "call".into(),
            device: Some(device.into()),
            size: size as u64,
            ..Default::default()
        }
    }

    /// `call`, with the response as it comes.
    pub fn call_stream(device: &str, size: usize) -> Self {
        Self {
            stream: true,
            ..Self::call(device, size)
        }
    }

    pub fn catalog() -> Self {
        Self {
            v: 1,
            op: "catalog".into(),
            ..Default::default()
        }
    }

    pub fn secret(name: &str) -> Self {
        Self {
            v: 1,
            op: "secret".into(),
            name: Some(name.into()),
            ..Default::default()
        }
    }
}

/// The header line of an answer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    pub v: u32,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errno: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The response comes in chunks, without a `size`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}

/// The text of an errno in an answer.
fn errno_text(errno: i32) -> String {
    if errno == crate::waiting::ERRNO_STILL_WAITING {
        return "still waiting".into();
    }
    std::io::Error::from_raw_os_error(errno).to_string()
}

/// Read one line of at most `MAX_HEADER` bytes, without its newline.
/// None when the line is too long, has no newline or the read failed.
pub fn read_line(r: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    r.by_ref()
        .take(MAX_HEADER as u64 + 1)
        .read_until(b'\n', &mut line)
        .ok()?;
    if line.pop() != Some(b'\n') {
        return None;
    }
    Some(line)
}

/// Write an answer: the header line, then the bytes.
fn answer(w: &mut impl Write, result: Result<&[u8], i32>) -> std::io::Result<()> {
    let reply = match result {
        Ok(data) => Reply {
            v: 1,
            ok: true,
            size: Some(data.len() as u64),
            ..Default::default()
        },
        Err(e) => Reply {
            v: 1,
            ok: false,
            errno: Some(e),
            error: Some(errno_text(e)),
            ..Default::default()
        },
    };
    let mut line = serde_json::to_vec(&reply).unwrap_or_default();
    line.push(b'\n');
    w.write_all(&line)?;
    if let Ok(data) = result {
        w.write_all(data)?;
    }
    w.flush()
}

/// The devices and secret files of one sandbox, served over a socket.
pub struct Server {
    sandbox: String,
    devices: Vec<Arc<dyn Device>>,
    secrets: Arc<SecretFiles>,
    open: AtomicUsize,
    running: AtomicUsize,
    /// The server of `paddock device serve-remote`: the caller fields of
    /// a request reach the devices as `Call::remote`, and `op: secret` is
    /// refused.
    remote: bool,
}

/// Decrements a counter when it goes.
struct Slot<'a>(&'a AtomicUsize);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Take one of `max` slots of `n`, or None when all are taken.
fn slot(n: &AtomicUsize, max: usize) -> Option<Slot<'_>> {
    if n.fetch_add(1, Ordering::SeqCst) >= max {
        n.fetch_sub(1, Ordering::SeqCst);
        return None;
    }
    Some(Slot(n))
}

impl Server {
    pub fn new(
        sandbox: &str,
        devices: Vec<Arc<dyn Device>>,
        secrets: Arc<SecretFiles>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sandbox: sandbox.to_string(),
            devices,
            secrets,
            open: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            remote: false,
        })
    }

    /// The server for the requests of another paddock (forward.rs):
    /// `sandbox` is the transport identity, `remote:PEER`. No secret
    /// files: a remote caller gets devices only.
    pub fn for_remote(sandbox: &str, devices: Vec<Arc<dyn Device>>) -> Arc<Self> {
        Arc::new(Self {
            sandbox: sandbox.to_string(),
            devices,
            secrets: SecretFiles::new(None, None),
            open: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            remote: true,
        })
    }

    /// Take a connection slot and serve the connection on its own thread.
    /// Over `MAX_OPEN` connections, the answer is EBUSY at once.
    pub fn accept(self: &Arc<Self>, mut conn: UnixStream) {
        let Some(taken) = slot(&self.open, MAX_OPEN) else {
            let _ = answer(&mut conn, Err(libc::EBUSY));
            return;
        };
        // The thread gives the slot back.
        std::mem::forget(taken);
        let me = self.clone();
        std::thread::spawn(move || {
            let _slot = Slot(&me.open);
            me.handle(conn);
        });
    }

    /// Serve one request on `conn`, then close it.
    pub fn handle(&self, conn: UnixStream) {
        let _ = conn.set_read_timeout(Some(READ_TIMEOUT));
        // One descriptor for both directions: a clone would double the
        // files of 64 open connections, and macOS starts with 256.
        let mut out = &conn;
        let mut r = BufReader::new(&conn);
        let _ = match self.serve(&mut r, &mut out) {
            Ok(Some(data)) => answer(&mut out, Ok(&data)),
            // A stream wrote its own answer.
            Ok(None) => Ok(()),
            Err(e) => answer(&mut out, Err(e)),
        };
    }

    /// The collected response, or None when the request was a stream and
    /// the answer went to `out` already.
    fn serve(
        &self,
        r: &mut BufReader<&UnixStream>,
        out: &mut &UnixStream,
    ) -> Result<Option<Vec<u8>>, i32> {
        let req: Request = read_line(r)
            .and_then(|l| serde_json::from_slice(&l).ok())
            .filter(|q: &Request| q.v == 1)
            .ok_or(libc::EINVAL)?;
        match req.op.as_str() {
            "catalog" if req.size == 0 => {
                Ok(Some(catalog_with(&self.devices, &self.secrets.names())))
            }
            // No secret file leaves this machine.
            "secret" if self.remote => Err(libc::EACCES),
            "secret" if req.size == 0 => {
                let name = req.name.as_deref().ok_or(libc::EINVAL)?;
                match self.secrets.open(name) {
                    Ok(v) => {
                        log::info!("sandbox {}: secret file {name} read", self.sandbox);
                        Ok(Some(v.as_bytes().to_vec()))
                    }
                    Err(FileError::Revoked) => Err(libc::EACCES),
                    Err(FileError::Missing) => Err(libc::ENOENT),
                }
            }
            "call" => {
                let name = req.device.as_deref().ok_or(libc::EINVAL)?;
                let dev = self
                    .devices
                    .iter()
                    .find(|d| d.name() == name)
                    .ok_or(libc::ENOENT)?
                    .clone();
                // Checked on the header, before the body is read.
                if req.size > dev.info().limits.max_request as u64 {
                    return Err(libc::EFBIG);
                }
                let mut body = vec![0u8; req.size as usize];
                r.read_exact(&mut body).map_err(|_| libc::EINVAL)?;
                let _run = slot(&self.running, MAX_RUNNING).ok_or(libc::EBUSY)?;
                // The caller text of a forwarded request: a per-sandbox
                // server knows its caller from the socket and drops it.
                let remote = if self.remote {
                    req.remote_caller()
                } else {
                    None
                };
                if req.stream {
                    self.run_stream(dev, body, remote, r.get_ref(), out)
                        .map(|()| None)
                } else {
                    self.run(dev, body, remote, r.get_ref()).map(Some)
                }
            }
            _ => Err(libc::EINVAL),
        }
    }

    /// Run one request under the deadline of the device. A closed
    /// connection cancels it.
    fn run(
        &self,
        dev: Arc<dyn Device>,
        request: Vec<u8>,
        remote: Option<RemoteCaller>,
        conn: &UnixStream,
    ) -> Result<Vec<u8>, i32> {
        use std::os::fd::AsRawFd;
        let info = dev.info();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(info.limits.deadline_seconds);
        let cancel = Arc::new(AtomicBool::new(false));
        let call = Call {
            sandbox: self.sandbox.clone(),
            deadline,
            cancel: cancel.clone(),
            remote,
        };
        let req_len = request.len();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = dev.clone();
        std::thread::spawn(move || {
            let _ = tx.send(worker.call(&call, &request));
        });
        let fd = conn.as_raw_fd();
        let result = loop {
            match rx.recv_timeout(WATCH) {
                Ok(Ok(out)) if out.len() > info.limits.max_response => break Err(libc::EMSGSIZE),
                Ok(Ok(out)) => break Ok(out),
                Ok(Err(_)) if Instant::now() >= deadline => break Err(libc::ETIMEDOUT),
                Ok(Err(e)) => break Err(e.errno()),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err(libc::EIO),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= deadline {
                cancel.store(true, Ordering::SeqCst);
                break Err(libc::ETIMEDOUT);
            }
            if crate::platform::peer_gone(fd) {
                cancel.store(true, Ordering::SeqCst);
                break Err(libc::ECANCELED);
            }
        };
        // Sizes and the outcome only: never the request or the response.
        let outcome = match &result {
            Ok(d) => format!("{} bytes back", d.len()),
            Err(e) if cancel.load(Ordering::SeqCst) && *e != libc::ETIMEDOUT => "cancelled".into(),
            Err(e) => format!("error {e}"),
        };
        log::info!(
            "sandbox {}: device {}: request of {req_len} bytes, {outcome}, {} ms",
            self.sandbox,
            dev.name(),
            start.elapsed().as_millis()
        );
        result
    }
}

/// A piece of a stream on its way from the device to the connection.
enum Piece {
    Bytes(Vec<u8>),
    End(Result<(), DevError>),
}

/// The sink of a streaming request on the socket: each piece goes to the
/// server thread, which writes it. A full queue makes the device wait;
/// a closed queue (the client went) makes it stop.
struct ChanSink {
    tx: SyncSender<Piece>,
    sent: usize,
    max: usize,
    too_large: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Sink for ChanSink {
    fn push(&mut self, bytes: &[u8]) {
        if self.sent + bytes.len() > self.max {
            self.too_large.store(true, Ordering::SeqCst);
            self.cancel.store(true, Ordering::SeqCst);
            return;
        }
        self.sent += bytes.len();
        if self.tx.send(Piece::Bytes(bytes.to_vec())).is_err() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Write one chunk of a stream: the length, then the bytes.
fn chunk(w: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

impl Server {
    /// Run one request whose response goes to `out` as it comes: the
    /// answer line at the first byte (or at the end for an empty stream),
    /// then the chunks, then the 0 chunk. A failure before the first byte
    /// is an errno for the answer line; one after it closes the connection
    /// without the 0 chunk.
    fn run_stream(
        &self,
        dev: Arc<dyn Device>,
        request: Vec<u8>,
        remote: Option<RemoteCaller>,
        conn: &UnixStream,
        out: &mut &UnixStream,
    ) -> Result<(), i32> {
        use std::os::fd::AsRawFd;
        let info = dev.info();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(info.limits.deadline_seconds);
        let cancel = Arc::new(AtomicBool::new(false));
        let too_large = Arc::new(AtomicBool::new(false));
        let call = Call {
            sandbox: self.sandbox.clone(),
            deadline,
            cancel: cancel.clone(),
            remote,
        };
        let req_len = request.len();
        let (tx, rx) = sync_channel(STREAM_QUEUE);
        let mut sink = ChanSink {
            tx: tx.clone(),
            sent: 0,
            max: info.limits.max_response,
            too_large: too_large.clone(),
            cancel: cancel.clone(),
        };
        let worker = dev.clone();
        std::thread::spawn(move || {
            let r = worker.call_stream(&call, &request, &mut sink);
            let _ = tx.send(Piece::End(r));
        });
        let fd = conn.as_raw_fd();
        let head = Reply {
            v: 1,
            ok: true,
            stream: true,
            ..Default::default()
        };
        let mut head = serde_json::to_vec(&head).unwrap_or_default();
        head.push(b'\n');
        let mut sent = 0usize;
        let mut head_sent = false;
        // Ok(()): a clean end. Err(e): the errno; after bytes it is the
        // close without the 0 chunk.
        let result: Result<(), i32> = loop {
            match rx.recv_timeout(WATCH) {
                Ok(Piece::Bytes(b)) => {
                    let wrote = if head_sent {
                        chunk(out, &b)
                    } else {
                        head_sent = true;
                        out.write_all(&head).and_then(|()| chunk(out, &b))
                    };
                    if wrote.is_err() {
                        cancel.store(true, Ordering::SeqCst);
                        break Err(libc::ECANCELED);
                    }
                    sent += b.len();
                    continue;
                }
                Ok(Piece::End(_)) if too_large.load(Ordering::SeqCst) => break Err(libc::EMSGSIZE),
                Ok(Piece::End(Ok(()))) => {
                    if !head_sent {
                        head_sent = true;
                        let _ = out.write_all(&head);
                    }
                    break chunk(out, &[]).map_err(|_| libc::ECANCELED);
                }
                Ok(Piece::End(Err(_))) if Instant::now() >= deadline => break Err(libc::ETIMEDOUT),
                Ok(Piece::End(Err(e))) => break Err(e.errno()),
                Err(RecvTimeoutError::Disconnected) => break Err(libc::EIO),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= deadline {
                cancel.store(true, Ordering::SeqCst);
                break Err(libc::ETIMEDOUT);
            }
            if crate::platform::peer_gone(fd) {
                cancel.store(true, Ordering::SeqCst);
                break Err(libc::ECANCELED);
            }
        };
        let outcome = match &result {
            Ok(()) => format!("{sent} bytes streamed"),
            Err(e) if cancel.load(Ordering::SeqCst) && *e != libc::ETIMEDOUT => {
                format!("cancelled after {sent} bytes")
            }
            Err(e) => format!("error {e} after {sent} bytes"),
        };
        log::info!(
            "sandbox {}: device {}: request of {req_len} bytes, {outcome}, {} ms",
            self.sandbox,
            dev.name(),
            start.elapsed().as_millis()
        );
        match result {
            Ok(()) => Ok(()),
            // The answer line went out: the close says the rest.
            Err(_) if head_sent => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// A device socket that serves. Dropping it stops the server and removes
/// the socket.
pub struct Serving {
    pub path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept. When the socket is gone already, the thread
        // cannot be woken: leave it, it holds nothing but the listener.
        let woke = UnixStream::connect(&self.path).is_ok();
        if let Some(t) = self.thread.take().filter(|_| woke) {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Serve `server` on a new socket at `path`, mode 0600.
pub fn serve(path: &Path, server: Arc<Server>) -> Result<Serving> {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::remove_file(path);
    let l =
        UnixListener::bind(path).with_context(|| format!("cannot listen on {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::spawn(move || {
        for conn in l.incoming() {
            if s2.load(Ordering::SeqCst) {
                return;
            }
            match conn {
                Ok(c) => server.accept(c),
                // Out of files (EMFILE, ENFILE) or a connection that went
                // away: wait a little instead of spinning on the error.
                Err(e) => {
                    log::debug!("devices socket accept: {e}");
                    std::thread::sleep(ACCEPT_BACKOFF);
                }
            }
        }
    });
    Ok(Serving {
        path: path.to_path_buf(),
        stop,
        thread: Some(thread),
    })
}

/// The client side: send one request on `conn` and read the answer. The
/// error is the errno of the answer, or of the connection.
pub fn request(conn: UnixStream, req: &Request, body: &[u8]) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    request_into(conn, req, body, &mut |b| {
        out.extend_from_slice(b);
        Ok(())
    })
    .map(|_| out)
    .map_err(|p| p.errno)
}

/// A request that failed: the errno, and how many response bytes came
/// before it (0 when none did).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partial {
    pub errno: i32,
    pub after: usize,
}

/// `request`, with the response given to `sink` as it comes. A `stream`
/// request gets chunks; a collected answer (an older server, or a request
/// without `stream`) comes whole. A failed `sink` ends the request with
/// its error. Ok: how many bytes came.
pub fn request_into(
    conn: UnixStream,
    req: &Request,
    body: &[u8],
    sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<usize, Partial> {
    let w = conn.try_clone().map_err(|e| Partial {
        errno: e.raw_os_error().unwrap_or(libc::EIO),
        after: 0,
    })?;
    request_io(conn, w, req, body, sink)
}

/// `request_into` over any transport: a reader and a writer, for example
/// the pipes of `ssh HOST paddock device serve-remote` (forward.rs). A
/// transport that closes before the answer line is `EIO`; after the first
/// byte of a stream it is `EIO` with `after` set.
pub fn request_io(
    r: impl Read,
    mut w: impl Write,
    req: &Request,
    body: &[u8],
    sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<usize, Partial> {
    let io = |e: std::io::Error| match e.raw_os_error() {
        // A closed pipe or socket is the transport gone: EIO, not EPIPE.
        Some(libc::EPIPE) | Some(libc::ECONNRESET) | None => libc::EIO,
        Some(n) => n,
    };
    let before = |errno: i32| Partial { errno, after: 0 };
    let mut line = serde_json::to_vec(req).map_err(|_| before(libc::EINVAL))?;
    line.push(b'\n');
    line.extend_from_slice(body);
    // The server can answer before it reads the body (EFBIG): a failed
    // write still reads the answer.
    let wrote = w.write_all(&line).and_then(|_| w.flush());
    let mut r = BufReader::new(r);
    let reply: Reply = match read_line(&mut r).and_then(|l| serde_json::from_slice(&l).ok()) {
        Some(reply) => reply,
        None => return Err(before(wrote.err().map(io).unwrap_or(libc::EIO))),
    };
    if !reply.ok {
        return Err(before(reply.errno.unwrap_or(libc::EIO)));
    }
    if !reply.stream {
        let size = reply.size.unwrap_or(0) as usize;
        if size > CLIENT_MAX_RESPONSE {
            return Err(before(libc::EMSGSIZE));
        }
        let mut out = vec![0u8; size];
        r.read_exact(&mut out).map_err(|e| before(io(e)))?;
        sink(&out).map_err(|e| before(io(e)))?;
        return Ok(size);
    }
    let mut got = 0usize;
    let mut buf = Vec::new();
    loop {
        let after = |errno: i32| Partial { errno, after: got };
        let mut len = [0u8; 4];
        // A close here, without the 0 chunk, is a failure after the
        // first byte: EIO, as the read of the file fails on Linux.
        r.read_exact(&mut len).map_err(|_| after(libc::EIO))?;
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 {
            return Ok(got);
        }
        if got + len > CLIENT_MAX_RESPONSE {
            return Err(after(libc::EMSGSIZE));
        }
        buf.resize(len, 0);
        r.read_exact(&mut buf).map_err(|_| after(libc::EIO))?;
        sink(&buf).map_err(|e| after(io(e)))?;
        got += len;
    }
}

/// Connect to the device socket at `path` and send one request.
pub fn request_at(path: &Path, req: &Request, body: &[u8]) -> Result<Vec<u8>, i32> {
    let conn = UnixStream::connect(path).map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))?;
    request(conn, req, body)
}

#[cfg(test)]
#[path = "socket_tests.rs"]
mod tests;
