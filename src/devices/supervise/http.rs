//! The small HTTP/1.1 server of the supervisor, on `httparse`.
//!
//! One thread per connection. `GET /v1/health` and `GET /v1/catalog` go to
//! the handler at once. `POST /v1/call` takes a slot: at most `concurrency`
//! requests at work and `QUEUE` connections waiting, `429` beyond. A
//! request body larger than `max_request` gets `413` before it is read.

use super::Log;
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Room for the request line and the headers.
pub const HEAD_ROOM: usize = crate::devices::http::HEAD_ROOM;
const MAX_HEADERS: usize = 64;
/// Connections that wait for a slot.
pub const QUEUE: usize = 16;
/// How long the server waits for the head and the body of a request.
const READ_WAIT: Duration = Duration::from_secs(30);

/// One request.
#[derive(Debug, Clone, Default)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
    }
}

/// A response with a body in memory.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn body(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    /// An answer of the supervisor itself, marked with `Paddock-Status`.
    pub fn paddock(status: u16, paddock: &str, text: &str) -> Self {
        Self::body(status, format!("{text}\n").into_bytes())
            .header("Paddock-Status", paddock)
            .header("Content-Type", "text/plain")
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// The head: status line and headers, `Content-Length`, `Connection:
    /// close`, the empty line.
    pub fn head(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        for (n, v) in &self.headers {
            out.push_str(&format!("{n}: {v}\r\n"));
        }
        out.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            self.body.len()
        ));
        out.into_bytes()
    }

    pub fn write(&self, w: &mut dyn Write) -> std::io::Result<()> {
        w.write_all(&self.head())?;
        w.write_all(&self.body)?;
        w.flush()
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Read one request: the head with httparse, then a `Content-Length` body
/// of at most `max_body` bytes. Err: the reply to send instead.
pub fn read_request(conn: &mut UnixStream, max_body: usize) -> Result<Req, Reply> {
    let _ = conn.set_read_timeout(Some(READ_WAIT));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let (req, rest) = loop {
        let n = match conn.read(&mut chunk) {
            Ok(0) => {
                return Err(Reply::paddock(
                    400,
                    "bad request",
                    "the request ended early",
                ))
            }
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Reply::paddock(408, "timeout", "no request within 30 s")),
        };
        buf.extend_from_slice(&chunk[..n]);
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut r = httparse::Request::new(&mut headers);
        match r.parse(&buf) {
            Ok(httparse::Status::Complete(at)) => {
                let req = Req {
                    method: r.method.unwrap_or("").to_string(),
                    path: r.path.unwrap_or("").to_string(),
                    headers: r
                        .headers
                        .iter()
                        .map(|h| {
                            (
                                h.name.to_string(),
                                String::from_utf8_lossy(h.value).into_owned(),
                            )
                        })
                        .collect(),
                    body: Vec::new(),
                };
                break (req, buf.split_off(at));
            }
            Ok(httparse::Status::Partial) if buf.len() > HEAD_ROOM => {
                return Err(Reply::paddock(
                    400,
                    "bad request",
                    "the request head is too large",
                ))
            }
            Ok(httparse::Status::Partial) => {}
            Err(_) => return Err(Reply::paddock(400, "bad request", "not an HTTP request")),
        }
    };
    let mut req = req;
    if req
        .header("transfer-encoding")
        .is_some_and(|v| !v.is_empty())
    {
        return Err(Reply::paddock(
            411,
            "bad request",
            "send Content-Length, not Transfer-Encoding",
        ));
    }
    let len: usize = match req.header("content-length") {
        None => 0,
        Some(v) => match v.parse() {
            Ok(n) => n,
            Err(_) => return Err(Reply::paddock(400, "bad request", "bad Content-Length")),
        },
    };
    if len > max_body {
        return Err(Reply::paddock(
            413,
            "request too large",
            &format!("the request has {len} bytes; the limit is {max_body}"),
        ));
    }
    let mut body = rest;
    while body.len() < len {
        let n = match conn.read(&mut chunk) {
            Ok(0) => return Err(Reply::paddock(400, "bad request", "the body ended early")),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Reply::paddock(408, "timeout", "the body did not arrive")),
        };
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    req.body = body;
    Ok(req)
}

/// The slots of `concurrency` and the bounded queue.
pub struct Slots {
    state: Mutex<(usize, usize)>,
    cv: Condvar,
    max: usize,
    queue: usize,
}

pub struct SlotGuard<'a>(&'a Slots);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let mut g = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        g.0 -= 1;
        self.0.cv.notify_one();
    }
}

