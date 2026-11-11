//! `paddock deviced NAME`: the shared instance of one device.
//!
//! One process per device per user. It owns the socket that every broker
//! connects to, keeps one device sandbox with the supervisor of supervise.rs
//! inside it, and adds what a shared lifetime needs:
//!
//! - **Leases.** Each broker holds one connection open to `GET /v1/lease`
//!   for the life of its run and sends nothing on it. The count of open
//!   leases is the number of runs that use the device. A broker that ends,
//!   or that `kill -9` removes, drops its lease by the close of the socket,
//!   which needs no timeout and no file to clean up.
//! - **Idle exit.** With no lease open and `idle` seconds without a
//!   request, the process exits. The socket unit or the LaunchAgent then
//!   starts it again on the next connection; without one the next broker
//!   starts it.
//! - **The table.** The instance reads the user configuration itself, never
//!   a project file, and compares its own view of the table with the
//!   `Paddock-Table` of each request. A mismatch is `503` with
//!   `Paddock-Status: reconfigured`, so a run that started before an edit
//!   never gets the program of the other table.
//! - **The caller per request.** Every request carries the caller headers,
//!   which the supervisor turns into `PADDOCK_CALLER` for the program, so a
//!   program that keeps per-caller state keys it by that.
//!
//! The requests themselves go to the supervisor in the device sandbox
//! unchanged, head and body, and the answer comes back as it arrives, so a
//! streaming device streams through the instance.

use super::super::config::{DeviceDef, Scope};
use super::super::sandbox::state::{self, InstanceState, SHARED_CALLER, SHARED_RUN};
use super::super::sandbox::{Instance, InstanceCtx, Launcher};
use super::super::supervise::http::{read_request, Reply};
use super::super::supervise::Log;
use super::super::DevicesCfg;
use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::io::{Read, Write};
use std::os::fd::AsRawFd as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The path that a broker holds open for the life of its run.
pub const LEASE_PATH: &str = "/v1/lease";
/// How often the accept loop looks at the stop flag and the idle time.
const TICK: Duration = Duration::from_millis(200);
/// How long a stop waits for the requests at work before it ends them.
const DRAIN: Duration = Duration::from_secs(2);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Ask an instance that runs in this process to stop (the unit tests).
pub fn ask_stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// The table of `name` from the user configuration, checked for a shared
/// device that this paddock can serve.
pub fn table_of(env: &HostEnv, name: &str) -> Result<DeviceDef> {
    let cfg = DevicesCfg::load(env)?;
    let def = cfg
        .user
        .iter()
        .find(|d| d.name == name)
        .cloned()
        .with_context(|| {
            format!(
                "no [devices.{name}] table in {}",
                env.paddock_config().join("config.toml").display()
            )
        })?;
    if def.scope != Scope::Shared {
        bail!(
            "[devices.{name}]: scope = \"{}\", so it has no shared instance",
            def.scope.name()
        );
    }
    if !def.enabled {
        bail!("[devices.{name}]: enabled = false");
    }
    if let Some(why) = def.not_yet() {
        bail!("{why}");
    }
    Ok(def)
}

/// The launcher of the device sandbox: Seatbelt on macOS, bubblewrap on
/// Linux, or a thread with no isolation when the test variable asks for it.
fn launcher(env: &HostEnv) -> Launcher {
    match env.var(super::LAUNCHER_VAR) {
        Some("thread") => Launcher::Thread,
        _ => Launcher::host(),
    }
}

/// The state of the running instance, shared by the threads.
struct Shared {
    env: HostEnv,
    def: DeviceDef,
    /// The view of the table that this instance serves.
    table: String,
    /// The socket of the supervisor in the device sandbox.
    inner: PathBuf,
    log: Arc<Log>,
    /// Open leases: the runs that use the device.
    leases: AtomicU64,
    /// Requests at work, which a stop waits for.
    busy: AtomicU64,
    /// Milliseconds since the start at the last request or lease change.
    last: AtomicU64,
    started: Instant,
    /// The state file as the start wrote it, rewritten when the count of
    /// users changes.
    record: Mutex<InstanceState>,
    /// The instance is ending: the state file is gone, and a lease thread
    /// that drops its lease now must not write it again.
    closing: AtomicBool,
}

