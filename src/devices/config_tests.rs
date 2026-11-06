use super::*;
use crate::devices::grant::Grant;
use crate::devices::DevicesCfg;
use crate::execbox::Home;
use std::collections::BTreeMap;

fn parse(text: &str) -> DeviceDef {
    DevicesCfg::parse(text).unwrap().user.remove(0)
}

fn err(text: &str) -> String {
    format!("{:#}", DevicesCfg::parse(text).unwrap_err())
}

fn limits(max: usize, timeout: u64) -> Limits {
    Limits {
        max_request: max,
        max_response: max,
        timeout,
        concurrency: 1,
    }
}

/// The three tables of the README of 0.2.1 parse to the same meaning as
/// before: the http proxy, the command through execbox with the `[exec]`
/// base, the paid device.
#[test]
fn the_tables_of_0_2_1_parse_to_the_same_device() {
    let ollama = parse(
        r#"
[devices.ollama]
kind = "http"
enabled = true
target = "127.0.0.1:11434"
methods = ["GET", "POST"]
paths = ["/api/"]
max_body = "1M"
timeout = 30
"#,
    );
    assert_eq!(
        ollama,
        DeviceDef {
            name: "ollama".into(),
            enabled: true,
            shape: Shape::HttpProxy,
            run: Run::Broker,
            scope: Scope::Run,
            start: Start::FirstRequest,
            stream: false,
            grant: None,
            mode: Mode::Ask,
            limits: limits(1 << 20, 30),
            idle: DEFAULT_IDLE,
            ready: DEFAULT_READY,
            cost: 0,
            cost_header: None,
            budget: None,
            command: vec![],
            sandbox: None,
            http: Some(HttpProxy {
                target: "127.0.0.1:11434".parse().unwrap(),
                methods: vec!["GET".into(), "POST".into()],
                paths: vec!["/api/".into()],
            }),
            old_keys: vec!["kind".into(), "max_body".into()],
        }
    );
    let jq = parse(
        r#"
[devices.jq]
kind = "command"
enabled = true
command = ["jq", "-c", "."]
max_body = "256K"
timeout = 30
network = false
"#,
    );
    assert_eq!(
        jq,
        DeviceDef {
            name: "jq".into(),
            enabled: true,
            shape: Shape::Exec,
            run: Run::Sandbox,
            scope: Scope::Run,
            start: Start::FirstRequest,
            stream: false,
            grant: None,
            mode: Mode::Ask,
            limits: limits(256 << 10, 30),
            idle: DEFAULT_IDLE,
            ready: DEFAULT_READY,
            cost: 0,
            cost_header: None,
            budget: None,
            command: vec!["jq".into(), "-c".into(), ".".into()],
            sandbox: Some(SandboxProfile {
                network: Net::None,
                exec_base: true,
                ..Default::default()
            }),
            http: None,
            old_keys: vec!["kind".into(), "max_body".into()],
        }
    );
    assert!(jq.not_yet().is_none(), "a 0.2.1 command device runs today");
    assert_eq!(jq.max_body(), 256 << 10);
    assert_eq!(jq.kind_name(), "command");
    let with_net = parse("[devices.c]\nkind = \"command\"\ncommand = [\"x\"]\nnetwork = true\n");
    assert_eq!(with_net.profile().network, Net::Egress);
    assert!(with_net.not_yet().is_none());
    let search = parse(
        r#"
[devices.search]
kind = "http"
enabled = true
target = "127.0.0.1:8090"
cost = 0.002
cost_header = "X-Cost"
budget = 1.0
"#,
    );
    assert_eq!(
        search,
        DeviceDef {
            name: "search".into(),
            enabled: true,
            shape: Shape::HttpProxy,
            run: Run::Broker,
            scope: Scope::Run,
            start: Start::FirstRequest,
            stream: false,
            grant: None,
            mode: Mode::Ask,
            limits: limits(DEFAULT_MAX_BODY, DEFAULT_TIMEOUT),
            idle: DEFAULT_IDLE,
            ready: DEFAULT_READY,
            cost: 2_000,
            cost_header: Some("X-Cost".into()),
            budget: Some(1_000_000),
            command: vec![],
            sandbox: None,
            http: Some(HttpProxy {
                target: "127.0.0.1:8090".parse().unwrap(),
                methods: vec!["GET".into()],
                paths: vec!["/".into()],
            }),
            old_keys: vec!["kind".into()],
        }
    );
    assert!(search.paid());
}

