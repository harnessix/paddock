//! The life of one device instance: the folder, the credential agents, the
//! `connect` listeners, the process in a systemd scope when the host has a
//! user manager, the health wait, the state file, and the stop.
//!
//! An instance is a child of the paddock process of the run. bwrap runs
//! with `--die-with-parent`, so no instance outlives its paddock process,
//! and `clear_stale` (state.rs) removes what a killed process left. The
//! supervisor's stderr (its own lines and the program's) goes to the log
//! of the instance on the host; nothing of the state folder is mounted into
//! the device sandbox.
//!
//! bwrap is started from a keeper thread that lives until bwrap ends.
//! `--die-with-parent` is the parent-death signal of the kernel, and the
//! kernel sends it when the thread that started the process ends, not
//! when the process does. An instance started on the thread of one
//! request (the first request of a `start = "first-request"` device comes
//! on a thread of its own) would die with that request. The keeper thread
//! ends with the paddock process, so the signal still ends the instance
//! then.
//!
//! `Launcher::Thread` runs the supervisor in a thread of this process with
//! no isolation, for the unit tests that have no bwrap. The spec is built
//! and checked the same way; only the launch differs.
//!
//! On macOS the launcher is Seatbelt (backend/seatbelt/device.rs): the
//! same keeper thread, no systemd scope, and the supervisor's socket in
//! the short socket folder of the device sandbox id. The supervisor ends
//! itself when its parent pid changes, so an instance does not outlive a
//! killed paddock process there either.

use super::creds::{self, CredPlan, Creds};
use super::spec::{self as sspec, InstanceCtx, SOCKET_NAME};
use super::state::{self, InstanceState};
use super::{connect, LogSink};
use crate::backend::seatbelt::device as seatbelt_device;
use crate::devices::config::DeviceDef;
use crate::devices::program::Program;
use crate::devices::supervise::{Options, Running, Supervisor};
use crate::env::HostEnv;
use anyhow::{anyhow, bail, Context as _, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// How the supervisor is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    /// In a bwrap device sandbox (the product on Linux).
    Bwrap,
    /// In a Seatbelt device sandbox (the product on macOS).
    Seatbelt,
    /// In a thread of this process, with no isolation (unit tests).
    Thread,
}

impl Launcher {
    /// The product launcher of this host: Seatbelt on macOS, else bwrap.
    pub fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::Seatbelt
        } else {
            Self::Bwrap
        }
    }

    /// The name in the state file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Bwrap => "bwrap",
            Self::Seatbelt => crate::backend::seatbelt::NAME,
            Self::Thread => "thread",
        }
    }
}

/// How long a stop waits after SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(3);

enum Process {
    /// bwrap, held by its keeper thread.
    Child(Keeper),
    Thread(Running),
}

/// The thread that started bwrap and waits for it (see the module note).
struct Keeper {
    pid: u32,
    /// False once bwrap ended.
    alive: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What the keeper thread reports once bwrap runs.
struct Launched {
    pid: u32,
    scope: Option<String>,
    stderr: Option<std::process::ChildStderr>,
}

/// One running instance. Dropping it stops the process and removes the
/// folder and the state file.
pub struct Instance {
    pub name: String,
    pub caller: String,
    pub dir: PathBuf,
    /// The socket of the supervisor on the host.
    pub socket: PathBuf,
    pub pid: u32,
    pub scope: Option<String>,
    pub log: PathBuf,
    /// How long the start took, to the health answer.
    pub start_time: Duration,
    env: HostEnv,
    spec_id: String,
    process: Option<Process>,
    _creds: Option<Creds>,
    _connect: Option<connect::Listeners>,
    /// The folder of the `connect` sockets (`N.sock`) on the host.
    connect_dir: PathBuf,
    log_thread: Option<std::thread::JoinHandle<()>>,
}

/// The scope of an instance: `paddock-dev-<caller>-<name>.scope`.
pub fn scope_name(caller: &str, name: &str) -> String {
    crate::freeze::scope_name(&format!("{caller}-{name}")).replacen(
        "paddock-sb-",
        "paddock-dev-",
        1,
    )
}

impl Instance {
    /// Start the instance of `def` for `ctx`. The error names the log
    /// command.
    pub fn start(
        env: &HostEnv,
        def: &DeviceDef,
        ctx: &InstanceCtx,
        launcher: Launcher,
    ) -> Result<Self> {
        Self::start_with(env, def, ctx, launcher, |_| {})
    }