impl Shared {
    fn touch(&self) {
        self.last
            .store(self.started.elapsed().as_millis() as u64, Ordering::SeqCst);
    }

    /// The time without a request or a lease change.
    fn quiet(&self) -> Duration {
        Duration::from_millis(
            (self.started.elapsed().as_millis() as u64)
                .saturating_sub(self.last.load(Ordering::SeqCst)),
        )
    }

    /// Write the number of open leases into the state file, so `paddock ps`
    /// and `paddock device ls` show it. The count is read under the record
    /// lock: two leases that change at once could otherwise write their
    /// counts in the wrong order and leave a stale number in the file.
    fn write_users(&self) {
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let mut g = self.record.lock().unwrap_or_else(|e| e.into_inner());
        g.users = Some(self.leases.load(Ordering::SeqCst) as u32);
        if let Err(e) = g.write(&self.env) {
            self.log
                .line(&format!("cannot write the state file: {e:#}"));
        }
    }

    /// The table that the request claims matches this instance's view.
    /// A request without the header (a health or catalog probe) passes.
    fn table_ok(&self, sent: Option<&str>) -> bool {
        sent.is_none_or(|s| s == self.table)
    }

    fn reconfigured(&self) -> Reply {
        Reply::paddock(
            503,
            "reconfigured",
            &format!(
                "the device {} was reconfigured: its table changed since this shared instance started, so this instance cannot serve your run. paddock device restart {}",
                self.def.name, self.def.name
            ),
        )
    }
}

