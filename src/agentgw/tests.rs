use super::shape::Values;
use super::*;
use crate::env::HostEnv;
use crate::profile::{Harness, HarnessFactory};
use crate::spec::Network;
use std::path::{Path, PathBuf};

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// A host in a temporary folder, with the PATH of this process (bwrap,
/// pasta and ip, when the host has them).
fn host(t: &Path) -> HostEnv {
    let mut vars: std::collections::BTreeMap<String, String> = [
        ("XDG_CONFIG_HOME", t.join("cfg")),
        ("XDG_DATA_HOME", t.join("data")),
        ("XDG_STATE_HOME", t.join("state")),
        ("XDG_CACHE_HOME", t.join("cache")),
        ("XDG_RUNTIME_DIR", t.join("run")),
    ]
    .into_iter()
    .map(|(k, p)| (k.to_string(), p.display().to_string()))
    .collect();
    vars.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    vars.insert("ANTHROPIC_API_KEY".into(), "sk-test".into());
    std::fs::create_dir_all(t.join("home")).unwrap();
    std::fs::create_dir_all(t.join("run")).unwrap();
    HostEnv::from_parts(t.join("home"), "demo".into(), 1000, 1000, vars)
}

fn openclaw(env: &HostEnv) -> crate::profile::Profile {
    HarnessFactory::new(env)
        .unwrap()
        .profile("openclaw")
        .unwrap()
        .clone()
}

/// The openclaw profile with a fake install under `t/opt`.
fn harness(env: &HostEnv, t: &Path) -> Harness {
    let dir = t.join("opt/openclaw/bin");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("openclaw");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    Harness {
        profile: openclaw(env),
        exe,
        install_mounts: vec![t.join("opt/openclaw")],
        path_dirs: Vec::new(),
        source: "test".into(),
    }
}

#[test]
fn the_openclaw_profile_has_a_gateway_shape_that_checks() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let p = openclaw(&env);
    let s = shape_of(&p).unwrap();
    let vals = Values {
        port: 19101,
        token: "t0k",
    };
    let args = s.gateway_args(&vals);
    // The gateway binds to the loopback, with token auth, on paddock's port.
    let at = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .map(|i| args[i + 1].as_str())
    };
    assert_eq!(at("--bind"), Some("loopback"), "{args:?}");
    assert_eq!(at("--auth"), Some("token"), "{args:?}");
    assert_eq!(at("--port"), Some("19101"), "{args:?}");
    assert!(
        !args.iter().any(|a| a.contains("t0k")),
        "the token is not an argument: {args:?}"
    );
    assert_eq!(s.gateway_env(&vals)["OPENCLAW_GATEWAY_TOKEN"], "t0k");
    // paddock never runs the installer of the host service.
    assert!(!args.iter().any(|a| a == "daemon"), "{args:?}");
    assert_eq!(s.url(&vals), "ws://127.0.0.1:19101");
    assert_eq!(
        s.client_args(&vals, "tui", &v(&["--session", "main"])),
        v(&[
            "tui",
            "--url",
            "ws://127.0.0.1:19101",
            "--token",
            "t0k",
            "--session",
            "main"
        ])
    );
    assert_ne!(
        Some(s.port),
        s.host_port,
        "paddock's port differs from OpenClaw's own"
    );
    // `paddock openclaw` stays the embedded runtime.
    assert_eq!(p.launch.args, v(&["chat"]));
}

#[test]
fn a_shape_without_port_or_token_is_refused() {
    let base = |args: &str, env: &str| {
        toml::from_str::<Shape>(&format!(
            "args = [{args}]\nenv = {{ {env} }}\nport = 19101\n[client]\ncommands = [\"tui\"]\nargs = []\n"
        ))
        .unwrap()
    };
    let e = base("\"run\"", "T = \"{token}\"").check("x").unwrap_err();
    assert!(format!("{e:#}").contains("{port}"), "{e:#}");
    let e = base("\"run\", \"{port}\"", "").check("x").unwrap_err();
    assert!(format!("{e:#}").contains("{token}"), "{e:#}");
    base("\"run\", \"{port}\"", "T = \"{token}\"")
        .check("x")
        .unwrap();
    // Unknown keys are errors, as everywhere in a profile.
    assert!(toml::from_str::<Shape>(
        "args = []\nport = 1\nbind = \"lan\"\n[client]\ncommands = []\nargs = []\n"
    )
    .is_err());
}

