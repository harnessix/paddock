//! The device sandbox: a paddock sandbox of its own for a device program
//! with `run = "sandbox"`, built from the profile `[devices.NAME.sandbox]`.
//!
//! One instance per agent sandbox (`scope = "run"`), started at the first
//! request or with the sandbox, ended with it. spec.rs builds the spec,
//! instance.rs starts and stops the process, creds.rs gives the
//! credentials of the profile, connect.rs serves the `connect` entries on
//! the host, and state.rs keeps the state files that `paddock ps`,
//! `paddock device` and `paddock freeze` read.
//!
//! `Instances` is the registry of one run: the `Devices` guard of the run
//! holds it and stops every instance on drop, `paddock device stop|restart
//! NAME --sandbox ID` reaches it through the kv socket of the run, and
//! `SandboxSource` is the `InstanceSource` of an `OutOfProcess` device.
//! The host instances of the run (host.rs, `run = "host"`) register here
//! too, so the same commands and the same end of the run reach them. A
//! supervisor that dies restarts at the next request after 1 and 2 seconds;
//! the third death within a minute makes the device `failed` until
//! `paddock device restart`.

pub mod connect;
pub mod creds;
pub mod instance;
pub mod spec;
pub mod state;

pub use instance::{Instance, Launcher};
pub use spec::InstanceCtx;

use super::config::{DeviceDef, Start};
use super::host::HostSource;
use super::outproc::InstanceSource;
use super::shared::SharedSource;
use super::DeviceCtx;
use crate::env::HostEnv;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Deaths within this window count toward `failed`.
const WINDOW: Duration = Duration::from_secs(60);
/// The wait before a restart after the first and the second death.
const BACKOFF: [u64; 2] = [1, 2];
/// The death that makes the device `failed`.
const MAX_DEATHS: usize = 3;

/// The instances of one run, one per device. `DeviceCtx` carries it to the
/// devices, the `Devices` guard stops it.
pub struct Instances {
    env: HostEnv,
    launcher: Launcher,
    sources: Mutex<Vec<Arc<SandboxSource>>>,
    /// The host instances of the run (`run = "host"`).
    hosts: Mutex<Vec<Arc<HostSource>>>,
    /// The shared instances that the run uses (`scope = "shared"`): one
    /// lease each, dropped when the run ends.
    shared: Mutex<Vec<Arc<SharedSource>>>,
}

impl Instances {
    pub fn new(env: HostEnv) -> Arc<Self> {
        Self::with_launcher(env, Launcher::host())
    }

    /// `Launcher::Thread` for tests without bwrap.
    pub fn with_launcher(env: HostEnv, launcher: Launcher) -> Arc<Self> {
        Arc::new(Self {
            env,
            launcher,
            sources: Mutex::new(Vec::new()),
            hosts: Mutex::new(Vec::new()),
            shared: Mutex::new(Vec::new()),
        })
    }

    /// The host source of `def` for the caller of `ctx`: one per device.
    pub fn host_source(&self, def: &DeviceDef, ctx: &DeviceCtx) -> Arc<HostSource> {
        let mut g = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.iter().find(|s| s.name() == def.name) {
            return s.clone();
        }
        let s = Arc::new(HostSource::new(def.clone(), ctx));
        g.push(s.clone());
        s
    }

    /// The shared source of `def` for this run: one per device, holding
    /// one lease on the instance of the user (shared.rs).
    pub fn shared_source(&self, def: &DeviceDef, ctx: &DeviceCtx) -> Arc<SharedSource> {
        let mut g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.iter().find(|s| s.name() == def.name) {
            return s.clone();
        }
        let s = Arc::new(SharedSource::new(def.clone(), ctx));
        g.push(s.clone());
        s
    }

    fn find_shared(&self, name: &str) -> Option<Arc<SharedSource>> {
        let g = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().find(|s| s.name() == name).cloned()
    }

    fn find_host(&self, name: &str) -> Option<Arc<HostSource>> {
        let g = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().find(|s| s.name() == name).cloned()
    }

    /// The source of `def` for the caller of `ctx`: one per device.
    pub fn source(&self, def: &DeviceDef, ctx: &DeviceCtx) -> Arc<SandboxSource> {
        let mut g = self.sources.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.iter().find(|s| s.def.name == def.name) {
            return s.clone();
        }
        let s = Arc::new(SandboxSource {
            def: def.clone(),
            env: self.env.clone(),
            ictx: InstanceCtx {
                caller: ctx.sandbox.clone(),
                project: ctx.project.clone(),
                harness: ctx.harness.clone(),
                chain: ctx.started_by.clone(),
                dir: state::instance_dir(&self.env, &def.name, &ctx.sandbox),
            },
            launcher: self.launcher,
            state: Mutex::new(SrcState::default()),
        });
        g.push(s.clone());
        s
    }

