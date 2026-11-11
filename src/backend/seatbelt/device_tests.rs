//! The Seatbelt profile, the environment and the launch of a device
//! sandbox: plain code, every platform. The paths of the table are fake
//! (`/Users/u/...`): `real` keeps a path that does not exist. Only the
//! configuration folder is real, for `[exec]` and `[credentials]`.

use super::*;
use crate::devices::sandbox::creds;
use crate::devices::sandbox::instance::Launcher;
use crate::devices::DevicesCfg;

const CREDENTIALS: &str = r#"
[credentials.k]
via = "agent"
keys = ["/Users/u/.ssh/id_test.pub"]

[credentials.tok]
via = "deliver"
as = "file"
var = "TEST_TOKEN"
"#;

const FULL: &str = r#"
[devices.d]
shape = "exec"
command = ["/Users/u/dev/d.sh"]
[devices.d.sandbox]
ro = ["/Users/u/dev"]
rw = ["/Users/u/out"]
caller = ["project:ro"]
connect = ["2222:mac.example:22"]
credentials = ["k", "tok"]
"#;

const PLAIN: &str = r#"
[devices.d]
shape = "exec"
command = ["/Users/u/dev/d.sh"]
[devices.d.sandbox]
ro = ["/Users/u/dev"]
"#;

struct World {
    _t: tempfile::TempDir,
    env: HostEnv,
    ctx: InstanceCtx,
    def: DeviceDef,
    plan: CredPlan,
}

/// `config` as the user configuration, with the credentials above.
fn world(config: &str) -> World {
    let t = tempfile::tempdir().unwrap();
    let cfg = t.path().join("cfg");
    std::fs::create_dir_all(cfg.join("paddock")).unwrap();
    std::fs::write(
        cfg.join("paddock/config.toml"),
        format!("{CREDENTIALS}\n{config}"),
    )
    .unwrap();
    let mut vars = BTreeMap::new();
    vars.insert("XDG_CONFIG_HOME".to_string(), cfg.display().to_string());
    vars.insert(
        "XDG_RUNTIME_DIR".to_string(),
        std::fs::canonicalize(t.path())
            .unwrap()
            .join("run")
            .display()
            .to_string(),
    );
    vars.insert(
        "HTTPS_PROXY".to_string(),
        "http://127.0.0.1:3128".to_string(),
    );
    let env = HostEnv::from_parts("/Users/u".into(), "u".into(), 501, 20, vars);
    let def = DevicesCfg::parse(config).unwrap().user.remove(0);
    let ctx = InstanceCtx {
        caller: "fake-abc123".into(),
        project: "/Users/u/src/app".into(),
        harness: "fake".into(),
        chain: Vec::new(),
        dir: crate::devices::sandbox::state::instance_dir(&env, &def.name, "fake-abc123"),
    };
    let plan = creds::plan(&env, &def.name, &def.profile().credentials).unwrap();
    World {
        _t: t,
        env,
        ctx,
        def,
        plan,
    }
}

const PADDOCK: &str = "/usr/local/bin/paddock";

fn sockets(w: &World) -> Sockets {
    Sockets::of(&w.env, &w.def, &w.ctx).unwrap()
}

fn profile(w: &World) -> String {
    let p = policy(
        &w.env,
        &w.def,
        &w.ctx,
        &w.plan,
        &sockets(w),
        Path::new(PADDOCK),
    )
    .unwrap();
    crate::seatbelt::render(&p).unwrap()
}

