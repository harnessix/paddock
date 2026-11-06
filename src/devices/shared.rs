//! `scope = "shared"`: one device instance per user for every sandbox that
//! has the grant.
//!
//! A model server that takes a minute to load, a cache or a daemon with a
//! warm state must not start again for each run. A shared device runs once
//! per user: the first broker that needs it starts `paddock deviced NAME`,
//! every broker of every run connects to the one socket
//! `$XDG_RUNTIME_DIR/paddock/devices/NAME.sock`, and the instance ends when
//! its last user ended and `idle` seconds passed without a request.
//!
//! The pieces:
//!
//! - [`ensure`] is what a broker calls. It connects to the socket when
//!   something serves there already, and otherwise starts the instance: the
//!   installed socket unit or LaunchAgent starts it on the first connection
//!   (install.rs), a `systemd-run --user` transient unit starts it when a
//!   user manager runs without an installed unit, and without either the
//!   first broker starts it as a child of its own paddock process and the
//!   state file names that sandbox as the `owner`.
//! - `paddock deviced NAME` (deviced.rs) is the instance: it owns the
//!   socket, starts the device sandbox around the supervisor of
//!   supervise.rs, counts the leases, and exits when it is idle.
//! - [`SharedSource`] (source.rs) is the `InstanceSource` of outproc.rs
//!   for a shared device. It holds one lease for the life of the run, so
//!   the instance knows how many runs use it, and a broker that dies by
//!   `kill -9` drops its lease by the close of that connection.
//! - [`table_hash`] is the view of the table that the broker sends in
//!   `Paddock-Table` on each request. An instance that reads another table
//!   answers `503` with `Paddock-Status: reconfigured`, so an edit of the
//!   user configuration can never be served half old and half new.
//!
//! What a shared device cannot have: a `caller` mount (config.rs refuses
//! it, because a mount of a running sandbox cannot change while the
//! instance serves other sandboxes), and users other than the one who owns
//! the runtime folder (the socket folder is that folder, mode 0700).

pub mod deviced;
pub mod install;
pub mod source;

pub use source::{Lease, Refusal, SharedSource};

use super::config::{DeviceDef, Scope};
use super::program::Program;
use super::sandbox::state::{InstanceState, SHARED_CALLER, SHARED_RUN};
use crate::env::HostEnv;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The folder of the shared device sockets under the runtime folder.
pub const SOCKET_DIR: &str = "devices";

/// `$XDG_RUNTIME_DIR/paddock/devices/NAME.sock`: where every broker of this
/// user reaches the shared instance of `name`.
///
/// macOS has no `XDG_RUNTIME_DIR`, and launchd must make the socket at
/// login, before any paddock ran, so the path has to exist then and has to
/// fit in the 104 bytes of `sun_path`. [`state_socket`] is that path, as
/// the gateway executor's is (gatewayd.rs). A home so deep that even that
/// path does not fit falls back to the short socket folder of `sockpath`.
pub fn socket_path(env: &HostEnv, name: &str) -> PathBuf {
    if cfg!(target_os = "macos") {
        let p = state_socket(env, name);
        if crate::sockpath::check(&p).is_ok() {
            return p;
        }
        return crate::sockpath::named_or_run(env, "", &format!("dev-{name}.sock"));
    }
    env.paddock_run()
        .join(SOCKET_DIR)
        .join(format!("{name}.sock"))
}

/// `<paddock state>/devices/NAME.sock`: stable and short, and its folder
/// exists before a login, when launchd makes the socket (macOS).
pub fn state_socket(env: &HostEnv, name: &str) -> PathBuf {
    env.paddock_state()
        .join(SOCKET_DIR)
        .join(format!("{name}.sock"))
}

/// `ListenStream=` of the socket unit: the same path as [`socket_path`],
/// with the runtime folder as the specifier `%t` that systemd expands.
pub fn listen_stream(name: &str) -> String {
    format!("%t/paddock/{SOCKET_DIR}/{name}.sock")
}

/// The lock that makes two brokers start one instance, not two: an
/// exclusive `flock` beside the socket. The socket unit and the LaunchAgent
/// need no lock, because the user manager owns the socket.
fn lock_path(env: &HostEnv, name: &str) -> PathBuf {
    let sock = socket_path(env, name);
    sock.with_file_name(format!(
        "{}.lock",
        sock.file_stem().unwrap_or_default().to_string_lossy()
    ))
}