#[test]
fn commands_are_clients_host_commands_or_ordinary_runs() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let p = openclaw(&env);
    let s = shape_of(&p).unwrap();
    assert_eq!(s.classify(&v(&["tui", "--x"])), Form::Client);
    assert_eq!(s.classify(&v(&["gateway", "run"])), Form::Host);
    assert_eq!(s.classify(&v(&["daemon", "install"])), Form::Host);
    assert_eq!(s.classify(&v(&["chat"])), Form::Other);
    assert_eq!(s.classify(&[]), Form::Other);
    let (f, cmd, extra) = form_of(&p, &s, "openclaw", &v(&["tui", "--deliver"]));
    assert_eq!(
        (f, cmd.as_str(), extra),
        (Form::Client, "tui", v(&["--deliver"]))
    );
    // Omarchy's launcher: `--tui` is the client, anything else is for the host.
    let (f, cmd, extra) = form_of(&p, &s, "omarchy-launch-openclaw", &v(&["--tui"]));
    assert_eq!(
        (f, cmd.as_str(), extra),
        (Form::Client, "tui", Vec::<String>::new())
    );
    let (f, _, _) = form_of(&p, &s, "omarchy-launch-openclaw", &[]);
    assert_eq!(f, Form::Host);
}

#[test]
fn the_gateway_sandbox_has_its_own_home_and_publishes_one_loopback_port() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let h = harness(&env, t.path());
    let s = shape_of(&h.profile).unwrap();
    let secrets = [("ANTHROPIC_API_KEY".to_string(), "sk-test".to_string())].into();
    let spec = spec::gateway(&env, &h, &s, Network::Egress, 19111, "tok", &secrets).unwrap();
    assert_eq!(spec.home_src, state::home(&env, "openclaw"));
    assert_ne!(spec.home_src, env.home, "never the host home");
    assert_eq!(spec.home, env.home, "the home is at the host path inside");
    assert_eq!(spec.publish, vec![19111]);
    assert!(spec.host_ports.is_empty(), "{:?}", spec.host_ports);
    assert_eq!(spec.network, Network::Egress);
    // No bind of the host home or of anything in it but the install.
    for m in &spec.mounts {
        if let Some(src) = &m.src {
            assert!(
                !src.starts_with(&env.home) || src.starts_with(t.path().join("opt")),
                "{m:?}"
            );
        }
    }
    assert!(!spec
        .mounts
        .iter()
        .any(|m| m.dst == env.home.join(".openclaw")));
    assert_eq!(spec.env["OPENCLAW_GATEWAY_TOKEN"].value, "tok");
    assert!(spec.env["OPENCLAW_GATEWAY_TOKEN"].secret);
    assert_eq!(spec.env["ANTHROPIC_API_KEY"].value, "sk-test");
    assert_eq!(spec.command[0], h.exe.display().to_string());
    assert!(spec.command.contains(&"19111".to_string()));
    // Mode host listens on the host: nothing to publish.
    let hostnet = spec::gateway(&env, &h, &s, Network::Host, 19111, "tok", &secrets).unwrap();
    assert!(hostnet.publish.is_empty());
}

#[test]
fn the_client_sandbox_reaches_only_the_gateway_port_and_not_its_home() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let h = harness(&env, t.path());
    let s = shape_of(&h.profile).unwrap();
    let spec = spec::client(&env, &h, &s, 19111, "tok", "tui", &v(&["-v"])).unwrap();
    assert_eq!(spec.home_src, state::client_home(&env, "openclaw"));
    let gw_home = state::home(&env, "openclaw");
    let gw_data = state::dir(&env, "openclaw");
    for m in &spec.mounts {
        if let Some(src) = &m.src {
            assert!(
                !src.starts_with(&gw_home) && !src.starts_with(&gw_data),
                "{m:?}"
            );
        }
    }
    assert!(!spec.home_src.starts_with(&gw_home));
    assert_eq!(
        spec.host_ports.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        vec![19111]
    );
    assert!(spec.publish.is_empty());
    assert!(
        !spec.env.contains_key("ANTHROPIC_API_KEY"),
        "the client needs no model key"
    );
    assert_eq!(
        spec.command[1..],
        v(&[
            "tui",
            "--url",
            "ws://127.0.0.1:19111",
            "--token",
            "tok",
            "-v"
        ])
    );
}

