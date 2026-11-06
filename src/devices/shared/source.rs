//! The two halves of a shared device that the broker of a run holds: the
//! lease on the instance of the user, and the `InstanceSource` that
//! outproc.rs asks for a socket.
//!
//! A lease is one open connection to `GET /v1/lease`. Nothing travels on
//! it. The instance counts the open ones as its users and exits when none
//! is left and `idle` passed, so a run that ends, and a paddock process
//! that `kill -9` removes, both drop their lease with no timeout and no
//! file to clean up.
//!
//! `SharedSource` is one per run and device. It starts the instance when
//! nothing serves, holds the one lease of the run, and reports a
//! reconfiguration: an instance that reads another table than this run
//! does refuses the lease, and then every request of the run fails with
//! `EIO` until `paddock device restart NAME`.

use super::{ensure, stop_instance, table_hash, users_of, RECONFIGURED_WHY};
use crate::devices::config::DeviceDef;
use crate::devices::outproc::InstanceSource;
use crate::devices::sandbox::state::{InstanceState, SHARED_CALLER};
use crate::devices::DeviceCtx;
use crate::env::HostEnv;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// How long the second try of a lease waits for the socket of an instance
/// that is stopping to close: the drain of deviced, and a second more.
const OLD_SOCKET_WAIT: Duration = Duration::from_secs(3);

/// One broker's lease: an open connection to `GET /v1/lease`. The instance
/// counts the open leases and exits when none is left and `idle` passed.
/// Dropping it closes the connection, so the end of a run drops the lease,
/// and so does `kill -9` of the paddock process.
#[derive(Debug)]
pub struct Lease {
    stream: UnixStream,
}

/// Why an instance refused a lease.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The instance serves another view of the table than this run has.
    pub reconfigured: bool,
    pub why: String,
}

impl Refusal {
    pub(crate) fn plain(why: String) -> Self {
        Self {
            reconfigured: false,
            why,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl Lease {
    /// Take a lease on the instance behind `socket`. A refusal names the
    /// table that the instance serves, when that is why.
    pub fn take(socket: &Path, caller: &str, table: &str) -> Result<Self, Refusal> {
        let mut s = UnixStream::connect(socket).map_err(|e| {
            Refusal::plain(format!(
                "cannot reach the shared instance at {}: {e}",
                socket.display()
            ))
        })?;
        let head = format!(
            "GET {} HTTP/1.1\r\nHost: paddock\r\nPaddock-Protocol: {}\r\nPaddock-Caller: {}\r\nPaddock-Table: {}\r\n\r\n",
            super::deviced::LEASE_PATH,
            crate::devices::program::PROTOCOL,
            crate::devices::program::header_value(caller),
            crate::devices::program::header_value(table)
        );
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        s.write_all(head.as_bytes())
            .map_err(|e| Refusal::plain(format!("cannot ask for a lease: {e}")))?;
        // The instance answers the head at once, then holds the connection
        // open and sends nothing more.
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if buf.len() > crate::devices::program::HEAD_ROOM {
                        return Err(Refusal::plain("the lease answer is too large".into()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Refusal::plain(format!("no lease answer: {e}"))),
            }
        }
        // The head, and whatever of the body came with it.
        let end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|n| n + 4)
            .unwrap_or(buf.len());
        let mut body = buf.split_off(end);
        let head_text = String::from_utf8_lossy(&buf).into_owned();
        let first = head_text.lines().next().unwrap_or("").to_string();
        if !first.contains(" 200 ") {
            let status = head_text
                .lines()
                .find_map(|l| l.strip_prefix("Paddock-Status: "))
                .unwrap_or("")
                .trim()
                .to_string();
            // The body of a refusal names the fix. It is short.
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = std::io::Read::take(&mut s, 4096).read_to_end(&mut body);
            let tail = String::from_utf8_lossy(&body).trim().to_string();
            let why = if tail.is_empty() {
                format!("the shared instance refused the lease ({first})")
            } else {
                tail
            };
            return Err(Refusal {
                reconfigured: status == crate::devices::outproc::RECONFIGURED,
                why,
            });
        }
        // The lease lives on this connection: nothing more is read from it
        // until it closes.
        s.set_nonblocking(true)
            .map_err(|e| Refusal::plain(format!("cannot hold the lease: {e}")))?;
        Ok(Self { stream: s })
    }

    /// True while the instance holds the other end.
    pub fn held(&self) -> bool {
        let mut b = [0u8; 1];
        match (&self.stream).read(&mut b) {
            Ok(0) => false,
            Ok(_) => true,
            Err(e) => matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ),
        }
    }
}

/// The `InstanceSource` of a shared device: one per run, holding one lease.
pub struct SharedSource {
    def: DeviceDef,
    env: HostEnv,
    /// The sandbox of the run, which owns the instance without a user
    /// manager.
    caller: String,
    table: String,
    lease: Mutex<Option<Lease>>,
    /// Why the instance refuses this run: the table changed since the
    /// instance started. A restart clears it.
    refused: Mutex<Option<String>>,
    /// The identity of the instance at the last connect, and the epoch that
    /// outproc.rs reads: a stop or a restart changes the identity, which
    /// bumps the epoch, which makes the device forget a `failed` state.
    seen: Mutex<(u64, u64)>,
    stops: AtomicU64,
}

impl SharedSource {
    pub fn new(def: DeviceDef, ctx: &DeviceCtx) -> Self {
        Self {
            table: table_hash(&def),
            def,
            env: ctx.env.clone(),
            caller: ctx.sandbox.clone(),
            lease: Mutex::new(None),
            refused: Mutex::new(None),
            seen: Mutex::new((0, 0)),
            stops: AtomicU64::new(0),
        }
    }

