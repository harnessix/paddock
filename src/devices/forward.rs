//! `run = "remote"`: a device that another paddock serves.
//!
//! ```toml
//! [devices.mac-build]
//! run = "remote"
//! target = "ssh://mac"           # or "unix:/path/to/devices.sock"
//! device = "build"               # the device name on the other side
//! ```
//!
//! `Forward` is the file in `/dev/paddock` on the forwarding side. Each
//! request opens the transport (transport.rs: a Unix socket, or `ssh HOST
//! paddock device serve-remote` with the user's ssh as the user), sends
//! one request in the framing of socket.rs with the caller as text, and
//! maps the answer: the errno of the other side is the errno here, a
//! stream arrives chunk by chunk, a transport that closes before the
//! answer is `EIO`. The local `timeout` is the whole round trip; the other
//! side has its own. A cancel closes the transport, and the other side
//! cancels as a close does there.
//!
//! The ssh transport runs a program on this host as the user, so it is a
//! host action: the card says "connects to HOST with your ssh as you" and
//! shows every session, and `mode = "allow"` is a parse error, as for a
//! host device. A socket transport is a connection, nothing more.
//!
//! The other side (serve.rs, `paddock device serve-remote`) runs the
//! device in its own device sandbox with its own table, card, audit and
//! log, and needs `grant = ["remote:HOST"]` or `remote:any` on that
//! table. Its session answers live in a file (session.rs). The guest of
//! the VM backend will be the same shape with vsock as the transport.

pub mod serve;
pub mod session;
pub mod transport;

pub use transport::Target;