/// The rules after the network section: the path rules, then the last
/// rule of the profile.
fn path_rules(sb: &str) -> Vec<String> {
    let at = sb.find("; network:").unwrap();
    sb[at..]
        .lines()
        .skip(1)
        .filter(|l| l.starts_with("(allow") || l.starts_with("(deny"))
        .filter(|l| !l.contains("(local ip") && !l.contains("(remote ip"))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_instance_has_two_short_socket_folders() {
    let w = world(FULL);
    let s = sockets(&w);
    assert_eq!(host_id("dev-d-abc123"), "dev-d-abc123-host");
    assert_eq!(s.supervisor.file_name().unwrap(), "dev.sock");
    assert!(
        s.supervisor.parent().unwrap().ends_with("dev-d-abc123"),
        "{}",
        s.supervisor.display()
    );
    assert!(
        s.host.ends_with("dev-d-abc123-host"),
        "{}",
        s.host.display()
    );
    assert_eq!(s.agent(), s.host.join("ssh-agent.sock"));
    assert_eq!(s.connect(), s.host.join("connect"));
    assert_ne!(s.supervisor.parent().unwrap(), s.host);
    crate::sockpath::check(&s.supervisor).unwrap();
    crate::sockpath::check(&s.agent()).unwrap();
    crate::sockpath::check(&s.connect().join("0.sock")).unwrap();
}

#[test]
fn the_profile_has_the_exact_rules_in_depth_order() {
    let w = world(FULL);
    let s = sockets(&w);
    let sb = profile(&w);
    let dir = w.ctx.dir.display().to_string();
    let sock_dir = real(s.supervisor.parent().unwrap()).display().to_string();
    let host = real(&s.host).display().to_string();
    // In the order of insertion; the profile sorts them by depth, stable.
    let rules: Vec<(&str, Vec<String>)> = vec![
        (
            "home",
            vec![
                format!("(allow file-read* file-write* (subpath \"{dir}/home\"))"),
                format!("(allow network-bind (local unix-socket (subpath \"{dir}/home\")))"),
                format!("(allow network-outbound (remote unix-socket (subpath \"{dir}/home\")))"),
            ],
        ),
        (
            "tmp",
            vec![
                format!("(allow file-read* file-write* (subpath \"{dir}/tmp\"))"),
                format!("(allow network-bind (local unix-socket (subpath \"{dir}/tmp\")))"),
                format!("(allow network-outbound (remote unix-socket (subpath \"{dir}/tmp\")))"),
            ],
        ),
        (
            "sock",
            vec![
                format!("(allow file-read* file-write* (subpath \"{sock_dir}\"))"),
                format!("(allow network-bind (local unix-socket (subpath \"{sock_dir}\")))"),
                format!("(allow network-outbound (remote unix-socket (subpath \"{sock_dir}\")))"),
            ],
        ),
        (
            PADDOCK,
            vec![
                format!("(allow file-read* (literal \"{PADDOCK}\"))"),
                format!("(deny file-write* (literal \"{PADDOCK}\"))"),
            ],
        ),
        (
            "/Users/u/dev",
            vec![
                "(allow file-read* (subpath \"/Users/u/dev\"))".into(),
                "(deny file-write* (subpath \"/Users/u/dev\"))".into(),
            ],
        ),
        (
            "/Users/u/out",
            vec![
                "(allow file-read* file-write* (subpath \"/Users/u/out\"))".into(),
                "(allow network-bind (local unix-socket (subpath \"/Users/u/out\")))".into(),
                "(allow network-outbound (remote unix-socket (subpath \"/Users/u/out\")))".into(),
            ],
        ),
        (
            "/Users/u/src/app",
            vec![
                "(allow file-read* (subpath \"/Users/u/src/app\"))".into(),
                "(deny file-write* (subpath \"/Users/u/src/app\"))".into(),
            ],
        ),
        (
            "/Users/u",
            vec!["(deny file-read* file-write* (subpath \"/Users/u\"))".into()],
        ),
        (
            "connect",
            vec![format!(
                "(allow network-outbound (remote unix-socket (subpath \"{host}/connect\")))"
            )],
        ),
        (
            "agent",
            vec![format!(
                "(allow network-outbound (remote unix-socket (path-literal \"{host}/ssh-agent.sock\")))"
            )],
        ),
        (
            "secret",
            vec![
                format!("(allow file-read* (subpath \"{dir}/secret\"))"),
                format!("(deny file-write* (subpath \"{dir}/secret\"))"),
            ],
        ),
    ];
    let depth = |rule: &str| {
        let q = rule.find('"').unwrap();
        let path = &rule[q + 1..rule[q + 1..].find('"').unwrap() + q + 1];
        Path::new(path).components().count()
    };
    let mut sorted: Vec<&(&str, Vec<String>)> = rules.iter().collect();
    sorted.sort_by_key(|(_, v)| depth(&v[0]));
    let mut want: Vec<String> = sorted.iter().flat_map(|(_, v)| v.clone()).collect();
    want.push(crate::seatbelt::LAST.0.to_string());
    assert_eq!(path_rules(&sb), want, "{sb}");
    // The real home is denied before the private home under it is allowed.
    let deny = sb
        .find("(deny file-read* file-write* (subpath \"/Users/u\"))")
        .unwrap();
    let home = sb.find(&format!("(subpath \"{dir}/home\")")).unwrap();
    assert!(deny < home, "{sb}");
    // Nothing allows the instance folder itself or the secret folder for
    // writing: the sandbox cannot replace a FIFO.
    assert!(
        !sb.contains(&format!("file-write* (subpath \"{dir}\")")),
        "{sb}"
    );
    assert!(
        !sb.contains(&format!(
            "(subpath \"{}\")",
            w.env.runtime_dir.as_ref().unwrap().display()
        )),
        "{sb}"
    );
}

#[test]
fn the_network_is_localhost_only_with_connect() {
    let w = world(FULL);
    let sb = profile(&w);
    assert!(
        sb.contains("; network: connect only to these localhost ports"),
        "{sb}"
    );
    assert!(
        sb.contains("(allow network-outbound (remote ip \"localhost:2222\"))"),
        "{sb}"
    );
    assert!(
        sb.contains("(allow network-bind (local ip \"localhost:*\"))"),
        "{sb}"
    );
    assert!(
        sb.contains("(allow network-inbound (local ip \"localhost:*\"))"),
        "{sb}"
    );
    let ip: Vec<&str> = sb
        .lines()
        .filter(|l| l.contains("(remote ip") || l.contains("(local ip"))
        .collect();
    assert_eq!(ip.len(), 3, "{ip:?}");
    assert!(!sb.contains("\"*:*\""), "{sb}");
    assert!(!sb.contains("mDNSResponder"), "{sb}");

    let w = world(PLAIN);
    let sb = profile(&w);
    assert!(sb.contains("; network: none"), "{sb}");
    assert!(!sb.contains("(remote ip"), "{sb}");
    assert!(!sb.contains("(local ip"), "{sb}");
    assert!(!sb.contains("ssh-agent.sock"), "{sb}");
    assert!(!sb.contains("/connect\""), "{sb}");
    assert!(!sb.contains("/secret\""), "{sb}");
    assert!(!sb.contains("/Users/u/src/app"), "{sb}");
    assert!(
        sb.contains("(deny file-read* file-write* (subpath \"/Users/u\"))"),
        "{sb}"
    );
}

#[test]
fn home_read_only_and_full_replace_the_deny() {
    let ro = world(&PLAIN.replace("ro = [\"/Users/u/dev\"]", "home = \"read-only\""));
    let sb = profile(&ro);
    assert!(
        sb.contains("(allow file-read* (subpath \"/Users/u\"))"),
        "{sb}"
    );
    assert!(
        sb.contains("(deny file-write* (subpath \"/Users/u\"))"),
        "{sb}"
    );
    assert!(
        !sb.contains("(deny file-read* file-write* (subpath \"/Users/u\"))"),
        "{sb}"
    );
    let full = world(&PLAIN.replace("ro = [\"/Users/u/dev\"]", "home = \"full\""));
    let sb = profile(&full);
    assert!(
        sb.contains("(allow file-read* file-write* (subpath \"/Users/u\"))"),
        "{sb}"
    );
}

#[test]
fn a_file_in_ro_is_a_literal_rule() {
    let t = tempfile::tempdir().unwrap();
    let note = t.path().join("note.txt");
    std::fs::write(&note, "x").unwrap();
    let w = world(&PLAIN.replace(
        "ro = [\"/Users/u/dev\"]",
        &format!("ro = [\"{}\"]", note.display()),
    ));
    let sb = profile(&w);
    let note = real(&note).display().to_string();
    assert!(
        sb.contains(&format!("(allow file-read* (literal \"{note}\"))")),
        "{sb}"
    );
    assert!(
        sb.contains(&format!("(deny file-write* (literal \"{note}\"))")),
        "{sb}"
    );
}

#[test]
fn the_environment_has_host_paths_and_no_linux_or_proxy_variables() {
    let w = world(FULL);
    let s = sockets(&w);
    let spec = sspec::spec(&w.env, &w.def, &w.ctx, &w.plan, Path::new(PADDOCK)).unwrap();
    let e = run_env(&w.env, &spec, &w.ctx, &w.plan, &s);
    let dir = w.ctx.dir.display().to_string();
    assert_eq!(e["HOME"], format!("{dir}/home"));
    assert_eq!(e["CFFIXED_USER_HOME"], format!("{dir}/home"));
    assert_eq!(e["TMPDIR"], format!("{dir}/tmp"));
    assert_eq!(e["PATH"], PATH);
    assert_eq!(e["SSH_AUTH_SOCK"], s.agent().display().to_string());
    assert_eq!(e["PADDOCK_SECRET_TOK"], format!("{dir}/secret/tok"));
    assert_eq!(e["PADDOCK_DEVICE"], "d");
    assert_eq!(e["TERM"], "dumb");
    assert_eq!(e["PADDOCK"], "1");
    for k in [
        "XDG_RUNTIME_DIR",
        "HTTPS_PROXY",
        "https_proxy",
        "NO_PROXY",
        "DISPLAY",
    ] {
        assert!(!e.contains_key(k), "{k} in {e:?}");
    }
    let w = world(PLAIN);
    let spec = sspec::spec(&w.env, &w.def, &w.ctx, &w.plan, Path::new(PADDOCK)).unwrap();
    let e = run_env(&w.env, &spec, &w.ctx, &w.plan, &sockets(&w));
    assert!(!e.contains_key("SSH_AUTH_SOCK"));
    assert!(!e.keys().any(|k| k.starts_with("PADDOCK_SECRET_")));
}

#[test]
fn the_launch_is_sandbox_exec_with_the_profile_and_host_paths() {
    let w = world(FULL);
    let s = sockets(&w);
    let spec = sspec::spec(&w.env, &w.def, &w.ctx, &w.plan, Path::new(PADDOCK)).unwrap();
    let l = launch(
        &w.env,
        &w.def,
        &w.ctx,
        &w.plan,
        &spec,
        &s,
        Path::new(PADDOCK),
    )
    .unwrap();
    assert_eq!(l.program, Path::new("/usr/bin/sandbox-exec"));
    let profile = w.ctx.dir.join("seatbelt.sb");
    assert_eq!(
        &l.args[..6],
        [
            "-f",
            &profile.display().to_string(),
            PADDOCK,
            "device-serve",
            "--socket",
            &s.supervisor.display().to_string(),
        ]
    );
    let at = l.args.iter().position(|a| a == "--connect").unwrap();
    assert_eq!(
        l.args[at + 1],
        format!("2222:{}", s.connect().join("0.sock").display())
    );
    assert!(l.args.iter().any(|a| a == "--env-pass"), "{:?}", l.args);
    assert!(
        !l.args.iter().any(|a| a.starts_with("/run/paddock")),
        "{:?}",
        l.args
    );
    assert_eq!(l.files.len(), 1);
    assert_eq!(l.files[0].0, profile);
    assert_eq!(l.files[0].2, 0o600);
    assert!(l.files[0].1.contains("(version 1)\n"), "{}", l.files[0].1);
    assert!(l.files[0]
        .1
        .contains("(deny file-read* file-write* (subpath \"/Users/u\"))"));
    assert_eq!(l.env["HOME"], w.ctx.dir.join("home").display().to_string());
}

#[test]
fn the_launcher_of_the_host_is_seatbelt_on_macos_only() {
    assert_eq!(
        Launcher::host() == Launcher::Seatbelt,
        cfg!(target_os = "macos")
    );
    assert_eq!(Launcher::Seatbelt.name(), "seatbelt");
    assert_eq!(Launcher::Bwrap.name(), "bwrap");
    let st = crate::devices::sandbox::state::InstanceState {
        device: "d".into(),
        caller: "fake-abc123".into(),
        run: "sandbox".into(),
        shape: "exec".into(),
        pid: 1,
        started: 1,
        socket: "/tmp/dev.sock".into(),
        scope: None,
        backend: Launcher::Seatbelt.name().into(),
        log: "/tmp/log".into(),
        dir: "/tmp/inst".into(),
        unit: None,
        owner: None,
        users: None,
    };
    assert_eq!(st.json(0)["backend"], "seatbelt");
    let text = serde_json::to_string(&st).unwrap();
    assert!(text.contains("\"backend\":\"seatbelt\""), "{text}");
}

#[test]
fn the_limits_notice_names_the_table() {
    assert_eq!(
        limits_notice("big"),
        "[devices.big.sandbox] mem and cpus are ignored: memory and cpu limits of devices need systemd. Not on macOS"
    );
}