/// The view of the table that a broker had when it first used the device.
/// The instance compares it with its own view and refuses to serve another
/// one. The rendered table covers every key of the device and of its
/// sandbox profile, so a changed `timeout`, mount or command changes it.
pub fn table_hash(def: &DeviceDef) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(def.to_toml().as_bytes());
    crate::manifest::hex(&h[..8])
}

/// Why every request of a run fails after the table changed under a
/// running shared instance. `paddock device restart NAME` clears it.
pub const RECONFIGURED_WHY: &str = "the device was reconfigured since this run started";

/// The notice that `ensure` returns when no user manager owns the
/// instance: it ends with the run that started it.
pub fn no_manager_notice(name: &str, owner: &str) -> String {
    format!(
        "the shared device {name} has no user manager here, so the sandbox {} owns its instance: it ends when that run ends. `paddock device install {name}` hands it to systemd or launchd",
        crate::ids::short(owner)
    )
}

/// How long `ensure` waits for the socket to appear after it started the
/// instance, on top of the device's own `ready`.
const START_GRACE: Duration = Duration::from_secs(5);

/// Test only: make `ensure` act as if this host had no user manager, so the
/// owner path of the Linux tests runs on a host that has systemd too.
const NO_MANAGER_VAR: &str = "PADDOCK_TEST_NO_USER_MANAGER";
/// Test only: the launcher of the device sandbox inside `paddock deviced`.
/// `thread` runs the supervisor in a thread with no isolation, for the
/// tests of hosts without bubblewrap. deviced logs which one it used.
pub const LAUNCHER_VAR: &str = "PADDOCK_TEST_DEVICE_LAUNCHER";

/// How the instance of `name` is started on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// A `.socket` unit of `paddock device install` owns the socket: the
    /// connection starts the service.
    SocketUnit(String),
    /// A LaunchAgent of `paddock device install` owns the socket.
    LaunchAgent(String),
    /// `systemd-run --user --unit paddock-device-NAME`.
    Transient(String),
    /// A child of this paddock process, which ends with the run.
    Owned,
}

impl Start {
    /// The unit or the label that owns the instance, for the state file.
    pub fn unit(&self) -> Option<String> {
        match self {
            Start::SocketUnit(u) | Start::LaunchAgent(u) | Start::Transient(u) => Some(u.clone()),
            Start::Owned => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Start::SocketUnit(_) => "socket unit",
            Start::LaunchAgent(_) => "launch agent",
            Start::Transient(_) => "systemd-run",
            Start::Owned => "owned by the run",
        }
    }
}

/// How this host starts the shared instance of `name`.
pub fn start_kind(env: &HostEnv, name: &str) -> Start {
    if install::installed(env, name) {
        return if cfg!(target_os = "macos") {
            Start::LaunchAgent(install::label(name))
        } else {
            Start::SocketUnit(install::socket_unit_name(name))
        };
    }
    if user_manager(env) {
        return Start::Transient(install::service_name(name));
    }
    Start::Owned
}

/// A systemd user manager that can run a transient unit: `systemd-run` is
/// on the path and the session bus is known.
fn user_manager(env: &HostEnv) -> bool {
    if cfg!(target_os = "macos") || env.var(NO_MANAGER_VAR).is_some() {
        return false;
    }
    env.var("DBUS_SESSION_BUS_ADDRESS").is_some() && env.which("systemd-run").is_some()
}

/// What `ensure` did: the socket to use and the notice to log when the
/// instance belongs to one run.
#[derive(Debug)]
pub struct Ensured {
    pub socket: PathBuf,
    /// The instance was started by this call.
    pub started: Option<Duration>,
    pub how: Start,
    pub notice: Option<String>,
}