use super::config::{DeviceDef, Mode, Run, Scope, Start};
use super::grant::GrantHit;
use super::socket::{self, Request};
use super::user::Gate;
use super::{Call, DevError, Device, DeviceCtx, Info, RemoteCaller, Sink};
use anyhow::{bail, Context as _, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How often a running request looks at the cancel flag and the deadline.
const WATCH: Duration = Duration::from_millis(50);
/// Chunks of a stream that wait for the reader before the transport
/// waits too.
const STREAM_QUEUE: usize = 16;
/// The wait for the catalog of the other side, after the first request.
const CATALOG_WAIT: Duration = Duration::from_secs(20);

/// The first words of the catalog text and the card of an ssh device.
pub fn ssh_mark(host: &str) -> String {
    format!("Connects to {host} with your ssh as you. ")
}

/// The keys that a remote table cannot have: they belong to the table on
/// the other side, or they make no sense for a forward. Called by the
/// parser after the common keys; `has_command` and `has_sandbox` say
/// whether the table wrote those keys.
pub fn check_table(
    at: &str,
    run: &Run,
    mode: Mode,
    scope: Scope,
    start: Start,
    has_command: bool,
    has_sandbox: bool,
) -> Result<()> {
    let Run::Remote { target, device } = run else {
        return Ok(());
    };
    let target = Target::parse(target).map_err(|e| anyhow::anyhow!("{at}: {e}"))?;
    if has_command {
        bail!(
            "{at}: command is for the table on the other side: run = \"remote\" forwards to the device {device:?} there, and the paddock there runs its own command"
        );
    }
    if has_sandbox {
        bail!(
            "{at}: a [devices.NAME.sandbox] table is for the other side: the paddock at {} runs the device {device:?} in its own device sandbox",
            target.render()
        );
    }
    if scope == Scope::Shared {
        bail!(
            "{at}: scope = \"shared\" is for the table on the other side: a forward has no instance of its own"
        );
    }
    if start == Start::WithSandbox {
        bail!(
            "{at}: start = \"with-sandbox\" is for the table on the other side: a forward opens its transport per request"
        );
    }
    if mode == Mode::Allow && target.host_action() {
        bail!(
            "{at}: mode = \"allow\" is refused for an ssh target: each request runs `ssh {}` on the host as you, so the first-request card shows every session, as for run = \"host\"",
            target.host().unwrap_or_default()
        );
    }
    Ok(())
}

/// The target and the remote device name of `def`.
pub fn target_of(def: &DeviceDef) -> Option<(Target, &str)> {
    match &def.run {
        Run::Remote { target, device } => Some((Target::parse(target).ok()?, device)),
        _ => None,
    }
}

/// The mark of the catalog text and the card: the ssh mark, or nothing
/// for a socket.
fn mark(target: &Target) -> String {
    match target.host() {
        Some(h) => ssh_mark(h),
        None => String::new(),
    }
}

/// What `.catalog` says before the other side's own text is known.
pub fn info(def: &DeviceDef) -> Info {
    let (request, response) = match target_of(def) {
        Some((t, device)) => (
            format!(
                "{}bytes for the device `{device}` of the paddock at {} (at most {} bytes), which runs it there in its own device sandbox and answers as that device does. The whole round trip takes at most {} seconds.",
                mark(&t),
                t.render(),
                def.limits.max_request,
                def.timeout()
            ),
            format!(
                "the bytes that the device `{device}` answers on the other side (at most {} bytes). Its errors arrive unchanged: EINVAL for a request it rejects, EACCES when the user there declined, ETIME when it still waits. EIO: the connection to {} failed (the catalog names the error).",
                def.limits.max_response,
                t.render()
            ),
        ),
        None => ("a request".into(), "a response".into()),
    };
    Info {
        request,
        response,
        limits: super::user::limits(def),
        shape: def.shape.name(),
        stream: def.stream,
    }
}

/// The first line of the card.
pub fn summary(def: &DeviceDef) -> String {
    match target_of(def) {
        Some((Target::Ssh(h), _)) => format!(
            "allow the device `{}` for this sandbox: it connects to {h} with your ssh as you",
            def.name
        ),
        _ => format!("allow the device `{}` for this sandbox", def.name),
    }
}

/// The detail line of the card: what runs here and what the other side
/// does.
pub fn describe(def: &DeviceDef) -> String {
    match target_of(def) {
        Some((Target::Ssh(h), device)) => format!(
            "each request runs `ssh {h} {}` ON THE HOST as you, with your ssh configuration and keys, and the paddock on {h} runs its device `{device}` there, in its own device sandbox, with its own card for you",
            transport::REMOTE_COMMAND.join(" ")
        ),
        Some((t, device)) => format!(
            "each request connects to {} and the paddock there runs its device `{device}`, in its own device sandbox, with its own card for you",
            t.render()
        ),
        None => "a remote device".into(),
    }
}

/// The warning line of `paddock doctor` for an ssh device.
pub fn doctor_line(def: &DeviceDef) -> Option<String> {
    let (t, device) = target_of(def)?;
    Some(match &t {
        Target::Ssh(h) => format!(
            "{}: each request runs `ssh {h}` as you and the paddock there runs its device {device}",
            def.name
        ),
        Target::Unix(p) => format!(
            "{}: each request connects to {} and the paddock there runs its device {device}",
            def.name,
            p.display()
        ),
    })
}

/// The audit fields of a request that another paddock forwarded
/// (`Call::remote`), for the serving side.
pub fn remote_fields(call: &Call, extra: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(r) = &call.remote else {
        return;
    };
    extra.insert("remote_caller".into(), r.sandbox.clone().into());
    extra.insert("remote_host".into(), r.host.clone().into());
    if !r.chain.is_empty() {
        extra.insert("remote_chain".into(), serde_json::json!(r.chain));
    }
}

/// A short word for the audit status of a failed request.
fn status_of(e: DevError) -> &'static str {
    match e {
        DevError::Invalid => "invalid",
        DevError::Failed => "failed",
        DevError::TooLarge => "too large",
        DevError::Cancelled => "cancelled",
        DevError::Waiting => "waiting",
        DevError::Denied => "denied",
        DevError::BudgetUsed => super::ledger::BUDGET_USED,
        DevError::Busy => "busy",
        DevError::RequestTooLarge => "request too large",
        DevError::TimedOut => "timeout",
    }
}

/// A piece of the answer on its way from the transport thread.
enum Piece {
    Bytes(Vec<u8>),
    /// How many bytes came, or the errno and how many bytes came before.
    End(Result<usize, socket::Partial>),
}

/// `/dev/paddock/NAME` for a device of another paddock.
pub struct Forward {
    def: DeviceDef,
    ctx: DeviceCtx,
    gate: Gate,
    target: Target,
    device: String,
    slots: Mutex<u32>,
    cv: Condvar,
    /// The entry of the other side's catalog, once asked. The thread
    /// that asks holds the other reference.
    catalog: Arc<Mutex<Option<serde_json::Value>>>,
    catalog_asked: AtomicBool,
    /// The last transport failure, for the catalog.
    last_error: Mutex<Option<String>>,
}

