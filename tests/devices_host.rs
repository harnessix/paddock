//! `run = "host"` with the paddock binary as the supervisor on the host:
//! the program sees the user's home and environment, runs in `cwd`, gets
//! the request on stdin, honors `timeout` and the exit mapping; a serve
//! program starts once per run and ends with it; the marks reach the
//! card, the catalog, the audit line, the state file, `paddock ps`,
//! `paddock device ls` and `check`. No harness, no sandbox: a temporary
//! home and scripts.

mod common;

use paddock::broker::{Answer, Approver, Pending};
use paddock::devices::config::DeviceDef;
use paddock::devices::host::{HostSource, HOST_MARK};
use paddock::devices::outproc::OutOfProcess;
use paddock::devices::{Call, DevError, Device, DeviceCtx, DevicesCfg, GrantHit};
use paddock::env::HostEnv;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MARK: &str = "marker-7f3a";

/// A temporary home with a marker file, a marker variable and a bin
/// folder on the PATH.
struct World {
    _tmp: common::TestDir,
    root: PathBuf,
    env: HostEnv,
}

impl World {
    fn new() -> Self {
        let tmp = common::tempdir();
        let root = tmp.path().to_path_buf();
        let home = root.join("home");
        for d in [
            "home/bin",
            "notes-real",
            "cfg/paddock",
            "run",
            "state",
            "project",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::set_permissions(
            root.join("run"),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        std::fs::write(home.join("marker.txt"), "the home file\n").unwrap();
        // `~/notes` is a link, as /tmp is on macOS: the program sees the
        // folder as named, not the real path.
        std::os::unix::fs::symlink(root.join("notes-real"), home.join("notes")).unwrap();
        let vars: BTreeMap<String, String> = [
            ("HOME", home.display().to_string()),
            (
                "PATH",
                format!("{}:/usr/bin:/bin", home.join("bin").display()),
            ),
            ("XDG_RUNTIME_DIR", root.join("run").display().to_string()),
            ("XDG_STATE_HOME", root.join("state").display().to_string()),
            ("XDG_CONFIG_HOME", root.join("cfg").display().to_string()),
            ("XDG_DATA_HOME", root.join("data").display().to_string()),
            ("XDG_CACHE_HOME", root.join("cache").display().to_string()),
            ("PADDOCK_TEST_MARK", MARK.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let env = HostEnv::from_parts(home, "demo".into(), 1000, 1000, vars);
        World {
            _tmp: tmp,
            root,
            env,
        }
    }

    fn script(&self, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let f = self.root.join("home/bin").join(name);
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        name.to_string()
    }

    fn def(&self, name: &str, shape: &str, command: &[&str], extra: &str) -> DeviceDef {
        let cmd: Vec<String> = command.iter().map(|c| format!("{c:?}")).collect();
        let text = format!(
            "[devices.{name}]\nshape = \"{shape}\"\nrun = \"host\"\nenabled = true\ncommand = [{}]\n{extra}",
            cmd.join(", ")
        );
        DevicesCfg::parse_with(
            &text,
            paddock::devices::config::ParseEnv {
                home: Some(&self.env.home),
                ..Default::default()
            },
        )
        .unwrap()
        .user
        .remove(0)
    }

    /// A device of `sandbox`, with its socket folder as a run has it.
    fn device(
        &self,
        d: DeviceDef,
        sandbox: &str,
        answer: Answer,
    ) -> (OutOfProcess, Arc<Recording>) {
        std::fs::create_dir_all(self.root.join("run/paddock").join(sandbox)).unwrap();
        let rec = Arc::new(Recording(answer, Mutex::new(Vec::new())));
        let ctx = DeviceCtx {
            instances: paddock::devices::sandbox::Instances::new(self.env.clone()),
            env: self.env.clone(),
            sandbox: sandbox.into(),
            harness: "fake".into(),
            project: self.root.join("project"),
            started_by: vec![],
            approver: rec.clone(),
        };
        let src = HostSource::with_binary(
            d.clone(),
            &self.env,
            sandbox,
            PathBuf::from(env!("CARGO_BIN_EXE_paddock")),
        );
        (
            OutOfProcess::new(d, ctx, Arc::new(src), GrantHit::NoKey),
            rec,
        )
    }

    fn audit(&self) -> String {
        std::fs::read_to_string(paddock::broker::audit_file(&self.env)).unwrap_or_default()
    }

    fn state_file(&self, name: &str, sandbox: &str) -> PathBuf {
        paddock::devices::sandbox::state::state_file(&self.env, name, sandbox)
    }

    fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env_clear()
            .envs(&self.env.vars)
            .current_dir(self.root.join("project"))
            .stdin(Stdio::null());
        c
    }
}

/// Records every card and answers with one answer.
struct Recording(Answer, Mutex<Vec<Pending>>);

impl Approver for Recording {
    fn ask(&self, _: &HostEnv, p: &Pending, _: Duration) -> Option<Answer> {
        self.1.lock().unwrap().push(p.clone());
        Some(self.0)
    }
}

fn call() -> Call {
    Call::for_test("box1", Duration::from_secs(60))
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn alive(pid: u32) -> bool {
    // SAFETY: a null signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn wait_until(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(
            t.elapsed() < within,
            "{what} did not happen within {within:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pid_of(w: &World, name: &str, sandbox: &str) -> u32 {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.state_file(name, sandbox)).unwrap()).unwrap();
    assert_eq!(v["run"], "host", "{v}");
    assert_eq!(v["device"], name);
    assert_eq!(v["caller"], sandbox);
    v["pid"].as_u64().unwrap() as u32
}

#[test]
fn a_host_exec_device_sees_the_home_and_the_environment_and_runs_in_cwd() {
    let w = World::new();
    let tool = w.script(
        "notes-search",
        "echo \"argc=$# args=$*\"\necho \"mark=$PADDOCK_TEST_MARK\"\necho \"home=$HOME\"\necho \"cwd=$(pwd)\"\necho \"caller=$PADDOCK_CALLER\"\ncat \"$HOME/marker.txt\"\necho \"editor=$EDITOR\"\necho --- \ncat\necho oops >&2",
    );
    let d = w.def(
        "notes",
        "exec",
        &[&tool, "-c", "."],
        "cwd = \"~/notes\"\nenv = { EDITOR = \"vi\" }\n",
    );
    let (dev, rec) = w.device(d, "box1", Answer::Session);
    let out = dev.call(&call(), b"--evil; rm -rf /\n$(id)").unwrap();
    let out = text(&out);
    let (head, body) = out.split_once("\n\n").unwrap();
    assert_eq!(head, "status: exit 0\nstderr: oops");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(
        lines[0], "argc=2 args=-c .",
        "the request is never an argument"
    );
    assert_eq!(lines[1], format!("mark={MARK}"), "the host environment");
    assert_eq!(lines[2], format!("home={}", w.env.home.display()));
    assert_eq!(
        lines[3],
        format!("cwd={}", w.env.home.join("notes").display())
    );
    assert_eq!(lines[4], "caller=box1");
    assert_eq!(lines[5], "the home file", "the home is the user's");
    assert_eq!(lines[6], "editor=vi", "the fixed env values");
    assert_eq!(lines[7], "---");
    assert_eq!(lines[8], "--evil; rm -rf /");
    assert_eq!(lines[9], "$(id)");
    // The card: ON THE HOST, with the command and cwd.
    let cards = rec.1.lock().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(
        cards[0].summary,
        "allow the device `notes` for this sandbox: it runs ON THE HOST as you"
    );
    assert!(
        cards[0].detail.contains("ON THE HOST as you"),
        "{}",
        cards[0].detail
    );
    assert!(
        cards[0]
            .detail
            .contains(&format!("in {}", w.env.home.join("notes").display())),
        "{}",
        cards[0].detail
    );
    drop(cards);
    // The audit line.
    let a = w.audit();
    for want in [
        "\"run\":\"host\"",
        "\"shape\":\"exec\"",
        "\"instance\":\"host\"",
        "\"status\":\"exit 0\"",
        "\"decision\":\"allowed for the session\"",
    ] {
        assert!(a.contains(want), "{want} missing in {a}");
    }
    assert!(!a.contains("evil"), "{a}");
    // The catalog.
    let dev: Arc<dyn Device> = Arc::new(dev);
    let v: serde_json::Value =
        serde_json::from_slice(&paddock::devices::catalog(std::slice::from_ref(&dev))).unwrap();
    let e = &v["devices"][0];
    assert_eq!(e["run"], "host");
    assert_eq!(e["instance"], "running");
    assert_eq!(e["instance_name"], "host");
    assert!(e["request"].as_str().unwrap().starts_with(HOST_MARK), "{e}");
    // The state file, while the supervisor runs.
    let pid = pid_of(&w, "notes", "box1");
    assert!(alive(pid));
    let log = w.root.join("state/paddock/devices/log/notes/box1.log");
    let l = std::fs::read_to_string(&log).unwrap();
    assert!(l.contains("on the host"), "{l}");
    assert!(l.contains("] oops"), "the program's stderr: {l}");
    assert!(!l.contains("evil"), "{l}");
    // The end of the run: the supervisor and the state file are gone.
    drop(dev);
    wait_until("the supervisor ended", Duration::from_secs(3), || {
        !alive(pid)
    });
    assert!(!w.state_file("notes", "box1").exists());
}

#[test]
fn timeout_and_the_exit_mapping_hold_on_the_host() {
    let w = World::new();
    let tool = w.script(
        "tool",
        "code=$(cat)\nif [ \"$code\" = sleep ]; then sleep 30; fi\nexit $code",
    );
    let d = w.def("tool", "exec", &[&tool], "timeout = 1\n");
    let (dev, _) = w.device(d, "box1", Answer::Session);
    assert_eq!(dev.call(&call(), b"65").unwrap_err(), DevError::Invalid);
    assert_eq!(dev.call(&call(), b"75").unwrap_err(), DevError::Waiting);
    assert_eq!(dev.call(&call(), b"77").unwrap_err(), DevError::Denied);
    let out = text(&dev.call(&call(), b"3").unwrap());
    assert!(out.starts_with("status: exit 3\n"), "{out}");
    let t = Instant::now();
    let out = text(&dev.call(&call(), b"sleep").unwrap());
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(
        out.starts_with("status: timeout\nmessage: the command did not finish in 1 seconds"),
        "{out}"
    );
    let a = w.audit();
    for want in [
        "\"status\":\"rejected\"",
        "\"status\":\"waiting\"",
        "\"status\":\"declined\"",
        "\"status\":\"exit 3\"",
        "\"status\":\"timeout\"",
    ] {
        assert!(a.contains(want), "{want} missing in {a}");
    }
    assert_eq!(a.matches("\"run\":\"host\"").count(), 5);
}

#[test]
fn a_host_serve_device_starts_once_per_run_and_ends_with_it() {
    if !Path::new("/usr/bin/python3").exists() {
        eprintln!("skipped: no /usr/bin/python3");
        return;
    }
    let w = World::new();
    let script = w.root.join("home/bin/upper.py");
    std::fs::write(&script, paddock::devices::program::EXAMPLE).unwrap();
    let d = w.def(
        "upper",
        "serve",
        &["/usr/bin/python3", "-I", &script.display().to_string()],
        "",
    );
    let (dev, rec) = w.device(d, "box1", Answer::Session);
    assert_eq!(dev.call(&call(), b"abc").unwrap(), b"ABC");
    let pid = pid_of(&w, "upper", "box1");
    assert_eq!(dev.call(&call(), b"def").unwrap(), b"DEF");
    assert_eq!(pid_of(&w, "upper", "box1"), pid, "one supervisor per run");
    assert_eq!(rec.1.lock().unwrap().len(), 1, "one card per run");
    let details = dev.details().unwrap();
    assert_eq!(details["shape"], "serve");
    assert_eq!(details["run"], "host");
    assert_eq!(details["instance"], "running");
    let a = w.audit();
    assert!(a.contains("\"run\":\"host\""), "{a}");
    assert!(a.contains("\"shape\":\"serve\""), "{a}");
    drop(dev);
    wait_until("the supervisor ended", Duration::from_secs(3), || {
        !alive(pid)
    });
    assert!(!w.state_file("upper", "box1").exists());
}

#[test]
fn a_missing_cwd_and_a_missing_program_say_so_in_the_envelope() {
    let w = World::new();
    let tool = w.script("tool", "cat");
    let d = w.def("gone", "exec", &[&tool], "cwd = \"~/gone\"\n");
    let (dev, _) = w.device(d, "box1", Answer::Session);
    let out = text(&dev.call(&call(), b"x").unwrap());
    assert_eq!(out, "status: error\nmessage: cwd ~/gone does not exist\n\n");
    // The folder appears: the next request works without a restart.
    std::fs::create_dir_all(w.env.home.join("gone")).unwrap();
    assert_eq!(
        text(&dev.call(&call(), b"x").unwrap()),
        "status: exit 0\n\nx"
    );
    let d = w.def("missing", "exec", &["notes-search"], "");
    let (dev, _) = w.device(d, "box2", Answer::Session);
    let out = text(&dev.call(&call(), b"x").unwrap());
    assert_eq!(
        out,
        "status: error\nmessage: notes-search is not on the PATH of the host\n\n"
    );
    let a = w.audit();
    assert!(a.contains("\"status\":\"error\""), "{a}");
}

#[test]
fn ps_ls_doctor_and_check_show_the_host_marks() {
    let w = World::new();
    let tool = w.script("notes-search", "cat");
    std::fs::write(
        w.root.join("cfg/paddock/config.toml"),
        format!(
            "[devices.notes]\nshape = \"exec\"\nrun = \"host\"\nenabled = true\ncommand = [\"{tool}\"]\ncwd = \"~/notes\"\n[devices.jq]\nshape = \"exec\"\nenabled = true\ncommand = [\"jq\"]\n"
        ),
    )
    .unwrap();
    // ls: HOST in capitals.
    let out = w.paddock().args(["device", "ls"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let s = text(&out.stdout);
    assert!(s.contains("notes  exec     HOST     run    on"), "{s}");
    assert!(s.contains("jq     exec     sandbox  run    on"), "{s}");
    // doctor: the heading and one line with the command.
    let cfg = DevicesCfg::load(&w.env).unwrap();
    let lines = paddock::devices::doctor::lines(&w.env, &cfg);
    assert!(
        lines.contains(&"  host     devices that run on the host as you, with your home, your keys and your network:".to_string()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&format!(
            "  host       notes: `{tool}` in {}",
            w.env.home.join("notes").display()
        )),
        "{lines:?}"
    );
    assert!(!lines.iter().any(|l| l.contains("jq:")), "{lines:?}");
    // check: the warning line first, then the start, the health, the stop.
    let out = w
        .paddock()
        .args(["device", "check", "notes"])
        .output()
        .unwrap();
    let s = text(&out.stdout);
    assert!(out.status.success(), "{s}\n{}", text(&out.stderr));
    assert!(
        s.starts_with(&format!(
            "notes runs ON THE HOST as you, with your home, your keys and your network: `{tool}` in {}\n",
            w.env.home.join("notes").display()
        )),
        "{s}"
    );
    assert!(
        s.contains("device notes: started on the host in") && s.contains("(health answered)"),
        "{s}"
    );
    assert!(s.contains("stopped after"), "{s}");
    // The check left no state file and no instance folder behind.
    let left: Vec<_> = std::fs::read_dir(w.root.join("state/paddock/devices/instances/notes"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // ps: the device row under its sandbox while the supervisor runs.
    let id = "fake-p-1";
    let run = w.root.join("run/paddock").join(id);
    std::fs::create_dir_all(&run).unwrap();
    let info = serde_json::json!({
        "id": id,
        "harness": "fake",
        "project": w.root.join("project"),
        "backend": "bwrap",
        "network": "none",
        "gui": "off",
        "pid": std::process::id(),
        "started": 10,
        "auth": "none",
        "host_pid": std::process::id(),
    });
    std::fs::write(run.join("info.json"), serde_json::to_vec(&info).unwrap()).unwrap();
    let d = cfg.user.iter().find(|u| u.name == "notes").unwrap().clone();
    let (dev, _) = w.device(d, id, Answer::Session);
    assert_eq!(
        text(&dev.call(&call(), b"x").unwrap()),
        "status: exit 0\n\nx"
    );
    let pid = pid_of(&w, "notes", id);
    let out = w.paddock().args(["ps", "--json"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let sb = v["sandboxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("{v}"));
    let row = &sb["devices"][0];
    assert_eq!(row["device"], "notes", "{v}");
    assert_eq!(row["run"], "host");
    assert_eq!(row["pid"], pid);
    assert_eq!(row["shape"], "exec");
    let out = w.paddock().arg("ps").output().unwrap();
    let s = text(&out.stdout);
    assert!(
        s.contains(&format!("device notes (host, running 0m, pid {pid})")),
        "{s}"
    );
    drop(dev);
    wait_until("the supervisor ended", Duration::from_secs(3), || {
        !alive(pid)
    });
    let out = w.paddock().args(["ps", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["sandboxes"][0]["devices"].is_null(), "{v}");
}