/// The bwrap command of both sandboxes, where the host has bwrap, pasta
/// and ip: the gateway's port goes to 127.0.0.1 of the host and nowhere
/// else, and the client reaches that one port of the host.
#[test]
fn bwrap_publishes_the_gateway_on_the_host_loopback_only() {
    use crate::backend::Backend as _;
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    if !cfg!(target_os = "linux")
        || ["bwrap", "pasta", "ip"]
            .iter()
            .any(|c| env.which(c).is_none())
    {
        eprintln!("skipped: needs bwrap, pasta and ip on PATH");
        return;
    }
    let h = harness(&env, t.path());
    let s = shape_of(&h.profile).unwrap();
    let gw = spec::gateway(
        &env,
        &h,
        &s,
        Network::Egress,
        19111,
        "tok",
        &Default::default(),
    )
    .unwrap();
    let l = crate::backend::bwrap::Bwrap
        .render(&env, &gw, false)
        .unwrap();
    let pair = |args: &[String], k: &str| {
        args.windows(2)
            .filter(|w| w[0] == k)
            .map(|w| w[1].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(pair(&l.args, "-t"), ["127.0.0.1/19111"], "{:?}", l.args);
    assert_eq!(pair(&l.args, "-T"), ["none"], "{:?}", l.args);
    let home = state::home(&env, "openclaw").display().to_string();
    let host_home = env.home.display().to_string();
    assert!(
        l.args
            .windows(3)
            .any(|w| w[0] == "--bind" && w[1] == home && w[2] == host_home),
        "the gateway home is the sandbox home: {:?}",
        l.args
    );
    assert!(
        !l.args
            .iter()
            .any(|a| a == "OPENCLAW_GATEWAY_TOKEN=tok" || a == "tok"),
        "the token is not an argument"
    );
    let c = spec::client(&env, &h, &s, 19111, "tok", "tui", &[]).unwrap();
    let l = crate::backend::bwrap::Bwrap.render(&env, &c, true).unwrap();
    assert_eq!(pair(&l.args, "-T"), ["19111"], "{:?}", l.args);
    assert_eq!(pair(&l.args, "-t"), ["none"], "{:?}", l.args);
    assert!(!l.args.iter().any(|a| a.starts_with(&home)), "{:?}", l.args);
}

#[test]
fn modes_without_a_listening_port_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    assert_eq!(spec::network(&env, None).unwrap(), Network::Egress);
    assert_eq!(
        spec::network(&env, Some(Network::Host)).unwrap(),
        Network::Host
    );
    for n in [Network::None, Network::Api, Network::Allowlist] {
        let e = spec::network(&env, Some(n)).unwrap_err();
        assert!(format!("{e:#}").contains("egress"), "{e:#}");
    }
}

#[test]
fn the_listener_table_names_every_address_on_the_port() {
    // 127.0.0.1:19111 and 0.0.0.0:19111 listen, 127.0.0.1:22 too, one
    // connection is established.
    let v4 = "  sl  local_address rem_address   st\n\
   0: 0100007F:4A67 00000000:0000 0A 0\n\
   1: 00000000:4A67 00000000:0000 0A 0\n\
   2: 0100007F:0016 00000000:0000 0A 0\n\
   3: 0100007F:4A67 0100007F:9C40 01 0\n";
    assert_eq!(
        launch::parse_listeners(v4, 19047, false),
        ["127.0.0.1", "0.0.0.0"]
    );
    let v6 = "  sl  local_address rem_address st\n\
   0: 00000000000000000000000001000000:4A67 00000000000000000000000000000000:0000 0A\n\
   1: 0000000000000000FFFF00000100007F:4A67 00000000000000000000000000000000:0000 0A\n\
   2: 00000000000000000000000000000000:4A67 00000000000000000000000000000000:0000 0A\n";
    let a = launch::parse_listeners(v6, 19047, true);
    assert_eq!(a, ["::1", "::ffff:127.0.0.1", "::"]);
    let loop_only: Vec<bool> = a.iter().map(|x| launch::is_loopback(x)).collect();
    assert_eq!(loop_only, [true, true, false]);
    assert!(!launch::is_loopback("0.0.0.0"));
    assert!(!launch::is_loopback("192.168.1.2"));
}

#[test]
fn the_units_run_the_paddock_supervisor_and_never_the_daemon() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let paddock = PathBuf::from("/opt/paddock/bin/paddock");
    let a = control::systemd_run_args(&env, "openclaw", &paddock);
    assert!(
        a.contains(&"--unit=paddock-gateway-openclaw".to_string()),
        "{a:?}"
    );
    assert!(
        a.contains(&"--property=Restart=on-abnormal".to_string()),
        "{a:?}"
    );
    let tail: Vec<&str> = a
        .iter()
        .skip_while(|x| *x != "--")
        .skip(1)
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        ["/opt/paddock/bin/paddock", "gateway-serve", "openclaw"]
    );
    assert!(
        a.iter().any(|x| x.starts_with("--setenv=XDG_STATE_HOME=")),
        "{a:?}"
    );
    assert!(
        !a.iter().any(|x| x.contains("sk-test")),
        "no secret in a unit: {a:?}"
    );
    let p = control::plist(&env, "openclaw", &paddock);
    assert!(p.starts_with(crate::units::PLIST_MARK_PREFIX), "{p}");
    assert!(
        p.contains("<string>com.harnessix.paddock.gateway.openclaw</string>"),
        "{p}"
    );
    assert!(p.contains("<string>gateway-serve</string>"), "{p}");
    assert!(p.contains("<key>Crashed</key>"), "{p}");
    assert!(!p.contains("daemon") && !p.contains("sk-test"), "{p}");
}