/// `paddock deviced NAME`: serve until a stop or until idle.
pub fn main(
    env: &HostEnv,
    name: &str,
    socket: Option<PathBuf>,
    owner: Option<&str>,
    unit: Option<&str>,
) -> Result<i32> {
    crate::update::refuse_in_sandbox(env, "paddock deviced")?;
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a plain sigaction(2) with a handler that stores a flag.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_term as extern "C" fn(libc::c_int) as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    // SAFETY: a closed connection must not kill the instance.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let def = table_of(env, name)?;
    let socket = socket.unwrap_or_else(|| super::socket_path(env, name));
    let log_file = state::log_file(env, name, SHARED_CALLER);
    if let Some(d) = log_file.parent() {
        crate::privfs::dir(d)?;
    }
    let log = Arc::new(Log::open(Some(&log_file))?);
    let lifetime = match owner {
        Some(o) => format!("owned by the run {o}, so it ends with that run"),
        None => "for every run of this user".to_string(),
    };
    let how = launcher(env);
    log.line(&format!(
        "start device {name} socket {} idle {} s {lifetime}, device sandbox {}",
        socket.display(),
        def.idle,
        how.name()
    ));
    if owner.is_some() {
        log.line(&super::no_manager_notice(name, owner.unwrap_or("")));
    }
    // The socket first: a second instance must find the address taken
    // before it starts a device sandbox.
    let listener = match listener(env, &socket)? {
        Some(l) => l,
        None => {
            log.line("another instance answers on the socket already; nothing to do");
            return Ok(0);
        }
    };
    let ctx = InstanceCtx {
        caller: SHARED_CALLER.to_string(),
        project: env.home.clone(),
        harness: SHARED_CALLER.to_string(),
        chain: Vec::new(),
        dir: state::instance_dir(env, name, SHARED_CALLER),
    };
    // The unit that started this process says so on the command line: the
    // environment of a unit need not show the user manager (no session bus
    // address), so `start_kind` cannot tell from here.
    // An instance that a run owns has no unit, whatever this host has.
    let unit = match owner {
        Some(_) => None,
        None => unit
            .map(str::to_string)
            .or_else(|| super::start_kind(env, name).unit()),
    };
    // The state file as the shared instance: written once, whole, by the
    // start. A reader that sees the new pid sees the unit and the owner
    // with it (`paddock ps`, `paddock device stop`, the tests).
    let shared_record = |r: &mut InstanceState| {
        r.run = SHARED_RUN.to_string();
        // The pid of this process, not of the device sandbox it holds:
        // this is what `paddock device stop` signals and what `paddock ps`
        // shows. The command line of this process names the socket, so
        // `alive` can tell a live instance from a reused pid.
        r.pid = std::process::id();
        r.unit = unit.clone();
        r.owner = owner.map(str::to_string);
        r.users = Some(0);
        r.socket = socket.clone();
    };
    let mut inst = Instance::start_with(env, &def, &ctx, how, shared_record)?;
    let mut record = InstanceState::shared_of(env, name)
        .or_else(|| {
            state::InstanceState::list(env)
                .into_iter()
                .find(|s| s.device == name && s.caller == SHARED_CALLER)
        })
        .unwrap_or_else(|| blank_record(env, &def, &inst, &socket));
    // A launcher that wrote no file: the same record, written now.
    if record.pid != std::process::id() || record.run != SHARED_RUN {
        shared_record(&mut record);
        record.scope = inst.scope.clone();
        record.write(env)?;
    }
    let shared = Arc::new(Shared {
        env: env.clone(),
        table: super::table_hash(&def),
        inner: inst.socket.clone(),
        def,
        log: log.clone(),
        leases: AtomicU64::new(0),
        busy: AtomicU64::new(0),
        last: AtomicU64::new(0),
        started: Instant::now(),
        record: Mutex::new(record),
        closing: AtomicBool::new(false),
    });
    // Without a user manager the instance belongs to the run that started
    // it: it ends when that paddock process ends, so a killed run leaves no
    // instance behind.
    if owner.is_some() {
        watch_parent(&log)?;
    }
    let idle = Duration::from_secs(shared.def.idle);
    let code = accept_loop(&listener, &shared, idle);
    log.line(&format!(
        "stop after {} s, {} leases open",
        shared.started.elapsed().as_secs(),
        shared.leases.load(Ordering::SeqCst)
    ));
    // No more state files from here: a lease thread that notices the stop
    // after `inst.stop()` removed the file must not write it again, or
    // `paddock device stop` would see an instance that is gone.
    shared.closing.store(true, Ordering::SeqCst);
    // Stop accepting first, then end the device sandbox: a request at work
    // sees its connection close, which the broker reports as a failure.
    drop(listener);
    let _ = std::fs::remove_file(&socket);
    inst.stop();
    // `inst.stop()` removes the state file of the instance. Remove it once
    // more, in case a lease thread wrote it between the flag and the stop.
    InstanceState::remove(env, name, SHARED_CALLER);
    code
}

/// End the instance when the paddock process that started it ends: the
/// parent pid changes when that process is gone, on every platform, and
/// also after `kill -9`.
///
/// No `PR_SET_PDEATHSIG`: that signal fires when the *thread* that started
/// this process ends, not the process. A run starts the instance from the
/// worker thread of its first request (fuse.rs), which ends a moment
/// later, so the signal would end the instance after one request.
fn watch_parent(log: &Arc<Log>) -> Result<()> {
    // SAFETY: getppid takes no argument and cannot fail.
    let parent = unsafe { libc::getppid() };
    let log = log.clone();
    std::thread::Builder::new()
        .name("deviced-owner".into())
        .spawn(move || loop {
            std::thread::sleep(TICK);
            if STOP.load(Ordering::SeqCst) {
                return;
            }
            // SAFETY: as above.
            if unsafe { libc::getppid() } != parent {
                log.line("the run that owns this instance ended: stopping");
                STOP.store(true, Ordering::SeqCst);
                return;
            }
        })
        .context("cannot start the owner watch thread")?;
    Ok(())
}

/// A state file for an instance whose own file is missing (a launcher that
/// writes none).
fn blank_record(env: &HostEnv, def: &DeviceDef, inst: &Instance, socket: &Path) -> InstanceState {
    InstanceState {
        device: def.name.clone(),
        caller: SHARED_CALLER.to_string(),
        run: SHARED_RUN.to_string(),
        shape: def.shape.name().to_string(),
        pid: inst.pid,
        started: crate::state::now(),
        socket: socket.to_path_buf(),
        scope: inst.scope.clone(),
        backend: String::new(),
        log: state::log_file(env, &def.name, SHARED_CALLER),
        dir: inst.dir.clone(),
        unit: None,
        owner: None,
        users: Some(0),
    }
}

