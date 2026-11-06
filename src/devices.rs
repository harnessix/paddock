//! Devices: host services that a sandbox uses as files in `/dev/paddock`.
//!
//! Each sandbox gets its own small FUSE file system. The paddock process on
//! the host serves it and mounts it in the run folder of the sandbox, without
//! root: `fusermount3` does the mount for the user. The backend then binds
//! that folder to `/dev/paddock` inside the sandbox, before the harness
//! starts. Because each sandbox has its own file system, the mount itself
//! tells paddock which sandbox calls. No request carries an identity.
//!
//! The protocol is the same for every device. One `open` is one request:
//! open the device read-write, write the whole request, then read the
//! response until end of file. The first read sends the request. A read
//! blocks until the response is ready or the deadline of the device passes
//! (then it fails with ETIMEDOUT). Closing the file early cancels the
//! request. `/dev/paddock/.catalog` is read-only and describes each device:
//! its request format, its limits and whether it is on.
//!
//! A device with `stream` in its catalog entry answers as it goes: a read
//! returns bytes as the program writes them (`Device::call_stream`, the
//! `Streaming` phase of fuse.rs, the chunks of socket.rs), and a failure
//! after the first byte fails the next read with EIO.
//!
//! Waiting devices (`wait`, `ask`, see blocking.rs) block until an event:
//! a time, or the user's answer. Each read waits at most the `timeout` of
//! its request; when that passes first, the read fails with ETIME ("still
//! waiting", `crate::waiting`), and the agent sends the same request again.
//!
//! A sandbox that another one started (`spawn`) gets at most the devices
//! of its parent: the parent's paddock records them in the child's job, and
//! the child's own `/dev/paddock` serves only those that the user
//! configuration still turns on.
//!
//! `/dev/paddock/secret/NAME` is not a device: it holds the value of a
//! `deliver` credential with `as = "file"` (credentials/secret.rs). A plain
//! read returns the value. Each open counts as one use, and after
//! `paddock credentials revoke` the file fails with EACCES.
//!
//! On macOS, Seatbelt cannot show a FUSE mount and macOS has no FUSE
//! without a kernel extension. There the same devices come over a Unix
//! socket per sandbox (`PADDOCK_DEVICES_SOCKET`, devices/socket.rs), with
//! `paddock-host device` as the client, and each secret file is a FIFO in
//! `$PADDOCK_DEVICES/secret/` (credentials/secret_fifo.rs).
//!
//! Devices are off unless `[devices]` in the user configuration turns them
//! on. The project `paddock.toml` cannot do this, because the agent can edit
//! that file. Requests and responses never go into logs or process
//! arguments; the log keeps the device name, the sizes and the outcome.

use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub mod blocking;
pub mod config;
pub mod doctor;
pub mod escape;
pub mod examples;
pub mod forward;
pub mod grant;
pub mod host;
pub mod http;
pub mod ledger;
pub mod outproc;
mod profile;
pub mod program;
mod reader;
mod render;
pub mod sandbox;
mod serve;
pub mod shared;
pub mod socket;
pub mod supervise;
pub mod user;

pub use config::DeviceDef;
pub use grant::{GrantCtx, GrantHit};
pub use program::{Collect, Sink};

pub use serve::{
    backend_support, clear_stale, doctor_lines, doctor_lines_on, fuse_available, hook_control,
    late, mount_dir, report, socket_dir, start, Devices, SOCKET_NAME,
};

#[cfg(target_os = "linux")]
mod fuse;
#[cfg(target_os = "linux")]
pub use fuse::{mount, DeviceFs, Mounted};

#[cfg(not(target_os = "linux"))]
mod fuse_stub;
#[cfg(not(target_os = "linux"))]
pub use fuse_stub::{mount, DeviceFs, Mounted};

/// Where the devices appear inside the sandbox.
pub const MOUNT_IN_SANDBOX: &str = "/dev/paddock";
/// The read-only description of the devices.
pub const CATALOG: &str = ".catalog";
/// The folder of the secret files.
pub const SECRET_DIR: &str = "secret";

