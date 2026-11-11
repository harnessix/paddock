//! `run = "host"`: the device program runs on the host as the user.
//!
//! `HostSource` is the `InstanceSource` of outproc.rs for a host device.
//! It starts `paddock device-serve --host` on the host, as the user, with
//! the whole environment that paddock got (`env.vars`) and the fixed
//! `env` values of the table, in `cwd` (the home by default), on the
//! socket `dev-NAME.sock` of the run's socket folder. One supervisor per
//! run, started at the first request, ended with the run. For `exec` the
//! supervisor forks the command per request; for `serve` it starts the
//! program once. The supervisor is the same process as in a device
//! sandbox, without a sandbox: one code path for the envelope, the exit
//! mapping, the timeout, the process group kill, the log and the
//! concurrency. The cost is one more process per run.
//!
//! The supervisor is started from a keeper thread that lives as long as
//! the source: on Linux the supervisor asks for SIGTERM when its parent
//! thread dies, and a request runs on a thread of its own.
//!
//! The marks: the catalog text (`info`), the card (`user::Gate`), the
//! state file with `run: host` (state.rs, for `paddock ps`), the audit
//! field `run`, `paddock device ls` and `paddock doctor`.

use super::config::{DeviceDef, Run, Shape};
use super::outproc::InstanceSource;
use super::sandbox::state::{self, InstanceState};
use super::{DeviceCtx, Info};
use crate::env::HostEnv;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// The first words of the catalog text and of the card of a host device.
pub const HOST_MARK: &str = "Runs on the host as you, with your home, your keys and your network. ";

/// How long the stop waits after SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// How often the keeper looks at the supervisor.
const TICK: Duration = Duration::from_millis(100);

/// What `.catalog` says about a host device.
pub fn info(def: &DeviceDef) -> Info {
    let (request, response) = match def.shape {
        Shape::Serve => (
            format!(
                "{HOST_MARK}bytes for the service `{}` (at most {} bytes).",
                def.command.join(" "),
                def.limits.max_request
            ),
            format!(
                "the bytes of its answer (at most {} bytes)",
                def.limits.max_response
            ),
        ),
        _ => (
            format!(
                "{HOST_MARK}bytes for the standard input of the command `{}`, which runs in {} (at most {} bytes). The request never adds arguments. The command runs for at most {} seconds.",
                def.command.join(" "),
                cwd_text(def),
                def.limits.max_request,
                def.timeout()
            ),
            format!(
                "header lines, an empty line, then the standard output (at most {} bytes). Header lines: status: exit CODE, signal NUMBER, timeout, error or disabled; stderr: LINE (the first {} lines of the standard error); message: TEXT. EACCES: the user declined the device. EDQUOT: the budget of a paid device is used.",
                def.limits.max_response,
                super::supervise::exec::STDERR_LINES
            ),
        ),
    };
    Info {
        request,
        response,
        limits: super::user::limits(def),
        shape: def.shape.name(),
        stream: def.stream,
    }
}

/// `cwd` of the table as text: the folder, or "your home".
pub fn cwd_text(def: &DeviceDef) -> String {
    match &def.run {
        Run::Host { cwd: Some(c), .. } => c.display().to_string(),
        _ => "your home".into(),
    }
}

/// The one line of the card: the command, where it runs, what it gets.
pub fn describe(def: &DeviceDef) -> String {
    let what = match def.shape {
        Shape::Serve => "the service",
        _ => "the command",
    };
    let input = match def.shape {
        Shape::Serve => "",
        _ => ", the request on its standard input",
    };
    format!(
        "{what} `{}` ON THE HOST as you, with your home, your keys and your network, in {}{input}",
        def.command.join(" "),
        cwd_text(def)
    )
}

/// The warning line of `paddock device check` and `paddock doctor`.
pub fn warning(def: &DeviceDef) -> String {
    format!(
        "{} runs ON THE HOST as you, with your home, your keys and your network: `{}` in {}",
        def.name,
        def.command.join(" "),
        cwd_text(def)
    )
}