#[test]
fn a_table_of_today_parses_every_key() {
    let d = parse(
        r#"
[devices.mac]
shape = "exec"
run = "sandbox"
command = ["~/.config/paddock/devices/mac/build.sh"]
enabled = true
grant = ["pasture:workers", "project:/home/demo/src/x", "harness:claude", "all"]
mode = "allow"
scope = "run"
start = "first-request"
stream = true
max_request = "4K"
max_response = "16M"
timeout = 1200
concurrency = 2
idle = 600
ready = 20
[devices.mac.sandbox]
network = "none"
connect = ["2222:mac.local:22", "8443:mac.example.org:443"]
ro = ["/etc/ssh/known_hosts_mac"]
rw = ["/tmp/mac-cache"]
home = "read-only"
env = { SSH = "ssh -p 2222 127.0.0.1" }
env_pass = ["TERM"]
credentials = ["mac-ssh"]
caller = ["project:ro"]
mem = "512M"
cpus = 1.5
"#,
    );
    assert_eq!(d.shape, Shape::Exec);
    assert_eq!(d.run, Run::Sandbox);
    assert_eq!(
        d.grant,
        Some(vec![
            Grant::Pasture("workers".into()),
            Grant::Project("/home/demo/src/x".into()),
            Grant::Harness("claude".into()),
            Grant::All,
        ])
    );
    assert_eq!(d.mode, Mode::Allow);
    assert!(d.stream);
    assert_eq!(
        d.limits,
        Limits {
            max_request: 4 << 10,
            max_response: 16 << 20,
            timeout: 1200,
            concurrency: 2
        }
    );
    assert_eq!((d.idle, d.ready), (600, 20));
    let p = d.sandbox.unwrap();
    assert_eq!(
        p,
        SandboxProfile {
            network: Net::None,
            allow: vec![],
            connect: vec![
                Connect {
                    local_port: 2222,
                    host: "mac.local".into(),
                    port: 22
                },
                Connect {
                    local_port: 8443,
                    host: "mac.example.org".into(),
                    port: 443
                }
            ],
            ro: vec!["/etc/ssh/known_hosts_mac".into()],
            rw: vec!["/tmp/mac-cache".into()],
            home: Home::ReadOnly,
            env: BTreeMap::from([("SSH".to_string(), "ssh -p 2222 127.0.0.1".to_string())]),
            env_pass: vec!["TERM".into()],
            credentials: vec!["mac-ssh".into()],
            caller: vec![CallerMount::ProjectRo],
            mem: Some(512 << 20),
            cpu_percent: Some(150),
            tmp: None,
            exec_base: false,
        }
    );
    assert!(p.reaches_something());
    assert!(!p.is_plain());
    let lines = p.reach_lines();
    assert!(
        lines
            .iter()
            .any(|l| l == "connect: 127.0.0.1:2222 inside reaches mac.local:22 from the host"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "credential: mac-ssh"),
        "{lines:?}"
    );
    // A serve table defaults to concurrency 4.
    let s = parse("[devices.s]\nshape = \"serve\"\ncommand = [\"./s.py\"]\n");
    assert_eq!(s.limits.concurrency, 4);
    assert!(
        s.not_yet().is_none(),
        "a serve table runs in its device sandbox"
    );
}

