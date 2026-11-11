//! The state files of device instances: one file per instance in
//! `<state>/devices/instances/<name>/<caller>.json`, written to a temp name
//! and renamed, so a reader sees a whole file or none. `paddock ps`,
//! `paddock device ls|logs|stop` and `paddock freeze` read them, and
//! `clear_stale` removes the files of instances whose process is gone,
//! with their folders. A device sandbox (instance.rs) and a host instance
//! (host.rs) write the same shape: `run` tells them apart.
//!
//! The log of an instance is `<state>/devices/log/<name>/<caller>.log`. It
//! stays after the instance ends, so `paddock device logs` can explain a
//! failure. The instance folder is `<run>/devices/<name>/<caller>/`.

use crate::env::HostEnv;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One running instance, as its state file records it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceState {
    pub device: String,
    /// The sandbox that the instance serves.
    pub caller: String,
    /// `sandbox`, `host` or `shared` (devices/shared.rs: one instance per
    /// user, behind `paddock deviced`).
    pub run: String,
    /// `exec` or `serve`.
    #[serde(default)]
    pub shape: String,
    pub pid: u32,
    /// Unix time of the start.
    pub started: u64,
    /// The socket of the supervisor on the host.
    pub socket: PathBuf,
    /// The systemd scope of the instance, when it has one.
    #[serde(default)]
    pub scope: Option<String>,
    /// What runs the instance: `bwrap`, `seatbelt`, `host` (the user's
    /// own process) or `thread` (the tests).
    #[serde(default)]
    pub backend: String,
    pub log: PathBuf,
    /// The instance folder.
    pub dir: PathBuf,
    /// `run = "shared"`: the systemd unit or the launchd label that owns
    /// the instance. None when no user manager does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// `run = "shared"`: the sandbox whose broker started the instance and
    /// whose end ends it, when no user manager owns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// `run = "shared"`: the brokers that hold a lease now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub users: Option<u32>,
}

/// The `caller` of a shared instance: one instance serves every sandbox,
/// so its state file is `instances/<name>/shared.json` and its log is
/// `log/<name>/shared.log`.
pub const SHARED_CALLER: &str = "shared";
/// The `run` of a shared instance.
pub const SHARED_RUN: &str = "shared";

/// `<state>/devices/instances`.
pub fn instances_dir(env: &HostEnv) -> PathBuf {
    env.paddock_state().join("devices").join("instances")
}

/// The state file of one instance.
pub fn state_file(env: &HostEnv, name: &str, caller: &str) -> PathBuf {
    instances_dir(env).join(name).join(format!("{caller}.json"))
}

/// `<state>/devices/log/<name>`.
pub fn log_dir(env: &HostEnv, name: &str) -> PathBuf {
    env.paddock_state().join("devices").join("log").join(name)
}

/// The log of one instance.
pub fn log_file(env: &HostEnv, name: &str, caller: &str) -> PathBuf {
    log_dir(env, name).join(format!("{caller}.log"))
}

/// The instance folder, in the run folder of the user.
pub fn instance_dir(env: &HostEnv, name: &str, caller: &str) -> PathBuf {
    env.paddock_run().join("devices").join(name).join(caller)
}

impl InstanceState {
    /// Write the file: a temp name in the same folder, then a rename.
    pub fn write(&self, env: &HostEnv) -> Result<()> {
        let f = state_file(env, &self.device, &self.caller);
        if let Some(d) = f.parent() {
            crate::privfs::dir(d).with_context(|| format!("cannot create {}", d.display()))?;
        }
        crate::privfs::write_atomic(&f, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("cannot write {}", f.display()))?;
        Ok(())
    }

    /// Remove the file of an instance. A missing file is fine.
    pub fn remove(env: &HostEnv, name: &str, caller: &str) {
        let f = state_file(env, name, caller);
        let _ = std::fs::remove_file(&f);
        if let Some(d) = f.parent() {
            let _ = std::fs::remove_dir(d);
        }
    }

