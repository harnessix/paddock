//! `kind = "http"` user devices: a loopback proxy. The request is a small
//! HTTP request. paddock checks the method and the path prefix, drops
//! `Authorization`, `Cookie` and hop-by-hop headers, and sends it to
//! `target` only. `target` must be a loopback address.
//!
//! The audit log keeps the method and the path, the status and the
//! duration. Never a body.

use super::config::{methods_ok, token_char, DeviceDef, HttpProxy};
use super::grant::GrantHit;
use super::ledger;
use super::user::Gate;
use super::{Call, DevError, Device, DeviceCtx, Info, Limits};
use crate::broker;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Room for the request line and headers of an HTTP request or response.
pub const HEAD_ROOM: usize = 16 * 1024;
/// Headers of one request.
const MAX_HEADERS: usize = 64;

pub fn info(def: &DeviceDef, p: &HttpProxy) -> Info {
    Info {
        request: format!(
            "an HTTP request as text: the first line METHOD PATH (methods {}; the path starts with one of {}), then header lines NAME: VALUE, an empty line, and the body (at most {} bytes). paddock sets Host, Content-Length and Connection, and drops Authorization, Cookie and hop-by-hop headers.",
            p.methods.join(", "),
            p.paths.join(", "),
            def.limits.max_request
        ),
        response: format!(
            "the HTTP response as text: the status line HTTP/1.1 CODE REASON, header lines, an empty line, and the body (at most {} bytes, chunked bodies decoded). A Paddock-Status header means that paddock answered itself: disabled, refused (method or path not allowed) or unreachable. EACCES: the user declined the device. EDQUOT: the budget of a paid device is used. Lines end with LF.",
            def.limits.max_response
        ),
        limits: super::user::limits(def),
        shape: "http",
        stream: false,
    }
}

/// A response that paddock makes itself, in the format of the device.
pub fn paddock_answer(code: u16, reason: &str, status: &str, text: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {code} {reason}\nPaddock-Status: {status}\nContent-Type: text/plain\nContent-Length: {}\n\n{text}\n",
        text.len() + 1
    )
    .into_bytes()
}

