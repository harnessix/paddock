//! Tests of `paddock device add|ls|show|example`.

use super::*;
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: DeviceCmd,
}

fn host(dir: &Path) -> HostEnv {
    let vars = [
        ("XDG_RUNTIME_DIR", dir.join("rt")),
        ("XDG_STATE_HOME", dir.join("st")),
        ("XDG_CONFIG_HOME", dir.join("cf")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.display().to_string()))
    .collect();
    HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars)
}

fn add_args(line: &[&str]) -> AddArgs {
    let mut v = vec!["paddock", "add"];
    v.extend(line);
    match Cli::parse_from(v).cmd {
        DeviceCmd::Add(a) => *a,
        _ => unreachable!(),
    }
}

#[test]
fn add_writes_a_table_that_loads_and_refuses_a_second_add() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        config_file(&env),
        "[devices]\necho = true\n[exec]\nnetwork = false\n",
    )
    .unwrap();
    let out = add(
        &env,
        &add_args(&[
            "mac",
            "--grant",
            "pasture:workers",
            "--connect",
            "2222:mac.local:22",
            "--ro",
            "~/.ssh/known_hosts_mac",
            "--credential",
            "mac-ssh",
            "--caller",
            "project:ro",
            "--stream",
            "--timeout",
            "900",
            "--max-request",
            "4K",
            "--max-response",
            "16M",
            "--exec",
            "./build.sh",
            "--fast",
        ]),
    )
    .unwrap();
    assert!(out.contains("[devices.mac]"), "{out}");
    assert!(
        out.contains("command = [\"./build.sh\", \"--fast\"]"),
        "{out}"
    );
    assert!(out.contains("# added to"), "{out}");
    let cfg = DevicesCfg::load(&env).unwrap();
    assert_eq!(cfg.enabled, ["echo", "mac"]);
    let d = &cfg.user[0];
    assert_eq!(d.name, "mac");
    assert_eq!(d.command, ["./build.sh", "--fast"]);
    assert!(d.stream);
    assert_eq!(d.limits.timeout, 900);
    assert_eq!(d.limits.max_request, 4 << 10);
    let p = d.sandbox.as_ref().unwrap();
    assert_eq!(p.connect[0].render(), "2222:mac.local:22");
    assert_eq!(p.ro, [t.path().join("home/.ssh/known_hosts_mac")]);
    assert_eq!(p.credentials, ["mac-ssh"]);
    // The rest of the file is as it was.
    let text = std::fs::read_to_string(config_file(&env)).unwrap();
    assert!(
        text.starts_with(
            "[devices]\necho = true\n[exec]\nnetwork = false\n\n# added by paddock device add\n"
        ),
        "{text}"
    );
    let e = format!(
        "{:#}",
        add(&env, &add_args(&["mac", "--exec", "x"])).unwrap_err()
    );
    assert!(e.starts_with("a [devices.mac] table exists in"), "{e}");
    // A bad value never reaches the file.
    let e = format!(
        "{:#}",
        add(
            &env,
            &add_args(&["two", "--connect", "22:mac.local:22", "--exec", "x"])
        )
        .unwrap_err()
    );
    assert!(e.contains("name a local port of 1024 or more"), "{e}");
    assert_eq!(std::fs::read_to_string(config_file(&env)).unwrap(), text);
    // ls lists it; --json gives the parsed table.
    let table = ls(&env, false).unwrap();
    assert!(
        table.contains("mac   exec     sandbox  run    on     pasture:workers"),
        "{table}"
    );
    assert!(
        table.contains("echo  builtin  builtin  -      on"),
        "{table}"
    );
    let j: serde_json::Value = serde_json::from_str(&ls(&env, true).unwrap()).unwrap();
    assert_eq!(j["devices"][0]["name"], "mac");
    assert_eq!(j["devices"][0]["shape"], "exec");
    assert_eq!(
        j["devices"][0]["sandbox"]["connect"][0]["host"],
        "mac.local"
    );
    // show prints the expanded paths and what the sandbox reaches.
    let s = show(&env, "mac").unwrap();
    assert!(
        s.contains(&format!(
            "ro          {}",
            t.path().join("home/.ssh/known_hosts_mac").display()
        )),
        "{s}"
    );
    assert!(
        s.contains("connect: 127.0.0.1:2222 inside reaches mac.local:22 from the host"),
        "{s}"
    );
    assert!(s.contains("mac-ssh  not in [credentials]"), "{s}");
    // A stream runs through the device sandbox: nothing is held back.
    assert!(!s.contains("not available yet"), "{s}");
    assert!(show(&env, "nope").is_err());
    assert!(show(&env, "echo").unwrap().contains("built-in"));
}

#[test]
fn a_host_device_shows_host_in_capitals_and_a_0_2_1_table_keeps_its_exec_base() {
    let t = tempfile::tempdir().unwrap();
    let env = host(t.path());
    std::fs::create_dir_all(env.paddock_config()).unwrap();
    std::fs::write(
        config_file(&env),
        "[exec]\nhome = \"read-only\"\npaths = [\"~/scratch\"]\n[devices.jq]\nkind = \"command\"\nenabled = true\ncommand = [\"jq\"]\n",
    )
    .unwrap();
    let out = add(
        &env,
        &add_args(&["notes", "--run", "host", "--exec", "notes-search"]),
    )
    .unwrap();
    assert!(out.contains("run = \"host\""), "{out}");
    let table = ls(&env, false).unwrap();
    assert!(
        table.contains("notes  exec     HOST     run    on"),
        "{table}"
    );
    let s = show(&env, "notes").unwrap();
    assert!(s.contains("runs on the host as you"), "{s}");
    assert!(!s.contains("not available yet"), "{s}");
    let s = show(&env, "jq").unwrap();
    assert!(
        s.contains("the [exec] keys of the user configuration as the base"),
        "{s}"
    );
    assert!(s.contains("home        read-only"), "{s}");
    assert!(
        s.contains(&format!(
            "rw          {}",
            t.path().join("home/scratch").display()
        )),
        "{s}"
    );
    assert!(s.contains("kind = \"command\""), "{s}");
}

#[test]
fn example_writes_an_executable_script() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("upper.py");
    let out = example(&p).unwrap();
    assert!(
        out.contains(&format!("paddock device add upper --exec {}", p.display())),
        "{out}"
    );
    let mode = std::fs::metadata(&p).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111);
    assert!(std::fs::read_to_string(&p)
        .unwrap()
        .contains("PADDOCK_DEVICE_SOCKET"));
    assert!(example(&p).is_err(), "never overwrites");
}