impl Forward {
    pub fn new(def: DeviceDef, ctx: DeviceCtx, grant: GrantHit) -> Self {
        let (target, device) = target_of(&def).expect("a remote table has a target");
        let device = device.to_string();
        let gate = Gate::new(ctx.clone(), &def, grant);
        Self {
            def,
            ctx,
            gate,
            target,
            device,
            slots: Mutex::new(0),
            cv: Condvar::new(),
            catalog: Arc::new(Mutex::new(None)),
            catalog_asked: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }

    /// A slot of `concurrency`, or None after `wait`.
    fn acquire(&self, wait: Duration) -> Option<Slot<'_>> {
        let max = self.def.limits.concurrency;
        let end = Instant::now() + wait;
        let mut g = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        while *g >= max {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *g += 1;
        Some(Slot(self))
    }

    /// The header of a request: the device on the other side, and this
    /// sandbox as text.
    fn header(&self, size: usize, stream: bool) -> Request {
        let req = if stream {
            Request::call_stream(&self.device, size)
        } else {
            Request::call(&self.device, size)
        };
        req.with_caller(&RemoteCaller::new(
            &self.ctx.sandbox,
            &crate::report::hostname(),
            &self.ctx.started_by,
        ))
    }

    fn note_error(&self, text: Option<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = text;
    }

    /// One request over one connection, watched for the cancel flag and
    /// the deadline. Ok: how many bytes came.
    fn exchange(
        &self,
        call: &Call,
        req: Request,
        body: &[u8],
        sink: &mut dyn Sink,
    ) -> Result<usize, DevError> {
        let conn = match self.target.open(&self.ctx.env) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("device {}: {e}", self.def.name);
                self.note_error(Some(e));
                return Err(DevError::Failed);
            }
        };
        let closer = conn.closer.clone();
        let (tx, rx) = sync_channel(STREAM_QUEUE);
        let body = body.to_vec();
        let t2 = tx.clone();
        std::thread::spawn(move || {
            let mut conn = conn;
            let r = socket::request_io(
                &mut conn.reader,
                &mut conn.writer,
                &req,
                &body,
                &mut |b: &[u8]| {
                    t2.send(Piece::Bytes(b.to_vec()))
                        .map_err(|_| std::io::Error::other("the reader went"))
                },
            );
            let _ = tx.send(Piece::End(r));
        });
        let mut got = 0usize;
        let result = loop {
            match rx.recv_timeout(WATCH) {
                Ok(Piece::Bytes(b)) => {
                    got += b.len();
                    sink.push(&b);
                    continue;
                }
                Ok(Piece::End(Ok(n))) => break Ok(n),
                Ok(Piece::End(Err(p))) => break Err(p),
                Err(RecvTimeoutError::Disconnected) => {
                    break Err(socket::Partial {
                        errno: libc::EIO,
                        after: got,
                    })
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if call.cancelled() {
                // The other side sees its connection go and cancels.
                closer.close();
                let e = if Instant::now() >= call.deadline {
                    DevError::TimedOut
                } else {
                    DevError::Cancelled
                };
                closer.finish();
                return Err(e);
            }
        };
        closer.finish();
        match result {
            Ok(n) => {
                self.note_error(None);
                Ok(n)
            }
            Err(p) => {
                let e = DevError::from_errno(p.errno);
                if e == DevError::Failed || e == DevError::Cancelled {
                    let said = closer.stderr();
                    let text = match (p.errno, said.is_empty()) {
                        (libc::EIO, true) => format!(
                            "the connection to {} closed before the answer",
                            self.target.render()
                        ),
                        (libc::EIO, false) => format!(
                            "the connection to {} closed before the answer: {said}",
                            self.target.render()
                        ),
                        (_, true) => format!(
                            "the paddock at {} answered error {}",
                            self.target.render(),
                            p.errno
                        ),
                        (_, false) => format!(
                            "the paddock at {} answered error {}: {said}",
                            self.target.render(),
                            p.errno
                        ),
                    };
                    log::warn!("device {}: {text}", self.def.name);
                    self.note_error(Some(text));
                }
                Err(e)
            }
        }
    }

    /// Ask the other side for its catalog once, on a thread, after the
    /// first request that the user allowed: the entry's own `request`
    /// and `response` text then show in the catalog here.
    fn ask_catalog(&self) {
        if self.catalog_asked.swap(true, Ordering::SeqCst) {
            return;
        }
        let (env, target, device, slot) = (
            self.ctx.env.clone(),
            self.target.clone(),
            self.device.clone(),
            self.catalog.clone(),
        );
        std::thread::spawn(
            move || match fetch_entry(&env, &target, &device, CATALOG_WAIT) {
                Ok(entry) => *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry),
                Err(e) => {
                    log::warn!("device {device}: cannot read the catalog of the other side: {e}")
                }
            },
        );
    }

