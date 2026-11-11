//! The device sandbox without bwrap: the spec, the refusals, and the
//! lifetime of an instance through `Launcher::Thread`, where the supervisor
//! runs in a thread of this process. tests/devices_sandbox.rs runs the
//! real bwrap sandbox.

use super::creds::{secret_var, CredPlan};
use super::spec::{self as sspec, InstanceCtx, INSTANCE_ORIGIN, SOCKET_IN};
use super::state::{self, InstanceState};
use super::*;
use crate::broker::Answer;
use crate::devices::grant::GrantHit;
use crate::devices::supervise::tests::script;
use crate::devices::user::tests::ctx;
use crate::devices::{Call, DevError, Device, DevicesCfg};
use crate::spec::MountKind;
use std::path::Path;

fn def(text: &str) -> DeviceDef {
    DevicesCfg::parse(text).unwrap().user.remove(0)
}

fn ictx(dir: &Path) -> InstanceCtx {
    InstanceCtx {
        caller: "box1".into(),
        project: dir.join("project"),
        harness: "fake".into(),
        chain: vec!["root1".into()],
        dir: dir.join("run/paddock/devices/tool/box1"),
    }
}

fn call() -> Call {
    Call::for_test("box1", Duration::from_secs(60))
}

#[test]
fn the_spec_has_the_mounts_the_env_and_the_supervisor_flags() {
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let env = &c.env;
    std::fs::create_dir_all(t.path().join("project")).unwrap();
    std::fs::create_dir_all(t.path().join("data")).unwrap();
    std::fs::write(t.path().join("known_hosts"), "x").unwrap();
    let d = def(&format!(
        "[devices.tool]\nshape = \"exec\"\nenabled = true\ncommand = [\"/opt/tool\", \"--fast\"]\ntimeout = 7\nmax_request = \"4K\"\nmax_response = \"16M\"\nconcurrency = 2\nready = 3\n[devices.tool.sandbox]\nconnect = [\"2222:mac.local:22\"]\nro = [\"{kh}\"]\nrw = [\"{data}\"]\ncaller = [\"project:ro\"]\nenv = {{ GIT_SSH = \"ssh -p 2222\" }}\nenv_pass = [\"TERM\", \"HOME\"]\ntmp = \"4G\"\n",
        kh = t.path().join("known_hosts").display(),
        data = t.path().join("data").display()
    ));
    let file_cred: crate::credentials::Credential =
        toml::from_str("via = \"deliver\"\nas = \"file\"\n").unwrap();
    let plan = CredPlan {
        agents: Vec::new(),
        files: vec![("gh-token".into(), file_cred)],
    };
    let ic = ictx(t.path());
    let s = sspec::spec(env, &d, &ic, &plan, Path::new("/usr/bin/paddock")).unwrap();
    assert_eq!(s.id, "dev-tool-box1");
    assert_eq!(s.harness, "device");
    assert_eq!(s.home, env.home, "~ in the program's files resolves");
    assert_eq!(s.home_src, ic.dir.join("home"));
    assert_eq!(s.workdir, s.home);
    assert_eq!(s.network, crate::spec::Network::None);
    let mount = |dst: &str| {
        s.mounts
            .iter()
            .find(|m| m.dst == Path::new(dst))
            .unwrap_or_else(|| panic!("no mount at {dst}: {:?}", s.mounts))
    };
    let kh = mount(&t.path().join("known_hosts").display().to_string());
    assert_eq!(kh.kind, MountKind::Ro);
    assert_eq!(kh.origin, "[devices.tool.sandbox] ro");
    assert_eq!(
        mount(&t.path().join("data").display().to_string()).kind,
        MountKind::Rw
    );
    let caller = mount("/run/paddock/caller/project");
    assert_eq!(caller.kind, MountKind::Ro);
    assert_eq!(
        caller.src.as_deref(),
        Some(t.path().join("project").as_path())
    );
    let inst = mount("/run/paddock/device");
    assert_eq!(
        (inst.kind, inst.origin.as_str()),
        (MountKind::Rw, INSTANCE_ORIGIN)
    );
    assert_eq!(inst.src.as_deref(), Some(ic.dir.as_path()));
    let con = mount("/run/paddock/connect");
    assert_eq!(con.kind, MountKind::Ro);
    let tmp = mount("/tmp");
    assert_eq!((tmp.kind, tmp.size), (MountKind::Tmpfs, Some(4 << 30)));
    for hidden in crate::execbox::ExecCfg::default().hide {
        assert_eq!(
            mount(&hidden.display().to_string()).kind,
            MountKind::MaskDir
        );
    }
    let tool = s.host_tools.iter().find(|t| t.name == "paddock").unwrap();
    assert_eq!(tool.src, Path::new("/usr/bin/paddock"));
    // The environment.
    assert_eq!(s.env["TERM"].value, "dumb");
    assert_eq!(s.env["PADDOCK_DEVICE"].value, "tool");
    assert_eq!(s.env["PADDOCK_DEVICE_PROTOCOL"].value, "1");
    assert_eq!(s.env["GIT_SSH"].value, "ssh -p 2222");
    assert_eq!(
        s.env["PADDOCK_SECRET_GH_TOKEN"].value,
        "/run/paddock/device/secret/gh-token"
    );
    assert!(
        !s.env.contains_key("HOME"),
        "HOME is set by the sandbox itself: {:?}",
        s.env.keys()
    );
    assert!(s.warnings.iter().any(|w| w.contains("env_pass: HOME")));
    // The command line of the supervisor.
    let cmd = s.command.join(" ");
    assert!(cmd.starts_with(&format!(
        "/run/paddock/bin/paddock device-serve --socket {SOCKET_IN} --shape exec --timeout 7 --max-request 4096 --max-response 16777216 --concurrency 2 --ready 3 --connect 2222:/run/paddock/connect/0.sock"
    )), "{cmd}");
    for flag in [
        "--env-pass GIT_SSH",
        "--env-pass TERM",
        "--env-pass PADDOCK_SECRET_GH_TOKEN",
    ] {
        assert!(cmd.contains(flag), "{flag} missing in {cmd}");
    }
    assert!(cmd.ends_with(" -- /opt/tool --fast"), "{cmd}");
    assert!(!cmd.contains("--stream"));
    assert_eq!(secret_var("gh-token"), "PADDOCK_SECRET_GH_TOKEN");
    assert_eq!(
        instance::scope_name("claude-app-1a2b3c", "tool"),
        "paddock-dev-claude-app-1a2b3c-tool.scope"
    );
    assert!(crate::freeze::ours(&instance::scope_name("x", "y")));
}

