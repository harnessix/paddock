//! The world of the shared device tests (tests/devices_shared.rs and
//! tests/devices_shared_units.rs): a temporary host with one shared device
//! `up`, the brokers that hold a lease (`paddock device check --hold`),
//! and the readers of the state file and the log of the instance.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A temporary host: a user configuration with one shared device, a state
/// folder, a runtime folder and a project.
pub struct World {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
    /// The device name of the paths below.
    pub name: String,
    /// `XDG_RUNTIME_DIR` of every paddock command: `root/run`, or the
    /// runtime folder of the real user manager (the systemd test).
    pub run: PathBuf,
}

impl World {
    pub fn new(config: &str) -> Self {
        let tmp = super::tempdir();
        let root = tmp.path().to_path_buf();
        for d in ["cfg/paddock", "state", "run", "project", "dev", "home"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::set_permissions(
            root.join("run"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(root.join("cfg/paddock/config.toml"), config).unwrap();
        let run = root.join("run");
        World {
            _tmp: tmp,
            root,
            name: "up".to_string(),
            run,
        }
    }

    /// A device program: it prints the caller of the request, then the
    /// request in capitals.
    pub fn script(&self, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let f = self.root.join("dev").join(name);
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        f.display().to_string()
    }

    pub fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", &self.run)
            // The instance starts a device sandbox; the supervisor runs in a
            // thread, so the test needs no bubblewrap.
            .env("PADDOCK_TEST_DEVICE_LAUNCHER", "thread")
            // Without a user manager the first broker owns the instance.
            .env("PADDOCK_TEST_NO_USER_MANAGER", "1")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .env_remove("PADDOCK")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .current_dir(self.root.join("project"));
        c
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("cfg/paddock/config.toml")
    }

    /// The socket of the shared instance, as the product places it:
    /// under the runtime folder on Linux, under the state folder (or the
    /// short socket folder) on macOS.
    pub fn socket(&self) -> PathBuf {
        let vars = [
            ("XDG_CONFIG_HOME", self.root.join("cfg")),
            ("XDG_STATE_HOME", self.root.join("state")),
            ("XDG_DATA_HOME", self.root.join("data")),
            ("XDG_CACHE_HOME", self.root.join("cache")),
            ("XDG_RUNTIME_DIR", self.run.clone()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.display().to_string()))
        .collect();
        let home = self.root.join("home");
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let env = paddock::env::HostEnv::from_parts(home, "t".into(), uid, 0, vars);
        paddock::devices::shared::socket_path(&env, &self.name)
    }

    pub fn state_file(&self) -> PathBuf {
        self.root.join(format!(
            "state/paddock/devices/instances/{}/shared.json",
            self.name
        ))
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.root.join(format!(
            "state/paddock/devices/log/{}/shared.log",
            self.name
        )))
        .unwrap_or_default()
    }

    /// The state file of the shared instance, or None when none runs.
    pub fn state(&self) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(self.state_file()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn users(&self) -> u32 {
        self.state().and_then(|v| v["users"].as_u64()).unwrap_or(0) as u32
    }

    pub fn pid(&self) -> u32 {
        self.state().and_then(|v| v["pid"].as_u64()).unwrap_or(0) as u32
    }

    /// A broker: one request, then it holds its lease for `hold` seconds.
    pub fn broker(&self, request: &str, hold: u64) -> Broker {
        let child = self
            .paddock()
            .args([
                "device",
                "check",
                "up",
                "--send",
                request,
                "--hold",
                &hold.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Broker(Some(child))
    }

    /// `paddock device ...`, the whole output and whether it worked.
    pub fn run(&self, args: &[&str]) -> (bool, String) {
        let out = self.paddock().args(args).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }
}

/// A broker process that holds one lease.
pub struct Broker(pub Option<Child>);

impl Broker {
    pub fn pid(&self) -> u32 {
        self.0.as_ref().map(|c| c.id()).unwrap_or(0)
    }

    /// `kill -9`: the lease must drop by the close of the socket.
    pub fn kill9(&mut self) {
        if let Some(c) = self.0.as_mut() {
            // SAFETY: a signal to a child of this test.
            unsafe {
                libc::kill(c.id() as i32, libc::SIGKILL);
            }
            let _ = c.wait();
        }
        self.0 = None;
    }

    pub fn wait(&mut self) -> String {
        let Some(mut c) = self.0.take() else {
            return String::new();
        };
        let mut out = String::new();
        if let Some(mut o) = c.stdout.take() {
            let _ = o.read_to_string(&mut out);
        }
        let mut err = String::new();
        if let Some(mut e) = c.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let _ = c.wait();
        format!("{out}{err}")
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Wait for `f`, up to `secs`. The last value, for the message.
pub fn until<T: std::fmt::Debug>(
    secs: u64,
    mut f: impl FnMut() -> T,
    ok: impl Fn(&T) -> bool,
) -> T {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let v = f();
        if ok(&v) || Instant::now() >= end {
            return v;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn config(script: &str, extra: &str) -> String {
    format!(
        "[devices.up]\nshape = \"exec\"\nenabled = true\ncommand = [\"{script}\"]\nscope = \"shared\"\nidle = 2\ntimeout = 10\n{extra}"
    )
}

pub fn world() -> World {
    let w = World::new("");
    let s = w.script(
        "up.sh",
        "printf 'caller=%s ' \"$PADDOCK_CALLER\"\ntr '[:lower:]' '[:upper:]'",
    );
    std::fs::write(w.config(), config(&s, "")).unwrap();
    w
}