/// Limits of one device. The catalog shows them.
#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub max_request: usize,
    pub max_response: usize,
    /// The longest time a request can take before its read fails.
    pub deadline_seconds: u64,
}

/// What `.catalog` says about one device.
#[derive(Debug, Clone, Serialize)]
pub struct Info {
    /// The format of the request bytes.
    pub request: String,
    /// The format of the response bytes.
    pub response: String,
    pub limits: Limits,
    /// `builtin`, `exec`, `serve` or `http`.
    pub shape: &'static str,
    /// Whether a read returns bytes as the program writes them.
    pub stream: bool,
}

impl Info {
    /// The entry of a built-in device: no stream, shape `builtin`.
    pub fn builtin(
        request: impl Into<String>,
        response: impl Into<String>,
        limits: Limits,
    ) -> Self {
        Self {
            request: request.into(),
            response: response.into(),
            limits,
            shape: "builtin",
            stream: false,
        }
    }
}

/// Why a request failed. The reader gets the errno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevError {
    /// The request does not have the format the device expects (EINVAL).
    Invalid,
    /// The device cannot run the request now (EIO).
    Failed,
    /// The response is larger than the device limit (EMSGSIZE).
    TooLarge,
    /// The client closed the file or the deadline passed (ECANCELED).
    Cancelled,
    /// The time limit of this read passed before the event (ETIME, "still
    /// waiting"). The client sends the same request again.
    Waiting,
    /// The user declined, for example dismissed a question (EACCES).
    Denied,
    /// The budget of a paid device is used (EDQUOT).
    BudgetUsed,
    /// The device has `concurrency` requests at work and a full queue
    /// (EBUSY).
    Busy,
    /// The request is larger than the device limit (EFBIG).
    RequestTooLarge,
    /// The deadline of the device passed on another paddock (ETIMEDOUT):
    /// a remote device (forward.rs) that did not answer in its time.
    TimedOut,
}

impl DevError {
    pub fn errno(self) -> i32 {
        match self {
            DevError::Invalid => libc::EINVAL,
            DevError::Failed => libc::EIO,
            DevError::TooLarge => libc::EMSGSIZE,
            DevError::Cancelled => libc::ECANCELED,
            DevError::Waiting => crate::waiting::ERRNO_STILL_WAITING,
            DevError::Denied => libc::EACCES,
            DevError::BudgetUsed => libc::EDQUOT,
            DevError::Busy => libc::EBUSY,
            DevError::RequestTooLarge => libc::EFBIG,
            DevError::TimedOut => libc::ETIMEDOUT,
        }
    }

    /// The error of an errno that another paddock answered (socket.rs):
    /// the same mapping backwards, `Failed` for every errno without one.
    pub fn from_errno(errno: i32) -> Self {
        if errno == crate::waiting::ERRNO_STILL_WAITING {
            return DevError::Waiting;
        }
        match errno {
            libc::EINVAL => DevError::Invalid,
            libc::EMSGSIZE => DevError::TooLarge,
            libc::ECANCELED => DevError::Cancelled,
            libc::EACCES => DevError::Denied,
            libc::EDQUOT => DevError::BudgetUsed,
            libc::EBUSY => DevError::Busy,
            libc::EFBIG => DevError::RequestTooLarge,
            libc::ETIMEDOUT => DevError::TimedOut,
            _ => DevError::Failed,
        }
    }
}

/// The caller as another paddock described it (`run = "remote"`,
/// forward.rs): text for the card and the audit line, never an identity.
/// The identity of a remote request is its transport (the ssh peer or
/// the socket), which `Call::sandbox` names as `remote:PEER`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteCaller {
    /// The sandbox id on the forwarding side.
    pub sandbox: String,
    /// The forwarding machine, as it names itself.
    pub host: String,
    /// The sandboxes above the caller there, root first.
    pub chain: Vec<String>,
}

/// The longest text a remote caller field keeps.
const REMOTE_TEXT: usize = 128;