#[test]
fn home_expands_when_the_host_is_known() {
    let text = "[devices.d]\nshape = \"exec\"\ncommand = [\"x\"]\ngrant = [\"project:~/src/x\"]\n[devices.d.sandbox]\nro = [\"~/.ssh/known_hosts_mac\"]\n";
    let home = std::path::Path::new("/home/demo");
    let d = DevicesCfg::parse_with(
        text,
        ParseEnv {
            home: Some(home),
            ..Default::default()
        },
    )
    .unwrap()
    .user
    .remove(0);
    assert_eq!(
        d.sandbox.unwrap().ro,
        [std::path::PathBuf::from("/home/demo/.ssh/known_hosts_mac")]
    );
    assert_eq!(
        d.grant,
        Some(vec![Grant::Project("/home/demo/src/x".into())])
    );
    // Without a host, `~` stays as written.
    let d = parse(text);
    assert_eq!(
        d.sandbox.unwrap().ro,
        [std::path::PathBuf::from("~/.ssh/known_hosts_mac")]
    );
    // The home itself is refused in rw, with or without `~`.
    let rw = |p: &str| {
        format!(
            "{:#}",
            DevicesCfg::parse_with(
                &format!("[devices.d]\nshape = \"exec\"\ncommand = [\"x\"]\n[devices.d.sandbox]\nrw = [\"{p}\"]\n"),
                ParseEnv {
        home: Some(home),
        ..Default::default()
    }
            )
            .unwrap_err()
        )
    };
    for p in ["/", "~", "/home/demo"] {
        assert_eq!(
            rw(p),
            format!("[devices.d.sandbox]: rw: {p} would open the whole system or home; use home = \"full\"")
        );
    }
}

