//! The client of a device program: HTTP/1.1 over a Unix socket.
//!
//! The broker reaches every out-of-process device program through this one
//! client, whatever its shape. Behind the socket is `paddock device-serve`
//! (supervise.rs), which forks an `exec` program per request or proxies to a
//! `serve` program, or a `serve` program on the host.
//!
//! One request is one connection: `POST /v1/call` with the headers of the
//! device protocol, a `Content-Length` body, `Connection: close`. The headers
//! come from the `Call` and the `DeviceCtx` of the broker, never from the
//! request bytes. The response status maps to a `DevError`, the body goes to
//! a `Sink` as it arrives, so a caller can stream it.
//!
//! Two enforcers of every limit: the supervisor stops at `max_response`
//! and at `timeout`, and this client does the same, so a supervisor bug
//! cannot flood or hang the broker.

use super::config::{DeviceDef, Shape};
use super::{Call, DevError};
use anyhow::{bail, Result};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

mod caller;
pub use caller::{caller_mount, Caller};

/// The device-facing protocol that this paddock speaks.
pub const PROTOCOL: u32 = 1;
/// Room for the status line and the headers of a response.
pub const HEAD_ROOM: usize = super::http::HEAD_ROOM;
/// The longest header value the client sends (a project path can be long,
/// nothing else is).
pub const MAX_HEADER_VALUE: usize = 1024;
/// How often a waiting read checks for a cancel.
const READ_TICK: Duration = Duration::from_millis(100);
/// How long the client waits for the supervisor's answer after the
/// program's own `timeout`.
const GRACE: Duration = Duration::from_secs(2);

/// The reference script: a device that upper-cases its request, as an `exec`
/// and as a `serve` program.
pub const EXAMPLE: &str = r#"#!/usr/bin/env python3
# A device that upper-cases its request. exec: stdin to stdout.
# serve: an HTTP server on $PADDOCK_DEVICE_SOCKET.
import os, sys, socketserver
from http.server import BaseHTTPRequestHandler
sock = os.environ.get("PADDOCK_DEVICE_SOCKET")
if not sock:
    sys.stdout.buffer.write(sys.stdin.buffer.read().upper()); sys.exit(0)
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass  # a Unix socket has no client address
    def do_GET(self):
        self.send_response(200); self.end_headers()
    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(n).upper()
        self.send_response(200); self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)
class S(socketserver.UnixStreamServer): pass
S(sock, H).serve_forever()
"#;

/// Where the response bytes go, as they arrive.
pub trait Sink {
    fn push(&mut self, bytes: &[u8]);
}

/// The simplest sink: the whole response in memory.
#[derive(Default)]
pub struct Collect(pub Vec<u8>);

impl Sink for Collect {
    fn push(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// What one call gave, beside the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The HTTP status of the response.
    pub http: u16,
    /// The status for the audit line: the envelope status of an `exec`
    /// program (`exit 0`, `timeout`), else the HTTP status.
    pub status: String,
    /// The body bytes that reached the sink.
    pub bytes: usize,
    /// `Paddock-Cost`, in micro units.
    pub cost: Option<u64>,
}

/// Why a call failed: the errno for the reader and the status for the
/// audit line (`timeout`, `503`, `bad response`, `cancelled`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub error: DevError,
    pub status: String,
    /// The HTTP status, when a response head arrived.
    pub http: Option<u16>,
}

impl Failure {
    pub fn new(error: DevError, status: &str) -> Self {
        Self {
            error,
            status: status.to_string(),
            http: None,
        }
    }
}

impl From<Failure> for DevError {
    fn from(f: Failure) -> Self {
        f.error
    }
}

/// One device program behind a socket.
pub struct Program {
    socket: PathBuf,
    caller: Caller,
    next: AtomicU64,
}