impl RemoteCaller {
    /// Text as another paddock sent it: cleaned for a terminal, cut to
    /// `REMOTE_TEXT` characters.
    pub fn new(sandbox: &str, host: &str, chain: &[String]) -> Self {
        let clean = |s: &str| {
            crate::broker::clean_text(s)
                .chars()
                .filter(|c| *c != '\n')
                .take(REMOTE_TEXT)
                .collect::<String>()
        };
        Self {
            sandbox: clean(sandbox),
            host: clean(host),
            chain: chain.iter().take(16).map(|c| clean(c)).collect(),
        }
    }

    /// One phrase for the card: "sandbox ID on HOST".
    pub fn text(&self) -> String {
        let mut s = match (self.sandbox.is_empty(), self.host.is_empty()) {
            (false, false) => format!("sandbox {} on {}", self.sandbox, self.host),
            (false, true) => format!("sandbox {}", self.sandbox),
            (true, false) => format!("a sandbox on {}", self.host),
            (true, true) => "an unnamed sandbox".into(),
        };
        if !self.chain.is_empty() {
            s.push_str(&format!(", started by {}", self.chain.join(" > ")));
        }
        s
    }
}

/// What a running request can see: the calling sandbox and whether the
/// client still waits.
pub struct Call {
    /// The sandbox id. It comes from the mount, never from the request.
    /// For a request of another paddock it is `remote:PEER`, from the
    /// transport.
    pub sandbox: String,
    pub deadline: Instant,
    cancel: Arc<AtomicBool>,
    /// What the forwarding paddock said about its caller: text for the
    /// card and the audit, set by the remote server of socket.rs only.
    pub remote: Option<RemoteCaller>,
}

impl Call {
    /// A call for tests of a device without a mount.
    pub fn for_test(sandbox: &str, deadline: Duration) -> Self {
        Self {
            sandbox: sandbox.to_string(),
            deadline: Instant::now() + deadline,
            cancel: Arc::new(AtomicBool::new(false)),
            remote: None,
        }
    }

    /// A call whose cancel flag the test holds.
    pub fn with_cancel(sandbox: &str, deadline: Duration, cancel: Arc<AtomicBool>) -> Self {
        Self {
            sandbox: sandbox.to_string(),
            deadline: Instant::now() + deadline,
            cancel,
            remote: None,
        }
    }

    /// True when the client closed the file or the deadline passed. A device
    /// that works for a long time checks this and stops early.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || Instant::now() >= self.deadline
    }
}

/// One device. A backend runs on the host with the rights of the user.
pub trait Device: Send + Sync {
    fn name(&self) -> &str;
    fn info(&self) -> Info;
    fn call(&self, call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError>;
    /// The response as it comes: each piece goes to `sink` when the
    /// device has it. A device with `stream` in its `Info` serves reads
    /// through this. The default answers whole, so a device that collects
    /// its response changes nothing. An error after the first piece means
    /// bytes, then the errno: the reader gets what came, and the next read
    /// fails.
    fn call_stream(
        &self,
        call: &Call,
        request: &[u8],
        sink: &mut dyn Sink,
    ) -> Result<(), DevError> {
        let out = self.call(call, request)?;
        sink.push(&out);
        Ok(())
    }
    /// "on", or "off" for the file of a user device that is not turned on.
    fn state(&self) -> &'static str {
        "on"
    }
    /// More fields for the catalog entry, for example the configuration of
    /// a user device.
    fn details(&self) -> Option<serde_json::Value> {
        None
    }
}

/// The built-in test device: the response is the request.
pub struct Echo;

impl Device for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn info(&self) -> Info {
        Info {
            shape: "builtin",
            stream: false,
            request: "any bytes".into(),
            response: "the same bytes".into(),
            limits: Limits {
                max_request: 64 * 1024,
                max_response: 64 * 1024,
                deadline_seconds: 10,
            },
        }
    }
    fn call(&self, _call: &Call, request: &[u8]) -> Result<Vec<u8>, DevError> {
        Ok(request.to_vec())
    }
}

/// What a device may know about the sandbox it serves, from the host. The
/// `ask` device needs it for the approval card.
#[derive(Clone)]
pub struct DeviceCtx {
    pub env: HostEnv,
    pub sandbox: String,
    pub harness: String,
    pub project: PathBuf,
    /// The sandboxes above this one, root first, the parent last.
    pub started_by: Vec<String>,
    /// Who answers the approval card of a user device.
    pub approver: Arc<dyn crate::broker::Approver>,
    /// The device instances of this sandbox (sandbox.rs): one per device
    /// with `run = "sandbox"`, ended with the sandbox.
    pub instances: Arc<sandbox::Instances>,
}

