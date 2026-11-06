//! Where the devices of a sandbox appear: a FUSE mount on Linux, a socket
//! and FIFOs on Seatbelt (macOS). The start, the guard that ends them, and
//! the lines of `paddock doctor`.

use super::{disabled, enabled, DeviceCtx, DeviceFs, DevicesCfg, GrantCtx, Mounted};
use super::{mount, socket, SECRET_DIR};
use crate::credentials::secret::fifo::SecretFifos;
use crate::credentials::secret::SecretFiles;
use crate::env::HostEnv;
use crate::spec::SandboxSpec;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The name of the device socket in the socket folder of a run.
pub const SOCKET_NAME: &str = "devices.sock";

/// The backend that gets the devices over a socket.
const SEATBELT: &str = crate::backend::seatbelt::NAME;

#[cfg(target_os = "linux")]
use super::fuse::lazy_unmount;
#[cfg(not(target_os = "linux"))]
use super::fuse_stub::lazy_unmount;

/// Whether this host can mount a user FUSE file system: `/dev/fuse` opens
/// and `fusermount3` (or `fusermount`) exists.
#[cfg(target_os = "linux")]
pub fn fuse_available(env: &HostEnv) -> Result<PathBuf, String> {
    let helper = env
        .which("fusermount3")
        .or_else(|| env.which("fusermount"))
        .ok_or("fusermount3 is not installed (package fuse3)")?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fuse")
        .map_err(|e| format!("cannot open /dev/fuse: {e}"))?;
    Ok(helper)
}

#[cfg(not(target_os = "linux"))]
pub fn fuse_available(_env: &HostEnv) -> Result<PathBuf, String> {
    Err(crate::platform::unavailable("the FUSE device mount").to_string())
}

/// Whether a backend can show the devices inside its sandbox.
///
/// Seatbelt gets them over a socket, with no FUSE. The VM does not get them
/// yet.
///
/// A FUSE mount answers only the user who mounted it, not even root. bwrap
/// and srt run as the user, so they bind the mount itself. The container
/// runtimes set up mounts as root or as a mapped root, which the mount
/// refuses. For them paddock binds the empty folder with slave propagation
/// and mounts the file system on it after the container started; the mount
/// then appears inside the container. That needs a run folder on a shared
/// mount, the systemd default.
pub fn backend_support(env: &HostEnv, backend: &str) -> Result<(), String> {
    match backend {
        "bwrap" | "srt" | SEATBELT => Ok(()),
        crate::backend::vm::NAME => {
            Err("the devices of /dev/paddock do not reach the VM yet".into())
        }
        "docker" | "podman" => {
            let run = env.paddock_run();
            if shared_mount(&run) {
                Ok(())
            } else {
                Err(format!(
                    "{} is not on a shared mount, so a later mount does not reach the container",
                    run.display()
                ))
            }
        }
        other => Err(format!("the {other} backend has no device support")),
    }
}

/// Where the device file system of a sandbox is mounted on the host. srt
/// binds the run folder of the sandbox read-only over everything in it, so
/// for srt the mount is outside that folder.
pub fn mount_dir(env: &HostEnv, backend: &str, id: &str) -> PathBuf {
    if backend == "srt" {
        env.paddock_run().join("devices").join(id)
    } else {
        env.paddock_run().join(id).join("dev")
    }
}

/// Container backends mount the devices after the start.
pub fn late(backend: &str) -> bool {
    matches!(backend, "docker" | "podman")
}

/// True when the mount that holds `path` has shared propagation.
fn shared_mount(path: &Path) -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let mut best: Option<(usize, bool)> = None;
    for line in info.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(mp) = f.get(4) else { continue };
        let mp = Path::new(mp);
        if !path.starts_with(mp) {
            continue;
        }
        let len = mp.as_os_str().len();
        let shared = f[6..]
            .iter()
            .take_while(|x| **x != "-")
            .any(|x| x.starts_with("shared:"));
        if best.is_none_or(|(l, _)| len >= l) {
            best = Some((len, shared));
        }
    }
    best.is_some_and(|(_, s)| s)
}

/// The devices of one sandbox. Dropping it unmounts them, or stops the
/// socket server and removes the secret FIFOs.
pub struct Devices {
    pub dir: PathBuf,
    mounted: Arc<Mutex<Option<Mounted>>>,
    late: Option<(DeviceFs, String)>,
    stop: Arc<AtomicBool>,
    waiter: Option<std::thread::JoinHandle<()>>,
    socket: Option<socket::Serving>,
    fifos: Option<SecretFifos>,
    /// The device instances of the sandbox (sandbox.rs).
    instances: Option<Arc<super::sandbox::Instances>>,
}

impl Devices {
    /// The device instances of the sandbox, for `paddock device stop` over
    /// the kv socket.
    pub fn instances(&self) -> Option<Arc<super::sandbox::Instances>> {
        self.instances.clone()
    }