/// One header value: no control characters, at most `MAX_HEADER_VALUE`
/// bytes, cut at a character boundary.
pub fn header_value(s: &str) -> String {
    let clean: String = crate::broker::clean_text(s)
        .chars()
        .filter(|c| *c != '\n' && *c != '\t')
        .collect();
    if clean.len() <= MAX_HEADER_VALUE {
        return clean;
    }
    let mut end = MAX_HEADER_VALUE;
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean[..end].to_string()
}

impl Program {
    /// The program behind `socket`. The folder must exist: it is the
    /// instance folder that the broker made.
    pub fn connect(socket: &Path) -> Result<Self> {
        let Some(dir) = socket.parent() else {
            bail!("{}: a socket path needs a folder", socket.display());
        };
        if !dir.is_dir() {
            bail!(
                "the instance folder {} does not exist; the device program cannot be reached",
                dir.display()
            );
        }
        Ok(Self {
            socket: socket.to_path_buf(),
            caller: Caller::default(),
            next: AtomicU64::new(1),
        })
    }

    pub fn with_caller(mut self, caller: Caller) -> Self {
        self.caller = caller;
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The headers of one request, in order.
    pub fn headers(&self, call: &Call, def: &DeviceDef, body: usize) -> Vec<(String, String)> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let left = call
            .deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .min(def.timeout());
        let c = &self.caller;
        let mut h = vec![
            ("Host".to_string(), "paddock".to_string()),
            ("Paddock-Protocol".to_string(), PROTOCOL.to_string()),
            ("Paddock-Device".to_string(), header_value(&c.device)),
            ("Paddock-Caller".to_string(), header_value(&c.sandbox)),
            (
                "Paddock-Caller-Harness".to_string(),
                header_value(&c.harness),
            ),
            (
                "Paddock-Caller-Project".to_string(),
                header_value(&c.project),
            ),
        ];
        if let Some(m) = &c.mount {
            h.push(("Paddock-Caller-Mount".to_string(), header_value(m)));
        }
        if !c.chain.is_empty() {
            h.push((
                "Paddock-Caller-Chain".to_string(),
                header_value(&c.chain.join(",")),
            ));
        }
        if let Some(t) = &c.table {
            h.push(("Paddock-Table".to_string(), header_value(t)));
        }
        h.push(("Paddock-Request".to_string(), id.to_string()));
        h.push(("Paddock-Deadline".to_string(), left.to_string()));
        h.push((
            "Paddock-Max-Response".to_string(),
            def.limits.max_response.to_string(),
        ));
        h.push(("Content-Length".to_string(), body.to_string()));
        h.push(("Connection".to_string(), "close".to_string()));
        h
    }