/// The supervisor of one host device for one run.
struct Keeper {
    pid: u32,
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Keeper {
    fn end(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.end();
    }
}

/// The instance source of a host device.
pub struct HostSource {
    def: DeviceDef,
    env: HostEnv,
    /// The sandbox that the instance serves, the instance name.
    caller: String,
    /// The paddock binary that runs `device-serve`.
    binary: PathBuf,
    keeper: Mutex<Option<Keeper>>,
}

impl HostSource {
    /// A source for `def`, serving the sandbox of `ctx`. The supervisor is
    /// this paddock binary.
    pub fn new(def: DeviceDef, ctx: &DeviceCtx) -> Self {
        let binary = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("paddock"));
        Self::with_binary(def, &ctx.env, &ctx.sandbox, binary)
    }

    /// `new` with the paddock binary named: tests run the lib without one.
    pub fn with_binary(def: DeviceDef, env: &HostEnv, caller: &str, binary: PathBuf) -> Self {
        Self {
            def,
            env: env.clone(),
            caller: caller.to_string(),
            binary,
            keeper: Mutex::new(None),
        }
    }

    /// The host that the program runs on.
    pub fn env(&self) -> &HostEnv {
        &self.env
    }

    /// The device name.
    pub fn name(&self) -> &str {
        &self.def.name
    }

    /// The table of the device.
    pub fn def(&self) -> &DeviceDef {
        &self.def
    }

    /// The working folder of the program: `cwd` with `~` expanded, or
    /// the home.
    pub fn cwd(&self) -> PathBuf {
        match &self.def.run {
            Run::Host { cwd: Some(c), .. } => {
                self.env.expand(&c.display().to_string(), &self.env.home)
            }
            _ => self.env.home.clone(),
        }
    }

    /// The socket of the instance: `dev-NAME.sock` in the socket folder
    /// of the run.
    pub fn socket_path(&self) -> Result<PathBuf, String> {
        crate::sockpath::named(
            &self.env,
            &self.caller,
            &format!("dev-{}.sock", self.def.name),
        )
        .map_err(|e| format!("{e:#}"))
    }

    /// The log of the instance.
    pub fn log(&self) -> PathBuf {
        state::log_file(&self.env, &self.def.name, &self.caller)
    }

    /// The instance folder: empty for a host device, there so that `paddock
    /// ps` and `clear_stale` treat it as any instance.
    pub fn dir(&self) -> PathBuf {
        state::instance_dir(&self.env, &self.def.name, &self.caller)
    }

    /// The sandbox that the instance serves.
    pub fn caller(&self) -> &str {
        &self.caller
    }

    /// The pid of the running supervisor.
    pub fn pid(&self) -> Option<u32> {
        let g = self.keeper.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref()
            .filter(|k| k.alive.load(Ordering::SeqCst))
            .map(|k| k.pid)
    }

    /// The command line of the supervisor.
    pub(crate) fn command(&self, socket: &Path, log: &Path) -> Command {
        let d = &self.def;
        let mut c = Command::new(&self.binary);
        c.arg("device-serve")
            .arg("--socket")
            .arg(socket)
            .arg("--shape")
            .arg(d.shape.name())
            .arg("--timeout")
            .arg(d.limits.timeout.to_string())
            .arg("--max-request")
            .arg(d.limits.max_request.to_string())
            .arg("--max-response")
            .arg(d.limits.max_response.to_string())
            .arg("--concurrency")
            .arg(d.limits.concurrency.to_string())
            .arg("--ready")
            .arg(d.ready.to_string())
            .arg("--log")
            .arg(log)
            .arg("--host")
            .arg("--cwd")
            .arg(self.cwd());
        if d.stream {
            c.arg("--stream");
        }
        c.arg("--").args(&d.command);
        c.env_clear().envs(&self.env.vars);
        if let Run::Host { env, .. } = &d.run {
            c.envs(env);
        }
        c.stdin(Stdio::null()).stdout(Stdio::null());
        c
    }