/// Each parse rule, with the exact text of its refusal.
#[test]
fn every_refusal_names_the_table_the_key_and_the_fix() {
    let exec = |extra: &str| format!("[devices.d]\nshape = \"exec\"\ncommand = [\"x\"]\n{extra}");
    assert_eq!(
        err("[devices.d]\nkind = \"command\"\nshape = \"exec\"\ncommand = [\"x\"]\n"),
        "[devices.d]: use shape or kind, not both"
    );
    assert_eq!(
        err("[devices.d]\ncommand = [\"x\"]\n"),
        "[devices.d]: shape must be \"exec\" or \"serve\" (or kind = \"http\" for a loopback proxy)"
    );
    assert_eq!(
        err("[devices.d]\nkind = \"http\"\ntarget = \"127.0.0.1:1\"\n[devices.d.sandbox]\nnetwork = \"none\"\n"),
        "[devices.d]: unknown key sandbox for http. Known: budget, cost, cost_header, enabled, grant, kind, max_body, max_request, max_response, methods, mode, paths, target, timeout"
    );
    assert_eq!(
        err(&exec("run = \"host\"\n[devices.d.sandbox]\nnetwork = \"none\"\n")),
        "[devices.d]: a [devices.d.sandbox] table is not allowed with run = \"host\": the command runs as you, with your files and network. Remove the table, or use run = \"sandbox\""
    );
    assert_eq!(
        err(&exec("run = \"host\"\nmode = \"allow\"\n")),
        "[devices.d]: mode = \"allow\" is refused for run = \"host\": a program that runs on the host as you shows the first-request card every session"
    );
    assert_eq!(
        err(&exec("scope = \"shared\"\n[devices.d.sandbox]\ncaller = [\"project:ro\"]\n")),
        "[devices.d]: a shared device serves many sandboxes and cannot mount the caller's project. Use scope = \"run\""
    );
    assert_eq!(
        err(&exec("max_body = \"1M\"\nmax_request = \"1M\"\n")),
        "[devices.d]: max_body sets both max_request and max_response; use max_body alone, or the two keys"
    );
    let d = parse(&exec("max_body = \"2M\"\n"));
    assert_eq!(
        (d.limits.max_request, d.limits.max_response),
        (2 << 20, 2 << 20)
    );
    assert_eq!(
        err(&exec("concurrency = 17\n")),
        "[devices.d]: concurrency is 1 to 16"
    );
    assert_eq!(
        err(&exec("concurrency = 0\n")),
        "[devices.d]: concurrency is 1 to 16"
    );
    assert_eq!(err(&exec("idle = 0\n")), "[devices.d]: idle is 1 to 86400");
    assert_eq!(
        err(&exec("ready = 301\n")),
        "[devices.d]: ready is 1 to 300"
    );
    assert_eq!(
        err(&exec("timeout = 0\n")),
        "[devices.d]: timeout is 1 to 3600"
    );
    assert_eq!(
        err(&exec("grant = [\"user:me\"]\n")),
        "[devices.d]: grant \"user:me\" is not a grant. The shapes: pasture:NAME, project:PATH, harness:NAME, remote:HOST, all"
    );
    assert_eq!(
        err(&exec("grant = []\n")),
        "[devices.d]: grant = [] grants nothing. Remove the key so that every sandbox sees the device, or list grants: pasture:NAME, project:PATH, harness:NAME, all"
    );
    assert_eq!(
        err(&exec("isolation = \"host\"\n")),
        "[devices.d]: isolation was never released. Use run = \"host\" or run = \"sandbox\""
    );
    assert_eq!(
        err(&exec("colour = 1\n")),
        "[devices.d]: unknown key colour for exec. Known: budget, command, concurrency, cost, cwd, device, enabled, env, env_pass, grant, idle, max_body, max_request, max_response, mode, ready, run, sandbox, scope, shape, start, stream, target, timeout"
    );
    assert_eq!(
        err(&exec("[devices.d.sandbox]\nvolumes = []\n")),
        "[devices.d.sandbox]: unknown key volumes for the sandbox profile. Known: allow, caller, connect, cpus, credentials, env, env_pass, home, mem, network, ro, rw, tmp"
    );
    assert_eq!(
        err(&exec("run = \"cloud\"\n")),
        "[devices.d]: run must be \"sandbox\" or \"host\" or \"remote\""
    );
    assert_eq!(
        err(&exec("mode = \"always\"\n")),
        "[devices.d]: mode must be \"ask\" or \"allow\""
    );
    assert_eq!(
        err(&exec("scope = \"user\"\n")),
        "[devices.d]: scope must be \"run\" or \"shared\""
    );
    assert_eq!(err(&exec("cwd = \"~/x\"\n")), "[devices.d]: cwd is for run = \"host\" only. A device sandbox sets its own working folder in [devices.d.sandbox]");
    assert_eq!(
        err(&exec("run = \"remote\"\n")),
        "[devices.d]: run = \"remote\" needs target = \"ssh://HOST\" or \"unix:/PATH\""
    );
    assert_eq!(
        err(&exec("run = \"remote\"\ntarget = \"ssh://mac\"\n")),
        "[devices.d]: run = \"remote\" needs device = \"NAME\", the device on the other side"
    );
    assert_eq!(
        err(&exec("[devices.d.sandbox]\ncaller = [\"project:rw\"]\n")),
        "[devices.d.sandbox]: caller project:rw is refused: a device never writes into the caller's tree. Use project:ro"
    );
    assert_eq!(
        err(&exec("[devices.d.sandbox]\nallow = [\"crates.io\"]\n")),
        "[devices.d.sandbox]: allow lists domains for network = \"api\"; set network = \"api\" or remove allow"
    );
    assert_eq!(
        err(&exec(
            "[devices.d.sandbox]\nconnect = [\"2222:a:1\", \"2222:b:2\"]\n"
        )),
        "[devices.d.sandbox]: connect uses the local port 2222 twice"
    );
    let many: Vec<String> = (0..17)
        .map(|i| format!("\"{}:h{i}.example:1\"", 2000 + i))
        .collect();
    assert_eq!(
        err(&exec(&format!(
            "[devices.d.sandbox]\nconnect = [{}]\n",
            many.join(", ")
        ))),
        "[devices.d.sandbox]: connect has 17 entries; at most 16"
    );
    assert_eq!(
        err(&exec("[devices.d.sandbox]\nconnect = [\"127.0.0.1:22\"]\n")),
        "[devices.d.sandbox]: connect \"127.0.0.1:22\": connect reaches another machine. For a service on this host use an http device"
    );
    assert_eq!(
        err(&exec("[devices.d.sandbox]\nconnect = [\"mac.local:22\"]\n")),
        "[devices.d.sandbox]: connect \"mac.local:22\": name a local port of 1024 or more: 2222:mac.local:22"
    );
    // The host run parses and runs (host.rs); the remote run is refused
    // at run time, not here.
    let host = parse(&exec(
        "run = \"host\"\ncwd = \"/tmp\"\nenv = { EDITOR = \"vi\" }\n",
    ));
    assert_eq!(
        host.run,
        Run::Host {
            cwd: Some("/tmp".into()),
            env: [("EDITOR".to_string(), "vi".to_string())].into(),
        }
    );
    assert_eq!(host.not_yet(), None);
    assert_eq!(
        parse("[devices.d]\nshape = \"serve\"\nrun = \"host\"\ncommand = [\"x\"]\n").not_yet(),
        None,
        "a serve program on the host needs no device sandbox"
    );
    assert_eq!(
        err(&exec("run = \"host\"\nenv_pass = [\"EDITOR\"]\n")),
        "[devices.d]: a host program gets your whole environment; env_pass is a key of the sandbox profile"
    );
    assert_eq!(
        err(&exec("env_pass = [\"EDITOR\"]\n")),
        "[devices.d]: env_pass belongs in [devices.d.sandbox], the profile of the device sandbox"
    );
    assert_eq!(
        err(&exec("env = { EDITOR = \"vi\" }\n")),
        "[devices.d]: env is for run = \"host\" only. A device sandbox sets env in [devices.d.sandbox]"
    );
    assert_eq!(
        err(&exec("run = \"host\"\nenv = { \"1x\" = \"vi\" }\n")),
        "[devices.d]: env: \"1x\" is not a variable name (letters, digits and _)"
    );
    // A remote table: no command, no shape needed (the other side's),
    // and the keys of the other side are refused with their place.
    let remote =
        parse("[devices.d]\nrun = \"remote\"\ntarget = \"ssh://mac\"\ndevice = \"build\"\n");
    assert_eq!(remote.not_yet(), None);
    assert_eq!(remote.shape, Shape::Exec);
    assert!(remote.command.is_empty());
    assert_eq!(
        err(&exec("run = \"remote\"\ntarget = \"ssh://mac\"\ndevice = \"build\"\n")),
        "[devices.d]: command is for the table on the other side: run = \"remote\" forwards to the device \"build\" there, and the paddock there runs its own command"
    );
    let remote_err = |extra: &str| {
        err(&format!(
            "[devices.d]\nrun = \"remote\"\ntarget = \"ssh://mac\"\ndevice = \"build\"\n{extra}"
        ))
    };
    assert_eq!(
        remote_err("[devices.d.sandbox]\nnetwork = \"none\"\n"),
        "[devices.d]: a [devices.NAME.sandbox] table is for the other side: the paddock at ssh://mac runs the device \"build\" in its own device sandbox"
    );
    assert!(remote_err("scope = \"shared\"\n")
        .contains("scope = \"shared\" is for the table on the other side"));
    assert!(remote_err("start = \"with-sandbox\"\n")
        .contains("start = \"with-sandbox\" is for the table on the other side"));
    assert_eq!(
        remote_err("mode = \"allow\"\n"),
        "[devices.d]: mode = \"allow\" is refused for an ssh target: each request runs `ssh mac` on the host as you, so the first-request card shows every session, as for run = \"host\""
    );
    // A socket target is a connection, not a host action: allow is fine.
    let sock = parse("[devices.d]\nrun = \"remote\"\ntarget = \"unix:/run/x.sock\"\ndevice = \"build\"\nmode = \"allow\"\n");
    assert_eq!(sock.mode, Mode::Allow);
    assert_eq!(
        err("[devices.d]\nrun = \"remote\"\ntarget = \"unix:relative.sock\"\ndevice = \"build\"\n"),
        "[devices.d]: target \"unix:relative.sock\": unix: needs an absolute socket path, for example unix:/tmp/paddock-mac/devices.sock"
    );
    assert_eq!(
        err("[devices.d]\nrun = \"remote\"\ntarget = \"ssh://-oProxyCommand=x\"\ndevice = \"build\"\n"),
        "[devices.d]: target \"ssh://-oProxyCommand=x\": ssh:// needs a host as ssh takes it, for example ssh://mac or ssh://me@mac.local:2222"
    );
    assert!(
        parse(&exec("stream = true\n")).not_yet().is_none(),
        "a stream runs through the device sandbox"
    );
    assert!(
        parse(&exec("scope = \"shared\"\n")).not_yet().is_none(),
        "a shared instance runs today (devices/shared.rs)"
    );
    // A shared instance serves every sandbox of the user, so a program
    // that runs on the host as the user cannot have one.
    assert!(err(
        "[devices.d]\nshape = \"exec\"\nrun = \"host\"\ncommand = [\"./d\"]\nscope = \"shared\"\n"
    )
    .contains("scope = \"shared\" needs run = \"sandbox\""));
    assert!(
        parse(&exec("[devices.d.sandbox]\ncredentials = [\"k\"]\n"))
            .not_yet()
            .is_none(),
        "a sandbox profile runs today"
    );
    assert!(parse(&exec("[devices.d.sandbox]\nnetwork = \"egress\"\n"))
        .not_yet()
        .is_none());
    // A user configuration cannot say `use`.
    assert!(
        err("[devices]\nuse = [\"mac\"]\n").contains("belongs in the paddock.toml of a project")
    );
}

