//! Unit tests of the pieces of a shared instance: the socket path and the
//! `ListenStream` of its unit, the table hash that catches a
//! reconfiguration, how this host starts the instance, the start lock that
//! keeps ten brokers from starting ten instances, and the notice of a host
//! without a user manager.
//!
//! The instance itself (leases, the idle exit, the caller per request, the
//! `reconfigured` answer, `kill -9` of a broker) runs as a real process in
//! tests/devices_shared.rs, which starts `paddock deviced` and real
//! brokers.

use super::*;
use crate::devices::DevicesCfg;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// A host with a temporary home, config, state and runtime folder.
fn world(dir: &std::path::Path) -> HostEnv {
    for d in ["home", "cfg", "state", "run"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    let vars: BTreeMap<String, String> = [
        ("XDG_CONFIG_HOME", dir.join("cfg").display().to_string()),
        ("XDG_STATE_HOME", dir.join("state").display().to_string()),
        ("XDG_RUNTIME_DIR", dir.join("run").display().to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    HostEnv::from_parts(dir.join("home"), "demo".into(), 1000, 1000, vars)
}

fn def(extra: &str) -> DeviceDef {
    DevicesCfg::parse(&format!(
        "[devices.mac]\nshape = \"exec\"\nenabled = true\ncommand = [\"./build.sh\"]\nscope = \"shared\"\n{extra}"
    ))
    .unwrap()
    .user
    .remove(0)
}

/// The broker and the instance must agree on one path, and the unit that
/// systemd installs must name the same one with its own specifier.
#[test]
fn the_socket_and_the_socket_unit_name_one_path() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    let sock = socket_path(&env, "mac");
    if cfg!(target_os = "macos") {
        // launchd makes the socket at login, so the path is under the
        // state folder, which exists then and is short enough.
        assert_eq!(sock, state_socket(&env, "mac"));
        assert!(crate::sockpath::check(&sock).is_ok(), "{sock:?}");
    } else {
        assert_eq!(sock, t.path().join("run/paddock/devices/mac.sock"));
        assert_eq!(listen_stream("mac"), "%t/paddock/devices/mac.sock");
    }
    // The lock sits beside the socket, never on the socket path itself.
    let lock = lock_path(&env, "mac");
    assert_ne!(lock, sock);
    assert_eq!(lock.parent(), sock.parent());
    assert!(lock.to_string_lossy().ends_with("mac.lock"), "{lock:?}");
}

/// The hash is the whole table as parsed, so any key that changes what the
/// program is or what it may reach changes it, and a table written another
/// way with the same meaning does not.
#[test]
fn the_table_hash_changes_with_every_key_that_matters() {
    let base = table_hash(&def(""));
    assert_eq!(base.len(), 16, "{base}");
    assert_eq!(base, table_hash(&def("")), "the same table, the same hash");
    for change in [
        "timeout = 60\n",
        "idle = 30\n",
        "max_request = \"2M\"\n",
        "concurrency = 4\n",
        "stream = true\n",
        "mode = \"allow\"\n",
        "grant = [\"all\"]\n",
        "[devices.mac.sandbox]\nnetwork = \"egress\"\n",
        "[devices.mac.sandbox]\nro = [\"/opt/sdk\"]\n",
    ] {
        assert_ne!(base, table_hash(&def(change)), "{change} must change it");
    }
    // The order of the keys in the file does not change the hash: the
    // table as parsed is what counts, not its text.
    assert_eq!(
        table_hash(&def("timeout = 60\nconcurrency = 2\n")),
        table_hash(&def("concurrency = 2\ntimeout = 60\n"))
    );
    // Another device with the same body is another table.
    let other = DevicesCfg::parse(
        "[devices.gpu]\nshape = \"exec\"\nenabled = true\ncommand = [\"./build.sh\"]\nscope = \"shared\"\n",
    )
    .unwrap()
    .user
    .remove(0);
    assert_ne!(base, table_hash(&other));
}

/// Without a user manager the first broker owns the instance, and the
/// notice names the sandbox whose end ends it.
#[test]
fn a_host_without_a_user_manager_lets_the_first_broker_own_it() {
    let t = tempfile::tempdir().unwrap();
    let mut env = world(t.path());
    assert_eq!(start_kind(&env, "mac"), Start::Owned);
    assert_eq!(Start::Owned.unit(), None);
    assert_eq!(Start::Owned.name(), "owned by the run");
    let n = no_manager_notice("mac", "sandbox-9f3c1d2e");
    assert!(n.contains("the shared device mac"), "{n}");
    assert!(n.contains(crate::ids::short("sandbox-9f3c1d2e")), "{n}");
    assert!(n.contains("ends when that run ends"), "{n}");
    assert!(n.contains("paddock device install mac"), "{n}");
    // The test variable hides the user manager on a host that has one.
    env.vars.insert(
        "DBUS_SESSION_BUS_ADDRESS".into(),
        "unix:path=/dev/null".into(),
    );
    if env.which("systemd-run").is_some() && !cfg!(target_os = "macos") {
        assert_eq!(
            start_kind(&env, "mac"),
            Start::Transient("paddock-device-mac".into())
        );
        assert_eq!(
            Start::Transient("paddock-device-mac".into())
                .unit()
                .unwrap(),
            "paddock-device-mac"
        );
    }
    env.vars.insert(NO_MANAGER_VAR.into(), "1".into());
    assert_eq!(start_kind(&env, "mac"), Start::Owned);
}

/// An installed unit owns the socket, so no broker runs `systemd-run` and
/// the error of a unit that is there but not listening names the fix.
#[test]
fn an_installed_unit_takes_over_the_start() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    let files = install::files(&env, "mac");
    let first = files.first().unwrap().clone();
    std::fs::create_dir_all(first.parent().unwrap()).unwrap();
    let text = if cfg!(target_os = "macos") {
        crate::units::plist_mark("paddock device uninstall mac") + "\n<plist/>\n"
    } else {
        install::unit_texts("mac", std::path::Path::new("/usr/bin/paddock"), &[])[0]
            .1
            .clone()
    };
    std::fs::write(&first, text).unwrap();
    assert!(install::installed(&env, "mac"));
    let how = start_kind(&env, "mac");
    if cfg!(target_os = "macos") {
        assert_eq!(how, Start::LaunchAgent(install::label("mac")));
    } else {
        assert_eq!(how, Start::SocketUnit("paddock-device-mac.socket".into()));
    }
    // Nothing listens on the socket: the error names the command that
    // turns the unit on, and no broker starts an instance of its own.
    let e = ensure(&env, &def(""), "box1").unwrap_err();
    assert!(e.contains("installed but nothing listens"), "{e}");
    if !cfg!(target_os = "macos") {
        assert!(e.contains("systemctl --user enable --now"), "{e}");
    }
    // A file that paddock did not write is not paddock's unit.
    std::fs::write(&first, "mine\n").unwrap();
    assert!(!install::installed(&env, "mac"));
    assert_eq!(start_kind(&env, "mac"), Start::Owned);
}

/// Ten brokers that start at once: the lock lets one in at a time, so one
/// instance starts, not ten.
#[test]
fn ten_brokers_that_start_at_once_take_the_lock_one_at_a_time() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    let inside = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let most = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let took = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut threads = Vec::new();
    for _ in 0..10 {
        let (env, inside, most, took) = (env.clone(), inside.clone(), most.clone(), took.clone());
        threads.push(std::thread::spawn(move || {
            let lock = Lock::take(&env, "mac").expect("the lock");
            took.fetch_add(1, Ordering::SeqCst);
            let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            inside.fetch_sub(1, Ordering::SeqCst);
            drop(lock);
        }));
    }
    for th in threads {
        th.join().unwrap();
    }
    assert_eq!(
        took.load(Ordering::SeqCst),
        10,
        "every broker gets its turn"
    );
    assert_eq!(
        most.load(Ordering::SeqCst),
        1,
        "two brokers were in the start at once"
    );
    assert!(lock_path(&env, "mac").is_file());
}