    fn find(&self, name: &str) -> Option<Arc<SandboxSource>> {
        let g = self.sources.lock().unwrap_or_else(|e| e.into_inner());
        g.iter().find(|s| s.def.name == name).cloned()
    }

    /// Start the instances of the devices with `start = "with-sandbox"`.
    /// Returns one notice per failure.
    pub fn start_with_sandbox(&self) -> Vec<String> {
        let sources: Vec<_> = self
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|s| s.def.start == Start::WithSandbox)
            .cloned()
            .collect();
        let mut notices = Vec::new();
        for s in sources {
            if let Err(e) = s.ensure_started() {
                notices.push(format!("the device {} did not start: {e}", s.def.name));
            }
        }
        let hosts: Vec<_> = self
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|s| s.def().start == Start::WithSandbox)
            .cloned()
            .collect();
        for s in hosts {
            if let Err(e) = s.ensure_started() {
                notices.push(format!("the device {} did not start: {e}", s.name()));
            }
        }
        let shared: Vec<_> = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|s| s.start() == Start::WithSandbox)
            .cloned()
            .collect();
        for s in shared {
            if let Err(e) = s.ensure_leased() {
                notices.push(format!("the device {} did not start: {e}", s.name()));
            }
        }
        notices
    }

    /// Stop every instance of the run.
    pub fn stop_all(&self) {
        let sources: Vec<_> = self
            .sources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for s in sources {
            s.stop();
        }
        let hosts: Vec<_> = self.hosts.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for s in hosts {
            s.stop();
        }
        // A shared instance outlives the run: the run only drops its lease,
        // and the instance exits when its last user ended and it is idle.
        let shared: Vec<_> = self
            .shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for s in shared {
            s.release();
        }
    }

    /// `paddock device stop NAME --sandbox ID`.
    pub fn stop(&self, name: &str) -> Result<serde_json::Value, String> {
        if let Some(s) = self.find_shared(name) {
            let was = s.stop();
            return Ok(serde_json::json!({ "device": name, "stopped": was, "scope": "shared" }));
        }
        if let Some(h) = self.find_host(name) {
            let was = h.stop();
            return Ok(serde_json::json!({ "device": name, "stopped": was }));
        }
        let s = self
            .find(name)
            .ok_or_else(|| format!("this sandbox has no device {name}"))?;
        let was = s.stop();
        Ok(serde_json::json!({ "device": name, "stopped": was }))
    }

    /// `paddock device restart NAME --sandbox ID`: stop, forget the
    /// failures, start again.
    pub fn restart(&self, name: &str) -> Result<serde_json::Value, String> {
        if let Some(s) = self.find_shared(name) {
            let took = s.restart()?;
            return Ok(serde_json::json!({
                "device": name,
                "scope": "shared",
                "started_ms": took.as_millis() as u64,
            }));
        }
        if let Some(h) = self.find_host(name) {
            h.stop();
            let (took, _) = h.ensure_started()?;
            return Ok(
                serde_json::json!({ "device": name, "started_ms": took.as_millis() as u64 }),
            );
        }
        let s = self
            .find(name)
            .ok_or_else(|| format!("this sandbox has no device {name}"))?;
        s.stop();
        {
            let mut g = s.state.lock().unwrap_or_else(|e| e.into_inner());
            g.deaths.clear();
            g.failed = None;
        }
        let took = s.ensure_started()?;
        Ok(serde_json::json!({ "device": name, "started_ms": took.as_millis() as u64 }))
    }

    /// The state of each device of the run.
    pub fn status(&self) -> serde_json::Value {
        let g = self.sources.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<serde_json::Value> = g
            .iter()
            .map(|s| {
                let st = s.state.lock().unwrap_or_else(|e| e.into_inner());
                serde_json::json!({
                    "device": s.def.name,
                    "run": "sandbox",
                    "instance": match (&st.inst, &st.failed) {
                        (_, Some(_)) => "failed",
                        (Some(_), None) => "running",
                        (None, None) => "stopped",
                    },
                    "pid": st.inst.as_ref().map(|i| i.pid),
                    "deaths": st.deaths.len(),
                })
            })
            .collect();
        drop(g);
        let h = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        v.extend(h.iter().map(|s| {
            let pid = s.pid();
            serde_json::json!({
                "device": s.name(),
                "run": "host",
                "instance": if pid.is_some() { "running" } else { "stopped" },
                "pid": pid,
                "deaths": 0,
            })
        }));
        drop(h);
        let sh = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        v.extend(sh.iter().map(|s| {
            serde_json::json!({
                "device": s.name(),
                "run": "shared",
                "instance": if s.leased() { "running" } else { "stopped" },
                "users": s.users(),
                "deaths": 0,
            })
        }));
        v.into()
    }
}

