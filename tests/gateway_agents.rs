//! Gateway agents: `paddock gateway openclaw start|stop|status|logs` and
//! `paddock openclaw tui`, with a fake `openclaw` on PATH.
//!
//! The fake is a small TCP echo server (`openclaw gateway run`) and its
//! client (`openclaw tui`). The server wants the token as the first line,
//! answers `OK` or `DENIED`, echoes each line after that, and exits with 3
//! on the line `CRASH`. It refuses to start without `--bind loopback`,
//! `--auth token` and the token variable, and it writes each start into
//! `~/.openclaw` of the home that it gets.
//!
//! The supervisor runs as paddock runs it on a host without a user manager
//! (a detached process). The gateway and the client run with no sandbox
//! (`PADDOCK_TEST_GATEWAY_LAUNCHER=plain`), so the tests need no
//! bubblewrap: the unit tests of src/agentgw check the sandboxes
//! themselves (home, loopback port, client port).

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const FAKE: &str = r#"#!/usr/bin/python3 -I
import os, socket, sys, threading, time
args = sys.argv[1:]
home = os.environ.get("HOME", "")
state = os.path.join(home, ".openclaw")
def opt(k):
    return args[args.index(k) + 1] if k in args else None
if args[:2] == ["gateway", "run"]:
    os.makedirs(state, exist_ok=True)
    with open(os.path.join(state, "gateway-starts"), "a") as f:
        f.write("%d %s\n" % (os.getpid(), " ".join(args)))
    token = os.environ.get("OPENCLAW_GATEWAY_TOKEN", "")
    if opt("--bind") != "loopback" or opt("--auth") != "token" or len(token) < 32:
        sys.exit(64)
    addr = "0.0.0.0" if os.path.exists(os.path.join(state, "bind-all")) else "127.0.0.1"
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((addr, int(opt("--port"))))
    s.listen(8)
    def serve(c):
        f = c.makefile("rwb")
        if f.readline().decode().strip() != "TOKEN " + token:
            f.write(b"DENIED\n"); f.flush(); c.close(); return
        f.write(b"OK\n"); f.flush()
        for l in f:
            t = l.decode().strip()
            if t == "CRASH":
                os._exit(3)
            f.write(("ECHO " + t + "\n").encode()); f.flush()
        c.close()
    while True:
        c, _ = s.accept()
        threading.Thread(target=serve, args=(c,), daemon=True).start()
elif args[:1] == ["tui"]:
    host, port = opt("--url").replace("ws://", "").split(":")
    c = socket.create_connection((host, int(port)), timeout=5)
    f = c.makefile("rwb")
    f.write(("TOKEN %s\n" % opt("--token")).encode()); f.flush()
    print("auth", f.readline().decode().strip())
    rest = args[args.index("--token") + 2:]
    msg = " ".join(rest) or "hello"
    f.write((msg.replace("HOLD", "hold") + "\n").encode()); f.flush()
    print("reply", f.readline().decode().strip())
    print("home", home)
    sys.stdout.flush()
    if "HOLD" in rest:
        time.sleep(4)
else:
    print("embedded", " ".join(args))
"#;

struct World {
    _tmp: common::TestDir,
    root: PathBuf,
}

impl World {
    fn new() -> Option<Self> {
        if !Path::new("/usr/bin/python3").exists() {
            eprintln!("skipped: no /usr/bin/python3 for the fake gateway");
            return None;
        }
        let tmp = common::tempdir();
        let root = tmp.path().to_path_buf();
        for d in [
            "home",
            "cfg/paddock",
            "state",
            "data",
            "cache",
            "run",
            "bin",
            "work",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let fake = root.join("bin/openclaw");
        std::fs::write(&fake, FAKE).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The host's own OpenClaw state, which nothing may touch.
        std::fs::create_dir_all(root.join("home/.openclaw")).unwrap();
        std::fs::write(
            root.join("home/.openclaw/openclaw.json"),
            "{\"host\": true}\n",
        )
        .unwrap();
        Some(World { _tmp: tmp, root })
    }

    fn paddock(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_paddock"));
        c.env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("USER", "t")
            .env("LANG", "C")
            .env("PADDOCK_TEST_GATEWAY_LAUNCHER", "plain")
            .env("PADDOCK_TEST_NO_USER_MANAGER", "1")
            // A test inside a sandbox with no_new_privs: no clean start
            // through the user manager (cleanstart.rs).
            .env("PADDOCK_CLEAN_START", "1")
            .stdin(Stdio::null())
            .current_dir(self.root.join("work"));
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.paddock().args(args).output().unwrap()
    }

    fn gw(&self) -> PathBuf {
        self.root.join("state/paddock/gateways/openclaw")
    }

    fn gw_home(&self) -> PathBuf {
        self.root.join("data/paddock/gateways/openclaw/home")
    }

    fn state(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&std::fs::read_to_string(self.gw().join("state.json")).ok()?).ok()
    }

