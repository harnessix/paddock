//! `run = "remote"` with the paddock binary as the other side: a fake
//! `ssh` on the PATH starts `paddock device serve-remote` with the
//! serving home's folders and an `SSH_CONNECTION`, as sshd would, and the
//! request of a forward device goes through it; `serve-remote --socket`
//! serves many connections and ends on SIGTERM; `check`, `ls`, `add` and
//! `--forget` on the command line. The device sandbox of the serving side
//! runs its program in a thread (`PADDOCK_TEST_DEVICE_LAUNCHER=thread`),
//! so the test needs no bubblewrap. No harness.

mod common;

use paddock::broker::{Answer, Approver, Pending};
use paddock::devices::forward::Forward;
use paddock::devices::socket::{request_at, Request};
use paddock::devices::{Call, Device, DeviceCtx, DevicesCfg, GrantHit};
use paddock::env::HostEnv;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Recording(Answer, Mutex<Vec<Pending>>);

impl Approver for Recording {
    fn ask(&self, _: &HostEnv, p: &Pending, _: Duration) -> Option<Answer> {
        self.1.lock().unwrap().push(p.clone());
        Some(self.0)
    }
}

/// One paddock home: its folders, its configuration, a command that
/// runs the binary with them.
struct Home {
    root: PathBuf,
}