impl Drop for Instances {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[derive(Default)]
struct SrcState {
    inst: Option<Instance>,
    deaths: VecDeque<Instant>,
    failed: Option<String>,
    epoch: u64,
}

/// The `InstanceSource` of a device sandbox.
pub struct SandboxSource {
    def: DeviceDef,
    env: HostEnv,
    ictx: InstanceCtx,
    launcher: Launcher,
    state: Mutex<SrcState>,
}

impl SandboxSource {
    /// The instance folder.
    pub fn dir(&self) -> &Path {
        &self.ictx.dir
    }

    /// Start the instance when it is not running. The time the start took,
    /// or zero when it ran already.
    pub fn ensure_started(&self) -> Result<Duration, String> {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = g.inst.as_mut() {
            if i.alive() {
                return Ok(Duration::ZERO);
            }
            // The supervisor died: count it, clean up, back off.
            let now = Instant::now();
            g.inst.take();
            g.deaths.push_back(now);
            while g
                .deaths
                .front()
                .is_some_and(|t| now.duration_since(*t) > WINDOW)
            {
                g.deaths.pop_front();
            }
            let n = g.deaths.len();
            log::warn!(
                "device {}: the instance of {} died ({n} within a minute)",
                self.def.name,
                self.ictx.caller
            );
            if n >= MAX_DEATHS {
                g.failed = Some(format!(
                    "the device sandbox exited {n} times within a minute"
                ));
            } else {
                std::thread::sleep(Duration::from_secs(BACKOFF[n.min(BACKOFF.len()) - 1]));
            }
        }
        if let Some(why) = &g.failed {
            return Err(why.clone());
        }
        let i = Instance::start(&self.env, &self.def, &self.ictx, self.launcher)
            .map_err(|e| format!("{e:#}"))?;
        let took = i.start_time;
        g.inst = Some(i);
        Ok(took)
    }

    /// Stop the instance. True when one ran. The next request starts a
    /// new one.
    pub fn stop(&self) -> bool {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.epoch += 1;
        match g.inst.take() {
            Some(mut i) => {
                i.stop();
                true
            }
            None => false,
        }
    }

    /// The running instance's pid, for tests.
    pub fn pid(&self) -> Option<u32> {
        let g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.inst.as_ref().map(|i| i.pid)
    }
}

impl InstanceSource for SandboxSource {
    fn socket(&self, _ctx: &DeviceCtx) -> Result<PathBuf, String> {
        self.ensure_started()?;
        let g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        g.inst
            .as_ref()
            .map(|i| i.socket.clone())
            .ok_or_else(|| "the instance is not running".to_string())
    }

    fn instance(&self) -> String {
        self.ictx.caller.clone()
    }

    fn log_path(&self) -> Option<PathBuf> {
        Some(state::log_file(
            &self.env,
            &self.def.name,
            &self.ictx.caller,
        ))
    }

    fn epoch(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).epoch
    }

    fn failed(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failed
            .clone()
    }

    fn fix_hint(&self) -> String {
        format!(
            "Read the log: paddock device logs {n} --sandbox {c}. Then: paddock device restart {n} --sandbox {c}",
            n = self.def.name,
            c = self.ictx.caller
        )
    }
}

/// The log of an instance on the host: the supervisor's stderr, appended to
/// a file capped at 10 MiB with one rotation (`.1`).
pub struct LogSink {
    path: PathBuf,
    file: std::fs::File,
    size: u64,
}

impl LogSink {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let file = crate::privfs::append(path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            file,
            size,
        })
    }

    fn write(&mut self, data: &[u8]) {
        if self.size + data.len() as u64 > super::supervise::LOG_CAP {
            let old = self.path.with_extension(match self.path.extension() {
                Some(e) => format!("{}.1", e.to_string_lossy()),
                None => "1".to_string(),
            });
            let _ = std::fs::rename(&self.path, &old);
            if let Ok(f) = crate::privfs::append(&self.path) {
                self.file = f;
                self.size = 0;
            }
        }
        if self.file.write_all(data).is_ok() {
            self.size += data.len() as u64;
        }
    }

    /// Copy `from` into the log until end of file.
    pub fn pump(mut self, mut from: impl Read) {
        let mut buf = [0u8; 8192];
        loop {
            match from.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.write(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