impl Slots {
    pub fn new(max: usize, queue: usize) -> Self {
        Self {
            state: Mutex::new((0, 0)),
            cv: Condvar::new(),
            max: max.max(1),
            queue,
        }
    }

    /// A slot now, or after a wait of at most `wait` in the queue. None:
    /// the queue is full or the wait passed.
    pub fn acquire(&self, wait: Duration) -> Option<SlotGuard<'_>> {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 < self.max {
            g.0 += 1;
            return Some(SlotGuard(self));
        }
        if g.1 >= self.queue {
            return None;
        }
        g.1 += 1;
        let end = Instant::now() + wait;
        let got = loop {
            if g.0 < self.max {
                break true;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break false;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        g.1 -= 1;
        if got {
            g.0 += 1;
            Some(SlotGuard(self))
        } else {
            None
        }
    }

    pub fn running(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).0
    }
}

/// What a `call` handler reports for the log line.
pub struct Done {
    pub status: u16,
    pub bytes_out: usize,
}

/// The shape behind the server.
pub trait Handler: Send + Sync {
    fn health(&self) -> Reply;
    fn catalog(&self) -> Reply;
    /// Answer a `POST /v1/call`: the handler writes the response itself.
    fn call(&self, conn: &mut UnixStream, req: &Req, id: u64) -> Done;
    /// Kill the children.
    fn shutdown(&self) {}
}

pub struct Server {
    listener: UnixListener,
    handler: Arc<dyn Handler>,
    slots: Arc<Slots>,
    log: Arc<Log>,
    max_request: usize,
    timeout: Duration,
    next: AtomicU64,
}

impl Server {
    pub fn bind(opts: &super::Options, handler: Arc<dyn Handler>, log: Arc<Log>) -> Result<Self> {
        let path: &Path = &opts.socket;
        if path.exists() {
            std::fs::remove_file(path)
                .with_context(|| format!("cannot remove the old socket {}", path.display()))?;
        }
        let listener = UnixListener::bind(path)
            .with_context(|| format!("cannot listen on {}", path.display()))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            handler,
            slots: Arc::new(Slots::new(opts.concurrency as usize, QUEUE)),
            log,
            max_request: opts.max_request,
            timeout: Duration::from_secs(opts.timeout),
            next: AtomicU64::new(1),
        })
    }

    /// Accept until `stop` is set.
    pub fn run(self: &Arc<Self>, stop: &AtomicBool) {
        let fd = self.listener.as_raw_fd();
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
            match self.listener.accept() {
                Ok((conn, _)) => {
                    let s = self.clone();
                    let _ = std::thread::Builder::new()
                        .name("device-serve-conn".into())
                        .spawn(move || s.connection(conn));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    self.log.line(&format!("accept failed: {e}"));
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    fn connection(&self, mut conn: UnixStream) {
        let _ = conn.set_nonblocking(false);
        let req = match read_request(&mut conn, self.max_request) {
            Ok(r) => r,
            Err(reply) => {
                self.log.line(&format!(
                    "request refused: {} {}",
                    reply.status,
                    reason(reply.status)
                ));
                let _ = reply.write(&mut conn);
                return;
            }
        };
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let start = Instant::now();
        let caller = req.header("paddock-caller").unwrap_or("-").to_string();
        let (status, out) = match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/v1/health") => {
                let r = self.handler.health();
                let _ = r.write(&mut conn);
                (r.status, r.body.len())
            }
            ("GET", "/v1/catalog") => {
                let r = self.handler.catalog();
                let _ = r.write(&mut conn);
                (r.status, r.body.len())
            }
            ("POST", "/v1/call") => {
                let wait = req
                    .header("paddock-deadline")
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(self.timeout)
                    .min(self.timeout);
                match self.slots.acquire(wait) {
                    Some(_slot) => {
                        let d = self.handler.call(&mut conn, &req, id);
                        (d.status, d.bytes_out)
                    }
                    None => {
                        let r = Reply::paddock(429, "busy", "the device is busy");
                        let _ = r.write(&mut conn);
                        (429, r.body.len())
                    }
                }
            }
            _ => {
                let r =
                    Reply::paddock(404, "not found", "paths: /v1/health, /v1/catalog, /v1/call");
                let _ = r.write(&mut conn);
                (404, r.body.len())
            }
        };
        self.log.line(&format!(
            "req {id} caller {} {} {} status {status} in {} out {out} ms {}",
            crate::broker::clean_text(&caller),
            req.method,
            req.path,
            req.body.len(),
            start.elapsed().as_millis()
        ));
    }
}