#[test]
fn the_whole_system_and_the_home_are_refused_and_network_none_has_no_net() {
    let e = DevicesCfg::parse(
        "[devices.t]\nshape = \"exec\"\nenabled = true\ncommand = [\"x\"]\n[devices.t.sandbox]\nrw = [\"/\"]\n",
    )
    .unwrap_err();
    assert!(
        format!("{e:#}").contains("rw: / would open the whole system or home"),
        "{e:#}"
    );
    // A 0.2.1 table with [exec] paths = ["/"] is refused at the spec.
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let cfg = t.path().join("cf/paddock");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut vars = c.env.vars.clone();
    vars.insert(
        "XDG_CONFIG_HOME".into(),
        t.path().join("cf").display().to_string(),
    );
    let env = crate::env::HostEnv::from_parts(c.env.home.clone(), "demo".into(), 1000, 1000, vars);
    std::fs::write(cfg.join("config.toml"), "[exec]\npaths = [\"/\"]\n").unwrap();
    let d = def("[devices.tool]\nkind = \"command\"\nenabled = true\ncommand = [\"x\"]\n");
    let e = sspec::resolved_profile(&env, &d).unwrap_err();
    assert!(
        format!("{e:#}").contains("would open the whole system or home"),
        "{e:#}"
    );
    // network = "none": no --share-net in the bwrap command line.
    let d = def("[devices.tool]\nshape = \"exec\"\nenabled = true\ncommand = [\"x\"]\n");
    let s = sspec::spec(
        &c.env,
        &d,
        &ictx(t.path()),
        &CredPlan::default(),
        Path::new("/usr/bin/paddock"),
    )
    .unwrap();
    assert_eq!(s.network, crate::spec::Network::None);
    if c.env.which("bwrap").is_some() {
        use crate::backend::Backend as _;
        let l = crate::backend::bwrap::Bwrap
            .render(&c.env, &s, false)
            .unwrap();
        assert!(l.args.contains(&"--unshare-all".to_string()));
        assert!(!l.args.contains(&"--share-net".to_string()), "{:?}", l.args);
        let i = l.args.iter().position(|a| a == "--size").unwrap();
        assert_eq!(
            &l.args[i..i + 4],
            &["--size", "1073741824", "--tmpfs", "/tmp"]
        );
        assert!(l.args.contains(&"--die-with-parent".to_string()));
    }
}