#[test]
fn a_port_in_use_is_skipped() {
    let busy = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let p = busy.local_addr().unwrap().port();
    let got = control::choose_port(Some(p), p).unwrap();
    assert_ne!(got, p);
    drop(busy);
}

#[test]
fn the_token_is_made_once_and_kept_private() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let a = state::token(&env, "openclaw").unwrap();
    assert_eq!(a.len(), 64);
    assert_eq!(state::token(&env, "openclaw").unwrap(), a);
    let f = state::dir(&env, "openclaw").join("token");
    use std::os::unix::fs::PermissionsExt as _;
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o077,
        0
    );
    assert_eq!(
        state::existing_token(&env, "openclaw").as_deref(),
        Some(a.as_str())
    );
}

#[test]
fn the_host_notice_says_how_to_move() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    let s = shape_of(&openclaw(&env)).unwrap();
    let n = attach::host_notice("openclaw", Some("its agent runs on the host"), &s, 19001);
    assert!(n.contains("127.0.0.1:19001"), "{n}");
    assert!(n.contains("paddock gateway openclaw start"), "{n}");
    assert!(n.starts_with("its agent runs on the host. "), "{n}");
}

#[test]
fn a_stopped_gateway_reports_its_last_port_and_no_pid() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    state::write_start(
        &env,
        "openclaw",
        &state::StartCfg {
            port: 19111,
            network: "egress".into(),
            backend: "auto".into(),
            how: "detached".into(),
            unit: None,
        },
    )
    .unwrap();
    let g = report::of(&env, "openclaw", None);
    assert_eq!(g.state, "stopped");
    assert_eq!(g.port, Some(19111));
    assert_eq!(g.pid, None);
    assert_eq!(g.clients, 0);
    let j = serde_json::to_value(&g).unwrap();
    for k in [
        "name", "state", "port", "url", "uptime", "clients", "restarts", "how",
    ] {
        assert!(j.get(k).is_some(), "{k} missing: {j}");
    }
    assert_eq!(report::all(&env).len(), 1);
    assert!(
        report::line(&g).starts_with("gateway openclaw"),
        "{}",
        report::line(&g)
    );
    // A profile without a [gateway] table gets no gateway.
    let mut p = openclaw(&env);
    p.gateway = None;
    assert!(shape_of(&p).is_err());
}

/// The test switch of the user manager holds on every system: on macOS a
/// LaunchAgent of a test would run in the real launchd domain, with one
/// label for all the tests at once (GATEWAY1-F2).
#[test]
fn the_test_switch_starts_the_supervisor_detached_on_every_system() {
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("PADDOCK_TEST_NO_USER_MANAGER".to_string(), "1".to_string());
    vars.insert(
        "DBUS_SESSION_BUS_ADDRESS".to_string(),
        "unix:path=/x".to_string(),
    );
    let env = HostEnv::from_parts("/home/demo".into(), "demo".into(), 1000, 1000, vars);
    assert_eq!(super::control::how(&env), "detached");
    let bare = HostEnv::from_parts(
        "/home/demo".into(),
        "demo".into(),
        1000,
        1000,
        Default::default(),
    );
    let want = if cfg!(target_os = "macos") {
        "launch agent"
    } else {
        "detached"
    };
    assert_eq!(super::control::how(&bare), want);
}