/// `ensure` is for shared tables only: a run instance belongs to its
/// sandbox.
#[test]
fn ensure_refuses_a_run_scope_table() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    let run =
        DevicesCfg::parse("[devices.mac]\nshape = \"exec\"\nenabled = true\ncommand = [\"./d\"]\n")
            .unwrap()
            .user
            .remove(0);
    let e = ensure(&env, &run, "box1").unwrap_err();
    assert!(e.contains("scope = \"shared\""), "{e}");
}

/// The command line of the instance names the device and the socket, so the
/// state file of a shared instance matches a live process (`alive`).
#[test]
fn the_command_line_names_the_device_and_the_socket() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    let sock = socket_path(&env, "mac");
    let argv = deviced_argv(&env, "mac", &sock, Some("box1"));
    let words: Vec<String> = argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(words[1], "deviced");
    assert_eq!(words[2], "mac");
    assert_eq!(words[3], "--socket");
    assert_eq!(words[4], sock.display().to_string());
    assert_eq!(words[5], "--owner");
    assert_eq!(words[6], "box1");
    assert_eq!(deviced_argv(&env, "mac", &sock, None).len(), 5);
}

/// The variables that the instance needs to read the same configuration as
/// the broker that started it.
#[test]
fn the_unit_passes_the_folders_of_the_user() {
    let t = tempfile::tempdir().unwrap();
    let mut env = world(t.path());
    env.vars.insert(LAUNCHER_VAR.into(), "thread".into());
    let v = unit_env(&env);
    let names: Vec<&str> = v.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"XDG_CONFIG_HOME"), "{names:?}");
    assert!(names.contains(&"XDG_STATE_HOME"), "{names:?}");
    assert!(names.contains(&LAUNCHER_VAR), "{names:?}");
}