    /// For container backends: wait until the container `paddock-<id>`
    /// runs, then mount. The container waits a few seconds for the
    /// catalog before the harness starts.
    pub fn after_start(&mut self, env: &HostEnv, backend: &str, id: &str) {
        let Some((fs, _)) = self.late.take() else {
            return;
        };
        let Some(cli) = env.which(backend) else {
            return;
        };
        let (dir, mounted, stop) = (self.dir.clone(), self.mounted.clone(), self.stop.clone());
        let name = format!("paddock-{id}");
        self.waiter = Some(std::thread::spawn(move || {
            let start = Instant::now();
            while !stop.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(120) {
                let running = std::process::Command::new(&cli)
                    .args(["inspect", "-f", "{{.State.Running}}", &name])
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
                if running {
                    match mount(&dir, fs) {
                        Ok(m) => *mounted.lock().unwrap() = Some(m),
                        Err(e) => log::warn!("cannot mount the devices: {e:#}"),
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }));
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        // The instances first: a request in flight ends with EIO instead
        // of a hang on a mount that goes away.
        if let Some(i) = self.instances.take() {
            i.stop_all();
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = self.waiter.take() {
            let _ = w.join();
        }
        if let Some(m) = self.mounted.lock().unwrap().take() {
            m.unmount();
        }
        self.socket.take();
        self.fifos.take();
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// At sandbox start: mount the devices of this sandbox in its run folder and
/// record the folder in the spec, for the backend. Container backends get
/// the empty folder now and the mount after the start (`after_start`). A
/// problem is a notice, not an error: the sandbox starts without devices.
/// `grants` says who this sandbox is (its project, harness, pasture, the
/// `--device` flags and the ceiling of a spawned sandbox), and
/// `started_by` the sandboxes above it. `secrets` holds the secret files.
#[allow(clippy::too_many_arguments)]
pub fn start(
    env: &HostEnv,
    spec: &mut SandboxSpec,
    backend: &str,
    test: bool,
    grants: &GrantCtx,
    started_by: &[String],
    gateway: Option<crate::gateway::Handler>,
    secrets: Arc<SecretFiles>,
) -> Option<Devices> {
    let cfg = match DevicesCfg::load(env) {
        Ok(c) => c,
        Err(e) => {
            spec.notices.push(format!("devices are off: {e:#}"));
            return None;
        }
    };
    if !cfg.mount {
        return None;
    }
    let instances = super::sandbox::Instances::new(env.clone());
    let ctx = DeviceCtx {
        env: env.clone(),
        sandbox: spec.id.clone(),
        harness: spec.harness.clone(),
        project: spec.project.clone(),
        started_by: started_by.to_vec(),
        approver: Arc::new(crate::broker::HostApprover),
        instances: instances.clone(),
    };
    let mut devices = enabled(&cfg, test, grants, &ctx);
    // Without an enabled device the mount is only a catalog, so a missing
    // FUSE is not worth a notice at each start. The gateway and the files
    // of off devices do not count: without the mount, paddock-host uses
    // the host socket.
    let quiet = devices.is_empty();
    let off = disabled(&cfg, &devices, grants, &ctx);
    devices.extend(off);
    if let Some(h) = gateway {
        devices.push(Arc::new(crate::gateway::GwDevice::new(&spec.id, h)));
    }
    let tell = |spec: &mut SandboxSpec, why: String| {
        if quiet {
            log::debug!("no /dev/paddock: {why}");
        } else {
            spec.notices
                .push(format!("devices are off in this sandbox: {why}"));
        }
    };
    if let Err(why) = backend_support(env, backend) {
        tell(spec, why);
        return None;
    }
    if backend == SEATBELT {
        return match start_socket(env, spec, devices, secrets) {
            Ok(mut d) => {
                d.instances = Some(instances.clone());
                spec.notices.extend(instances.start_with_sandbox());
                Some(d)
            }
            Err(e) => {
                tell(spec, format!("{e:#}"));
                None
            }
        };
    }
    if let Err(why) = fuse_available(env) {
        tell(spec, why);
        return None;
    }
    clear_stale(env);
    let dir = mount_dir(env, backend, &spec.id);
    let fs = DeviceFs::with_secrets(&spec.id, devices, secrets);
    let mut out = Devices {
        dir: dir.clone(),
        mounted: Arc::new(Mutex::new(None)),
        late: None,
        stop: Arc::new(AtomicBool::new(false)),
        waiter: None,
        socket: None,
        fifos: None,
        instances: Some(instances.clone()),
    };
    if late(backend) {
        if let Err(e) = crate::privfs::dir(&dir) {
            tell(spec, format!("cannot create {}: {e}", dir.display()));
            return None;
        }
        out.late = Some((fs, backend.to_string()));
    } else {
        match mount(&dir, fs) {
            Ok(m) => *out.mounted.lock().unwrap() = Some(m),
            Err(e) => {
                tell(spec, format!("{e:#}"));
                return None;
            }
        }
    }
    spec.devices = Some(dir);
    // `start = "with-sandbox"`: a failure is a notice and the device is
    // `failed` until `paddock device restart`.
    spec.notices.extend(instances.start_with_sandbox());
    Some(out)
}

/// `paddock device stop|restart NAME --sandbox ID` comes over the kv socket
/// of the run and reaches the device instances here.
pub fn hook_control(kv: Option<&crate::kv::Served>, devices: Option<&Devices>) {
    let (Some(k), Some(inst)) = (kv, devices.and_then(|d| d.instances())) else {
        return;
    };
    k.set_device_control(Arc::new(move |action: &str, name: &str| match action {
        "stop" => inst.stop(name),
        "restart" => inst.restart(name),
        _ => Ok(inst.status()),
    }));
}

/// The folder of the devices of a Seatbelt run on the host: it holds the
/// secret FIFOs, and `$PADDOCK_DEVICES` names it in the sandbox. It is in
/// the run folder, which the profile allows read-only.
pub fn socket_dir(env: &HostEnv, id: &str) -> PathBuf {
    env.paddock_run().join(id).join("devices")
}

/// Seatbelt: serve the devices on `devices.sock` in the socket folder of
/// the run, and the secret files as FIFOs in `socket_dir`.
fn start_socket(
    env: &HostEnv,
    spec: &mut SandboxSpec,
    devices: Vec<Arc<dyn super::Device>>,
    secrets: Arc<SecretFiles>,
) -> anyhow::Result<Devices> {
    use anyhow::Context as _;
    let dir = socket_dir(env, &spec.id);
    crate::privfs::dir(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = crate::sockpath::named(env, &spec.id, SOCKET_NAME)?;
    let server = socket::Server::new(&spec.id, devices, secrets.clone());
    let serving = socket::serve(&path, server)?;
    let fifos = crate::credentials::secret::fifo::start(&dir.join(SECRET_DIR), secrets, &spec.id)?;
    spec.devices = Some(dir.clone());
    spec.device_socket = Some(path);
    Ok(Devices {
        dir,
        mounted: Arc::new(Mutex::new(None)),
        late: None,
        stop: Arc::new(AtomicBool::new(false)),
        waiter: None,
        socket: Some(serving),
        fifos: Some(fifos),
        instances: None,
    })
}

/// Unmount the device file systems that a killed paddock process left
/// behind. Every access to such a mount fails until it is unmounted.
///
/// The running list alone cannot tell a dead mount from a fresh one: a
/// run mounts its devices before it registers itself (run.rs registers
/// the sandbox after it spawned it), so a second run that starts at the
/// same time would sweep the mount of the first, and that sandbox would
/// start with an empty `/dev/paddock`. So each mount that no registered
/// sandbox owns is asked instead: the file system of a process that is
/// gone refuses the question, a live one answers.
pub fn clear_stale(env: &HostEnv) {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return;
    };
    let running: Vec<String> = crate::state::Running::list(env)
        .into_iter()
        .map(|r| r.id)
        .collect();
    for mp in stale_mounts(&info, &env.paddock_run(), &running, probe) {
        log::debug!("unmounting the stale device mount {}", mp.display());
        lazy_unmount(&mp);
    }
}

/// What a device mount answers when asked for its file system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mount {
    /// The process that serves it is there.
    Answers,
    /// The process that served it is gone: the mount is stale.
    Gone,
    /// No answer within the wait, or an error that says nothing about
    /// the process. Left alone.
    Silent,
}

/// How long a probe waits for the file system to answer.
const PROBE_WAIT: Duration = Duration::from_secs(2);

/// Ask the file system at `mp` for its statistics. `statfs` always
/// reaches the process behind a FUSE mount (a `stat` can be answered from
/// the attribute cache), and a process that is gone makes the kernel
/// answer `ENOTCONN`. A thread, so a file system that hangs cannot hang
/// the start of a sandbox.
fn probe(mp: &Path) -> Mount {
    let (tx, rx) = std::sync::mpsc::channel();
    let path = mp.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(statfs_errno(&path));
    });
    match rx.recv_timeout(PROBE_WAIT) {
        Ok(None) => Mount::Answers,
        Ok(Some(e)) if e == libc::ENOTCONN || e == libc::ECONNABORTED => Mount::Gone,
        Ok(Some(_)) | Err(_) => Mount::Silent,
    }
}

/// The errno of `statfs(path)`, None when it succeeds.
fn statfs_errno(path: &Path) -> Option<i32> {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return Some(libc::EINVAL);
    };
    // SAFETY: a zeroed statfs buffer that the call fills, and a C string
    // that lives for the call.
    let rc = unsafe {
        let mut st: libc::statfs = std::mem::zeroed();
        libc::statfs(c.as_ptr(), &mut st)
    };
    (rc != 0).then(|| {
        std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO)
    })
}

/// The `fuse.paddock` mounts of `info` (the text of `/proc/self/mountinfo`)
/// under `run` to unmount: `<id>/dev`, or `devices/<id>` for srt, of a
/// sandbox that is not in `running` and whose file system `probe` finds
/// gone. A registered sandbox owns its mount and is not asked.
pub(crate) fn stale_mounts(
    info: &str,
    run: &Path,
    running: &[String],
    probe: impl Fn(&Path) -> Mount,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in info.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(sep) = f.iter().position(|x| *x == "-") else {
            continue;
        };
        if f.get(sep + 1) != Some(&"fuse.paddock") {
            continue;
        }
        let Some(mp) = f.get(4).map(Path::new) else {
            continue;
        };
        let Ok(rel) = mp.strip_prefix(run) else {
            continue;
        };
        let mut parts = rel.components();
        let (Some(id), Some(dev), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        // <id>/dev, or devices/<id> for srt.
        let (id, dev) = if id.as_os_str() == "devices" {
            (
                dev.as_os_str().to_string_lossy(),
                std::ffi::OsStr::new("dev"),
            )
        } else {
            (id.as_os_str().to_string_lossy(), dev.as_os_str())
        };
        if dev != "dev" || running.iter().any(|r| *r == id) {
            continue;
        }
        if probe(mp) == Mount::Gone {
            out.push(mp.to_path_buf());
        }
    }
    out
}

/// For `paddock doctor`: one line per backend.
pub fn report(env: &HostEnv) -> BTreeMap<&'static str, Result<(), String>> {
    let mut out = BTreeMap::new();
    let fuse = fuse_available(env).map(|_| ());
    for b in ["bwrap", "podman", "docker", "srt"] {
        out.insert(b, fuse.clone().and_then(|_| backend_support(env, b)));
    }
    out
}