#[test]
fn credential_refusals_name_the_fix() {
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let cfg = t.path().join("cf/paddock");
    std::fs::create_dir_all(&cfg).unwrap();
    let mut vars = c.env.vars.clone();
    vars.insert(
        "XDG_CONFIG_HOME".into(),
        t.path().join("cf").display().to_string(),
    );
    let env = crate::env::HostEnv::from_parts(c.env.home.clone(), "demo".into(), 1000, 1000, vars);
    std::fs::write(
        cfg.join("config.toml"),
        "[credentials.var]\nvia = \"deliver\"\n[credentials.file]\nvia = \"deliver\"\nas = \"file\"\n[credentials.gw]\nvia = \"gateway\"\npreset = \"github\"\n[credentials.key]\nvia = \"agent\"\nkeys = [\"~/.ssh/id_test.pub\"]\n[credentials.dock]\nvia = \"agent\"\nkind = \"docker\"\nregistries = [\"ghcr.io\"]\n",
    )
    .unwrap();
    let refused = |name: &str| {
        format!(
            "{:#}",
            creds::plan(&env, "tool", &[name.to_string()]).unwrap_err()
        )
    };
    assert_eq!(
        refused("var"),
        "[devices.tool.sandbox] credentials: var: a device gets a secret as a file, not as a variable: as = \"file\""
    );
    assert!(
        refused("gw")
            .contains("a device sandbox has no gateway. Use via = \"deliver\" with as = \"file\""),
        "{}",
        refused("gw")
    );
    assert!(refused("dock").contains("a device sandbox gets an ssh agent only"));
    assert!(refused("nope").contains("nope is not a [credentials.nope] table"));
    let p = creds::plan(&env, "tool", &["file".into(), "key".into()]).unwrap();
    assert_eq!(p.files.len(), 1);
    assert_eq!(p.agents.len(), 1);
    assert_eq!(
        sspec::program_env_names(&Default::default(), &p),
        ["SSH_AUTH_SOCK", "PADDOCK_SECRET_FILE"]
    );
}

/// A device through the sandbox source, with the supervisor in a thread.
fn device(c: &crate::devices::DeviceCtx, text: &str) -> std::sync::Arc<dyn Device> {
    def(text).build(c, Ok(GrantHit::NoKey))
}

#[test]
fn the_first_request_starts_the_instance_and_a_stop_restarts_it_at_the_next_one() {
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let s = script(t.path(), "tool", "cat");
    let d = device(
        &c,
        &format!("[devices.tool]\nshape = \"exec\"\nenabled = true\ncommand = [\"{s}\"]\n"),
    );
    let file = state::state_file(&c.env, "tool", "box1");
    assert!(
        !file.exists(),
        "first-request starts nothing before a request"
    );
    assert_eq!(d.details().unwrap()["instance"], "stopped");
    assert_eq!(d.call(&call(), b"abc").unwrap(), b"status: exit 0\n\nabc");
    let st: InstanceState = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!((st.device.as_str(), st.caller.as_str()), ("tool", "box1"));
    assert_eq!(st.pid, std::process::id());
    assert!(st.alive());
    assert_eq!(st.run, "sandbox");
    assert!(st.socket.exists());
    assert_eq!(st.log, state::log_file(&c.env, "tool", "box1"));
    assert_eq!(d.details().unwrap()["instance"], "running");
    let listed = InstanceState::of_caller(&c.env, "box1");
    assert_eq!(listed, vec![st.clone()]);
    assert!(c.env.paddock_run().join("devices/tool/box1").is_dir());
    // paddock device stop NAME --sandbox ID
    let r = c.instances.stop("tool").unwrap();
    assert_eq!(r["stopped"], true);
    assert!(!file.exists());
    assert!(!st.dir.exists(), "the folder goes with the instance");
    assert!(!st.socket.exists());
    assert_eq!(c.instances.status()[0]["instance"], "stopped");
    // The next request starts a new one.
    assert_eq!(
        d.call(&call(), b"again").unwrap(),
        b"status: exit 0\n\nagain"
    );
    assert!(file.exists());
    assert!(c.instances.stop("nope").is_err());
    // The log has the supervisor's lines and no body.
    let log = std::fs::read_to_string(&st.log).unwrap();
    assert!(log.contains("start shape exec"), "{log}");
    assert!(!log.contains("again"), "{log}");
    // Dropping the registry stops everything.
    let inst = c.instances.clone();
    drop(d);
    drop(c);
    inst.stop_all();
    assert!(!file.exists());
}

#[test]
fn with_sandbox_starts_at_the_sandbox_start_and_a_failure_is_a_notice() {
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let s = script(t.path(), "tool", "cat");
    let _d = device(
        &c,
        &format!("[devices.tool]\nshape = \"exec\"\nenabled = true\nstart = \"with-sandbox\"\ncommand = [\"{s}\"]\n"),
    );
    let _bad = device(
        &c,
        &format!("[devices.bad]\nshape = \"serve\"\nenabled = true\nstart = \"with-sandbox\"\nready = 1\ncommand = [\"{s}\"]\n"),
    );
    assert!(!state::state_file(&c.env, "tool", "box1").exists());
    let notices = c.instances.start_with_sandbox();
    assert!(state::state_file(&c.env, "tool", "box1").exists());
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with("the device bad did not start: the device bad did not start:")
            && notices[0].contains("see paddock device logs bad --sandbox box1"),
        "{}",
        notices[0]
    );
    assert!(!state::state_file(&c.env, "bad", "box1").exists());
}