/// A built-in device, ready to serve.
pub fn builtin(name: &str, ctx: &DeviceCtx) -> Option<Arc<dyn Device>> {
    match name {
        "echo" => Some(Arc::new(Echo)),
        "wait" => Some(Arc::new(blocking::Wait)),
        "ask" => Some(blocking::Ask::start(ctx.clone())),
        _ => None,
    }
}

/// The description of a built-in device, for the catalog when it is off.
pub fn builtin_info(name: &str) -> Option<Info> {
    match name {
        "echo" => Some(Echo.info()),
        "wait" => Some(blocking::Wait.info()),
        "ask" => Some(blocking::ask_info()),
        _ => None,
    }
}

pub const BUILTIN: &[&str] = &["echo", "ask", "wait"];

/// `[devices]` of `~/.config/paddock/config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicesCfg {
    /// Mount `/dev/paddock` at all. On by default: an empty mount costs
    /// nothing and changes no harness.
    pub mount: bool,
    /// Devices that the user turned on.
    pub enabled: Vec<String>,
    /// `[devices.NAME]` tables, on or off (config.rs).
    pub user: Vec<DeviceDef>,
}

impl Default for DevicesCfg {
    fn default() -> Self {
        Self {
            mount: true,
            enabled: Vec::new(),
            user: Vec::new(),
        }
    }
}

impl DevicesCfg {
    pub fn load(env: &HostEnv) -> Result<Self> {
        let file = env.paddock_config().join("config.toml");
        let Ok(text) = std::fs::read_to_string(&file) else {
            return Ok(Self::default());
        };
        Self::parse_with(&text, config::ParseEnv::host(&env.home))
            .with_context(|| format!("[devices] in {} is invalid", file.display()))
    }

    /// Parse without a host: `~` in a path stays as written.
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_with(text, config::ParseEnv::default())
    }

    pub fn parse_with(text: &str, penv: config::ParseEnv<'_>) -> Result<Self> {
        let v: toml::Value = toml::from_str(text)?;
        let mut cfg = Self::default();
        let Some(t) = v.get("devices") else {
            return Ok(cfg);
        };
        let t = t.as_table().context("[devices] must be a table")?;
        for (k, v) in t {
            if let Some(table) = v.as_table() {
                let d = DeviceDef::parse_with(k, table, penv)?;
                if d.enabled {
                    cfg.enabled.push(k.clone());
                }
                cfg.user.push(d);
                continue;
            }
            if k == "use" {
                bail!(
                    "[devices] use = [...] belongs in the paddock.toml of a project, not in the user configuration. In the user configuration, grant a device with grant = [...] in its [devices.NAME] table"
                );
            }
            let on = v.as_bool().with_context(|| {
                format!("[devices] {k} must be true or false, or a [devices.{k}] table")
            })?;
            if k == "mount" {
                cfg.mount = on;
                continue;
            }
            if !BUILTIN.contains(&k.as_str()) {
                bail!(
                    "[devices] {k}: unknown device. Known: {}",
                    BUILTIN.join(", ")
                );
            }
            if on && !cfg.enabled.contains(k) {
                cfg.enabled.push(k.clone());
            }
        }
        Ok(cfg)
    }
}

/// The text of `.catalog`: every known device, with its state.
pub fn catalog(on: &[Arc<dyn Device>]) -> Vec<u8> {
    catalog_with(on, &[])
}

/// The catalog, with the secret files of the sandbox by name.
pub fn catalog_with(on: &[Arc<dyn Device>], secrets: &[String]) -> Vec<u8> {
    catalog_on(cfg!(target_os = "macos"), on, secrets)
}

