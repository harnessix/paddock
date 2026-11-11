//! The world of the device sandbox tests (tests/devices_sandbox.rs and
//! tests/devices_sandbox_runs.rs): a temporary home with a user
//! configuration, device scripts in `root/dev`, harness profiles, and the
//! paddock commands that the tests drive (`device check --send`, `run`,
//! `ps --json`), with the readers of their output.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn fuse_works() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/fuse")
        .is_ok()
}

pub fn python() -> Option<&'static str> {
    Path::new("/usr/bin/python3")
        .exists()
        .then_some("/usr/bin/python3")
}

pub struct World {
    _tmp: tempfile::TempDir,
    pub root: PathBuf,
}

impl World {
    /// A world with `config` as the user configuration and the device
    /// scripts in `root/dev` (bound read-only into every device sandbox
    /// that names it in `ro`).
    pub fn new(config: &str) -> Self {
        let tmp = super::tempdir();
        let root = tmp.path().to_path_buf();
        for d in [
            "cfg/paddock/harness.d",
            "run",
            "project",
            "dev",
            "data",
            "state",
            "home",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::set_permissions(
            root.join("run"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(root.join("project/hello.txt"), "hello from the caller\n").unwrap();
        std::fs::write(root.join("cfg/paddock/config.toml"), config).unwrap();
        World { _tmp: tmp, root }
    }

    pub fn script(&self, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let f = self.root.join("dev").join(name);
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        f.display().to_string()
    }

    pub fn harness(&self, name: &str, script: &str) {
        std::fs::write(
            self.root.join(format!("cfg/paddock/harness.d/{name}.toml")),
            format!(
                "name = \"{name}\"\n[locate]\ncommands = [\"sh\"]\n[launch]\nargs = [\"-c\", {}]\n",
                serde_json::to_string(script).unwrap()
            ),
        )
        .unwrap();
    }

    pub fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("PADDOCK_TEST_CONNECT_LOCAL", "1")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .current_dir(self.root.join("project"));
        c
    }

    /// `paddock device check NAME --send REQUEST`: the whole output.
    pub fn check(&self, name: &str, send: &str) -> String {
        let out = self
            .paddock()
            .args(["device", "check", name, "--send", send])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "check {name}:\n{text}\n{err}");
        text
    }

    /// `paddock run NAME` with bubblewrap (the Linux tests).
    pub fn run(&self, name: &str) -> std::process::Child {
        self.run_on(name, "bwrap")
    }

    /// `paddock run NAME --backend BACKEND`, network none, a clean
    /// environment, the project of the world.
    pub fn run_on(&self, name: &str, backend: &str) -> std::process::Child {
        self.paddock()
            .args([
                "run",
                name,
                "--backend",
                backend,
                "--network",
                "none",
                "--env",
                "clean",
                "-y",
            ])
            .arg(self.root.join("project"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    pub fn state_dir(&self, device: &str) -> PathBuf {
        self.root
            .join("state/paddock/devices/instances")
            .join(device)
    }

    pub fn state_files(&self, device: &str) -> Vec<serde_json::Value> {
        std::fs::read_dir(self.state_dir(device))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str(&t).ok())
            .collect()
    }

    pub fn audit(&self) -> String {
        std::fs::read_to_string(self.root.join("state/paddock/host-audit.jsonl"))
            .unwrap_or_default()
    }

    pub fn ps(&self) -> serde_json::Value {
        let out = self.paddock().args(["ps", "--json"]).output().unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or_default()
    }
}

/// The response part of a `check --send` output.
pub fn response(out: &str) -> String {
    out.split_once("response:\n")
        .map(|(_, r)| {
            r.rsplit_once("\nstopped after")
                .map(|(a, _)| a)
                .unwrap_or(r)
        })
        .unwrap_or_else(|| panic!("no response in:\n{out}"))
        .to_string()
}

/// The body after the envelope of an exec device.
pub fn body(out: &str) -> String {
    let r = response(out);
    r.split_once("\n\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or(r)
}

pub fn started_ms(out: &str) -> u64 {
    out.split_once("started in ")
        .and_then(|(_, r)| r.split_once(' '))
        .and_then(|(n, _)| n.parse().ok())
        .unwrap_or_else(|| panic!("no start time in:\n{out}"))
}

pub fn alive(pid: u64) -> bool {
    // SAFETY: a null signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

pub fn wait_for(what: &str, within: Duration, mut f: impl FnMut() -> bool) -> Duration {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < within,
            "{what} did not happen within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    t.elapsed()
}

pub fn can_run() -> bool {
    if !super::bwrap_works() {
        eprintln!("skipped: bwrap cannot create a user namespace here");
        return false;
    }
    true
}

/// The configuration of the view tests: one device per thing to prove.
pub fn view_config(w: &World, port: u16) -> String {
    let dev = w.root.join("dev").display().to_string();
    let home = w.root.join("home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/key"), "private fixture").unwrap();
    std::fs::write(home.join(".bash_history"), "private fixture").unwrap();
    let home = home.display();
    format!(
        r#"
[devices.up]
shape = "exec"
enabled = true
command = ["{dev}/up.sh"]
[devices.up.sandbox]
ro = ["{dev}"]

[devices.ups]
shape = "serve"
enabled = true
command = ["/usr/bin/python3", "-I", "{dev}/upper.py"]
[devices.ups.sandbox]
ro = ["{dev}"]

[devices.caller]
shape = "exec"
enabled = true
command = ["{dev}/caller.sh"]
[devices.caller.sandbox]
ro = ["{dev}"]
caller = ["project:ro"]

[devices.rof]
shape = "exec"
enabled = true
command = ["{dev}/rof.sh"]
[devices.rof.sandbox]
ro = ["{dev}", "{dev}/secret-note.txt"]
home = "hidden"
env = {{ NOTE = "{dev}/secret-note.txt", HOST_HOME = "{home}" }}

[devices.net]
shape = "exec"
enabled = true
timeout = 10
command = ["{dev}/net.sh"]
[devices.net.sandbox]
ro = ["{dev}"]
env = {{ PORT = "{port}" }}

[devices.con]
shape = "exec"
enabled = true
timeout = 10
command = ["{dev}/net.sh"]
[devices.con.sandbox]
ro = ["{dev}"]
connect = ["2222:127.0.0.1:{port}"]
env = {{ PORT = "2222" }}

[credentials.tok]
via = "deliver"
as = "file"
var = "TEST_TOKEN"

[devices.sec]
shape = "exec"
enabled = true
command = ["{dev}/sec.sh"]
[devices.sec.sandbox]
ro = ["{dev}"]
credentials = ["tok"]
"#
    )
}

/// A TCP server on the loopback that answers `pong:` and the bytes it got.
pub fn echo_server() -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    l.set_nonblocking(true).unwrap();
    let t = std::thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(120);
        let mut all = Vec::new();
        while Instant::now() < end {
            match l.accept() {
                Ok((mut s, _)) => {
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut buf = [0u8; 1024];
                    if let Ok(n) = s.read(&mut buf) {
                        all.extend_from_slice(&buf[..n]);
                        let _ = s.write_all(b"pong:");
                        let _ = s.write_all(&buf[..n]);
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        all
    });
    (port, t)
}