    /// The entry of the other side's catalog, when it was read already.
    pub fn remote_entry(&self) -> Option<serde_json::Value> {
        self.catalog
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// One request: the gate, the budget, a `concurrency` slot, the
    /// exchange, the audit line.
    fn call_into(
        &self,
        call: &Call,
        request: &[u8],
        sink: &mut dyn Sink,
        stream: bool,
    ) -> Result<(), DevError> {
        let start = Instant::now();
        if request.len() > self.def.limits.max_request {
            return Err(DevError::RequestTooLarge);
        }
        let bytes_in = format!("{} bytes in", request.len());
        let decision = match self.gate.check(call) {
            Ok(d) => d,
            Err(e) => {
                self.gate
                    .audit_with(&bytes_in, "denied", "denied", start, self.extra());
                return Err(e);
            }
        };
        let charge = match self.gate.reserve() {
            Ok(c) => c,
            Err(e) => {
                let status = match e {
                    DevError::BudgetUsed => super::ledger::BUDGET_USED,
                    DevError::Denied => "no budget",
                    _ => "failed",
                };
                self.gate
                    .audit_with(&bytes_in, &decision, status, start, self.extra());
                return Err(e);
            }
        };
        let wait = call
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(self.def.timeout()));
        let Some(_slot) = self.acquire(wait) else {
            self.gate.settle_cost(charge, None, false, "busy");
            self.gate
                .audit_with(&bytes_in, &decision, "busy", start, self.extra());
            return Err(DevError::Busy);
        };
        let req = self.header(request.len(), stream);
        let r = self.exchange(call, req, request, sink);
        let (status, detail, ran) = match &r {
            Ok(n) => ("ok".to_string(), format!("{bytes_in}, {n} bytes out"), true),
            Err(e) => {
                let ran = !matches!(e, DevError::Failed | DevError::Busy | DevError::Waiting);
                (status_of(*e).to_string(), bytes_in.clone(), ran)
            }
        };
        self.gate.settle_cost(charge, None, ran, &status);
        self.gate
            .audit_with(&detail, &decision, &status, start, self.extra());
        if r.is_ok() {
            self.ask_catalog();
        }
        r.map(|_| ())
    }

    fn extra(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert("target".into(), self.target.render().into());
        m.insert("transport".into(), self.target.transport().into());
        m.insert("remote_device".into(), self.device.clone().into());
        m
    }
}

struct Slot<'a>(&'a Forward);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut g = self.0.slots.lock().unwrap_or_else(|e| e.into_inner());
        *g -= 1;
        self.0.cv.notify_one();
    }
}

impl Device for Forward {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn info(&self) -> Info {
        let mut i = info(&self.def);
        let g = self.catalog.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = &*g {
            let m = mark(&self.target);
            if let Some(r) = c.get("request").and_then(|v| v.as_str()) {
                i.request = format!(
                    "{m}Forwarded to the device `{}` of the paddock at {}, which says: {}",
                    self.device,
                    self.target.render(),
                    crate::broker::clean_text(r)
                );
            }
            if let Some(r) = c.get("response").and_then(|v| v.as_str()) {
                i.response = crate::broker::clean_text(r);
            }
        }
        i
    }

    fn details(&self) -> Option<serde_json::Value> {
        let mut v = super::user::details(&self.def);
        self.gate.details(&mut v);
        v["target"] = self.target.render().into();
        v["transport"] = self.target.transport().into();
        v["device"] = self.device.clone().into();
        v["instance"] = "remote".into();
        v["instance_name"] = self.target.render().into();
        if let Some(c) = &*self.catalog.lock().unwrap_or_else(|e| e.into_inner()) {
            let mut remote = serde_json::Map::new();
            for k in [
                "state",
                "shape",
                "run",
                "platform",
                "stream",
                "how_to_enable",
            ] {
                if let Some(x) = c.get(k) {
                    remote.insert(k.into(), x.clone());
                }
            }
            v["remote"] = remote.into();
        }
        if let Some(e) = &*self.last_error.lock().unwrap_or_else(|e| e.into_inner()) {
            v["last_error"] = e.clone().into();
            v["how_to_fix"] = format!(
                "the last request of {} did not reach its device: {e}. The user runs `paddock device check {}` on the host",
                self.def.name, self.def.name
            )
            .into();
        }
        Some(v)
    }

    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        let mut sink = super::Collect::default();
        self.call_into(call, request, &mut sink, false)?;
        Ok(sink.0)
    }

    fn call_stream(
        &self,
        call: &Call,
        request: &[u8],
        sink: &mut dyn Sink,
    ) -> Result<(), DevError> {
        self.call_into(call, request, sink, self.def.stream)
    }
}