    fn status(&self) -> serde_json::Value {
        let o = self.run(&["gateway", "openclaw", "status", "--json"]);
        assert!(o.status.success(), "{}", text(&o));
        serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()["gateway"].clone()
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.gw().join("token")).unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.gw().join("gateway.log")).unwrap_or_default()
    }

    fn start(&self, port: u16) -> Output {
        self.run(&["gateway", "openclaw", "start", "--port", &port.to_string()])
    }

    /// The host's `~/.openclaw`, file by file.
    fn host_openclaw(&self) -> Vec<(String, String)> {
        let d = self.root.join("home/.openclaw");
        let mut v: Vec<(String, String)> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read_to_string(e.path()).unwrap_or_default(),
                )
            })
            .collect();
        v.sort();
        v
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.run(&["gateway", "openclaw", "stop"]);
        // A supervisor that a failed test left behind.
        if let Some(pid) = self.state().and_then(|s| s["pid"].as_u64()) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
}

fn text(o: &Output) -> String {
    format!(
        "status {}\nstdout:\n{}\nstderr:\n{}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The first answer of the gateway to `token`.
fn hello(port: u16, token: &str) -> Option<String> {
    let s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut w = s.try_clone().ok()?;
    writeln!(w, "TOKEN {token}").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    Some(line.trim().to_string())
}

fn until<T>(secs: u64, mut f: impl FnMut() -> T, ok: impl Fn(&T) -> bool) -> T {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let v = f();
        if ok(&v) || Instant::now() >= end {
            return v;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn alive(pid: u64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[test]
fn start_attach_status_and_stop_keep_the_host_state_untouched() {
    let Some(w) = World::new() else { return };
    let before = w.host_openclaw();
    let port = free_port();
    let o = w.start(port);
    assert!(o.status.success(), "{}\nlog:\n{}", text(&o), w.log());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("running"),
        "{}",
        text(&o)
    );

    // The token: made by paddock, private, accepted; another one is not.
    let token = w.token();
    assert_eq!(token.len(), 64, "{token}");
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(w.gw().join("token"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "{mode:o}");
    assert_eq!(hello(port, &token).as_deref(), Some("OK"));
    assert_eq!(hello(port, "wrong").as_deref(), Some("DENIED"));

    // Loopback only: the gateway was told so, and only 127.0.0.1 listens.
    let starts = std::fs::read_to_string(w.gw_home().join(".openclaw/gateway-starts")).unwrap();
    assert!(starts.contains("--bind loopback"), "{starts}");
    assert!(starts.contains(&format!("--port {port}")), "{starts}");
    assert!(
        !starts.contains(&token),
        "the token is not an argument: {starts}"
    );
    if let Ok(t) = std::fs::read_to_string("/proc/net/tcp") {
        let hex = format!(":{port:04X} ");
        let lines: Vec<&str> = t
            .lines()
            .filter(|l| l.contains(&hex) && l.contains(" 0A "))
            .collect();
        assert!(
            !lines.is_empty() && lines.iter().all(|l| l.contains("0100007F:")),
            "{lines:?}"
        );
    }

    let st = w.status();
    assert_eq!(st["state"], "running", "{st}");
    assert_eq!(st["port"], port);
    assert_eq!(st["url"], format!("ws://127.0.0.1:{port}"));
    assert_eq!(st["how"], "detached");
    assert_eq!(st["clients"], 0);
    assert!(st["uptime"].as_u64().is_some(), "{st}");

    // A client attaches through paddock with the URL and the token.
    let o = w.run(&["openclaw", "tui", "hi"]);
    assert!(o.status.success(), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("auth OK"), "{out}");
    assert!(out.contains("reply ECHO hi"), "{out}");
    let client_home = w.root.join("data/paddock/gateways/openclaw/client/home");
    assert!(
        out.contains(&format!("home {}", client_home.display())),
        "{out}"
    );

    // The shim's form, `paddock run openclaw -- tui`, and a client that
    // stays: the status counts it while it runs.
    let held = w
        .paddock()
        .args(["run", "openclaw", "--", "tui", "HOLD"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let n = until(
        10,
        || w.status()["clients"].as_u64().unwrap_or(0),
        |n| *n == 1,
    );
    assert_eq!(n, 1, "{}", w.status());
    let o = held.wait_with_output().unwrap();
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("reply ECHO hold"),
        "{}",
        text(&o)
    );
    assert_eq!(
        until(
            5,
            || w.status()["clients"].as_u64().unwrap_or(9),
            |n| *n == 0
        ),
        0
    );

    // ps and board show it, for the launcher and the TUI.
    let o = w.run(&["ps", "--json"]);
    let ps: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(ps["gateways"][0]["name"], "openclaw", "{ps}");
    assert_eq!(ps["gateways"][0]["state"], "running", "{ps}");
    let o = w.run(&["ps"]);
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("gateway openclaw"),
        "{}",
        text(&o)
    );
    let o = w.run(&["board", "--json"]);
    let b: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(b["gateways"][0]["port"], port, "{b}");

    // A second start finds the one that runs.
    let pid = w.state().unwrap()["pid"].as_u64().unwrap();
    let o = w.start(port);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(w.state().unwrap()["pid"].as_u64().unwrap(), pid);

    let logs = w.run(&["gateway", "openclaw", "logs"]);
    assert!(
        String::from_utf8_lossy(&logs.stdout).contains("listens on"),
        "{}",
        text(&logs)
    );

    let o = w.run(&["gateway", "openclaw", "stop"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("stopped"),
        "{}",
        text(&o)
    );
    assert!(!alive(pid), "the supervisor still runs");
    assert!(
        until(5, || hello(port, &token).is_none(), |g| *g),
        "the port still answers"
    );
    assert_eq!(w.status()["state"], "stopped");
    assert!(!w.gw().join("secrets.json").exists());
    // The token stays for the next start.
    assert_eq!(w.token(), token);

    assert_eq!(w.host_openclaw(), before, "the host's ~/.openclaw changed");
    assert!(w.gw_home().join(".openclaw/gateway-starts").exists());
}

#[test]
fn a_gateway_that_crashes_starts_again_with_the_same_token() {
    let Some(w) = World::new() else { return };
    let port = free_port();
    let o = w.start(port);
    assert!(o.status.success(), "{}\nlog:\n{}", text(&o), w.log());
    let token = w.token();
    let first = w.state().unwrap()["child"].as_u64().unwrap();
    // The client sends the line that makes the fake gateway exit with 3.
    let _ = w.run(&["openclaw", "tui", "CRASH"]);
    let st = until(
        15,
        || w.state().unwrap_or_default(),
        |s| s["restarts"] == 1 && s["phase"] == "running",
    );
    assert_eq!(st["restarts"], 1, "{st}\n{}", w.log());
    assert_eq!(st["phase"], "running", "{st}\n{}", w.log());
    assert_eq!(st["last_exit"], "exit status 3", "{st}");
    assert_ne!(st["child"].as_u64().unwrap(), first, "{st}");
    assert_eq!(hello(port, &token).as_deref(), Some("OK"));
    assert_eq!(w.token(), token);
    let g = w.status();
    assert_eq!(g["restarts"], 1, "{g}");
    assert!(
        w.log().contains("the gateway ended: exit status 3"),
        "{}",
        w.log()
    );
}

#[test]
fn without_a_gateway_the_client_offers_a_start() {
    let Some(w) = World::new() else { return };
    // No terminal: nobody can answer, so paddock says how to start one.
    let o = w.run(&["openclaw", "tui", "hi"]);
    assert!(!o.status.success(), "{}", text(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("paddock gateway openclaw start"), "{err}");
    assert!(w.state().is_none());
    // --yes answers the offer.
    let o = w.run(&["run", "openclaw", "--yes", "--", "tui", "hi"]);
    assert!(o.status.success(), "{}\nlog:\n{}", text(&o), w.log());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("reply ECHO hi"),
        "{}",
        text(&o)
    );
    assert_eq!(w.status()["state"], "running");
}

#[test]
fn host_gateway_commands_are_refused_in_an_agent_sandbox() {
    let Some(w) = World::new() else { return };
    for args in [
        &["run", "openclaw", "--", "gateway", "run"][..],
        &["openclaw", "daemon", "install"][..],
    ] {
        let o = w.run(args);
        assert!(!o.status.success(), "{args:?}: {}", text(&o));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains("paddock gateway openclaw start"),
            "{args:?}: {err}"
        );
    }
    assert!(w.state().is_none());
    assert!(!w.gw_home().join(".openclaw").exists());
}

#[test]
fn a_gateway_on_another_address_than_the_loopback_is_stopped() {
    let Some(w) = World::new() else { return };
    if !Path::new("/proc/net/tcp").exists() {
        eprintln!("skipped: the check reads /proc/net/tcp");
        return;
    }
    std::fs::create_dir_all(w.gw_home().join(".openclaw")).unwrap();
    std::fs::write(w.gw_home().join(".openclaw/bind-all"), "").unwrap();
    let o = w.start(free_port());
    assert!(!o.status.success(), "{}", text(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("not on the loopback only"), "{err}");
    assert_eq!(w.status()["state"], "failed");
}

#[test]
fn a_gateway_on_the_host_gets_the_notice_and_the_way_to_move() {
    let Some(w) = World::new() else { return };
    // A gateway that the user started without paddock, on the host port
    // of a profile override.
    let host = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let hp = host.local_addr().unwrap().port();
    let builtin = include_str!("../profiles/openclaw.toml");
    let text_of = builtin.replace("host_port = 19001", &format!("host_port = {hp}"));
    assert_ne!(text_of, builtin);
    std::fs::create_dir_all(w.root.join("cfg/paddock/harness.d")).unwrap();
    std::fs::write(w.root.join("cfg/paddock/harness.d/openclaw.toml"), text_of).unwrap();
    let o = w.run(&["openclaw", "tui"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains(&format!("answers on 127.0.0.1:{hp}")),
        "{}",
        text(&o)
    );
    assert!(err.contains("paddock gateway openclaw start"), "{err}");
    // `paddock gateway openclaw status` names it too, and so does doctor.
    let o = w.run(&["gateway", "openclaw", "status", "--json"]);
    let j: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(j["gateway"]["host_gateway"], hp, "{j}");
    drop(host);
}

#[test]
fn omarchy_starts_attach_to_the_sandboxed_gateway() {
    let Some(w) = World::new() else { return };
    // Omarchy's launcher would start the host gateway: it must not run.
    let launcher = w.root.join("bin/omarchy-launch-openclaw");
    std::fs::write(&launcher, "#!/bin/sh\necho HOST LAUNCHER \"$@\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = w.start(free_port());
    assert!(o.status.success(), "{}\nlog:\n{}", text(&o), w.log());
    for args in [
        &["wrap", "--", "omarchy-launch-openclaw", "--tui"][..],
        &["wrap", "--", "openclaw", "tui"][..],
    ] {
        let o = w.run(args);
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{args:?}: {}", text(&o));
        assert!(out.contains("reply ECHO hello"), "{args:?}: {}", text(&o));
        assert!(!out.contains("HOST LAUNCHER"), "{args:?}: {}", text(&o));
    }
    // A start of the host gateway itself runs as before, with the notice.
    let o = w.run(&["wrap", "--", "omarchy-launch-openclaw"]);
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("HOST LAUNCHER"),
        "{}",
        text(&o)
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("paddock gateway openclaw start"),
        "{}",
        text(&o)
    );
}

/// macOS: the fake gateway in Seatbelt, the backend of a real start, with
/// the data folder under the home as on a Mac (`~/.local/share`). The
/// project and the home of the gateway are then under the host home, and
/// the sandbox enters them at their own paths.
#[cfg(target_os = "macos")]
#[test]
fn the_fake_gateway_answers_from_seatbelt() {
    let Some(w) = World::new() else { return };
    let data = w.root.join("home/.local/share");
    std::fs::create_dir_all(&data).unwrap();
    let seatbelt = |args: &[&str]| {
        w.paddock()
            .env_remove("PADDOCK_TEST_GATEWAY_LAUNCHER")
            .env("XDG_DATA_HOME", &data)
            .args(args)
            .output()
            .unwrap()
    };
    let before = w.host_openclaw();
    let port = free_port();
    let o = seatbelt(&["gateway", "openclaw", "start", "--port", &port.to_string()]);
    assert!(o.status.success(), "{}\nlog:\n{}", text(&o), w.log());
    let o = seatbelt(&["gateway", "openclaw", "status", "--json"]);
    assert!(o.status.success(), "{}", text(&o));
    let st = serde_json::from_slice::<serde_json::Value>(&o.stdout).unwrap()["gateway"].clone();
    assert_eq!(st["backend"], "seatbelt", "{st}");
    assert_eq!(st["how"], "detached", "{st}");
    assert_eq!(
        hello(port, &w.token()).as_deref(),
        Some("OK"),
        "{}",
        w.log()
    );
    assert_eq!(hello(port, "wrong").as_deref(), Some("DENIED"));
    let gw_home = data.join("paddock/gateways/openclaw/home");
    let starts = std::fs::read_to_string(gw_home.join(".openclaw/gateway-starts")).unwrap();
    assert!(starts.contains(&format!("--port {port}")), "{starts}");
    let o = seatbelt(&["gateway", "openclaw", "stop"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(w.host_openclaw(), before, "the host's ~/.openclaw changed");
}