    /// `start`, with `adjust` applied to the state file before it is
    /// written: the one write that a reader can see holds the whole record.
    /// The shared instance (devices/shared/deviced.rs) sets its run, its pid
    /// and its unit here, so no reader of `paddock ps` or of the file meets
    /// a new pid without them.
    pub fn start_with(
        env: &HostEnv,
        def: &DeviceDef,
        ctx: &InstanceCtx,
        launcher: Launcher,
        adjust: impl FnOnce(&mut InstanceState),
    ) -> Result<Self> {
        let t = Instant::now();
        // A shared instance has one log, with no sandbox of its own
        // (devices/shared.rs).
        let hint = if ctx.caller == state::SHARED_CALLER {
            format!("see paddock device logs {}", def.name)
        } else {
            format!(
                "see paddock device logs {} --sandbox {}",
                def.name, ctx.caller
            )
        };
        state::clear_stale(env);
        Self::start_inner(env, def, ctx, launcher, t, adjust).map_err(|e| anyhow!("{e:#}; {hint}"))
    }

    fn start_inner(
        env: &HostEnv,
        def: &DeviceDef,
        ctx: &InstanceCtx,
        launcher: Launcher,
        t: Instant,
        adjust: impl FnOnce(&mut InstanceState),
    ) -> Result<Self> {
        let dir = ctx.dir.clone();
        state::remove_dir(&dir);
        for d in [
            dir.clone(),
            dir.join("home"),
            dir.join("empty"),
            dir.join("tmp"),
        ] {
            crate::privfs::dir(&d).with_context(|| format!("cannot create {}", d.display()))?;
        }
        // The sockets: in the instance folder, or in the short socket
        // folders of the device sandbox id (seatbelt/device.rs). Seatbelt
        // always uses the short folders; the thread and bwrap launchers use
        // them for the host-side sockets when the instance folder is too
        // deep for a socket address, as a run folder in a long macOS
        // temporary folder is.
        let seatbelt_sockets = match launcher {
            Launcher::Seatbelt => Some(seatbelt_device::Sockets::of(env, def, ctx)?),
            // Only when they fit: else the check below names the path of
            // the instance folder.
            Launcher::Bwrap | Launcher::Thread
                if !instance_sockets_fit(&dir, profile_connects(env, def)) =>
            {
                seatbelt_device::Sockets::of(env, def, ctx)
                    .ok()
                    .filter(short_dirs_made)
            }
            Launcher::Bwrap | Launcher::Thread => None,
        };
        let (socket, agent_dir, connect_dir) = match &seatbelt_sockets {
            // bwrap: the supervisor binds dev.sock inside the sandbox, in
            // the instance folder; only the host-side sockets move.
            Some(s) if launcher == Launcher::Bwrap => {
                (dir.join(SOCKET_NAME), s.host.clone(), s.connect())
            }
            Some(s) => (s.supervisor.clone(), s.host.clone(), s.connect()),
            None => (dir.join(SOCKET_NAME), dir.clone(), dir.join("connect")),
        };
        crate::sockpath::check(&socket)?;
        let log = state::log_file(env, &def.name, &ctx.caller);
        if let Some(d) = log.parent() {
            crate::privfs::dir(d).with_context(|| format!("cannot create {}", d.display()))?;
        }
        let profile = sspec::resolved_profile(env, def)?;
        let plan = creds::plan(env, &def.name, &profile.credentials)?;
        let paddock = std::env::current_exe().context("cannot find the paddock executable")?;
        let mut spec = sspec::spec(env, def, ctx, &plan, &paddock)?;
        if launcher == Launcher::Bwrap && seatbelt_sockets.is_some() {
            sspec::move_host_sockets(&mut spec, &dir, &agent_dir, &connect_dir);
        }
        let mut me = Self {
            name: def.name.clone(),
            caller: ctx.caller.clone(),
            dir: dir.clone(),
            socket: socket.clone(),
            pid: 0,
            scope: None,
            log: log.clone(),
            start_time: Duration::ZERO,
            env: env.clone(),
            spec_id: spec.id.clone(),
            process: None,
            _creds: None,
            _connect: None,
            connect_dir: connect_dir.clone(),
            log_thread: None,
        };
        let who = connect::Who {
            env: env.clone(),
            device: def.name.clone(),
            caller: ctx.caller.clone(),
        };
        if !profile.connect.is_empty() {
            me._connect = Some(connect::start(who, &connect_dir, &profile.connect)?);
        }
        me._creds = Some(creds::start(
            env,
            &plan,
            &dir,
            &agent_dir,
            &ctx.caller,
            &spec,
        )?);
        if launcher == Launcher::Seatbelt
            && (profile.mem.is_some() || profile.cpu_percent.is_some())
        {
            log::warn!("{}", seatbelt_device::limits_notice(&def.name));
        }
        match launcher {
            Launcher::Bwrap => me.launch_bwrap(&spec, &profile)?,
            Launcher::Seatbelt => {
                let sockets = seatbelt_sockets
                    .as_ref()
                    .expect("the Seatbelt launcher made its sockets above");
                me.launch_seatbelt(def, ctx, &plan, &spec, sockets, &paddock)?
            }
            Launcher::Thread => me.launch_thread(def, &profile, &plan, &log)?,
        }
        let ready = Duration::from_secs(def.ready);
        if let Err(e) = me.wait_ready(ready) {
            me.stop();
            bail!("the device {} did not start: {e}", def.name);
        }
        me.start_time = t.elapsed();
        let mut record = InstanceState {
            device: def.name.clone(),
            caller: ctx.caller.clone(),
            // `adjust` of devices/shared/deviced.rs makes this `shared` for
            // the one instance of a `scope = "shared"` device.
            run: "sandbox".into(),
            shape: def.shape.name().into(),
            pid: me.pid,
            started: crate::state::now(),
            socket: socket.clone(),
            scope: me.scope.clone(),
            backend: launcher.name().into(),
            log: log.clone(),
            dir: dir.clone(),
            unit: None,
            owner: None,
            users: None,
        };
        adjust(&mut record);
        record.write(env)?;
        log::info!(
            "device {}: instance for {} started in {:?} (pid {})",
            def.name,
            ctx.caller,
            me.start_time,
            me.pid
        );
        Ok(me)
    }