    /// Start the supervisor from a keeper thread and wait for its pid.
    fn start(&self) -> Result<Keeper, String> {
        let socket = self.socket_path()?;
        let dir = socket
            .parent()
            .ok_or_else(|| format!("{}: a socket path needs a folder", socket.display()))?;
        crate::privfs::dir(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let log = self.log();
        if let Some(p) = log.parent() {
            crate::privfs::dir(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        }
        let inst_dir = self.dir();
        crate::privfs::dir(&inst_dir)
            .map_err(|e| format!("cannot create {}: {e}", inst_dir.display()))?;
        let stderr = crate::privfs::append(&log)
            .map_err(|e| format!("cannot open the log {}: {e}", log.display()))?;
        let mut cmd = self.command(&socket, &log);
        cmd.stderr(stderr);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel::<Result<u32, String>>();
        let (stop2, alive2) = (stop.clone(), alive.clone());
        let env = self.env.clone();
        let (name, caller) = (self.def.name.clone(), self.caller.clone());
        let record = InstanceState {
            device: name.clone(),
            caller: caller.clone(),
            run: "host".into(),
            shape: self.def.shape.name().into(),
            pid: 0,
            started: crate::state::now(),
            socket: socket.clone(),
            scope: None,
            backend: "host".into(),
            log: log.clone(),
            dir: inst_dir.clone(),
            unit: None,
            owner: None,
            users: None,
        };
        let thread = std::thread::Builder::new()
            .name(format!("device-host-{name}"))
            .spawn(move || {
                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Err(format!("cannot start the supervisor: {e}")));
                        alive2.store(false, Ordering::SeqCst);
                        return;
                    }
                };
                let pid = child.id();
                let mut rec = record;
                rec.pid = pid;
                if let Err(e) = rec.write(&env) {
                    log::warn!("device {name}: cannot write the state file: {e:#}");
                }
                let _ = tx.send(Ok(pid));
                let mut term_at: Option<Instant> = None;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {}
                        Err(_) => break,
                    }
                    if stop2.load(Ordering::SeqCst) {
                        match term_at {
                            None => {
                                // SAFETY: a signal to the supervisor.
                                unsafe {
                                    libc::kill(pid as i32, libc::SIGTERM);
                                }
                                term_at = Some(Instant::now());
                            }
                            Some(t) if t.elapsed() >= STOP_GRACE => {
                                let _ = child.kill();
                                let _ = child.wait();
                                break;
                            }
                            Some(_) => {}
                        }
                    }
                    std::thread::sleep(TICK);
                }
                alive2.store(false, Ordering::SeqCst);
                InstanceState::remove(&env, &name, &caller);
                state::remove_dir(&inst_dir);
            })
            .map_err(|e| format!("cannot start the keeper thread: {e}"))?;
        let mut keeper = Keeper {
            pid: 0,
            socket,
            stop,
            alive,
            thread: Some(thread),
        };
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(pid)) => keeper.pid = pid,
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("the supervisor did not start within 10 s".into()),
        }
        Ok(keeper)
    }

    /// Stop the supervisor, if it runs. True when one ran.
    pub fn stop(&self) -> bool {
        let mut g = self.keeper.lock().unwrap_or_else(|e| e.into_inner());
        match g.take() {
            Some(mut k) => {
                k.end();
                true
            }
            None => false,
        }
    }

    /// Start the supervisor when it is not running. The time the start
    /// took, or zero when it ran already, and the socket.
    pub fn ensure_started(&self) -> Result<(Duration, PathBuf), String> {
        let mut g = self.keeper.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = g.as_mut() {
            if k.alive.load(Ordering::SeqCst) {
                return Ok((Duration::ZERO, k.socket.clone()));
            }
            // The supervisor is gone (the user killed it): start again.
            k.end();
            *g = None;
        }
        let t = Instant::now();
        let k = self.start()?;
        let sock = k.socket.clone();
        *g = Some(k);
        Ok((t.elapsed(), sock))
    }
}

impl InstanceSource for HostSource {
    fn socket(&self, _ctx: &DeviceCtx) -> Result<PathBuf, String> {
        self.ensure_started().map(|(_, s)| s)
    }

    fn instance(&self) -> String {
        "host".into()
    }

    fn log_path(&self) -> Option<PathBuf> {
        Some(self.log())
    }
}

impl Drop for HostSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