    pub fn name(&self) -> &str {
        &self.def.name
    }

    /// The view of the table that this run sends in `Paddock-Table`.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The log of the shared instance.
    pub fn log(&self) -> PathBuf {
        crate::devices::sandbox::state::log_file(&self.env, &self.def.name, SHARED_CALLER)
    }

    /// Start the instance when nothing serves, take a lease, and return the
    /// socket. The notice of a host without a user manager goes to the log
    /// once per run.
    pub fn ensure_leased(&self) -> Result<PathBuf, String> {
        // An instance that is exiting on idle can accept the connection of
        // this call and then close it. That is a window of milliseconds and
        // it is nobody's fault, so one retry starts a new instance instead
        // of failing the request.
        match self.lease_once() {
            Ok(p) => Ok(p),
            Err(r) if r.reconfigured => {
                log::warn!("device {}: {}", self.def.name, r.why);
                *self.refused.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(RECONFIGURED_WHY.to_string());
                Err(r.why)
            }
            Err(first) => {
                log::debug!(
                    "device {}: the shared instance did not give a lease ({}); trying once more",
                    self.def.name,
                    first.why
                );
                self.wait_for_the_old_socket_to_close();
                self.lease_once().map_err(|again| {
                    if again.reconfigured {
                        *self.refused.lock().unwrap_or_else(|p| p.into_inner()) =
                            Some(RECONFIGURED_WHY.to_string());
                    }
                    again.why
                })
            }
        }
    }

    /// One try: start the instance when nothing serves, then take a lease.
    fn lease_once(&self) -> Result<PathBuf, Refusal> {
        let e = ensure(&self.env, &self.def, &self.caller).map_err(Refusal::plain)?;
        if let Some(n) = &e.notice {
            log::warn!("device {}: {n}", self.def.name);
        }
        if let Some(took) = e.started {
            log::info!(
                "device {}: the shared instance started in {:?} ({})",
                self.def.name,
                took,
                e.how.name()
            );
        }
        let mut g = self.lease.lock().unwrap_or_else(|p| p.into_inner());
        if !g.as_ref().is_some_and(|l| l.held()) {
            *g = Some(Lease::take(&e.socket, &self.caller, &self.table)?);
        }
        drop(g);
        *self.refused.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.note_identity();
        Ok(e.socket)
    }