    /// bwrap, in a scope of its own when the host has a user manager and
    /// paddock itself is not in a scope already (a spawned child).
    fn launch_bwrap(
        &mut self,
        spec: &crate::spec::SandboxSpec,
        profile: &crate::devices::profile::SandboxProfile,
    ) -> Result<()> {
        use crate::backend::Backend as _;
        let env = &self.env;
        let launch = crate::backend::bwrap::Bwrap.render(env, spec, false)?;
        crate::run::write_files(&launch)?;
        let inherited = crate::freeze::own_scope();
        let new_scope = (inherited.is_none()
            && (env.var("DBUS_SESSION_BUS_ADDRESS").is_some() || env.runtime_dir.is_some()))
        .then(|| scope_name(&self.caller, &self.name));
        let mut cmd = match &new_scope {
            Some(_) => {
                let me = std::env::current_exe().context("cannot find the paddock executable")?;
                let mut c = Command::new(me);
                c.arg(crate::freeze::SCOPE_EXEC)
                    .arg(&launch.program)
                    .args(&launch.args);
                c
            }
            None => {
                let mut c = Command::new(&launch.program);
                c.args(&launch.args);
                c
            }
        };
        cmd.env_clear().envs(&launch.env);
        let (mem, cpu) = (profile.mem, profile.cpu_percent);
        self.launch_child(cmd, &launch.program, new_scope, inherited, spec, mem, cpu)
    }

    /// `sandbox-exec` with the profile of backend/seatbelt/device.rs: a
    /// plain child, no scope.
    fn launch_seatbelt(
        &mut self,
        def: &DeviceDef,
        ctx: &InstanceCtx,
        plan: &CredPlan,
        spec: &crate::spec::SandboxSpec,
        sockets: &seatbelt_device::Sockets,
        paddock: &std::path::Path,
    ) -> Result<()> {
        let launch = seatbelt_device::launch(&self.env, def, ctx, plan, spec, sockets, paddock)?;
        crate::run::write_files(&launch)?;
        let mut cmd = Command::new(&launch.program);
        cmd.args(&launch.args)
            .env_clear()
            .envs(&launch.env)
            .current_dir(ctx.dir.join("home"));
        self.launch_child(cmd, &launch.program, None, None, spec, None, None)
    }