/// The socket of the shared instance of `def`, started when nothing serves
/// there yet. `owner` is the sandbox of the broker that asks, which owns the
/// instance on a host without a user manager.
pub fn ensure(env: &HostEnv, def: &DeviceDef, owner: &str) -> Result<Ensured, String> {
    if def.scope != Scope::Shared {
        return Err(format!(
            "[devices.{}]: ensure is for scope = \"shared\" devices",
            def.name
        ));
    }
    let socket = socket_path(env, &def.name);
    let how = start_kind(env, &def.name);
    // A unit that owns the socket starts the instance on the connection.
    // Without one, a live socket means another broker started it already.
    if alive_at(&socket, &how) {
        return Ok(Ensured {
            socket,
            started: None,
            how,
            notice: None,
        });
    }
    let t = Instant::now();
    // One instance, not one per broker: the lock is held across the start
    // and the health wait, and whoever waits for it finds the socket live.
    let _lock = Lock::take(env, &def.name)?;
    if alive_at(&socket, &how) {
        return Ok(Ensured {
            socket,
            started: None,
            how,
            notice: None,
        });
    }
    let notice = match &how {
        Start::SocketUnit(unit) => {
            return Err(format!(
                "the socket unit {unit} is installed but nothing listens on {}: systemctl --user enable --now {unit}",
                socket.display()
            ))
        }
        Start::LaunchAgent(label) => {
            return Err(format!(
                "the LaunchAgent {label} is installed but nothing listens on {}: paddock device install {}",
                socket.display(),
                def.name
            ))
        }
        Start::Transient(unit) => match start_transient(env, def, &socket, unit) {
            Ok(()) => None,
            // A user manager that refuses the unit must not cost the run
            // its device: this broker owns the instance instead, and says
            // why.
            Err(e) => {
                log::warn!("device {}: {e}", def.name);
                start_owned(env, def, &socket, owner)?;
                Some(format!(
                    "{e}. {}",
                    no_manager_notice(&def.name, owner)
                ))
            }
        },
        Start::Owned => {
            start_owned(env, def, &socket, owner)?;
            Some(no_manager_notice(&def.name, owner))
        }
    };
    wait_ready(&socket, Duration::from_secs(def.ready) + START_GRACE)?;
    Ok(Ensured {
        socket,
        started: Some(t.elapsed()),
        how,
        notice,
    })
}

/// Something answers on `socket`. With a unit that owns the socket, the
/// connection itself starts the service, so a connection that opens is
/// enough; without one, a socket file of an instance that died refuses the
/// connection and is removed.
fn alive_at(socket: &Path, how: &Start) -> bool {
    match UnixStream::connect(socket) {
        Ok(_) => true,
        Err(_) => {
            if matches!(how, Start::Transient(_) | Start::Owned) && socket.exists() {
                // The instance is gone and left its socket file: the next
                // bind needs the path free.
                let _ = std::fs::remove_file(socket);
            }
            false
        }
    }
}

/// How long a start waits for the user manager to unload the unit of the
/// same name that just stopped. `--collect` unloads a transient unit once
/// it is inactive, in the next turn of the manager's loop, and
/// `systemd-run` refuses a name that is still loaded.
const UNLOAD_WAIT: Duration = Duration::from_secs(10);

/// `systemd-run --user`: a transient service that outlives every run.
///
/// The unit of the last instance can still be loaded when the next broker
/// comes, right after `systemctl --user stop` or `paddock device restart`
/// ended it: the manager unloads it a moment later, and a failed one stays
/// until `reset-failed`. A start that met that unit would fall back to an
/// instance that the run owns, or that `paddock device restart` owns and
/// ends with the command. So the start waits for the unload first, and a
/// `systemd-run` that still finds the name taken tries once more after
/// another wait.
fn start_transient(
    env: &HostEnv,
    def: &DeviceDef,
    socket: &Path,
    unit: &str,
) -> Result<(), String> {
    wait_unloaded(unit);
    match systemd_run(env, def, socket, unit) {
        Err(e) if unit_exists(&e) => {
            log::debug!("device {}: {e}; waiting for the unit to go", def.name);
            wait_unloaded(unit);
            systemd_run(env, def, socket, unit)
        }
        other => other,
    }
}

/// The error of `systemd-run` for a name that the manager still has.
fn unit_exists(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("already loaded") || e.contains("already exists")
}

/// `LoadState` and `ActiveState` of `unit`, from `systemctl --user show`.
/// `("not-found", "inactive")` when the manager has no such unit, and
/// None when `systemctl` cannot be asked.
fn unit_states(unit: &str) -> Option<(String, String)> {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "show", "-p", "LoadState,ActiveState", "--"])
        .arg(format!("{unit}.service"))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_unit_states(&String::from_utf8_lossy(&out.stdout)))
}