/// The protocol of the device socket, for the catalog on macOS.
const SOCKET_PROTOCOL: &str = "There is no /dev/paddock on macOS: the devices come over a socket, and `paddock-host device NAME [REQUEST | -]` sends one request and prints the response. The exit codes: 75 still waiting (the timeout of the request passed before the event of a waiting device such as wait or ask: run the same command again), 3 the user declined, 4 the device is off or unknown, 5 the device did not answer in its time limit, 8 the device rejected the request, 10 the request or response is too large, 11 the budget of a paid device is used (the user raises it on the host). An http or command device of the user asks the user once per sandbox at its first request; an off one answers with a disabled status. A device with stream: true answers as it goes: `paddock-host device` prints what it got, and a failure after the first byte makes it exit with 7 after what it printed (75 when the device says still waiting). A stream is read once, in order.";

/// `catalog_with` for macOS (`socket`: no file per device) or Linux.
pub fn catalog_on(socket: bool, on: &[Arc<dyn Device>], secrets: &[String]) -> Vec<u8> {
    let mut devices = Vec::new();
    let mut names: Vec<&str> = on.iter().map(|d| d.name()).collect();
    for b in BUILTIN {
        if !names.contains(b) {
            names.push(b);
        }
    }
    for name in names {
        let served = on.iter().find(|d| d.name() == name);
        let (state, info) = match served {
            Some(d) => (d.state(), d.info()),
            None => match builtin_info(name) {
                Some(i) => ("off", i),
                None => continue,
            },
        };
        let mut entry = serde_json::json!({
            "name": name,
            "state": state,
            "shape": info.shape,
            "run": "builtin",
            "stream": info.stream,
            "grant": "builtin",
            "instance": "none",
            "request": info.request,
            "response": info.response,
            "limits": info.limits,
        });
        if let Some(serde_json::Value::Object(more)) = served.and_then(|d| d.details()) {
            for (k, v) in more {
                entry[k] = v;
            }
        }
        if served.is_some() && socket {
            // Not "command": a command device names its program there.
            entry["invoke"] = format!("paddock-host device {name}").into();
        } else if served.is_some() {
            entry["path"] = format!("{MOUNT_IN_SANDBOX}/{name}").into();
        }
        if state != "on" && entry.get("how_to_enable").is_none() {
            entry["how_to_enable"] = format!(
                "the user sets [devices] {name} = true in ~/.config/paddock/config.toml on the host"
            )
            .into();
        }
        devices.push(entry);
    }
    let file_protocol = "Open a device read-write, write the whole request, then read the response until end of file. One open is one request. The first read sends the request and blocks until the response is ready. Closing the file early cancels the request. A read fails with ETIMEDOUT after the deadline, EFBIG means the request is too large, EINVAL that the device rejected it. A waiting device (wait, ask) fails a read with ETIME (still waiting) when the timeout of the request passes before the event: send the same request again. EACCES means that the user declined. EDQUOT means that the budget of a paid device is used: the user raises it on the host. An http or command device of the user asks the user once per sandbox at its first request; an off one answers with a disabled status. `paddock-host device NAME` does all of this and exits with 75 for still waiting. A device with stream: true answers as it goes: read in order to the end of file. A failure after the first byte fails the read with EIO. paddock-host device prints what it got and exits with 7. A stream is read once, in order: a read at an earlier offset fails with ESPIPE. Without stream the response comes whole, and a failure is one errno before any byte.";
    let mut doc = serde_json::json!({
        "version": 2,
        "protocol": if socket { SOCKET_PROTOCOL } else { file_protocol },
        "devices": devices,
    });
    if !secrets.is_empty() {
        doc["secrets"] = secrets
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n,
                    "path": crate::credentials::secret::fifo::path_on(socket, n),
                })
            })
            .collect::<Vec<_>>()
            .into();
        let example = crate::credentials::secret::fifo::path_on(socket, "GH_TOKEN");
        let revoked = if socket {
            "After a revoke the file is gone (ENOENT). `paddock-host device secret/NAME` prints the value too."
        } else {
            "EACCES means that the user revoked it."
        };
        doc["secrets_how"] = format!("Each file holds the value of a credential that the user gave this sandbox. Read it with a plain read, no request: cat FILE. Each open counts as one use. Use it in one command only, for example GH_TOKEN=$(cat \"{example}\") gh pr list, or give the path to a tool that reads a token file. Do not copy the value into files or logs. {revoked}").into();
    }
    let mut out = serde_json::to_vec_pretty(&doc).unwrap_or_default();
    out.push(b'\n');
    out
}