/// The table of the instance comes from the user configuration, and a
/// table that is not a shared device has no instance.
#[test]
fn the_instance_reads_the_user_configuration_only() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    let cfg = env.paddock_config();
    std::fs::create_dir_all(&cfg).unwrap();
    let e = deviced::table_of(&env, "mac").unwrap_err();
    assert!(format!("{e:#}").contains("no [devices.mac] table"), "{e:#}");
    std::fs::write(
        cfg.join("config.toml"),
        "[devices.mac]\nshape = \"exec\"\ncommand = [\"./d\"]\nenabled = true\nscope = \"shared\"\nidle = 2\n",
    )
    .unwrap();
    let d = deviced::table_of(&env, "mac").unwrap();
    assert_eq!(d.idle, 2);
    assert_eq!(d.scope, Scope::Shared);
    // A run instance and a device that is off have no shared instance.
    std::fs::write(
        cfg.join("config.toml"),
        "[devices.mac]\nshape = \"exec\"\ncommand = [\"./d\"]\nenabled = true\n",
    )
    .unwrap();
    let e = deviced::table_of(&env, "mac").unwrap_err();
    assert!(format!("{e:#}").contains("scope = \"run\""), "{e:#}");
    std::fs::write(
        cfg.join("config.toml"),
        "[devices.mac]\nshape = \"exec\"\ncommand = [\"./d\"]\nenabled = false\nscope = \"shared\"\n",
    )
    .unwrap();
    let e = deviced::table_of(&env, "mac").unwrap_err();
    assert!(format!("{e:#}").contains("enabled = false"), "{e:#}");
}

/// A broker that cannot reach the socket says so, and names the path.
#[test]
fn a_lease_on_nothing_names_the_socket() {
    let t = tempfile::tempdir().unwrap();
    let sock = t.path().join("missing.sock");
    let e = Lease::take(&sock, "box1", "abcd").unwrap_err();
    assert!(!e.reconfigured, "{e}");
    assert!(e.why.contains("cannot reach the shared instance"), "{e}");
    assert!(e.why.contains("missing.sock"), "{e}");
}

/// No instance, no users: `paddock device ls` says `shared, stopped`.
#[test]
fn users_of_an_instance_that_does_not_run_is_none() {
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    assert_eq!(users_of(&env, "mac"), None);
    assert!(InstanceState::shared_of(&env, "mac").is_none());
}