impl Home {
    fn new(tmp: &Path, name: &str, config: &str) -> Self {
        let root = tmp.join(name);
        for d in ["cfg/paddock", "state", "run", "project", "home/bin"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::set_permissions(
            root.join("run"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(root.join("cfg/paddock/config.toml"), config).unwrap();
        Home { root }
    }

    fn vars(&self) -> BTreeMap<String, String> {
        [
            ("HOME", self.root.join("home").display().to_string()),
            (
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("home/bin").display()),
            ),
            (
                "XDG_CONFIG_HOME",
                self.root.join("cfg").display().to_string(),
            ),
            (
                "XDG_STATE_HOME",
                self.root.join("state").display().to_string(),
            ),
            (
                "XDG_DATA_HOME",
                self.root.join("data").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME",
                self.root.join("cache").display().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR",
                self.root.join("run").display().to_string(),
            ),
            ("PADDOCK_TEST_DEVICE_LAUNCHER", "thread".to_string()),
            ("LANG", "C".to_string()),
            ("LC_ALL", "C".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    fn env(&self) -> HostEnv {
        HostEnv::from_parts(
            self.root.join("home"),
            "demo".into(),
            1000,
            1000,
            self.vars(),
        )
    }

    fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env_clear()
            .envs(self.vars())
            .current_dir(self.root.join("project"))
            .stdin(Stdio::null());
        c
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let f = self.root.join("home/bin").join(name);
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        f
    }

    fn audit(&self) -> String {
        std::fs::read_to_string(self.root.join("state/paddock/host-audit.jsonl"))
            .unwrap_or_default()
    }

    /// The forward device `name` of this home, for the sandbox `box1`.
    fn forward(&self, name: &str) -> (Forward, Arc<Recording>) {
        let env = self.env();
        let def = DevicesCfg::load(&env)
            .unwrap()
            .user
            .into_iter()
            .find(|d| d.name == name)
            .unwrap();
        let rec = Arc::new(Recording(Answer::Session, Mutex::new(Vec::new())));
        let ctx = DeviceCtx {
            instances: paddock::devices::sandbox::Instances::new(env.clone()),
            env,
            sandbox: "box1".into(),
            harness: "fake".into(),
            project: self.root.join("project"),
            started_by: vec![],
            approver: rec.clone(),
        };
        (Forward::new(def, ctx, GrantHit::NoKey), rec)
    }
}

const SERVING: &str = "[devices.upper]\nshape = \"exec\"\nenabled = true\ncommand = [\"tr\", \"a-z\", \"A-Z\"]\ngrant = [\"remote:10.0.0.7\", \"remote:local\"]\nmode = \"allow\"\n\n[devices.private]\nshape = \"exec\"\nenabled = true\ncommand = [\"cat\"]\n";

/// A fake `ssh` in the forwarding home that runs the serving paddock on
/// its standard input and output, with the serving folders and the
/// `SSH_CONNECTION` that sshd would set.
fn fake_ssh(forwarding: &Home, serving: &Home) -> PathBuf {
    let argv = forwarding.root.join("ssh-argv");
    let vars: Vec<String> = serving
        .vars()
        .into_iter()
        .map(|(k, v)| format!("{k}={v:?}"))
        .collect();
    forwarding.script(
        "ssh",
        &format!(
            "printf '%s\\n' \"$@\" > {}\nexec env -i {} SSH_CONNECTION=\"10.0.0.7 51234 10.0.0.2 22\" {} device serve-remote",
            argv.display(),
            vars.join(" "),
            env!("CARGO_BIN_EXE_paddock")
        ),
    );
    argv
}

#[test]
fn a_request_goes_through_ssh_to_the_paddock_binary_on_the_other_side() {
    let tmp = common::tempdir();
    let serving = Home::new(tmp.path(), "serving", SERVING);
    let forwarding = Home::new(
        tmp.path(),
        "forwarding",
        "[devices.mac-upper]\nrun = \"remote\"\ntarget = \"ssh://me@mac\"\ndevice = \"upper\"\nenabled = true\n\n[devices.mac-private]\nrun = \"remote\"\ntarget = \"ssh://me@mac\"\ndevice = \"private\"\nenabled = true\n",
    );
    let argv = fake_ssh(&forwarding, &serving);
    let (f, rec) = forwarding.forward("mac-upper");
    let call = Call::for_test("box1", Duration::from_secs(60));
    let out = f.call(&call, b"abc").unwrap();
    assert_eq!(out, b"status: exit 0\n\nABC");
    assert_eq!(
        std::fs::read_to_string(&argv).unwrap(),
        "-T\n-o\nBatchMode=yes\n--\nssh://me@mac\npaddock\ndevice\nserve-remote\n"
    );
    let cards = rec.1.lock().unwrap();
    assert_eq!(
        cards[0].summary,
        "allow the device `mac-upper` for this sandbox: it connects to me@mac with your ssh as you"
    );
    // The serving side: its own audit line names the ssh peer and the
    // caller's text, and its device sandbox log has the request.
    let b = serving.audit();
    let line: serde_json::Value = serde_json::from_str(b.lines().next().unwrap()).unwrap();
    assert_eq!(line["sandbox"], "remote:10.0.0.7");
    assert_eq!(line["device"], "upper");
    assert_eq!(line["grant"], "remote 10.0.0.7");
    assert_eq!(line["remote_caller"], "box1");
    assert_eq!(line["status"], "exit 0");
    assert!(!b.contains("abc") && !b.contains("ABC"));
    let a = forwarding.audit();
    let line: serde_json::Value = serde_json::from_str(a.lines().next().unwrap()).unwrap();
    assert_eq!(line["run"], "remote");
    assert_eq!(line["transport"], "ssh");
    assert_eq!(line["status"], "ok");
    // The instance of the serving side ended with its process.
    let states: Vec<_> = paddock::devices::sandbox::state::InstanceState::list(&serving.env())
        .into_iter()
        .filter(|s| s.alive())
        .collect();
    assert!(states.is_empty(), "{states:?}");
    // A device without a remote grant on the other side: disabled.
    let (p, _) = forwarding.forward("mac-private");
    let out = String::from_utf8(p.call(&call, b"abc").unwrap()).unwrap();
    assert!(
        out.starts_with("status: disabled\nmessage: The device private is not granted to remote callers: the user on this machine grants it to remote callers with grant = [\"remote:10.0.0.7\"]"),
        "{out}"
    );
    // The catalog of the other side merged into the entry here.
    let t0 = Instant::now();
    while f.remote_entry().is_none() && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let entry = f.remote_entry().expect("the catalog of the other side");
    // The other side is this machine in the test.
    assert_eq!(entry["platform"], std::env::consts::OS);
    assert_eq!(entry["run"], "sandbox");
    // `paddock device check` reaches the other side as the user.
    let out = forwarding
        .paddock()
        .args(["device", "check", "mac-upper"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.starts_with("mac-upper: reached ssh://me@mac in "),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "device    upper there: state on, shape exec, run sandbox, platform {}",
            std::env::consts::OS
        )),
        "{text}"
    );
    let out = forwarding
        .paddock()
        .args(["device", "ls"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("mac-upper"), "{text}");
    let row = text.lines().find(|l| l.starts_with("mac-upper")).unwrap();
    assert!(row.contains("remote"), "{row}");
    assert!(row.contains("every sandbox (no grant)"), "{row}");
}

#[test]
fn serve_remote_on_a_socket_takes_many_connections_and_ends_on_sigterm() {
    let tmp = common::tempdir();
    let serving = Home::new(tmp.path(), "serving", SERVING);
    let sock = tmp.path().join("s").join("devices.sock");
    let mut child = serving
        .paddock()
        .args(["device", "serve-remote", "--socket"])
        .arg(&sock)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let t = Instant::now();
    while !sock.exists() && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(sock.exists(), "the socket appeared");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(sock.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    // Two requests on two connections, and the catalog.
    for word in ["abc", "def"] {
        let out = request_at(&sock, &Request::call("upper", 3), word.as_bytes()).unwrap();
        assert_eq!(
            out,
            format!("status: exit 0\n\n{}", word.to_uppercase()).into_bytes()
        );
    }
    let cat = request_at(&sock, &Request::catalog(), &[]).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&cat).unwrap();
    let names: Vec<&str> = v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["state"] == "on")
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["upper"], "private has no remote grant");
    // No secret file for a remote caller.
    assert_eq!(
        request_at(&sock, &Request::secret("x"), &[]).unwrap_err(),
        libc::EACCES
    );
    // A forward to the socket from another home: the peer is `local`.
    let forwarding = Home::new(
        tmp.path(),
        "forwarding",
        &format!(
            "[devices.up]\nrun = \"remote\"\ntarget = \"unix:{}\"\ndevice = \"upper\"\nenabled = true\nmode = \"allow\"\n",
            sock.display()
        ),
    );
    let (f, _) = forwarding.forward("up");
    let call = Call::for_test("box1", Duration::from_secs(30));
    assert_eq!(f.call(&call, b"xyz").unwrap(), b"status: exit 0\n\nXYZ");
    let line: serde_json::Value =
        serde_json::from_str(serving.audit().lines().last().unwrap()).unwrap();
    assert_eq!(line["sandbox"], "remote:local");
    assert_eq!(line["remote_caller"], "box1");
    // SIGTERM: the process ends, the socket goes, the instance too.
    // SAFETY: the pid is the child's.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let t = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "serve-remote did not end"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status:?}");
    assert!(!sock.exists(), "the socket is removed");
    let states: Vec<_> = paddock::devices::sandbox::state::InstanceState::list(&serving.env())
        .into_iter()
        .filter(|s| s.alive())
        .collect();
    assert!(states.is_empty(), "{states:?}");
}

#[test]
fn add_writes_a_remote_table_and_forget_removes_the_session_file() {
    let tmp = common::tempdir();
    let home = Home::new(tmp.path(), "h", "");
    let out = home
        .paddock()
        .args([
            "device",
            "add",
            "mac-build",
            "--run",
            "remote",
            "--target",
            "ssh://mac",
            "--device",
            "build",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for line in [
        "[devices.mac-build]",
        "run = \"remote\"",
        "target = \"ssh://mac\"",
        "device = \"build\"",
        "enabled = true",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}: {text}");
    }
    assert!(!text.contains("command"), "{text}");
    let cfg = DevicesCfg::load(&home.env()).unwrap();
    assert_eq!(cfg.user[0].name, "mac-build");
    // The flags that do not go together.
    let out = home
        .paddock()
        .args([
            "device",
            "add",
            "x",
            "--target",
            "ssh://mac",
            "--device",
            "b",
            "--exec",
            "true",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--run remote takes no --exec or --serve")
    );
    let out = home
        .paddock()
        .args(["device", "add", "x", "--run", "remote", "--exec", "true"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--run remote needs --target"));
    // `show` prints the other machine.
    let out = home
        .paddock()
        .args(["device", "show", "mac-build"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("runs on another machine: device build of ssh://mac"),
        "{text}"
    );
    assert!(!text.contains("not available yet"), "{text}");
    // `--forget`.
    let env = home.env();
    paddock::devices::forward::session::save(&env, "10.0.0.7", "upper", true);
    let p = paddock::devices::forward::session::path(&env);
    assert!(p.exists());
    let out = home
        .paddock()
        .args(["device", "serve-remote", "--forget"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout)
        .starts_with("forgot the session answers of remote callers"));
    assert!(!p.exists());
}