/// The `LoadState=` and `ActiveState=` lines of `systemctl show`.
fn parse_unit_states(text: &str) -> (String, String) {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .unwrap_or("")
            .trim()
            .to_string()
    };
    (field("LoadState"), field("ActiveState"))
}

/// Wait up to [`UNLOAD_WAIT`] for the manager to unload a unit of this
/// name that is on its way out. A failed one is cleared with
/// `reset-failed`. A unit that is active is left alone: something else is
/// starting or running it, and `systemd-run` says so.
fn wait_unloaded(unit: &str) {
    let end = Instant::now() + UNLOAD_WAIT;
    let mut reset = false;
    loop {
        let Some((load, active)) = unit_states(unit) else {
            return;
        };
        if load != "loaded" {
            return;
        }
        match active.as_str() {
            "failed" if !reset => {
                reset = true;
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "reset-failed", "--"])
                    .arg(format!("{unit}.service"))
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            "inactive" | "deactivating" | "failed" => {}
            _ => return,
        }
        if Instant::now() >= end {
            log::debug!("the unit {unit} is still {load} and {active} after {UNLOAD_WAIT:?}");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One `systemd-run --user` of the instance.
fn systemd_run(env: &HostEnv, def: &DeviceDef, socket: &Path, unit: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new("systemd-run");
    cmd.args(["--user", "--quiet", "--collect"])
        .arg(format!("--unit={unit}"))
        .arg(format!("--description=paddock device {}", def.name))
        .arg("--property=Restart=no")
        .arg("--property=KillMode=mixed");
    for (k, v) in unit_env(env) {
        cmd.arg(format!("--setenv={k}={v}"));
    }
    // The runtime folder of this broker, which holds the socket and the
    // instance folder. The user manager's own can differ (a test, or a
    // shell that set another one).
    if let Some(rt) = &env.runtime_dir {
        cmd.arg(format!("--setenv=XDG_RUNTIME_DIR={}", rt.display()));
    }
    cmd.arg("--");
    for a in deviced_argv(env, &def.name, socket, None) {
        cmd.arg(a);
    }
    cmd.arg("--unit").arg(unit);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run systemd-run --user: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "systemd-run --user --unit={unit} failed with {}: {}",
            out.status,
            crate::broker::clean_text(String::from_utf8_lossy(&out.stderr).trim())
        ));
    }
    Ok(())
}