    /// One request. The body of the response goes to `sink` as it comes.
    pub fn call(
        &self,
        call: &Call,
        def: &DeviceDef,
        req: &[u8],
        sink: &mut dyn Sink,
    ) -> Result<Outcome, Failure> {
        let end = (Instant::now() + Duration::from_secs(def.timeout()) + GRACE).min(call.deadline);
        let mut s = match UnixStream::connect(&self.socket) {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "device {}: cannot connect to {}: {e}",
                    self.caller.device,
                    self.socket.display()
                );
                return Err(Failure::new(DevError::Failed, "unreachable"));
            }
        };
        let mut head = String::from("POST /v1/call HTTP/1.1\r\n");
        for (n, v) in self.headers(call, def, req.len()) {
            head.push_str(&format!("{n}: {v}\r\n"));
        }
        head.push_str("\r\n");
        let _ = s.set_write_timeout(Some(Duration::from_secs(def.timeout()).max(GRACE)));
        // The server can answer before it read the body (413): a failed
        // write is not the end, the response is.
        let _ = s.write_all(head.as_bytes()).and_then(|()| s.write_all(req));
        let limit = match def.shape {
            Shape::Exec => def.limits.max_response + HEAD_ROOM,
            _ => def.limits.max_response,
        };
        let mut reader = Response::new(limit);
        let _ = s.set_read_timeout(Some(READ_TICK));
        let mut chunk = [0u8; 64 * 1024];
        loop {
            if call.cancelled() {
                return Err(Failure::new(DevError::Cancelled, "cancelled"));
            }
            if Instant::now() >= end {
                return Err(reader.fail(DevError::Failed, "timeout"));
            }
            match s.read(&mut chunk) {
                Ok(0) => return reader.finish(sink, def),
                Ok(n) => match reader.feed(&chunk[..n], sink) {
                    Ok(Progress::More) => {}
                    Ok(Progress::Done) => return reader.finish(sink, def),
                    Err(e) => return Err(reader.fail(e, reader.failure())),
                },
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return reader.finish(sink, def),
            }
        }
    }

    /// One small request without a body: the status, the headers and the
    /// body, or the error text.
    pub fn get(&self, path: &str, wait: Duration) -> Result<Answer, String> {
        let mut s = UnixStream::connect(&self.socket).map_err(|e| e.to_string())?;
        let _ = s.set_write_timeout(Some(wait.max(Duration::from_secs(1))));
        s.write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: paddock\r\nPaddock-Protocol: {PROTOCOL}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        let _ = s.set_read_timeout(Some(wait.max(Duration::from_millis(100))));
        let mut buf = Vec::new();
        let end = Instant::now() + wait.max(Duration::from_millis(100));
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > HEAD_ROOM + (1 << 20) {
                        return Err("the answer is too large".into());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    if Instant::now() >= end {
                        return Err(format!("no answer: {e}"));
                    }
                }
            }
        }
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut resp = httparse::Response::new(&mut headers);
        match resp.parse(&buf) {
            Ok(httparse::Status::Complete(n)) => {
                let code = resp.code.unwrap_or(0);
                let hs = resp
                    .headers
                    .iter()
                    .map(|h| {
                        (
                            h.name.to_string(),
                            String::from_utf8_lossy(h.value).into_owned(),
                        )
                    })
                    .collect();
                Ok((code, hs, buf[n..].to_vec()))
            }
            _ => Err("no HTTP response".into()),
        }
    }

    /// Wait up to `wait` for `GET /v1/health` to answer 200. The error
    /// names why the program is not usable.
    pub fn health(&self, wait: Duration) -> Result<(), String> {
        let end = Instant::now() + wait;
        let mut last;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.get("/v1/health", left.min(Duration::from_secs(2))) {
                Ok((code, headers, _)) => {
                    if let Some(min) = header(&headers, "paddock-protocol-min")
                        .and_then(|v| v.trim().parse::<u32>().ok())
                    {
                        if min > PROTOCOL {
                            return Err(format!(
                                "the device program needs protocol {min}; this paddock speaks {PROTOCOL}"
                            ));
                        }
                    }
                    if code == 200 {
                        return Ok(());
                    }
                    last = match header(&headers, "paddock-status") {
                        Some(s) => format!("health answered {code} ({s})"),
                        None => format!("health answered {code}"),
                    };
                    if header(&headers, "paddock-status") == Some("failed") {
                        return Err(last);
                    }
                }
                Err(e) => last = e,
            }
            if Instant::now() >= end {
                return Err(format!(
                    "did not answer /v1/health within {} s: {last}",
                    wait.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `GET /v1/catalog`: the JSON object of the program, when it has one.
    pub fn catalog(&self) -> Option<serde_json::Value> {
        let (code, _, body) = self.get("/v1/catalog", Duration::from_secs(2)).ok()?;
        if code != 200 {
            return None;
        }
        let v: serde_json::Value = serde_json::from_slice(&body).ok()?;
        v.is_object().then_some(v)
    }
}

const MAX_HEADERS: usize = 64;

/// A small answer: the status, the headers and the body.
pub type Answer = (u16, Vec<(String, String)>, Vec<u8>);

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// What `Response::feed` reports.
#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    More,
    Done,
}

/// How the body ends.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Framing {
    /// `Content-Length`: this many bytes left.
    Length(usize),
    /// `Transfer-Encoding: chunked`.
    Chunked(Chunked),
    /// No length: the body ends with the connection.
    UntilClose,
    /// No body (204).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Chunked {
    /// Waiting for a size line.
    Size,
    /// Inside a chunk: bytes left.
    Data(usize),
    /// After a chunk: the CRLF.
    AfterData,
    /// After the last chunk: trailer lines until an empty one.
    Trailers,
}

/// The parser of one response, fed as bytes arrive.
pub struct Response {
    limit: usize,
    buf: Vec<u8>,
    head: Option<Head>,
    framing: Framing,
    bytes: usize,
    /// The first bytes of the body, for the envelope status.
    first: Vec<u8>,
    trailers: Vec<(String, String)>,
    done: bool,
    /// A broken response: the status for the audit.
    bad: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct Head {
    code: u16,
    headers: Vec<(String, String)>,
}

impl Response {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            buf: Vec::new(),
            head: None,
            framing: Framing::UntilClose,
            bytes: 0,
            first: Vec::new(),
            trailers: Vec::new(),
            done: false,
            bad: None,
        }
    }

    /// More bytes from the socket.
    pub fn feed(&mut self, data: &[u8], sink: &mut dyn Sink) -> Result<Progress, DevError> {
        if self.done {
            return Ok(Progress::Done);
        }
        if self.head.is_none() {
            self.buf.extend_from_slice(data);
            let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
            let mut resp = httparse::Response::new(&mut headers);
            // The head must be complete within HEAD_ROOM bytes. The body
            // can follow it in the same read: a fast program's first 64
            // KiB arrive at once, and the head is only its first lines.
            let n = match resp.parse(&self.buf) {
                Ok(httparse::Status::Complete(n)) if n <= HEAD_ROOM => n,
                Ok(httparse::Status::Partial) if self.buf.len() <= HEAD_ROOM => {
                    return Ok(Progress::More)
                }
                Ok(_) => {
                    self.bad = Some("bad response");
                    return Err(DevError::Failed);
                }
                Err(_) => {
                    self.bad = Some("bad response");
                    return Err(DevError::Failed);
                }
            };
            let code = resp.code.unwrap_or(0);
            let hs: Vec<(String, String)> = resp
                .headers
                .iter()
                .map(|h| {
                    (
                        h.name.to_string(),
                        String::from_utf8_lossy(h.value).into_owned(),
                    )
                })
                .collect();
            self.framing = if code == 204 || code == 304 || (100..200).contains(&code) {
                Framing::None
            } else if header(&hs, "transfer-encoding")
                .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
            {
                Framing::Chunked(Chunked::Size)
            } else if let Some(l) = header(&hs, "content-length") {
                match l.parse::<usize>() {
                    Ok(l) => Framing::Length(l),
                    Err(_) => {
                        self.bad = Some("bad response");
                        return Err(DevError::Failed);
                    }
                }
            } else {
                Framing::UntilClose
            };
            self.head = Some(Head { code, headers: hs });
            let rest = self.buf.split_off(n);
            self.buf.clear();
            return self.body(&rest, sink);
        }
        self.body(data, sink)
    }

    fn push(&mut self, data: &[u8], sink: &mut dyn Sink) -> Result<(), DevError> {
        if data.is_empty() {
            return Ok(());
        }
        self.bytes += data.len();
        if self.bytes > self.limit {
            self.bad = Some("too large");
            return Err(DevError::TooLarge);
        }
        if self.first.len() < 64 {
            let take = (64 - self.first.len()).min(data.len());
            self.first.extend_from_slice(&data[..take]);
        }
        // The body of a status that fails the call (a 400, a 503) is the
        // envelope or the message that explains it, not the device's
        // answer: it stays out of the reader, so the reader's first read
        // fails with the errno of the status (EINVAL for rejected) and not
        // with a failure after bytes it never asked for.
        if self.delivers() {
            sink.push(data);
        }
        Ok(())
    }

    /// The head says that the call is a success so far: its body is the
    /// response and goes to the sink as it comes.
    fn delivers(&self) -> bool {
        self.head
            .as_ref()
            .is_some_and(|h| map_status(h.code, header(&h.headers, "paddock-status")).is_ok())
    }

    fn body(&mut self, data: &[u8], sink: &mut dyn Sink) -> Result<Progress, DevError> {
        match self.framing.clone() {
            Framing::None => {
                self.done = true;
                Ok(Progress::Done)
            }
            Framing::UntilClose => {
                self.push(data, sink)?;
                Ok(Progress::More)
            }
            Framing::Length(left) => {
                let take = left.min(data.len());
                self.push(&data[..take], sink)?;
                let left = left - take;
                self.framing = Framing::Length(left);
                if left == 0 {
                    self.done = true;
                    return Ok(Progress::Done);
                }
                Ok(Progress::More)
            }
            Framing::Chunked(_) => self.chunked(data, sink),
        }
    }

    fn chunked(&mut self, data: &[u8], sink: &mut dyn Sink) -> Result<Progress, DevError> {
        self.buf.extend_from_slice(data);
        loop {
            let Framing::Chunked(state) = self.framing.clone() else {
                unreachable!()
            };
            match state {
                Chunked::Data(left) => {
                    let take = left.min(self.buf.len());
                    let piece: Vec<u8> = self.buf.drain(..take).collect();
                    self.push(&piece, sink)?;
                    let left = left - take;
                    self.framing = Framing::Chunked(if left == 0 {
                        Chunked::AfterData
                    } else {
                        Chunked::Data(left)
                    });
                    if left > 0 {
                        return Ok(Progress::More);
                    }
                }
                Chunked::AfterData => {
                    if self.buf.starts_with(b"\r\n") {
                        self.buf.drain(..2);
                    } else if self.buf.starts_with(b"\n") {
                        self.buf.drain(..1);
                    } else if self.buf.is_empty() || self.buf == b"\r" {
                        return Ok(Progress::More);
                    } else {
                        self.bad = Some("bad response");
                        return Err(DevError::Failed);
                    }
                    self.framing = Framing::Chunked(Chunked::Size);
                }
                Chunked::Size => {
                    let Some(eol) = self.buf.iter().position(|b| *b == b'\n') else {
                        if self.buf.len() > HEAD_ROOM {
                            self.bad = Some("bad response");
                            return Err(DevError::Failed);
                        }
                        return Ok(Progress::More);
                    };
                    let line: Vec<u8> = self.buf.drain(..=eol).collect();
                    let text = String::from_utf8_lossy(&line);
                    let size = text.trim_end_matches(['\r', '\n']);
                    let size = size.split(';').next().unwrap_or("").trim();
                    let Ok(size) = usize::from_str_radix(size, 16) else {
                        self.bad = Some("bad response");
                        return Err(DevError::Failed);
                    };
                    self.framing = Framing::Chunked(if size == 0 {
                        Chunked::Trailers
                    } else {
                        Chunked::Data(size)
                    });
                }
                Chunked::Trailers => {
                    let Some(eol) = self.buf.iter().position(|b| *b == b'\n') else {
                        if self.buf.len() > HEAD_ROOM {
                            self.bad = Some("bad response");
                            return Err(DevError::Failed);
                        }
                        return Ok(Progress::More);
                    };
                    let line: Vec<u8> = self.buf.drain(..=eol).collect();
                    let text = String::from_utf8_lossy(&line);
                    let text = text.trim_end_matches(['\r', '\n']);
                    if text.is_empty() {
                        self.done = true;
                        return Ok(Progress::Done);
                    }
                    if let Some((n, v)) = text.split_once(':') {
                        self.trailers
                            .push((n.trim().to_string(), v.trim().to_string()));
                    }
                }
            }
        }
    }

    /// The status text when the call failed: for the audit line.
    pub fn failure(&self) -> &'static str {
        self.bad.unwrap_or("closed")
    }

    /// A failure with the HTTP status when a head arrived.
    pub fn fail(&self, error: DevError, status: &str) -> Failure {
        Failure {
            error,
            status: status.to_string(),
            http: self.head.as_ref().map(|h| h.code),
        }
    }

    /// The connection ended or the body is complete: the outcome.
    pub fn finish(&mut self, _sink: &mut dyn Sink, def: &DeviceDef) -> Result<Outcome, Failure> {
        let Some(head) = self.head.clone() else {
            let why = if self.buf.is_empty() {
                "closed"
            } else {
                "bad response"
            };
            return Err(self.fail(DevError::Failed, why));
        };
        let complete = self.done || matches!(self.framing, Framing::UntilClose | Framing::None);
        if !complete {
            return Err(self.fail(DevError::Failed, "bad response"));
        }
        let code = head.code;
        let paddock = header(&head.headers, "paddock-status").map(|s| s.to_string());
        let cost =
            header(&head.headers, "paddock-cost").and_then(|v| super::ledger::parse_amount(v).ok());
        if let Err((e, text)) = map_status(code, paddock.as_deref()) {
            return Err(self.fail(e, text));
        }
        if let Some(exit) = header(&self.trailers, "paddock-exit") {
            if exit != "0" {
                if header(&self.trailers, "paddock-status") == Some("too large") {
                    return Err(self.fail(DevError::TooLarge, "too large"));
                }
                // The bytes reached the sink; the last read fails.
                return Err(self.fail(DevError::Failed, &stream_status(exit)));
            }
        }
        let status = match (def.shape, def.stream) {
            (Shape::Exec, false) => {
                envelope_status(&self.first).unwrap_or_else(|| code.to_string())
            }
            _ => code.to_string(),
        };
        Ok(Outcome {
            http: code,
            status,
            bytes: self.bytes,
            cost,
        })
    }
}