/// The lines of `paddock doctor` about devices.
pub fn doctor_lines(env: &HostEnv) -> Vec<String> {
    doctor_lines_on(env, cfg!(target_os = "macos"))
}

/// `doctor_lines` for macOS (`macos`, no FUSE: Seatbelt over a socket, the
/// VM not yet) or Linux.
pub fn doctor_lines_on(env: &HostEnv, macos: bool) -> Vec<String> {
    let mut v = Vec::new();
    if macos {
        v.push(format!(
            "  ok       devices on {SEATBELT}: over a socket, `paddock-host device`; secret files at $PADDOCK_DEVICES/secret/NAME"
        ));
        v.push(format!(
            "  ok       device sandboxes on {SEATBELT}: run = \"sandbox\" under sandbox-exec; no mem or cpus limit, and `paddock freeze` does not reach them"
        ));
        if let Err(e) = backend_support(env, crate::backend::vm::NAME) {
            v.push(format!(
                "  no       devices on {}: {e}",
                crate::backend::vm::NAME
            ));
        }
        config_line(env, &mut v);
        return v;
    }
    match fuse_available(env) {
        Ok(helper) => v.push(format!(
            "  ok       FUSE: {} and /dev/fuse",
            helper.display()
        )),
        Err(e) => v.push(format!(
            "  no       FUSE: {e}. Sandboxes start without /dev/paddock"
        )),
    }
    for (b, r) in report(env) {
        match r {
            Ok(()) if b == "srt" => v.push(format!(
                "  ok       devices on {b}: at the path in $PADDOCK_DEVICES, not /dev/paddock"
            )),
            Ok(()) if late(b) => v.push(format!(
                "  ok       devices on {b}: /dev/paddock, mounted after the container starts"
            )),
            Ok(()) => v.push(format!("  ok       devices on {b}: /dev/paddock")),
            Err(e) => v.push(format!("  no       devices on {b}: {e}")),
        }
    }
    config_line(env, &mut v);
    v
}

/// The line about `[devices]` of the user configuration.
fn config_line(env: &HostEnv, v: &mut Vec<String>) {
    match DevicesCfg::load(env) {
        Ok(c) if !c.mount => v.push("  off      /dev/paddock: [devices] mount = false".into()),
        Ok(c) if c.enabled.is_empty() => v.push(
            "  off      no device is on; turn one on with [devices] NAME = true in the user configuration".into(),
        ),
        Ok(c) => {
            v.push(format!("  on       devices: {}", c.enabled.join(", ")));
            v.extend(super::doctor::lines(env, &c));
        }
        Err(e) => v.push(format!("  error    {e:#}")),
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