/// The entry of `device` in the catalog of the other side.
pub fn fetch_entry(
    env: &crate::env::HostEnv,
    target: &Target,
    device: &str,
    wait: Duration,
) -> Result<serde_json::Value, String> {
    let text = fetch_catalog(env, target, wait)?;
    let v: serde_json::Value =
        serde_json::from_slice(&text).map_err(|e| format!("the catalog is not JSON: {e}"))?;
    v["devices"]
        .as_array()
        .and_then(|d| d.iter().find(|e| e["name"] == device))
        .cloned()
        .ok_or_else(|| format!("the paddock at {} has no device {device}", target.render()))
}

/// The catalog of the other side: one connection, `op: catalog`.
pub fn fetch_catalog(
    env: &crate::env::HostEnv,
    target: &Target,
    wait: Duration,
) -> Result<Vec<u8>, String> {
    let conn = target.open(env)?;
    let closer = conn.closer.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut conn = conn;
        let mut out = Vec::new();
        let r = socket::request_io(
            &mut conn.reader,
            &mut conn.writer,
            &Request::catalog(),
            &[],
            &mut |b: &[u8]| {
                out.extend_from_slice(b);
                Ok(())
            },
        );
        let _ = tx.send(r.map(|_| out));
    });
    let r = match rx.recv_timeout(wait) {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(p)) => Err(format!("error {}", p.errno)),
        Err(_) => {
            closer.close();
            Err(format!("no answer within {} s", wait.as_secs()))
        }
    };
    closer.finish();
    r.map_err(|e| {
        let said = closer.stderr();
        if said.is_empty() {
            format!("the paddock at {}: {e}", target.render())
        } else {
            format!("the paddock at {}: {e}: {said}", target.render())
        }
    })
}

/// `paddock device check NAME` for a remote device: reach the other side,
/// read its catalog, print the entry of the device there. The user runs
/// this on the host, so ssh runs as the user without a card.
pub fn probe(env: &crate::env::HostEnv, def: &DeviceDef) -> Result<String> {
    let (target, device) = target_of(def).context("not a remote table")?;
    let t = Instant::now();
    let text = fetch_catalog(env, &target, Duration::from_secs(def.timeout().min(60)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let ms = t.elapsed().as_millis();
    let v: serde_json::Value = serde_json::from_slice(&text).context("the catalog is not JSON")?;
    let mut out = format!("{}: reached {} in {ms} ms\n", def.name, target.render());
    let Some(e) = v["devices"]
        .as_array()
        .and_then(|d| d.iter().find(|e| e["name"] == device))
    else {
        // The tables of the other side; its built-in devices are not
        // for remote callers and stay out of the list.
        let names: Vec<&str> = v["devices"]
            .as_array()
            .map(|d| {
                d.iter()
                    .filter(|e| e["grant"] != "builtin")
                    .filter_map(|e| e["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        bail!(
            "{out}the paddock there has no device {device}. Its devices: {}",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        );
    };
    let field = |k: &str| e[k].as_str().unwrap_or("-").to_string();
    out.push_str(&format!(
        "device    {device} there: state {}, shape {}, run {}, platform {}, stream {}\n",
        field("state"),
        field("shape"),
        field("run"),
        field("platform"),
        e["stream"].as_bool().unwrap_or(false)
    ));
    if let Some(h) = e["how_to_enable"].as_str() {
        out.push_str(&format!("off       {h}\n"));
    }
    if let Some(h) = e["how_to_fix"].as_str() {
        out.push_str(&format!("failed    {h}\n"));
    }
    out.push_str(&format!("request   {}\n", field("request")));
    out.push_str(&format!("response  {}\n", field("response")));
    Ok(out)
}

#[cfg(test)]
#[path = "forward/serve_tests.rs"]
mod serve_tests;
#[cfg(test)]
#[path = "forward_tests.rs"]
pub(crate) mod tests;