    /// Start `cmd` from the keeper thread (see the module note), move it
    /// into `new_scope` when there is one, and pump its stderr to the log.
    #[allow(clippy::too_many_arguments)]
    fn launch_child(
        &mut self,
        mut cmd: Command,
        program: &std::path::Path,
        new_scope: Option<String>,
        inherited: Option<String>,
        spec: &crate::spec::SandboxSpec,
        mem: Option<u64>,
        cpu: Option<u32>,
    ) -> Result<()> {
        let env = &self.env;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        {
            use std::os::unix::process::CommandExt as _;
            // Its own group, so a stop reaches the sandbox and what it started.
            cmd.process_group(0);
        }
        let (tx, rx) = mpsc::channel::<Result<Launched>>();
        let alive = Arc::new(AtomicBool::new(true));
        let keeper_alive = alive.clone();
        let keeper_env = env.clone();
        let (device, spec_id) = (self.name.clone(), spec.id.clone());
        let program = program.to_path_buf();
        let thread = std::thread::Builder::new()
            .name(format!("device-{}", self.name))
            .spawn(move || {
                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        keeper_alive.store(false, Ordering::SeqCst);
                        let _ = tx.send(Err(anyhow!("cannot start {}: {e}", program.display())));
                        return;
                    }
                };
                let pid = child.id();
                let scope = match new_scope {
                    Some(name) => {
                        let moved = crate::freeze::wait_stopped(pid).and_then(|_| {
                            crate::freeze::move_to_scope_limited(
                                &keeper_env,
                                pid,
                                &name,
                                &spec_id,
                                mem,
                                cpu,
                            )
                        });
                        crate::freeze::resume(pid);
                        match moved {
                            Ok(()) => Some(name),
                            Err(e) => {
                                log::debug!(
                                    "device {device}: no scope for the instance, so it cannot be frozen: {e:#}"
                                );
                                None
                            }
                        }
                    }
                    None => inherited,
                };
                let _ = tx.send(Ok(Launched {
                    pid,
                    scope,
                    stderr: child.stderr.take(),
                }));
                // The keeper lives as long as bwrap: see the module note.
                let _ = child.wait();
                keeper_alive.store(false, Ordering::SeqCst);
            })
            .context("cannot start the keeper thread of the instance")?;
        let launched = match rx.recv() {
            Ok(r) => r?,
            Err(_) => bail!("the keeper thread of the instance ended before it started bwrap"),
        };
        self.pid = launched.pid;
        self.scope = launched.scope;
        if let Some(se) = launched.stderr {
            let sink = LogSink::open(&self.log)?;
            self.log_thread = Some(std::thread::spawn(move || sink.pump(se)));
        }
        self.process = Some(Process::Child(Keeper {
            pid: launched.pid,
            alive,
            thread: Some(thread),
        }));
        Ok(())
    }

    /// The supervisor in a thread, with host paths (tests).
    fn launch_thread(
        &mut self,
        def: &DeviceDef,
        profile: &crate::devices::profile::SandboxProfile,
        plan: &CredPlan,
        log: &std::path::Path,
    ) -> Result<()> {
        let mut o = Options::new(&self.socket, def.shape, def.command.clone());
        o.timeout = def.limits.timeout;
        o.max_request = def.limits.max_request;
        o.max_response = def.limits.max_response;
        o.concurrency = def.limits.concurrency;
        o.stream = def.stream;
        o.ready = def.ready;
        o.connect = profile
            .connect
            .iter()
            .enumerate()
            .map(|(n, c)| (c.local_port, self.connect_dir.join(format!("{n}.sock"))))
            .collect();
        o.log = Some(log.to_path_buf());
        o.env_pass = sspec::program_env_names(profile, plan);
        o.env = profile
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // The filtering agent of the instance, as the sandbox launchers
        // bind it: the program signs through it, never through the host's.
        if let Some(sock) = self._creds.as_ref().and_then(|c| c.agent_socket.clone()) {
            o.env
                .push(("SSH_AUTH_SOCK".into(), sock.display().to_string()));
        }
        let r = Supervisor::start(o)?.spawn();
        self.pid = std::process::id();
        self.process = Some(Process::Thread(r));
        Ok(())
    }

    /// Wait for the socket and a health answer, up to `ready`.
    fn wait_ready(&mut self, ready: Duration) -> Result<(), String> {
        let end = Instant::now() + ready.max(Duration::from_secs(1));
        while !self.socket.exists() {
            if !self.alive() {
                return Err("the supervisor exited before it listened".into());
            }
            if Instant::now() >= end {
                return Err(format!(
                    "the supervisor did not listen on its socket within {} s",
                    ready.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let p = Program::connect(&self.socket).map_err(|e| e.to_string())?;
        p.health(end.saturating_duration_since(Instant::now()))
    }

    /// True while the process runs.
    pub fn alive(&mut self) -> bool {
        match self.process.as_mut() {
            Some(Process::Child(k)) => k.alive.load(Ordering::SeqCst),
            Some(Process::Thread(_)) => self.socket.exists(),
            None => false,
        }
    }

    /// SIGTERM, three seconds, SIGKILL; then the agents, the folder and
    /// the state file go.
    pub fn stop(&mut self) {
        match self.process.take() {
            Some(Process::Child(mut k)) => {
                let pid = k.pid as i32;
                let running = || k.alive.load(Ordering::SeqCst);
                // SAFETY: a signal to the group that the child leads.
                unsafe {
                    libc::kill(-pid, libc::SIGTERM);
                }
                let end = Instant::now() + STOP_GRACE;
                while running() && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if running() {
                    // SAFETY: as above, then the leader itself.
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
                // The keeper reaps bwrap and ends.
                if let Some(t) = k.thread.take() {
                    let _ = t.join();
                }
                if let Some(unit) = &self.scope {
                    kill_scope(unit);
                }
            }
            Some(Process::Thread(mut r)) => r.stop(),
            None => {}
        }
        if let Some(t) = self.log_thread.take() {
            let _ = t.join();
        }
        self._creds.take();
        self._connect.take();
        InstanceState::remove(&self.env, &self.name, &self.caller);
        state::remove_dir(&self.dir);
        let _ = std::fs::remove_dir_all(self.env.paddock_run().join(&self.spec_id));
        // macOS: the socket folders of the device sandbox id.
        seatbelt_device::remove_sockets(&self.env, &self.spec_id);
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The number of `connect` entries of the resolved profile of `def`, 0
/// when the profile does not resolve (the start names that error later).
fn profile_connects(env: &HostEnv, def: &DeviceDef) -> usize {
    sspec::resolved_profile(env, def).map_or(0, |p| p.connect.len())
}

/// Every socket of an instance in its own folder `dir` fits a socket
/// address: `dev.sock`, the ssh agent socket and `connect/N.sock` for each
/// of `connects` entries.
fn instance_sockets_fit(dir: &std::path::Path, connects: usize) -> bool {
    crate::sockpath::check(&dir.join(SOCKET_NAME)).is_ok()
        && crate::sockpath::check(&dir.join(creds::AGENT_SOCKET)).is_ok()
        && (connects == 0
            || crate::sockpath::check(&dir.join("connect").join(format!("{}.sock", connects - 1)))
                .is_ok())
}

/// The two short socket folders exist: `sockpath` makes them on macOS,
/// and this makes them on Linux, where only the thread launcher uses them.
fn short_dirs_made(s: &seatbelt_device::Sockets) -> bool {
    [s.supervisor.parent(), Some(s.host.as_path())]
        .into_iter()
        .flatten()
        .all(|d| crate::privfs::dir(d).is_ok())
}

/// What is left in a scope after the group is gone: a process that left
/// the group. Best effort.
fn kill_scope(unit: &str) {
    if !crate::freeze::ours(unit) {
        return;
    }
    let _ = Command::new("systemctl")
        .args(["--user", "kill", "--signal=SIGKILL", unit])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_socket_of_the_instance_folder_must_fit() {
        let max = crate::sockpath::max_len();
        // A folder where `/ssh-agent.sock` (the longest fixed name) fits
        // with 1 byte to spare.
        let dir = PathBuf::from(format!("/{}", "d".repeat(max - 17)));
        assert!(crate::sockpath::check(&dir.join(creds::AGENT_SOCKET)).is_ok());
        assert!(instance_sockets_fit(&dir, 0));
        // connect/0.sock to connect/9.sock fit; connect/10.sock does not.
        assert!(instance_sockets_fit(&dir, 10));
        assert!(!instance_sockets_fit(&dir, 11));
        // One byte deeper: the agent socket no longer fits.
        let deeper = PathBuf::from(format!("/{}", "d".repeat(max - 16)));
        assert!(!instance_sockets_fit(&deeper, 0));
        assert!(instance_sockets_fit(std::path::Path::new("/r/i"), 12));
    }
}