#[test]
fn connect_entries_parse() {
    let c = Connect::parse("2222:mac.local:22", false).unwrap();
    assert_eq!(
        (c.local_port, c.host.as_str(), c.port),
        (2222, "mac.local", 22)
    );
    assert_eq!(c.render(), "2222:mac.local:22");
    let c = Connect::parse("mac.local:8443", false).unwrap();
    assert_eq!((c.local_port, c.port), (8443, 8443));
    assert!(
        Connect::parse("203.0.113.7:2222", false).is_ok(),
        "a public address is a machine"
    );
    for (bad, text) in [
        ("mac.local", "use LOCAL_PORT:HOST:PORT or HOST:PORT"),
        ("a:b:c:d", "use LOCAL_PORT:HOST:PORT or HOST:PORT"),
        ("mac.local:0", "\"0\" is not a port"),
        ("2222:mac.local:70000", "\"70000\" is not a port"),
        ("abc:mac.local:22", "\"abc\" is not a port"),
        (
            "22:mac.local:22",
            "name a local port of 1024 or more: 2222:mac.local:22",
        ),
        (
            "80:web.example:80",
            "name a local port of 1024 or more: 8080:web.example:80",
        ),
        (
            "443:web.example:443",
            "name a local port of 1024 or more: 8443:web.example:443",
        ),
        ("2222:localhost:22", "connect reaches another machine"),
        ("2222:127.0.0.1:22", "connect reaches another machine"),
        ("2222:192.168.1.5:22", "connect reaches another machine"),
        ("2222:10.0.0.1:22", "connect reaches another machine"),
        ("2222:169.254.1.1:22", "connect reaches another machine"),
        ("2222:0.0.0.0:22", "connect reaches another machine"),
        ("2222:bad_host:22", "is not a host name or an address"),
        ("2222::22", "is not a host name or an address"),
    ] {
        let e = format!("{:#}", Connect::parse(bad, false).unwrap_err());
        assert!(e.starts_with(&format!("connect {bad:?}: ")), "{bad}: {e}");
        assert!(e.contains(text), "{bad}: {e}");
    }
}