#[test]
fn clear_stale_removes_a_dead_instance_and_its_folder() {
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let dir = state::instance_dir(&c.env, "tool", "gone1");
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let dead = InstanceState {
        device: "tool".into(),
        caller: "gone1".into(),
        run: "sandbox".into(),
        shape: "exec".into(),
        pid: 4_194_000,
        started: 1,
        socket: dir.join("dev.sock"),
        scope: None,
        backend: "thread".into(),
        log: state::log_file(&c.env, "tool", "gone1"),
        dir: dir.clone(),
        unit: None,
        owner: None,
        users: None,
    };
    dead.write(&c.env).unwrap();
    let live = InstanceState {
        caller: "live1".into(),
        pid: std::process::id(),
        ..dead.clone()
    };
    live.write(&c.env).unwrap();
    assert!(!dead.alive() && live.alive());
    assert_eq!(state::clear_stale(&c.env), ["tool/gone1"]);
    assert!(!dir.exists());
    assert!(!state::state_file(&c.env, "tool", "gone1").exists());
    assert!(state::state_file(&c.env, "tool", "live1").exists());
    assert_eq!(InstanceState::list(&c.env).len(), 1);
    // The file is written whole: a reader sees the old one or the new one.
    let f = state::state_file(&c.env, "tool", "live1");
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (f2, s2) = (f.clone(), stop.clone());
    let reader = std::thread::spawn(move || {
        let mut n = 0;
        while !s2.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok(text) = std::fs::read_to_string(&f2) {
                serde_json::from_str::<InstanceState>(&text).expect("a whole file");
                n += 1;
            }
        }
        n
    });
    for i in 0..200u64 {
        InstanceState {
            started: i,
            ..live.clone()
        }
        .write(&c.env)
        .unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(reader.join().unwrap() > 0);
}

#[test]
fn three_deaths_make_the_device_failed_with_how_to_fix_and_restart_clears_it() {
    let t = tempfile::tempdir().unwrap();
    let (c, _) = ctx(t.path(), Answer::Session);
    let s = script(t.path(), "tool", "cat");
    let d = device(
        &c,
        &format!("[devices.tool]\nshape = \"exec\"\nenabled = true\ncommand = [\"{s}\"]\n"),
    );
    let src = c.instances.source(
        &def(&format!(
            "[devices.tool]\nshape = \"exec\"\nenabled = true\ncommand = [\"{s}\"]\n"
        )),
        &c,
    );
    assert_eq!(d.call(&call(), b"1").unwrap(), b"status: exit 0\n\n1");
    let sock = state::InstanceState::list(&c.env)
        .into_iter()
        .find(|instance| instance.device == "tool" && instance.caller == "box1")
        .unwrap()
        .socket;
    // The supervisor dies (the socket goes): the next request restarts it
    // after the backoff, twice; the third death is `failed`.
    let t0 = Instant::now();
    for n in 1..=2 {
        std::fs::remove_file(&sock).unwrap();
        // The request after a death fails once (the old connection), then
        // the next one restarts the instance.
        let first = d.call(&call(), b"x");
        let out = match first {
            Ok(o) => o,
            Err(_) => d.call(&call(), b"x").unwrap(),
        };
        assert_eq!(out, b"status: exit 0\n\nx", "restart {n}");
        assert!(src.pid().is_some());
    }
    assert!(
        t0.elapsed() >= Duration::from_secs(3),
        "backoff 1 s then 2 s"
    );
    std::fs::remove_file(&sock).unwrap();
    let _ = d.call(&call(), b"x");
    assert_eq!(d.call(&call(), b"x").unwrap_err(), DevError::Failed);
    let details = d.details().unwrap();
    assert_eq!(details["instance"], "failed");
    let fix = details["how_to_fix"].as_str().unwrap();
    assert_eq!(
        fix,
        "the device program of tool is failed: the device sandbox exited 3 times within a minute. Read the log: paddock device logs tool --sandbox box1. Then: paddock device restart tool --sandbox box1"
    );
    assert_eq!(c.instances.status()[0]["instance"], "failed");
    // paddock device restart NAME --sandbox ID
    let r = c.instances.restart("tool").unwrap();
    assert!(r["started_ms"].is_u64());
    assert_eq!(d.call(&call(), b"back").unwrap(), b"status: exit 0\n\nback");
    assert_eq!(d.details().unwrap()["instance"], "running");
}