/// The names of the devices to serve: the user's choice, plus `echo` when
/// `test` is set (`paddock doctor --escape` tests the device mount with
/// it). A user device needs a grant that covers `ctx` (grant.rs); a
/// spawned sandbox gets at most the devices of its parent (the ceiling
/// of `ctx`).
pub fn enabled_names(cfg: &DevicesCfg, test: bool, ctx: &GrantCtx) -> Vec<String> {
    grants(cfg, test, ctx).into_iter().map(|(n, _)| n).collect()
}

/// `enabled_names` with the grant that gave each device.
pub fn grants(cfg: &DevicesCfg, test: bool, ctx: &GrantCtx) -> Vec<(String, GrantHit)> {
    if !cfg.mount {
        return Vec::new();
    }
    let mut names = cfg.enabled.clone();
    if test && !names.iter().any(|n| n == "echo") {
        names.push("echo".into());
    }
    // A built-in device is for the sandboxes of this host: a remote caller
    // (forward.rs) gets only tables with a `remote:` grant.
    let within = |n: &str| {
        ctx.remote.is_none()
            && ctx
                .ceiling
                .as_ref()
                .is_none_or(|c| c.iter().any(|x| x == n))
    };
    names
        .into_iter()
        .filter_map(|n| match cfg.user.iter().find(|u| u.name == n) {
            Some(u) => grant::visible(u, ctx).map(|h| (n.clone(), h)),
            None if builtin_info(&n).is_some() && within(&n) => Some((n, GrantHit::NoKey)),
            None => None,
        })
        .collect()
}

/// The devices to serve. See `enabled_names`.
pub fn enabled(
    cfg: &DevicesCfg,
    test: bool,
    grants_ctx: &GrantCtx,
    ctx: &DeviceCtx,
) -> Vec<Arc<dyn Device>> {
    grants(cfg, test, grants_ctx)
        .into_iter()
        .filter_map(|(n, hit)| match cfg.user.iter().find(|u| u.name == n) {
            Some(u) => Some(u.build(ctx, Ok(hit))),
            None => builtin(&n, ctx),
        })
        .collect()
}

/// The files of user devices that this sandbox does not get: they answer
/// with a disabled status that names what to turn on or grant.
pub fn disabled(
    cfg: &DevicesCfg,
    served: &[Arc<dyn Device>],
    grants_ctx: &GrantCtx,
    ctx: &DeviceCtx,
) -> Vec<Arc<dyn Device>> {
    cfg.user
        .iter()
        .filter(|u| !served.iter().any(|d| d.name() == u.name))
        .map(|u| {
            let why = grant::status(u, grants_ctx)
                .err()
                .unwrap_or(grant::Why::Off);
            u.build(ctx, Err(why))
        })
        .collect()
}

/// The devices that a sandbox gets at its start, by name, for the ceiling
/// of its children. A configuration error gives none, the same as `start`.
pub fn names_for(env: &HostEnv, test: bool, ctx: &GrantCtx) -> Vec<String> {
    match DevicesCfg::load(env) {
        Ok(c) => enabled_names(&c, test, ctx),
        Err(_) => Vec::new(),
    }
}

/// `--device NAME` of `paddock run`: the name must be a user device that
/// is on. The error names the fix.
pub fn check_requested(env: &HostEnv, names: &[String]) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let cfg = DevicesCfg::load(env)?;
    let file = env.paddock_config().join("config.toml");
    for n in names {
        if BUILTIN.contains(&n.as_str()) {
            bail!(
                "--device {n}: {n} is a built-in device; the user turns it on with [devices] {n} = true in {}",
                file.display()
            );
        }
        match cfg.user.iter().find(|u| u.name == *n) {
            None => bail!(
                "--device {n}: there is no [devices.{n}] table in {}. `paddock device ls` lists the devices",
                file.display()
            ),
            Some(u) if !u.enabled => bail!(
                "--device {n}: [devices.{n}] is off; set enabled = true in {}",
                file.display()
            ),
            Some(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