/// A parsed request of an http device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReq {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Parse the request format: `METHOD PATH`, header lines, an empty line,
/// the body. Lines end with LF or CRLF. Without an empty line the whole
/// request is the head and the body is empty.
pub fn parse_http(raw: &[u8], max_body: usize) -> Result<HttpReq, DevError> {
    let (head, body) = match find(raw, b"\n\n")
        .map(|i| (i, 2))
        .into_iter()
        .chain(find(raw, b"\r\n\r\n").map(|i| (i, 4)))
        .min_by_key(|(i, _)| *i)
    {
        Some((i, n)) => (&raw[..i], &raw[i + n..]),
        None => (raw, &raw[raw.len()..]),
    };
    if head.len() > HEAD_ROOM {
        return Err(DevError::Invalid);
    }
    if body.len() > max_body {
        return Err(DevError::TooLarge);
    }
    let head = std::str::from_utf8(head).map_err(|_| DevError::Invalid)?;
    let mut lines = head.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let first = lines.next().unwrap_or("").trim();
    let mut parts = first.split(' ').filter(|p| !p.is_empty());
    let method = parts.next().ok_or(DevError::Invalid)?;
    let path = parts.next().ok_or(DevError::Invalid)?;
    match parts.next() {
        None | Some("HTTP/1.1") | Some("HTTP/1.0") => {}
        Some(_) => return Err(DevError::Invalid),
    }
    if parts.next().is_some() || !methods_ok(method) {
        return Err(DevError::Invalid);
    }
    let mut headers = Vec::new();
    for l in lines {
        if l.is_empty() {
            continue;
        }
        let (n, v) = l.split_once(':').ok_or(DevError::Invalid)?;
        let v = v.trim();
        if n.is_empty()
            || !n.chars().all(token_char)
            || v.chars().any(|c| c.is_control() && c != '\t')
        {
            return Err(DevError::Invalid);
        }
        headers.push((n.to_string(), v.to_string()));
        if headers.len() > MAX_HEADERS {
            return Err(DevError::Invalid);
        }
    }
    Ok(HttpReq {
        method: method.to_string(),
        path: path.to_string(),
        headers,
        body: body.to_vec(),
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A path without tricks that a server could resolve outside a prefix:
/// visible ASCII only, no `.` or `..` segments, no `//`, no encoded dot,
/// slash or backslash, no fragment.
pub(crate) fn path_ok(p: &str) -> bool {
    if !p.starts_with('/') || !p.chars().all(|c| c.is_ascii_graphic()) || p.contains(['#', '\\']) {
        return false;
    }
    let path = p.split('?').next().unwrap_or("");
    let lower = path.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00"]
        .iter()
        .any(|x| lower.contains(x))
        || path.contains("//")
    {
        return false;
    }
    !path.split('/').any(|s| s == "." || s == "..")
}

/// Why the device refuses this request, or None when it may go.
pub fn refusal(req: &HttpReq, methods: &[String], paths: &[String]) -> Option<String> {
    if !methods.contains(&req.method) {
        return Some(format!(
            "the method {} is not allowed; allowed: {}",
            req.method,
            methods.join(", ")
        ));
    }
    if !path_ok(&req.path) {
        return Some("the path has characters or segments that the device does not allow".into());
    }
    let path = req.path.split('?').next().unwrap_or("");
    if !paths.iter().any(|p| path.starts_with(p.as_str())) {
        return Some(format!(
            "the path is outside the allowed prefixes: {}",
            paths.join(", ")
        ));
    }
    None
}

/// Headers that never pass: hop-by-hop headers, credentials of the
/// sandbox, and the ones that paddock sets itself.
const DROP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "proxy-authorization",
    "proxy-authenticate",
    "authorization",
    "cookie",
    "host",
    "content-length",
    "expect",
];

/// The headers that pass, without `DROP` and without the headers that a
/// `Connection` header names.
pub fn strip_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    let named: Vec<String> = headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(',').map(|x| x.trim().to_ascii_lowercase()))
        .collect();
    headers
        .iter()
        .filter(|(n, _)| {
            let l = n.to_ascii_lowercase();
            !DROP.contains(&l.as_str()) && !named.contains(&l)
        })
        .cloned()
        .collect()
}

/// `/dev/paddock/NAME` for an `http` device.
pub struct Http {
    def: DeviceDef,
    proxy: HttpProxy,
    gate: Gate,
}

impl Http {
    pub fn new(def: DeviceDef, ctx: DeviceCtx, grant: GrantHit) -> Self {
        let gate = Gate::new(ctx, &def, grant);
        let proxy = def.http.clone().unwrap_or_else(|| HttpProxy {
            target: SocketAddr::from(([127, 0, 0, 1], 9)),
            methods: Vec::new(),
            paths: Vec::new(),
        });
        Self { def, proxy, gate }
    }