/// A shared instance that refuses the lease with `reconfigured`: the run
/// learns why, every request of the run fails until a restart, and the
/// catalog of the device names the fix. A small server stands in for the
/// instance, so this needs no `paddock deviced`.
#[test]
fn a_reconfigured_refusal_reaches_the_catalog_of_the_device() {
    use crate::devices::outproc::{InstanceSource, OutOfProcess};
    use crate::devices::{Device, GrantHit};
    let t = tempfile::tempdir().unwrap();
    let env = world(t.path());
    // mode = "allow": the card is not the subject here.
    let d = def("mode = \"allow\"\n");
    let sock = socket_path(&env, "mac");
    crate::privfs::dir(sock.parent().unwrap()).unwrap();
    let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let server = std::thread::spawn(move || {
        for c in l.incoming() {
            if s2.load(Ordering::SeqCst) {
                return;
            }
            let Ok(mut c) = c else { return };
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut c, &mut buf);
            let body = "the device mac was reconfigured: paddock device restart mac\n";
            let head = format!(
                "HTTP/1.1 503 Service Unavailable\r\nPaddock-Status: reconfigured\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut c, head.as_bytes());
            let _ = std::io::Write::write_all(&mut c, body.as_bytes());
        }
    });
    let ctx = crate::devices::DeviceCtx {
        env: env.clone(),
        sandbox: "box1".into(),
        harness: "fake".into(),
        project: t.path().join("home"),
        started_by: Vec::new(),
        approver: std::sync::Arc::new(crate::broker::DenyAll),
        instances: crate::devices::sandbox::Instances::new(env.clone()),
    };
    let src = std::sync::Arc::new(SharedSource::new(d.clone(), &ctx));
    // The lease is refused, and the run knows that it is a reconfiguration.
    let e = src.ensure_leased().unwrap_err();
    assert!(e.contains("reconfigured"), "{e}");
    assert!(e.contains("paddock device restart mac"), "{e}");
    assert_eq!(
        InstanceSource::failed(src.as_ref()).as_deref(),
        Some(RECONFIGURED_WHY)
    );
    // The device is `failed` with a how_to_fix that names the restart, so
    // the agent reads the fix in `.catalog`.
    let dev = OutOfProcess::new(d, ctx, src.clone(), GrantHit::NoKey);
    let call = crate::devices::Call::for_test("box1", Duration::from_secs(5));
    assert_eq!(
        dev.call(&call, b"x").unwrap_err(),
        crate::devices::DevError::Failed
    );
    let v = dev.details().unwrap();
    let fix = v["how_to_fix"].as_str().unwrap_or_default();
    assert!(fix.contains("reconfigured"), "{fix}");
    assert!(fix.contains("paddock device restart mac"), "{fix}");
    assert_eq!(v["instance"], "failed", "{v}");
    stop.store(true, Ordering::SeqCst);
    let _ = std::os::unix::net::UnixStream::connect(&sock);
    let _ = server.join();
}

/// The states of a unit come from the two lines of `systemctl show`, in
/// any order, and a unit the manager does not have reads as not found.
#[test]
fn the_unit_states_are_read_from_systemctl_show() {
    assert_eq!(
        parse_unit_states("ActiveState=inactive\nLoadState=loaded\n"),
        ("loaded".to_string(), "inactive".to_string())
    );
    assert_eq!(
        parse_unit_states("LoadState=not-found\nActiveState=inactive\n"),
        ("not-found".to_string(), "inactive".to_string())
    );
    assert_eq!(parse_unit_states(""), (String::new(), String::new()));
}

/// Only the refusal of a name that is still loaded is worth a second try.
#[test]
fn only_a_unit_that_still_exists_gets_a_second_systemd_run() {
    assert!(unit_exists(
        "systemd-run --user --unit=paddock-device-up failed with exit status: 1: Failed to start transient service unit: Unit paddock-device-up.service was already loaded or has a fragment file."
    ));
    assert!(unit_exists(
        "Unit paddock-device-up.service already exists."
    ));
    assert!(!unit_exists(
        "Failed to connect to bus: No such file or directory"
    ));
}