#[test]
fn only_a_debug_build_lets_the_test_variable_allow_a_local_connect() {
    use crate::devices::reader::local_connect_from_env;
    let set = Some(std::ffi::OsStr::new("1"));
    assert!(
        !local_connect_from_env(true, set),
        "a release build ignores it"
    );
    assert!(!local_connect_from_env(true, None));
    assert!(!local_connect_from_env(false, None));
    assert!(local_connect_from_env(false, set));

    let text = "[devices.d]\nshape = \"exec\"\nrun = \"sandbox\"\ncommand = [\"x\"]\n\
                [devices.d.sandbox]\nconnect = [\"2222:127.0.0.1:22\"]\n";
    let with = |allow_local_connect| {
        DevicesCfg::parse_with(
            text,
            ParseEnv {
                home: None,
                allow_local_connect,
            },
        )
    };
    // The release path with the variable set gives the flag false: refused.
    let e = format!("{:#}", with(local_connect_from_env(true, set)).unwrap_err());
    assert!(e.contains("connect reaches another machine"), "{e}");
    let e = format!("{:#}", DevicesCfg::parse(text).unwrap_err());
    assert!(e.contains("connect reaches another machine"), "{e}");
    assert_eq!(
        with(true).unwrap().user[0]
            .sandbox
            .as_ref()
            .unwrap()
            .connect[0]
            .render(),
        "2222:127.0.0.1:22"
    );
}