    fn send(&self, call: &Call, target: SocketAddr, req: &HttpReq) -> Result<Vec<u8>, Upstream> {
        let timeout = self.def.timeout();
        let end = Instant::now() + Duration::from_secs(timeout);
        let connect = Duration::from_secs(5).min(Duration::from_secs(timeout));
        let mut s = TcpStream::connect_timeout(&target, connect)
            .map_err(|e| Upstream::Unreachable(format!("cannot connect to {target}: {e}")))?;
        let _ = s.set_nodelay(true);
        let mut out = format!("{} {} HTTP/1.1\r\nHost: {target}\r\n", req.method, req.path);
        for (n, v) in strip_headers(&req.headers) {
            out.push_str(&format!("{n}: {v}\r\n"));
        }
        if !req.body.is_empty() || matches!(req.method.as_str(), "POST" | "PUT" | "PATCH") {
            out.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
        }
        out.push_str("Connection: close\r\n\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&req.body);
        s.set_write_timeout(Some(Duration::from_secs(timeout))).ok();
        s.write_all(&bytes)
            .map_err(|e| Upstream::Unreachable(format!("cannot send to {target}: {e}")))?;
        s.set_read_timeout(Some(Duration::from_millis(200))).ok();
        let limit = self.def.limits.max_response + HEAD_ROOM;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            if let Some(r) = complete(&buf, req.method == "HEAD", false) {
                return r;
            }
            if call.cancelled() {
                return Err(Upstream::Cancelled);
            }
            if Instant::now() >= end {
                return Err(Upstream::NoAnswer(format!(
                    "{target} did not answer in {timeout} seconds"
                )));
            }
            match s.read(&mut chunk) {
                Ok(0) => {
                    return complete(&buf, req.method == "HEAD", true).unwrap_or_else(|| {
                        Err(Upstream::NoAnswer(format!(
                            "{target} closed the connection before a full response"
                        )))
                    })
                }
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > limit {
                        return Err(Upstream::TooLarge);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => {
                    return Err(Upstream::NoAnswer(format!(
                        "cannot read from {target}: {e}"
                    )))
                }
            }
        }
    }
}

/// Why a request to the service failed.
#[derive(Debug)]
pub enum Upstream {
    /// The request never reached the service: it costs nothing.
    Unreachable(String),
    /// The service got the request but gave no usable answer (a timeout,
    /// a closed connection, a broken response): it costs the declared cost.
    NoAnswer(String),
    TooLarge,
    Cancelled,
}

/// The response for the agent when `buf` holds a whole HTTP response, or
/// None when more bytes must come. `eof`: the server closed the connection.
pub(crate) fn complete(
    buf: &[u8],
    head_only: bool,
    eof: bool,
) -> Option<Result<Vec<u8>, Upstream>> {
    let bad = |m: &str| Some(Err(Upstream::NoAnswer(m.to_string())));
    let mut start = 0;
    loop {
        let rest = &buf[start..];
        let end = find(rest, b"\r\n\r\n")
            .map(|i| (i, 4))
            .into_iter()
            .chain(find(rest, b"\n\n").map(|i| (i, 2)))
            .min_by_key(|(i, _)| *i);
        let Some((hend, n)) = end else {
            return if eof {
                bad("the service sent no complete response head")
            } else {
                None
            };
        };
        let head = String::from_utf8_lossy(&rest[..hend]).into_owned();
        let body = &rest[hend + n..];
        let mut lines = head.split('\n').map(|l| l.trim_end_matches('\r'));
        let status = lines.next().unwrap_or("");
        let mut sp = status.splitn(3, ' ');
        let (ver, code) = (sp.next().unwrap_or(""), sp.next().unwrap_or(""));
        let reason = sp.next().unwrap_or("").trim();
        let Ok(code) = code.parse::<u16>() else {
            return bad("the service sent no HTTP status line");
        };
        if !ver.starts_with("HTTP/") {
            return bad("the service sent no HTTP status line");
        }
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
            .collect();
        if (100..200).contains(&code) && code != 101 {
            // An interim response: the real one follows.
            start += hend + n;
            continue;
        }
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        let no_body = head_only || code == 204 || code == 304 || code == 101;
        let body: Vec<u8> = if no_body {
            Vec::new()
        } else if get("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
        {
            match dechunk(body) {
                Some(Ok(b)) => b,
                Some(Err(())) => return bad("the service sent a broken chunked body"),
                None if eof => return bad("the service closed the connection in a chunked body"),
                None => return None,
            }
        } else if let Some(len) = get("content-length") {
            let Ok(len) = len.parse::<usize>() else {
                return bad("the service sent a bad Content-Length");
            };
            if body.len() < len {
                return if eof {
                    bad("the service closed the connection before the end of the body")
                } else {
                    None
                };
            }
            body[..len].to_vec()
        } else if eof {
            body.to_vec()
        } else {
            return None;
        };
        let mut out = format!("HTTP/1.1 {code} {}\n", broker::clean_text(reason).trim());
        for (n, v) in strip_headers(&headers) {
            if n.eq_ignore_ascii_case("set-cookie") || !n.chars().all(token_char) {
                continue;
            }
            let v: String = v.chars().filter(|c| !c.is_control()).collect();
            out.push_str(&format!("{n}: {v}\n"));
        }
        out.push_str(&format!("Content-Length: {}\n\n", body.len()));
        let mut out = out.into_bytes();
        out.extend_from_slice(&body);
        return Some(Ok(out));
    }
}

/// Decode a chunked body: None while it is incomplete.
pub(crate) fn dechunk(mut data: &[u8]) -> Option<Result<Vec<u8>, ()>> {
    let mut out = Vec::new();
    loop {
        let eol = find(data, b"\n")?;
        let line = std::str::from_utf8(&data[..eol]).map_err(|_| ());
        let Ok(line) = line else {
            return Some(Err(()));
        };
        let size = line.trim_end_matches('\r');
        let size = size.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size, 16) else {
            return Some(Err(()));
        };
        data = &data[eol + 1..];
        if size == 0 {
            // Trailers end with an empty line. paddock drops them.
            loop {
                let eol = find(data, b"\n")?;
                let empty = data[..eol].iter().all(|b| *b == b'\r');
                data = &data[eol + 1..];
                if empty {
                    return Some(Ok(out));
                }
            }
        }
        if data.len() < size {
            return None;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        } else if data.starts_with(b"\n") {
            data = &data[1..];
        } else if data.is_empty() || data == b"\r" {
            return None;
        } else {
            return Some(Err(()));
        }
    }
}

impl Device for Http {
    fn name(&self) -> &str {
        &self.def.name
    }
    fn asks(&self, _call: &Call) -> bool {
        self.gate.asks()
    }
    fn info(&self) -> Info {
        info(&self.def, &self.proxy)
    }
    fn details(&self) -> Option<serde_json::Value> {
        let mut v = super::user::details(&self.def);
        self.gate.details(&mut v);
        Some(v)
    }
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let start = Instant::now();
        let req = parse_http(request, self.def.limits.max_request)?;
        // The query can hold private values: the audit keeps the path only.
        let detail = format!(
            "{} {}",
            req.method,
            broker::clean_text(req.path.split('?').next().unwrap_or(""))
        );
        if let Some(why) = refusal(&req, &self.proxy.methods, &self.proxy.paths) {
            self.gate.audit(&detail, "refused", "refused", start);
            return Ok(paddock_answer(403, "Forbidden", "refused", &why));
        }
        let decision = match self.gate.check(call) {
            Ok(d) => d,
            Err(e) => {
                self.gate.audit(&detail, "denied", "denied", start);
                return Err(e);
            }
        };
        let charge = match self.gate.reserve() {
            Ok(c) => c,
            Err(e) => {
                let status = match e {
                    DevError::BudgetUsed => ledger::BUDGET_USED,
                    DevError::Denied => "no budget",
                    _ => "failed",
                };
                self.gate.audit(&detail, &decision, status, start);
                return Err(e);
            }
        };
        match self.send(call, self.proxy.target, &req) {
            Ok(out) => {
                let code = String::from_utf8_lossy(out.get(9..12).unwrap_or_default()).to_string();
                self.gate.settle(charge, Some(&out), true, &code);
                self.gate.audit(&detail, &decision, &code, start);
                Ok(out)
            }
            Err(Upstream::Unreachable(why)) => {
                self.gate.settle(charge, None, false, "unreachable");
                self.gate.audit(&detail, &decision, "unreachable", start);
                Ok(paddock_answer(502, "Bad Gateway", "unreachable", &why))
            }
            Err(Upstream::NoAnswer(why)) => {
                self.gate.settle(charge, None, true, "no answer");
                self.gate.audit(&detail, &decision, "no answer", start);
                Ok(paddock_answer(502, "Bad Gateway", "no answer", &why))
            }
            Err(Upstream::TooLarge) => {
                self.gate.settle(charge, None, true, "too large");
                self.gate.audit(&detail, &decision, "too large", start);
                Err(DevError::TooLarge)
            }
            Err(Upstream::Cancelled) => {
                self.gate.settle(charge, None, true, "cancelled");
                self.gate.audit(&detail, &decision, "cancelled", start);
                Err(DevError::Cancelled)
            }
        }
    }
}

/// The limits of the catalog, with the head room of the HTTP format.
pub(crate) fn catalog_limits(def: &DeviceDef) -> Limits {
    Limits {
        max_request: def.limits.max_request + HEAD_ROOM,
        max_response: def.limits.max_response + HEAD_ROOM,
        deadline_seconds: super::user::APPROVAL_WAIT + def.timeout() + 5,
    }
}
