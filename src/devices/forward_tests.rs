//! `run = "remote"` between two paddock homes in one process: the serving
//! home serves its devices with `Server::for_remote` (the device sandbox
//! runs its program in a thread, no bwrap), the forwarding home has a
//! `Forward` for one of them. The ssh transport uses a fake `ssh` on the
//! PATH that bridges its standard input and output to the serving socket.
//! tests/devices_remote.rs runs the real `paddock device serve-remote`.

use super::serve::{devices_for, PEER_LOCAL};
use super::*;
use crate::broker::{Answer, Approver, Pending};
use crate::devices::supervise::tests::script;
use crate::devices::{catalog, DevicesCfg};
use crate::env::HostEnv;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// Records every card and answers with one answer.
pub(crate) struct Recording(pub Answer, pub Mutex<Vec<Pending>>);

impl Approver for Recording {
    fn ask(&self, _: &HostEnv, p: &Pending, _: Duration) -> Option<Answer> {
        self.1.lock().unwrap().push(p.clone());
        Some(self.0)
    }
}

/// A paddock home under `dir/name`: its own run, state and configuration
/// folders, the device sandboxes in threads, a recording approver.
pub(crate) struct Home {
    pub env: HostEnv,
    pub approver: Arc<Recording>,
    pub dir: PathBuf,
}