#[test]
fn a_device_renders_to_toml_and_parses_back_the_same() {
    let text = r#"
[devices.mac]
shape = "exec"
run = "sandbox"
command = ["/home/demo/bin/build.sh", "--fast"]
enabled = true
grant = ["pasture:workers", "project:/home/demo/src/x"]
mode = "allow"
stream = true
max_request = "4K"
max_response = "16M"
timeout = 1200
concurrency = 2
idle = 600
ready = 20
cost = 0.002
budget = 1.5
[devices.mac.sandbox]
network = "api"
allow = ["crates.io"]
connect = ["2222:mac.local:22"]
ro = ["/etc/ssh/known_hosts_mac"]
rw = ["/tmp/mac-cache"]
home = "read-only"
env = { SSH = "ssh -p 2222 127.0.0.1" }
env_pass = ["TERM"]
credentials = ["mac-ssh"]
caller = ["project:ro"]
mem = "512M"
cpus = 1.5
"#;
    let d = parse(text);
    let back = parse(&d.to_toml());
    assert_eq!(back, d, "{}", d.to_toml());
    // The other shapes too: a proxy, a serve, a host run, a 0.2.1 table.
    for t in [
        "[devices.o]\nkind = \"http\"\nenabled = true\ntarget = \"127.0.0.1:11434\"\nmethods = [\"GET\", \"POST\"]\npaths = [\"/api/\"]\nmax_body = \"1M\"\ncost = 0.002\ncost_header = \"X-Cost\"\n",
        "[devices.s]\nshape = \"serve\"\ncommand = [\"./s.py\"]\nscope = \"shared\"\nstart = \"with-sandbox\"\ncost_header = \"X-Cost\"\n",
        "[devices.h]\nshape = \"exec\"\nrun = \"host\"\ncommand = [\"notes\"]\ncwd = \"/tmp\"\nenv = { EDITOR = \"vi\", PAGER = \"cat\" }\n",
        "[devices.r]\nrun = \"remote\"\ntarget = \"ssh://mac\"\ndevice = \"build\"\n",
        "[devices.jq]\nkind = \"command\"\nenabled = true\ncommand = [\"jq\", \"-c\", \".\"]\nmax_body = \"256K\"\nnetwork = true\n",
    ] {
        let d = parse(t);
        let mut back = parse(&d.to_toml());
        // The spelling of a rendered table is today's, except the
        // `[exec]` base of a 0.2.1 command, which keeps `kind`.
        back.old_keys = d.old_keys.clone();
        assert_eq!(back, d, "{t}\n{}", d.to_toml());
    }
}

#[test]
fn sizes_and_names() {
    assert_eq!(parse_size("512").unwrap(), 512);
    assert_eq!(parse_size("256K").unwrap(), 256 * 1024);
    assert_eq!(parse_size("1m").unwrap(), 1 << 20);
    assert_eq!(parse_size("64M").unwrap(), MAX_BODY_CAP);
    assert!(parse_size("65M").is_err());
    assert!(parse_size("1G").is_err());
    assert!(parse_size("K").is_err());
    assert!(parse_size("1.5M").is_err());
    for (n, s) in [
        (512, "512"),
        (256 << 10, "256K"),
        (1 << 20, "1M"),
        (1536, "1536"),
    ] {
        assert_eq!(render_size(n), s);
        assert_eq!(parse_size(s).unwrap(), n);
    }
    assert!(
        check_name("use").is_err(),
        "use is the key of a project file"
    );
    assert!(check_name("mac-build_2").is_ok());
}

#[test]
fn a_tilde_in_the_program_path_expands_against_the_home() {
    let home = Path::new("/home/demo");
    let text = "[devices.b]\nshape = \"exec\"\ncommand = [\"~/bin/build.sh\", \"~/not-a-path\"]\n";
    let d = DevicesCfg::parse_with(
        text,
        ParseEnv {
            home: Some(home),
            ..Default::default()
        },
    )
    .unwrap()
    .user
    .remove(0);
    assert_eq!(d.command, ["/home/demo/bin/build.sh", "~/not-a-path"]);
    // Without a host, as the README tests parse: as written.
    let d = DevicesCfg::parse(text).unwrap().user.remove(0);
    assert_eq!(d.command, ["~/bin/build.sh", "~/not-a-path"]);
}