    fn read(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Every state file, whatever the state of its process.
    pub fn list(env: &HostEnv) -> Vec<Self> {
        let mut out = Vec::new();
        let Ok(names) = std::fs::read_dir(instances_dir(env)) else {
            return out;
        };
        for n in names.flatten() {
            let Ok(files) = std::fs::read_dir(n.path()) else {
                continue;
            };
            for f in files.flatten() {
                if f.path().extension().is_some_and(|e| e == "json") {
                    if let Some(s) = Self::read(&f.path()) {
                        out.push(s);
                    }
                }
            }
        }
        out.sort_by(|a, b| (&a.device, &a.caller).cmp(&(&b.device, &b.caller)));
        out
    }

    /// The live instances of one sandbox.
    pub fn of_caller(env: &HostEnv, caller: &str) -> Vec<Self> {
        Self::list(env)
            .into_iter()
            .filter(|s| s.caller == caller && s.alive())
            .collect()
    }

    /// The live instances of one device.
    pub fn of_device(env: &HostEnv, name: &str) -> Vec<Self> {
        Self::list(env)
            .into_iter()
            .filter(|s| s.device == name && s.alive())
            .collect()
    }

    /// True while the process of the instance runs. A reused pid does not
    /// count: the command line of the process must name the instance
    /// folder or the socket (a host supervisor has `--socket PATH`), unless
    /// the instance runs in this process (tests).
    pub fn alive(&self) -> bool {
        if self.pid == std::process::id() {
            return true;
        }
        if !crate::state::alive(self.pid) {
            return false;
        }
        let Ok(cmdline) = std::fs::read(format!("/proc/{}/cmdline", self.pid)) else {
            // No /proc: trust the pid.
            return true;
        };
        let dir = self.dir.display().to_string();
        let socket = self.socket.display().to_string();
        cmdline.split(|b| *b == 0).any(|a| {
            let a = String::from_utf8_lossy(a);
            a == dir || a == socket
        })
    }

    /// For `paddock ps`: the state and the runtime.
    pub fn json(&self, now: u64) -> serde_json::Value {
        serde_json::json!({
            "device": self.device,
            "run": self.run,
            "shape": self.shape,
            "pid": self.pid,
            "started": self.started,
            "running_seconds": now.saturating_sub(self.started),
            "scope": self.scope,
            "backend": self.backend,
            "log": self.log,
            "unit": self.unit,
            "owner": self.owner,
            "users": self.users,
        })
    }

    /// The shared instances of the user, one per device (`run = "shared"`).
    pub fn shared(env: &HostEnv) -> Vec<Self> {
        Self::list(env)
            .into_iter()
            .filter(|s| s.run == SHARED_RUN && s.alive())
            .collect()
    }

    /// The shared instance of one device, whatever its state.
    pub fn shared_of(env: &HostEnv, name: &str) -> Option<Self> {
        Self::read(&state_file(env, name, SHARED_CALLER)).filter(|s| s.run == SHARED_RUN)
    }
}

/// Remove the state files of instances whose process is gone, with their
/// folders and any mount left under them. Returns what it removed, as
/// `device/caller`.
pub fn clear_stale(env: &HostEnv) -> Vec<String> {
    let mut gone = clear_abandoned_writes(env);
    for s in InstanceState::list(env) {
        if s.alive() {
            continue;
        }
        log::debug!(
            "device {}: the instance of {} is gone (pid {})",
            s.device,
            s.caller,
            s.pid
        );
        remove_dir(&s.dir);
        // macOS: the socket folders of the dead instance.
        crate::backend::seatbelt::device::remove_sockets(
            env,
            &super::spec::sandbox_id(&s.device, &s.caller),
        );
        InstanceState::remove(env, &s.device, &s.caller);
        gone.push(format!("{}/{}", s.device, s.caller));
    }
    gone
}

/// Recover atomic state writes whose writer died before the rename.
/// A live writer or a still-live supervisor keeps its file and sockets.
fn clear_abandoned_writes(env: &HostEnv) -> Vec<String> {
    let mut gone = Vec::new();
    for device in std::fs::read_dir(instances_dir(env))
        .into_iter()
        .flatten()
        .flatten()
    {
        for entry in std::fs::read_dir(device.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((_, suffix)) = name.split_once(".json.paddock-tmp-") else {
                continue;
            };
            let Some(writer) = suffix.split('-').next().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if crate::state::alive(writer) {
                continue;
            }
            let path = entry.path();
            if let Some(state) = InstanceState::read(&path) {
                if state.alive() {
                    continue;
                }
                remove_dir(&state.dir);
                crate::backend::seatbelt::device::remove_sockets(
                    env,
                    &super::spec::sandbox_id(&state.device, &state.caller),
                );
                gone.push(format!("{}/{}", state.device, state.caller));
            }
            let _ = std::fs::remove_file(path);
        }
    }
    gone
}

/// Remove an instance folder. A mount left under it is detached first.
pub fn remove_dir(dir: &Path) {
    if !dir.exists() {
        return;
    }
    for mp in mounts_under(dir) {
        lazy_unmount(&mp);
    }
    let _ = std::fs::remove_dir_all(dir);
    // The device folder above it, when this was its last instance.
    if let Some(parent) = dir.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

/// The mount points under `dir`, deepest first.
fn mounts_under(dir: &Path) -> Vec<PathBuf> {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = info
        .lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(PathBuf::from)
        .filter(|mp| mp.starts_with(dir))
        .collect();
    v.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    v
}

fn lazy_unmount(path: &Path) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: a valid C string; umount2 only reads it.
            unsafe {
                libc::umount2(c.as_ptr(), libc::MNT_DETACH);
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &Path) -> HostEnv {
        let vars = [("XDG_STATE_HOME", dir.join("state").display().to_string())]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars)
    }