/// A child of this paddock process. It ends with this process, so the
/// instance ends with the run that started it.
fn start_owned(env: &HostEnv, def: &DeviceDef, socket: &Path, owner: &str) -> Result<(), String> {
    if let Some(d) = socket.parent() {
        crate::privfs::dir(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    let argv = deviced_argv(env, &def.name, socket, Some(owner));
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(&env.vars)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start paddock deviced {}: {e}", def.name))?;
    // Reap the instance when it exits, so it leaves no zombie behind in a
    // run that lasts for hours.
    let _ = std::thread::Builder::new()
        .name(format!("deviced-{}-reaper", def.name))
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

/// The command line of the instance.
pub fn deviced_argv(
    env: &HostEnv,
    name: &str,
    socket: &Path,
    owner: Option<&str>,
) -> Vec<std::ffi::OsString> {
    let paddock = crate::launchers::launcher_exe(env)
        .or_else(|_| std::env::current_exe())
        .unwrap_or_else(|_| PathBuf::from("paddock"));
    let mut v: Vec<std::ffi::OsString> = vec![
        paddock.into(),
        "deviced".into(),
        name.into(),
        "--socket".into(),
        socket.into(),
    ];
    if let Some(o) = owner {
        v.push("--owner".into());
        v.push(o.into());
    }
    v
}

/// The variables that the instance needs to read the same configuration as
/// paddock: the folders of the user and the runtime folder of the socket.
pub fn unit_env(env: &HostEnv) -> Vec<(String, String)> {
    let mut v = crate::units::xdg_env(env);
    for k in [NO_MANAGER_VAR, LAUNCHER_VAR] {
        if let Some(val) = env.var(k) {
            v.push((k.to_string(), val.to_string()));
        }
    }
    v
}

/// Wait for the socket of the instance to answer `/v1/health`.
fn wait_ready(socket: &Path, wait: Duration) -> Result<(), String> {
    let end = Instant::now() + wait;
    while UnixStream::connect(socket).is_err() {
        if Instant::now() >= end {
            return Err(format!(
                "the shared instance did not listen on {} within {} s",
                socket.display(),
                wait.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let p = Program::connect(socket).map_err(|e| format!("{e:#}"))?;
    p.health(
        end.saturating_duration_since(Instant::now())
            .max(Duration::from_secs(1)),
    )
}

/// An exclusive `flock` on the lock file beside the socket, held while one
/// broker starts the instance.
struct Lock(std::fs::File);

impl Lock {
    fn take(env: &HostEnv, name: &str) -> Result<Self, String> {
        let path = lock_path(env, name);
        if let Some(d) = path.parent() {
            crate::privfs::dir(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        use std::os::fd::AsRawFd as _;
        let fd = f.as_raw_fd();
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            // SAFETY: flock on a descriptor that this function owns.
            let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                return Ok(Self(f));
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::WouldBlock {
                return Err(format!("cannot lock {}: {e}", path.display()));
            }
            if Instant::now() >= end {
                return Err(format!(
                    "another broker has been starting the shared instance of {name} for a minute"
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd as _;
        // SAFETY: the descriptor is ours and open until this returns.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// `paddock device stop NAME`: end the shared instance now. True when one
/// ran. Open requests of other brokers get `503`.
pub fn stop_instance(env: &HostEnv, name: &str) -> Result<bool, String> {
    let Some(st) = InstanceState::shared_of(env, name).filter(|s| s.alive()) else {
        // No instance, but a unit may hold the socket: stop the service so
        // the next connection starts a fresh one.
        stop_unit(env, name);
        return Ok(false);
    };
    stop_unit(env, name);
    let pid = st.pid as i32;
    if pid > 0 && pid != std::process::id() as i32 {
        // SAFETY: a signal to the instance that the state file names.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    } else if pid == std::process::id() as i32 {
        // The instance runs in this process (the tests): ask it to stop.
        deviced::ask_stop();
    }
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if InstanceState::shared_of(env, name).is_none_or(|s| !s.alive()) {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "the shared instance of {name} did not end within 10 s (pid {})",
        st.pid
    ))
}

/// Stop the unit that owns the instance, when one does, so that it does not
/// start it again before the next request.
fn stop_unit(env: &HostEnv, name: &str) {
    #[cfg(not(target_os = "macos"))]
    {
        if user_manager(env) {
            let unit = install::service_name(name);
            let _ = crate::units::systemctl(&["stop", &format!("{unit}.service")]);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let label = install::label(name);
        if crate::units::agent_loaded(env.uid, &label) {
            let service = format!("{}/{label}", crate::units::launchd_domain(env.uid));
            // launchd keeps the socket and starts the job again on the next
            // connection.
            let _ = crate::units::launchctl(&["kill", "SIGTERM", &service]);
        }
    }
}

/// `paddock device restart NAME` from a terminal: stop the instance, and
/// start a new one that reads the table as it is now.
///
/// The new one starts only where something other than this command owns
/// its lifetime: an installed unit, or a transient unit of the user
/// manager. Without either, the instance belongs to the run that starts
/// it, and a start from this short-lived process would end with this
/// process, so the restart stops the old one and says that the next
/// request of a run starts the new one. None: nothing was started.
pub fn restart_instance(env: &HostEnv, def: &DeviceDef) -> Result<Option<Duration>, String> {
    stop_instance(env, &def.name)?;
    if start_kind(env, &def.name) == Start::Owned {
        return Ok(None);
    }
    let e = ensure(env, def, SHARED_CALLER)?;
    Ok(Some(e.started.unwrap_or(Duration::ZERO)))
}

/// One line per shared instance for `paddock ps`, and the `users` of
/// `paddock device ls`.
pub fn users_of(env: &HostEnv, name: &str) -> Option<u32> {
    InstanceState::shared_of(env, name)
        .filter(|s| s.alive() && s.run == SHARED_RUN)
        .map(|s| s.users.unwrap_or(0))
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