/// The listener: the socket that systemd or launchd passes, or our own.
/// None when another instance answers on the path already.
fn listener(env: &HostEnv, socket: &Path) -> Result<Option<UnixListener>> {
    if let Some(l) = crate::units::passed(super::install::SOCKET_KEY)? {
        return Ok(Some(l));
    }
    let _ = env;
    if let Some(d) = socket.parent() {
        crate::privfs::dir(d)?;
    }
    crate::sockpath::check(socket)?;
    match UnixListener::bind(socket) {
        Ok(l) => Ok(Some(l)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(socket).is_ok() {
                return Ok(None);
            }
            // The socket file of an instance that ended.
            std::fs::remove_file(socket)?;
            Ok(Some(UnixListener::bind(socket)?))
        }
        Err(e) => Err(e).with_context(|| format!("cannot listen on {}", socket.display())),
    }
}

/// Accept until the stop flag is set or the instance is idle.
fn accept_loop(l: &UnixListener, shared: &Arc<Shared>, idle: Duration) -> Result<i32> {
    l.set_nonblocking(true)?;
    let fd = l.as_raw_fd();
    let mut stopping: Option<Instant> = None;
    loop {
        if stopping.is_none() && STOP.load(Ordering::SeqCst) {
            shared.log.line("SIGTERM: no new requests");
            stopping = Some(Instant::now());
        }
        if let Some(t) = stopping {
            if shared.busy.load(Ordering::SeqCst) == 0 || t.elapsed() >= DRAIN {
                return Ok(0);
            }
        }
        let mut p = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll on our own listener.
        let n = unsafe { libc::poll(&mut p, 1, TICK.as_millis() as libc::c_int) };
        if n <= 0 {
            let no_users = shared.leases.load(Ordering::SeqCst) == 0;
            if stopping.is_none()
                && no_users
                && shared.busy.load(Ordering::SeqCst) == 0
                && shared.quiet() >= idle
            {
                shared
                    .log
                    .line(&format!("idle for {} s with no user: exit", idle.as_secs()));
                return Ok(0);
            }
            continue;
        }
        match l.accept() {
            Ok((conn, _)) => {
                let s = shared.clone();
                let stopping = stopping.is_some();
                let _ = std::thread::Builder::new()
                    .name("deviced-conn".into())
                    .spawn(move || connection(&s, conn, stopping));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                shared.log.line(&format!("accept failed: {e}"));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// One connection: a lease to hold, or a request to pass to the supervisor.
fn connection(shared: &Arc<Shared>, mut conn: UnixStream, stopping: bool) {
    let _ = conn.set_nonblocking(false);
    let req = match read_request(&mut conn, shared.def.limits.max_request) {
        Ok(r) => r,
        Err(reply) => {
            let _ = reply.write(&mut conn);
            return;
        }
    };
    let caller = req.header("paddock-caller").unwrap_or("-").to_string();
    if stopping {
        let r = Reply::paddock(503, "stopping", "the shared instance is stopping");
        let _ = r.write(&mut conn);
        shared.log.line(&format!(
            "caller {} {} refused: stopping",
            clean(&caller),
            req.path
        ));
        return;
    }
    if !shared.table_ok(req.header("paddock-table")) {
        let r = shared.reconfigured();
        let _ = r.write(&mut conn);
        shared.log.line(&format!(
            "caller {} {} refused: the table changed (this instance serves {})",
            clean(&caller),
            req.path,
            shared.table
        ));
        return;
    }
    if req.method == "GET" && req.path == LEASE_PATH {
        hold_lease(shared, conn, &caller);
        return;
    }
    shared.busy.fetch_add(1, Ordering::SeqCst);
    shared.touch();
    let start = Instant::now();
    let out = pass_through(shared, &mut conn, &req);
    shared.touch();
    shared.busy.fetch_sub(1, Ordering::SeqCst);
    match out {
        Ok(n) => shared.log.line(&format!(
            "caller {} {} {} in {} out {n} ms {}",
            clean(&caller),
            req.method,
            req.path,
            req.body.len(),
            start.elapsed().as_millis()
        )),
        Err(e) => {
            shared
                .log
                .line(&format!("caller {} {}: {e}", clean(&caller), req.path));
            // A stop cuts the request at work: the broker learns why.
            let r = if STOP.load(Ordering::SeqCst) {
                Reply::paddock(
                    503,
                    "stopping",
                    "the shared instance stopped while this request ran",
                )
            } else {
                Reply::paddock(503, "failed", "the device program is not reachable")
            };
            let _ = r.write(&mut conn);
        }
    }
}

fn clean(s: &str) -> String {
    crate::broker::clean_text(s)
}

/// Count the lease, answer the head, and hold the connection until the
/// broker closes it. Nothing more is written on it.
fn hold_lease(shared: &Arc<Shared>, mut conn: UnixStream, caller: &str) {
    let n = shared.leases.fetch_add(1, Ordering::SeqCst) + 1;
    shared.write_users();
    shared.touch();
    shared
        .log
        .line(&format!("lease taken by {}: {n} users", clean(caller)));
    let head = format!(
        "HTTP/1.1 200 OK\r\nPaddock-Status: lease\r\nPaddock-Users: {n}\r\nPaddock-Table: {}\r\n\r\n",
        shared.table
    );
    if conn.write_all(head.as_bytes()).is_ok() {
        let _ = conn.flush();
        let _ = conn.set_read_timeout(Some(TICK));
        let mut buf = [0u8; 256];
        loop {
            match conn.read(&mut buf) {
                // The broker closed the connection: its run ended, or it
                // was killed.
                Ok(0) => break,
                // A broker sends nothing on a lease. Anything it does send
                // is ignored, and a flood of it ends the lease.
                Ok(_) => break,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    if STOP.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    // The time first, then the count: the accept loop exits when the
    // count is 0 and the time is `idle` old, so the other order lets it
    // see no user and the time of the last request, and exit at once
    // instead of `idle` seconds after the last user left.
    shared.touch();
    let n = shared
        .leases
        .fetch_sub(1, Ordering::SeqCst)
        .saturating_sub(1);
    shared.write_users();
    shared
        .log
        .line(&format!("lease of {} dropped: {n} users", clean(caller)));
}

/// Pass the request to the supervisor in the device sandbox and copy its
/// answer back as it comes. The bytes that reached the broker.
fn pass_through(
    shared: &Arc<Shared>,
    conn: &mut UnixStream,
    req: &super::super::supervise::http::Req,
) -> Result<usize, String> {
    let mut up = UnixStream::connect(&shared.inner).map_err(|e| {
        format!(
            "cannot reach the supervisor at {}: {e}",
            shared.inner.display()
        )
    })?;
    let mut head = format!("{} {} HTTP/1.1\r\n", req.method, req.path);
    for (n, v) in &req.headers {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let wait = Duration::from_secs(shared.def.timeout() + 5);
    let _ = up.set_write_timeout(Some(wait));
    up.write_all(head.as_bytes())
        .and_then(|()| up.write_all(&req.body))
        .and_then(|()| up.flush())
        .map_err(|e| format!("cannot send the request to the supervisor: {e}"))?;
    let _ = up.set_read_timeout(Some(wait));
    let mut out = 0usize;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match up.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                out += n;
                if conn.write_all(&chunk[..n]).is_err() {
                    // The broker is gone (a cancel, or `kill -9`).
                    return Ok(out);
                }
                let _ = conn.flush();
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if out > 0 => {
                // The answer started: the broker gets what came and reads
                // the failure on its next read.
                shared
                    .log
                    .line(&format!("the answer stopped after {out} bytes: {e}"));
                return Ok(out);
            }
            Err(e) => return Err(format!("no answer from the supervisor: {e}")),
        }
    }
}