    fn inst(dir: &Path, caller: &str, pid: u32) -> InstanceState {
        InstanceState {
            device: "notes".into(),
            caller: caller.into(),
            run: "host".into(),
            shape: "exec".into(),
            pid,
            started: 10,
            socket: dir.join("dev-notes.sock"),
            scope: None,
            backend: "host".into(),
            log: dir.join("log"),
            dir: dir.join("inst").join(caller),
            unit: None,
            owner: None,
            users: None,
        }
    }

    /// The shape that a host instance writes: whole or absent, stale when
    /// its pid is gone, skipped when half-written.
    #[test]
    fn a_host_state_file_is_whole_and_a_dead_pid_is_stale() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path());
        let me = std::process::id();
        inst(t.path(), "box1", me).write(&e).unwrap();
        // A pid that no process has: the largest pid is far below this.
        inst(t.path(), "box2", 4_000_000).write(&e).unwrap();
        let p = state_file(&e, "notes", "box1");
        assert!(p.is_file());
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["run"], "host");
        assert_eq!(v["shape"], "exec");
        assert_eq!(v["pid"], me);
        // No temporary file is left beside it.
        let mut names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|f| f.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["box1.json", "box2.json"], "{names:?}");
        let live: Vec<String> = InstanceState::of_device(&e, "notes")
            .iter()
            .map(|i| i.caller.clone())
            .collect();
        assert_eq!(live, ["box1"]);
        let row = InstanceState::of_caller(&e, "box1")[0].json(70);
        assert_eq!(row["run"], "host");
        assert_eq!(row["running_seconds"], 60);
        assert!(InstanceState::of_caller(&e, "box2").is_empty());
        assert_eq!(clear_stale(&e), ["notes/box2"]);
        assert!(!state_file(&e, "notes", "box2").exists());
        assert!(p.is_file(), "the live one stays");
        // A half-written file is skipped, never a panic.
        std::fs::write(state_file(&e, "notes", "box3"), b"{\"device\": \"no").unwrap();
        assert_eq!(InstanceState::list(&e).len(), 1);
        InstanceState::remove(&e, "notes", "box1");
        assert!(InstanceState::of_device(&e, "notes").is_empty());
    }

    #[test]
    fn interrupted_atomic_state_is_cleaned_without_touching_a_live_writer() {
        let t = tempfile::tempdir().unwrap();
        let e = env(t.path());
        let dead = inst(t.path(), "orphan", 4_000_000);
        std::fs::create_dir_all(&dead.dir).unwrap();
        let state = state_file(&e, "notes", "orphan");
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        let orphan = state.with_file_name("orphan.json.paddock-tmp-4000000-1-0");
        std::fs::write(&orphan, serde_json::to_vec(&dead).unwrap()).unwrap();
        let active = state.with_file_name(format!(
            "active.json.paddock-tmp-{}-1-0",
            std::process::id()
        ));
        std::fs::write(&active, serde_json::to_vec(&dead).unwrap()).unwrap();
        let incomplete = state.with_file_name("incomplete.json.paddock-tmp-4000000-1-1");
        std::fs::write(&incomplete, "{").unwrap();
        assert!(InstanceState::list(&e).is_empty());
        assert_eq!(clear_stale(&e), ["notes/orphan"]);
        assert!(!orphan.exists());
        assert!(!incomplete.exists());
        assert!(!dead.dir.exists());
        assert!(active.exists(), "a live writer's file stays");
    }
}