    /// An instance that is stopping answers on its socket while it drains
    /// the requests at work (`deviced::DRAIN`), and refuses a lease with
    /// `stopping` as long as it does. A second try within that window
    /// would meet the same refusal, so wait for the socket to close
    /// first, bounded by the drain. A socket that nothing answers on, or
    /// that a unit holds, costs nothing or the bound.
    fn wait_for_the_old_socket_to_close(&self) {
        let socket = super::socket_path(&self.env, &self.def.name);
        let end = std::time::Instant::now() + OLD_SOCKET_WAIT;
        while std::time::Instant::now() < end && UnixStream::connect(&socket).is_ok() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Drop the lease of this run. The instance exits when no lease is left
    /// and `idle` passed.
    pub fn release(&self) -> bool {
        let mut g = self.lease.lock().unwrap_or_else(|p| p.into_inner());
        g.take().is_some()
    }

    /// True while this run holds a lease.
    pub fn leased(&self) -> bool {
        let g = self.lease.lock().unwrap_or_else(|p| p.into_inner());
        g.as_ref().is_some_and(|l| l.held())
    }

    /// `start` of the table: a shared device with `with-sandbox` takes its
    /// lease when the sandbox starts, which also starts the instance.
    pub fn start(&self) -> crate::devices::config::Start {
        self.def.start
    }

    /// The runs that hold a lease now, as the state file records it.
    pub fn users(&self) -> u32 {
        users_of(&self.env, &self.def.name).unwrap_or(0)
    }

    /// `paddock device restart NAME` from inside the run: end the instance,
    /// start one that reads the table as it is now, and take a lease again.
    pub fn restart(&self) -> Result<Duration, String> {
        self.release();
        self.stops.fetch_add(1, Ordering::SeqCst);
        stop_instance(&self.env, &self.def.name)?;
        // This run is long-lived, so it can own the new instance where no
        // user manager does.
        let t = std::time::Instant::now();
        self.ensure_leased()?;
        Ok(t.elapsed())
    }

    /// `paddock device stop NAME` from inside the run: drop the lease and
    /// end the instance.
    pub fn stop(&self) -> bool {
        let had = self.release();
        self.stops.fetch_add(1, Ordering::SeqCst);
        stop_instance(&self.env, &self.def.name).unwrap_or(false) || had
    }

    /// The identity of the running instance: its pid and start time. Zero
    /// when none runs.
    fn identity(&self) -> u64 {
        match InstanceState::shared_of(&self.env, &self.def.name).filter(|s| s.alive()) {
            Some(s) => u64::from(s.pid) << 32 | (s.started & 0xffff_ffff),
            None => 0,
        }
    }

    /// Remember the instance that this run just connected to, so `epoch`
    /// does not report the start that this run itself did.
    fn note_identity(&self) {
        let id = self.identity();
        let mut g = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        g.0 = id;
    }
}

impl InstanceSource for SharedSource {
    fn socket(&self, _ctx: &DeviceCtx) -> Result<PathBuf, String> {
        self.ensure_leased()
    }

    fn instance(&self) -> String {
        SHARED_CALLER.to_string()
    }

    fn log_path(&self) -> Option<PathBuf> {
        Some(self.log())
    }

    /// A stop or a restart of the shared instance, from this run or from
    /// another, gives a new number, so the device connects again and
    /// forgets a `failed` state.
    fn epoch(&self) -> u64 {
        let id = self.identity();
        let mut g = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if g.0 != id {
            g.0 = id;
            g.1 += 1;
            drop(g);
            // Another instance: whatever the old one refused is over.
            *self.refused.lock().unwrap_or_else(|p| p.into_inner()) = None;
            let g = self.seen.lock().unwrap_or_else(|p| p.into_inner());
            return g.1 + self.stops.load(Ordering::SeqCst);
        }
        g.1 + self.stops.load(Ordering::SeqCst)
    }

    /// Why every request of this run fails until the instance is
    /// restarted: the table changed since the instance read it.
    fn failed(&self) -> Option<String> {
        self.refused
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn fix_hint(&self) -> String {
        format!(
            "Read the log: paddock device logs {n}. Then: paddock device restart {n}",
            n = self.def.name
        )
    }
}

impl Drop for SharedSource {
    fn drop(&mut self) {
        self.release();
    }
}