/// `Paddock-Exit: 3` is `exit 3`; `signal 9` and `timeout` stay.
fn stream_status(exit: &str) -> String {
    match exit.parse::<i32>() {
        Ok(n) => format!("exit {n}"),
        Err(_) => exit.to_string(),
    }
}

/// The `status:` line of an envelope, from its first bytes.
fn envelope_status(first: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(first).ok()?;
    let line = text.lines().next()?;
    line.strip_prefix("status: ").map(|s| s.to_string())
}

/// The status mapping of the device protocol. Err: the errno for the
/// reader and the status text for the audit.
pub fn map_status(code: u16, paddock: Option<&str>) -> Result<(), (DevError, &'static str)> {
    match code {
        200 | 204 => Ok(()),
        202 => Err((DevError::Waiting, "waiting")),
        400 => Err((DevError::Invalid, "rejected")),
        403 => Err((DevError::Denied, "declined")),
        413 => Err((DevError::RequestTooLarge, "request too large")),
        429 => Err((DevError::Busy, "busy")),
        _ => Err(match paddock {
            // A shared instance that reads another table than this run
            // does (devices/shared.rs).
            Some("reconfigured") => (DevError::Failed, super::outproc::RECONFIGURED),
            Some("too large") => (DevError::TooLarge, "too large"),
            Some("failed") => (DevError::Failed, "failed"),
            Some("timeout") => (DevError::Failed, "timeout"),
            Some("restarting") => (DevError::Failed, "restarting"),
            Some("starting") => (DevError::Failed, "starting"),
            _ => (DevError::Failed, status_text(code)),
        }),
    }
}

/// The HTTP status as a static text, for the audit line.
fn status_text(code: u16) -> &'static str {
    match code {
        500 => "500",
        501 => "501",
        502 => "502",
        503 => "503",
        504 => "504",
        404 => "404",
        405 => "405",
        408 => "408",
        411 => "411",
        c if (500..600).contains(&c) => "5xx",
        c if (400..500).contains(&c) => "4xx",
        c if (300..400).contains(&c) => "3xx",
        _ => "bad response",
    }
}

#[cfg(test)]
#[path = "program_tests.rs"]
mod tests;