impl Home {
    pub fn new(dir: &Path, name: &str, config: &str, answer: Answer) -> Self {
        let dir = dir.join(name);
        for d in ["run", "state", "bin", "home", "cfg/paddock", "project"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("cfg/paddock/config.toml"), config).unwrap();
        let vars: BTreeMap<String, String> = [
            ("XDG_RUNTIME_DIR", dir.join("run").display().to_string()),
            ("XDG_STATE_HOME", dir.join("state").display().to_string()),
            ("XDG_CONFIG_HOME", dir.join("cfg").display().to_string()),
            (
                "PATH",
                format!("{}:/usr/bin:/bin", dir.join("bin").display()),
            ),
            (crate::devices::shared::LAUNCHER_VAR, "thread".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let env = HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars);
        Self {
            env,
            approver: Arc::new(Recording(answer, Mutex::new(Vec::new()))),
            dir,
        }
    }

    /// The context of an agent sandbox `box1` of this home.
    pub fn ctx(&self) -> DeviceCtx {
        DeviceCtx {
            instances: crate::devices::sandbox::Instances::with_launcher(
                self.env.clone(),
                crate::devices::sandbox::Launcher::Thread,
            ),
            env: self.env.clone(),
            sandbox: "box1".into(),
            harness: "fake".into(),
            project: self.dir.join("project"),
            started_by: vec!["root1".into()],
            approver: self.approver.clone(),
        }
    }

    /// The forward device of the table `name` of this home.
    pub fn forward(&self, name: &str) -> Forward {
        let def = DevicesCfg::load(&self.env)
            .unwrap()
            .user
            .into_iter()
            .find(|d| d.name == name)
            .unwrap();
        Forward::new(def, self.ctx(), GrantHit::NoKey)
    }

    pub fn audit(&self) -> String {
        std::fs::read_to_string(crate::broker::audit_file(&self.env)).unwrap_or_default()
    }

    pub fn cards(&self) -> Vec<Pending> {
        self.approver.1.lock().unwrap().clone()
    }

    /// Serve the devices of this home to remote callers on `sock`.
    pub fn serve(&self, sock: &Path) -> (socket::Serving, Arc<crate::devices::sandbox::Instances>) {
        let (devices, instances) =
            devices_for(&self.env, PEER_LOCAL, self.approver.clone()).unwrap();
        let server = socket::Server::for_remote(&serve::identity(PEER_LOCAL), devices);
        (socket::serve(sock, server).unwrap(), instances)
    }
}

pub(crate) fn upper_config(grant: &str, extra: &str) -> String {
    format!(
        "[devices.upper]\nshape = \"exec\"\nenabled = true\ncommand = [\"tr\", \"a-z\", \"A-Z\"]\n{grant}{extra}"
    )
}

pub(crate) fn forward_config(target: &str, extra: &str) -> String {
    format!(
        "[devices.mac-upper]\nrun = \"remote\"\ntarget = \"{target}\"\ndevice = \"upper\"\nenabled = true\n{extra}"
    )
}

pub(crate) fn call() -> Call {
    Call::for_test("box1", Duration::from_secs(20))
}

/// A short socket path: the temporary folder is long, sockets are not.
pub(crate) fn sock_path(dir: &Path) -> PathBuf {
    dir.join("s").join("devices.sock")
}

#[test]
fn a_request_crosses_two_homes_with_a_card_and_an_audit_line_on_each_side() {
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let serving = Home::new(
        t.path(),
        "serving",
        &upper_config("grant = [\"remote:any\"]\n", ""),
        Answer::Session,
    );
    let (_guard, _inst) = serving.serve(&sock);
    let forwarding = Home::new(
        t.path(),
        "forwarding",
        &forward_config(&format!("unix:{}", sock.display()), ""),
        Answer::Session,
    );
    let f = forwarding.forward("mac-upper");
    let out = f.call(&call(), b"abc").unwrap();
    assert_eq!(out, b"status: exit 0\n\nABC");
    // The card on the forwarding side: a socket, so no host mark.
    let local = forwarding.cards();
    assert_eq!(local.len(), 1);
    assert_eq!(
        local[0].summary,
        "allow the device `mac-upper` for this sandbox"
    );
    assert!(
        local[0].detail.starts_with(&format!(
            "each request connects to unix:{} and the paddock there runs its device `upper`",
            sock.display()
        )),
        "{}",
        local[0].detail
    );
    // The card on the serving side names the peer and the caller's text.
    let remote = serving.cards();
    assert_eq!(remote.len(), 1, "{remote:?}");
    assert_eq!(
        remote[0].summary,
        format!(
            "allow the device `upper` for a remote caller from local (sandbox box1 on {}, started by root1)",
            crate::report::hostname()
        )
    );
    assert!(
        remote[0].detail.contains(
            "is what that paddock says about its caller, not something this machine can check"
        ),
        "{}",
        remote[0].detail
    );
    assert_eq!(remote[0].sandbox, "remote:local");
    assert_eq!(remote[0].harness, "remote");
    // One audit line on each side, with the transport and the caller text.
    let a = forwarding.audit();
    assert_eq!(a.lines().count(), 1, "{a}");
    let line: serde_json::Value = serde_json::from_str(a.lines().next().unwrap()).unwrap();
    assert_eq!(line["device"], "mac-upper");
    assert_eq!(line["run"], "remote");
    assert_eq!(line["transport"], "unix");
    assert_eq!(line["remote_device"], "upper");
    assert_eq!(line["status"], "ok");
    assert_eq!(line["detail"], "3 bytes in, 19 bytes out");
    let b = serving.audit();
    assert_eq!(b.lines().count(), 1, "{b}");
    let line: serde_json::Value = serde_json::from_str(b.lines().next().unwrap()).unwrap();
    assert_eq!(line["device"], "upper");
    assert_eq!(line["sandbox"], "remote:local");
    assert_eq!(line["run"], "sandbox");
    assert_eq!(line["grant"], "remote local");
    assert_eq!(line["remote_caller"], "box1");
    assert_eq!(line["remote_host"], crate::report::hostname());
    assert_eq!(line["remote_chain"][0], "root1");
    assert_eq!(line["status"], "exit 0");
    // Never a body on either side.
    assert!(!a.contains("abc") && !a.contains("ABC"));
    assert!(!b.contains("abc") && !b.contains("ABC"));
    // The second request: the session answer holds, no new card, and the
    // catalog of the other side was merged after the first one.
    assert_eq!(f.call(&call(), b"xyz").unwrap(), b"status: exit 0\n\nXYZ");
    assert_eq!(forwarding.cards().len(), 1);
    assert_eq!(serving.cards().len(), 1);
    let t0 = Instant::now();
    while f.remote_entry().is_none() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let entry = f.remote_entry().expect("the catalog of the other side");
    assert_eq!(entry["name"], "upper");
    assert_eq!(entry["run"], "sandbox");
    let v: serde_json::Value = serde_json::from_slice(&catalog(&[Arc::new(f)])).unwrap();
    let e = &v["devices"][0];
    assert_eq!(e["name"], "mac-upper");
    assert_eq!(e["run"], "remote");
    assert_eq!(e["target"], format!("unix:{}", sock.display()));
    assert_eq!(e["transport"], "unix");
    assert_eq!(e["device"], "upper");
    assert_eq!(e["instance"], "remote");
    assert_eq!(e["remote"]["run"], "sandbox");
    assert_eq!(e["remote"]["shape"], "exec");
    assert!(
        e["request"]
            .as_str()
            .unwrap()
            .starts_with("Forwarded to the device `upper` of the paddock at unix:"),
        "{}",
        e["request"]
    );
    assert!(
        e["request"]
            .as_str()
            .unwrap()
            .contains("standard input of the command `tr a-z A-Z`"),
        "the other side's own text: {}",
        e["request"]
    );
    assert!(e["last_error"].is_null());
    assert!(e["platform"].is_null(), "the platform is the other side's");
}

/// What a test device of the serving side does with a request.
type Answering = Box<dyn Fn(&Call, &mut dyn Sink) -> Result<(), DevError> + Send + Sync>;

/// A device of the serving side for the errno and stream tests.
struct Answers(&'static str, Answering);

impl Device for Answers {
    fn name(&self) -> &str {
        self.0
    }
    fn info(&self) -> Info {
        Info::builtin(
            "any",
            "any",
            super::super::Limits {
                max_request: 1024,
                max_response: 1024,
                deadline_seconds: 5,
            },
        )
    }
    fn call(&self, call: &Call, _: &[u8]) -> Result<Vec<u8>, DevError> {
        let mut c = super::super::Collect::default();
        (self.1)(call, &mut c)?;
        Ok(c.0)
    }
    fn call_stream(&self, call: &Call, _: &[u8], sink: &mut dyn Sink) -> Result<(), DevError> {
        (self.1)(call, sink)
    }
}

fn serve_devices(sock: &Path, devices: Vec<Arc<dyn Device>>) -> socket::Serving {
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    socket::serve(sock, socket::Server::for_remote("remote:local", devices)).unwrap()
}

fn forward_to(t: &Path, sock: &Path, device: &str, extra: &str) -> (Forward, Home) {
    let home = Home::new(
        t,
        "forwarding",
        &format!(
            "[devices.fwd]\nrun = \"remote\"\ntarget = \"unix:{}\"\ndevice = \"{device}\"\nenabled = true\nmode = \"allow\"\n{extra}",
            sock.display()
        ),
        Answer::Session,
    );
    let f = home.forward("fwd");
    (f, home)
}

#[test]
fn the_errno_of_the_other_side_is_the_errno_here_and_a_closed_transport_is_eio() {
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    let devices: Vec<Arc<dyn Device>> = vec![
        Arc::new(Answers("invalid", Box::new(|_, _| Err(DevError::Invalid)))),
        Arc::new(Answers("waiting", Box::new(|_, _| Err(DevError::Waiting)))),
        Arc::new(Answers("denied", Box::new(|_, _| Err(DevError::Denied)))),
        Arc::new(Answers(
            "budget",
            Box::new(|_, _| Err(DevError::BudgetUsed)),
        )),
        Arc::new(Answers(
            "slow",
            Box::new(|c, s| {
                while !c.cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                s.push(b"late");
                Err(DevError::Cancelled)
            }),
        )),
    ];
    let _guard = serve_devices(&sock, devices);
    for (name, want) in [
        ("invalid", DevError::Invalid),
        ("waiting", DevError::Waiting),
        ("denied", DevError::Denied),
        ("budget", DevError::BudgetUsed),
        // Unknown on the other side: ENOENT, which has no name here.
        ("nothere", DevError::Failed),
    ] {
        let (f, home) = forward_to(t.path(), &sock, name, "");
        assert_eq!(f.call(&call(), b"x").unwrap_err(), want, "{name}");
        let line: serde_json::Value =
            serde_json::from_str(home.audit().lines().last().unwrap()).unwrap();
        assert_eq!(line["status"], status_of(want), "{name}");
        if want == DevError::Failed {
            assert_eq!(
                f.details().unwrap()["last_error"],
                format!("the paddock at unix:{} answered error 2", sock.display())
            );
        }
    }
    // The local timeout is the whole round trip: the other side's slow
    // device is cut at the local deadline, which it sees as a close.
    let (f, _home) = forward_to(t.path(), &sock, "slow", "timeout = 1\n");
    let t0 = Instant::now();
    let call = Call::for_test("box1", Duration::from_secs(1));
    assert_eq!(f.call(&call, b"x").unwrap_err(), DevError::TimedOut);
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    assert_eq!(DevError::TimedOut.errno(), libc::ETIMEDOUT);
    // A transport that closes before the answer: EIO at once.
    let closed = t.path().join("s").join("closed.sock");
    let l = std::os::unix::net::UnixListener::bind(&closed).unwrap();
    std::thread::spawn(move || {
        for c in l.incoming() {
            drop(c);
        }
    });
    let (f, home) = forward_to(t.path(), &closed, "upper", "");
    let t0 = Instant::now();
    assert_eq!(f.call(&call, b"x").unwrap_err(), DevError::Failed);
    assert!(t0.elapsed() < Duration::from_secs(2));
    let line: serde_json::Value =
        serde_json::from_str(home.audit().lines().last().unwrap()).unwrap();
    assert_eq!(line["status"], "failed");
    assert_eq!(
        f.details().unwrap()["last_error"],
        format!(
            "the connection to unix:{} closed before the answer",
            closed.display()
        )
    );
    // No socket at all: EIO with the connect error.
    let (f, _home) = forward_to(t.path(), &t.path().join("none.sock"), "upper", "");
    assert_eq!(f.call(&call, b"x").unwrap_err(), DevError::Failed);
    assert!(f.details().unwrap()["last_error"]
        .as_str()
        .unwrap()
        .starts_with("cannot connect to "));
}

/// A sink that records each piece and when it came.
struct Pieces(Vec<(Vec<u8>, Instant)>);

impl Sink for Pieces {
    fn push(&mut self, bytes: &[u8]) {
        self.0.push((bytes.to_vec(), Instant::now()));
    }
}

#[test]
fn a_stream_of_the_other_side_arrives_in_order_as_it_comes() {
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    let devices: Vec<Arc<dyn Device>> = vec![Arc::new(Answers(
        "count",
        Box::new(|_, s| {
            for i in 1..=3u8 {
                s.push(&[b'0' + i]);
                std::thread::sleep(Duration::from_millis(150));
            }
            Ok(())
        }),
    ))];
    let _guard = serve_devices(&sock, devices);
    let (f, _home) = forward_to(t.path(), &sock, "count", "stream = true\n");
    assert!(f.info().stream);
    let mut got = Pieces(Vec::new());
    let t0 = Instant::now();
    f.call_stream(&call(), b"x", &mut got).unwrap();
    let total = t0.elapsed();
    let bytes: Vec<u8> = got.0.iter().flat_map(|(b, _)| b.clone()).collect();
    assert_eq!(bytes, b"123");
    assert!(got.0.len() >= 2, "pieces as they come: {}", got.0.len());
    let first = got.0[0].1.duration_since(t0);
    assert!(
        first + Duration::from_millis(200) < total,
        "the first piece came before the end: {first:?} of {total:?}"
    );
    // Without `stream` the forward asks for the whole answer.
    let (f, _home) = forward_to(t.path(), &sock, "count", "");
    assert_eq!(f.call(&call(), b"x").unwrap(), b"123");
}

#[test]
fn a_device_without_the_remote_grant_answers_disabled_with_the_grant_to_add() {
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    // `all` and a missing grant key cover every sandbox of that host, but
    // no remote caller.
    for grant in ["", "grant = [\"all\"]\n", "grant = [\"remote:10.0.0.1\"]\n"] {
        let serving = Home::new(
            t.path(),
            "serving",
            &upper_config(grant, ""),
            Answer::Session,
        );
        let (_guard, _inst) = serving.serve(&sock);
        let (f, _home) = forward_to(t.path(), &sock, "upper", "");
        let out = String::from_utf8(f.call(&call(), b"abc").unwrap()).unwrap();
        assert_eq!(
            out,
            "status: disabled\nmessage: The device upper is not granted to remote callers: the user on this machine grants it to remote callers with grant = [\"remote:local\"] or grant = [\"remote:any\"] in [devices.upper].\n\n",
            "{grant:?}"
        );
        assert!(serving.cards().is_empty(), "no card for an off device");
        // The catalog of the serving side says so too.
        let (devices, _) = devices_for(&serving.env, "local", serving.approver.clone()).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&catalog(&devices)).unwrap();
        let e = v["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "upper")
            .unwrap();
        assert_eq!(e["state"], "off");
        assert!(e["how_to_enable"]
            .as_str()
            .unwrap()
            .contains("grant = [\"remote:HOST\"]"));
        // The built-in devices are not for remote callers.
        assert!(v["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["name"] != "echo" || e["state"] == "off"));
        std::fs::remove_dir_all(t.path().join("serving")).unwrap();
        std::fs::remove_dir_all(t.path().join("forwarding")).unwrap();
    }
    // `remote:local` covers the socket peer.
    let serving = Home::new(
        t.path(),
        "serving",
        &upper_config("grant = [\"remote:local\"]\n", ""),
        Answer::Session,
    );
    let (_guard, _inst) = serving.serve(&sock);
    let (f, _home) = forward_to(t.path(), &sock, "upper", "");
    assert_eq!(f.call(&call(), b"abc").unwrap(), b"status: exit 0\n\nABC");
}

#[test]
fn a_cancel_closes_the_transport_and_the_program_on_the_other_side_dies_within_500_ms() {
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let pidfile = t.path().join("pid");
    let s = script(
        t.path(),
        "sleeper.sh",
        // The pid comes by a rename: the file is never there but empty.
        &format!(
            "echo $$ > {p}.tmp && mv {p}.tmp {p}\nsleep 30\n",
            p = pidfile.display()
        ),
    );
    let serving = Home::new(
        t.path(),
        "serving",
        &format!(
            "[devices.sleeper]\nshape = \"exec\"\nenabled = true\ncommand = [\"{s}\"]\ngrant = [\"remote:any\"]\ntimeout = 60\n"
        ),
        Answer::Session,
    );
    let (_guard, _inst) = serving.serve(&sock);
    let (f, home) = forward_to(t.path(), &sock, "sleeper", "timeout = 60\n");
    let f = Arc::new(f);
    let cancel = Arc::new(AtomicBool::new(false));
    let call = Call::with_cancel("box1", Duration::from_secs(60), cancel.clone());
    let f2 = f.clone();
    let worker = std::thread::spawn(move || f2.call(&call, b"x"));
    let t0 = Instant::now();
    while !pidfile.exists() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = std::fs::read_to_string(&pidfile)
        .expect("the program started")
        .trim()
        .parse()
        .unwrap();
    // The program runs; the reader goes.
    std::thread::sleep(Duration::from_millis(100));
    assert!(alive(pid));
    cancel.store(true, Ordering::SeqCst);
    let t1 = Instant::now();
    assert_eq!(worker.join().unwrap().unwrap_err(), DevError::Cancelled);
    while alive(pid) && t1.elapsed() < Duration::from_millis(500) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !alive(pid),
        "the program on the other side died within 500 ms, not {:?}",
        t1.elapsed()
    );
    let line: serde_json::Value =
        serde_json::from_str(home.audit().lines().last().unwrap()).unwrap();
    assert_eq!(line["status"], "cancelled");
    let t0 = Instant::now();
    while serving.audit().is_empty() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let line: serde_json::Value =
        serde_json::from_str(serving.audit().lines().last().unwrap()).unwrap();
    assert_eq!(line["status"], "cancelled", "{line}");
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks that the pid exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn the_texts_of_a_remote_table_and_the_gate_never_takes_allow_for_ssh() {
    let def = |target: &str, extra: &str| {
        DevicesCfg::parse(&format!(
            "[devices.mac-build]\nrun = \"remote\"\ntarget = \"{target}\"\ndevice = \"build\"\nenabled = true\n{extra}"
        ))
        .unwrap()
        .user
        .remove(0)
    };
    let ssh = def("ssh://mac", "");
    let i = info(&ssh);
    assert_eq!(
        i.request,
        "Connects to mac with your ssh as you. bytes for the device `build` of the paddock at ssh://mac (at most 1048576 bytes), which runs it there in its own device sandbox and answers as that device does. The whole round trip takes at most 30 seconds."
    );
    assert!(i
        .response
        .starts_with("the bytes that the device `build` answers on the other side"));
    assert_eq!(i.shape, "exec");
    assert_eq!(
        summary(&ssh),
        "allow the device `mac-build` for this sandbox: it connects to mac with your ssh as you"
    );
    assert_eq!(
        doctor_line(&ssh).unwrap(),
        "mac-build: each request runs `ssh mac` as you and the paddock there runs its device build"
    );
    let sock = def("unix:/run/u/devices.sock", "mode = \"allow\"\n");
    assert!(!info(&sock).request.starts_with("Connects"));
    assert_eq!(
        summary(&sock),
        "allow the device `mac-build` for this sandbox"
    );
    assert_eq!(
        doctor_line(&sock).unwrap(),
        "mac-build: each request connects to /run/u/devices.sock and the paddock there runs its device build"
    );
    assert_eq!(
        super::super::user::info(&ssh).request,
        info(&ssh).request,
        "the shared info picks the remote text"
    );
    assert_eq!(super::super::user::describe(&ssh), describe(&ssh));
    let d = super::super::user::details(&ssh);
    assert_eq!(d["run"], "remote");
    assert_eq!(d["target"], "ssh://mac");
    assert_eq!(d["device"], "build");
    assert!(d["command"].is_null());
    assert!(d["platform"].is_null());
    // `paddock device show` prints the target.
    assert!(ssh.to_toml().contains("target = \"ssh://mac\""));
    assert!(!ssh.to_toml().contains("command"));
    // The doctor lines group remote devices under their heading.
    let cfg = DevicesCfg::parse(&format!(
        "{}\n{}",
        ssh.to_toml(),
        sock.to_toml().replace("mac-build", "sock-build")
    ))
    .unwrap();
    let lines = super::super::doctor::remote_lines(&cfg);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].contains("devices of another paddock"));
    assert!(lines[1].contains("mac-build: each request runs `ssh mac`"));
    // The Target type.
    assert_eq!(
        Target::parse("ssh://mac").unwrap(),
        Target::Ssh("mac".into())
    );
    assert_eq!(
        Target::parse("unix:/a/b.sock").unwrap(),
        Target::Unix("/a/b.sock".into())
    );
    assert!(Target::parse("ssh://").is_err());
    assert!(Target::parse("ssh://a b").is_err());
    assert!(Target::parse("tcp://x")
        .unwrap_err()
        .contains("must be ssh://HOST or unix:/PATH"));
    assert!(Target::Ssh("mac".into()).host_action());
    assert!(!Target::Unix("/x".into()).host_action());
    // The gate: ssh is a host action, so the card shows every session
    // and the grant text says what allowed it.
    let t = tempfile::tempdir().unwrap();
    let home = Home::new(t.path(), "h", "", Answer::Session);
    let g = Gate::new(home.ctx(), &ssh, GrantHit::NoKey);
    assert_eq!(g.check(&call()).unwrap(), "allowed for the session");
    assert_eq!(home.cards().len(), 1);
    let g = Gate::new(home.ctx(), &sock, GrantHit::NoKey);
    assert_eq!(
        g.check(&call()).unwrap(),
        "allowed by grant all (no grant key)",
        "a socket target takes mode = \"allow\""
    );
    assert_eq!(home.cards().len(), 1);
}

#[test]
fn remote_caller_text_is_cleaned_and_cut_and_the_errno_mapping_is_complete() {
    let r = RemoteCaller::new(
        "box\u{202E}1\n",
        &"h".repeat(300),
        &["a".into(), "b\x1b[2J".into()],
    );
    assert_eq!(r.sandbox, "box1");
    assert_eq!(r.host.len(), 128);
    assert_eq!(r.chain, ["a", "b[2J"]);
    assert_eq!(
        RemoteCaller::new("box1", "lin", &[]).text(),
        "sandbox box1 on lin"
    );
    assert_eq!(RemoteCaller::new("", "", &[]).text(), "an unnamed sandbox");
    assert_eq!(RemoteCaller::new("", "lin", &[]).text(), "a sandbox on lin");
    let req = Request::call("x", 1).with_caller(&r);
    let back: Request = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
    assert_eq!(back.remote_caller().unwrap(), r);
    assert!(Request::call("x", 1).remote_caller().is_none());
    for e in [
        DevError::Invalid,
        DevError::Failed,
        DevError::TooLarge,
        DevError::Cancelled,
        DevError::Waiting,
        DevError::Denied,
        DevError::BudgetUsed,
        DevError::Busy,
        DevError::RequestTooLarge,
        DevError::TimedOut,
    ] {
        assert_eq!(DevError::from_errno(e.errno()), e);
    }
    assert_eq!(DevError::from_errno(libc::ENOENT), DevError::Failed);
}

#[test]
fn session_answers_of_remote_callers_hold_for_eight_hours_in_a_file() {
    let t = tempfile::tempdir().unwrap();
    let home = Home::new(t.path(), "h", "", Answer::Session);
    let env = &home.env;
    assert_eq!(session::load(env, "10.0.0.7", "upper"), None);
    session::save(env, "10.0.0.7", "upper", true);
    session::save(env, "10.0.0.7", "build", false);
    assert_eq!(session::load(env, "10.0.0.7", "upper"), Some(true));
    assert_eq!(session::load(env, "10.0.0.7", "build"), Some(false));
    assert_eq!(session::load(env, "10.0.0.8", "upper"), None);
    assert_eq!(session::load(env, "local", "upper"), None);
    let p = session::path(env);
    assert!(p.ends_with("paddock/devices/remote-sessions.json"));
    let text = std::fs::read_to_string(&p).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let until = v["10.0.0.7\tupper"]["until"].as_u64().unwrap();
    assert!(until > crate::state::now() + 8 * 3600 - 5);
    // An old entry is gone.
    let mut v = v;
    v["10.0.0.7\tupper"]["until"] = (crate::state::now() - 1).into();
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(session::load(env, "10.0.0.7", "upper"), None);
    assert_eq!(session::load(env, "10.0.0.7", "build"), Some(false));
    session::forget(env).unwrap();
    assert!(!p.exists());
    session::forget(env).unwrap();
    // The gate of a remote grant reads and writes it; a local grant
    // never does.
    let def = DevicesCfg::parse(&upper_config("grant = [\"remote:any\"]\n", ""))
        .unwrap()
        .user
        .remove(0);
    let mut ctx = home.ctx();
    ctx.sandbox = "remote:10.0.0.7".into();
    let g = Gate::new(ctx.clone(), &def, GrantHit::Remote("10.0.0.7".into()));
    let mut c = call();
    c.remote = Some(RemoteCaller::new("box1", "lin", &[]));
    assert_eq!(g.check(&c).unwrap(), "allowed for the session");
    assert_eq!(session::load(env, "10.0.0.7", "upper"), Some(true));
    let g = Gate::new(ctx, &def, GrantHit::Remote("10.0.0.7".into()));
    assert_eq!(g.check(&c).unwrap(), "allowed for the session");
    assert_eq!(home.cards().len(), 1, "the file answered the second gate");
    let g = Gate::new(home.ctx(), &def, GrantHit::All);
    assert_eq!(g.check(&call()).unwrap(), "allowed for the session");
    assert_eq!(home.cards().len(), 2);
    assert!(!std::fs::read_to_string(&p).unwrap().contains("box1\t"));
}

#[test]
fn check_reaches_the_other_side_and_prints_its_entry() {
    let t = tempfile::tempdir().unwrap();
    let sock = sock_path(t.path());
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let serving = Home::new(
        t.path(),
        "serving",
        &upper_config("grant = [\"remote:any\"]\n", ""),
        Answer::Session,
    );
    let (_guard, _inst) = serving.serve(&sock);
    let forwarding = Home::new(
        t.path(),
        "forwarding",
        &format!(
            "{}\n[devices.gone]\nrun = \"remote\"\ntarget = \"unix:{}\"\ndevice = \"nothere\"\nenabled = true\n",
            forward_config(&format!("unix:{}", sock.display()), ""),
            sock.display()
        ),
        Answer::Session,
    );
    let cfg = DevicesCfg::load(&forwarding.env).unwrap();
    let table = |n: &str| cfg.user.iter().find(|d| d.name == n).unwrap();
    let out = probe(&forwarding.env, table("mac-upper")).unwrap();
    assert!(
        out.starts_with(&format!("mac-upper: reached unix:{} in ", sock.display())),
        "{out}"
    );
    // The platform is the one of the serving side, which is this machine.
    let there = format!(
        "device    upper there: state on, shape exec, run sandbox, platform {}, stream false\n",
        std::env::consts::OS
    );
    assert!(out.contains(&there), "{out}");
    assert!(
        out.contains("request   bytes for the standard input of the command `tr a-z A-Z`"),
        "{out}"
    );
    let e = format!("{:#}", probe(&forwarding.env, table("gone")).unwrap_err());
    assert!(
        e.contains("the paddock there has no device nothere. Its devices: upper"),
        "{e}"
    );
    let e = format!(
        "{:#}",
        probe(
            &forwarding.env,
            &DevicesCfg::parse(
                "[devices.x]\nrun = \"remote\"\ntarget = \"unix:/nowhere.sock\"\ndevice = \"u\"\n"
            )
            .unwrap()
            .user[0]
        )
        .unwrap_err()
    );
    assert!(e.starts_with("cannot connect to /nowhere.sock"), "{e}");
    assert!(serving.cards().is_empty(), "a catalog read shows no card");
}
