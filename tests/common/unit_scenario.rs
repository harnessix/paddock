//! The shared device under the real systemd user manager of the host, as
//! a function: tests/devices_shared_units.rs runs it once, and
//! tests/devices_stress.rs twice at once in parallel threads.
//!
//! The scenario talks to the real user manager, so it uses the real
//! runtime folder (the socket unit listens under `%t`, which is that
//! folder), a device name of its own (unit names are global for the
//! user: the pid, a counter and the clock make it), and a temporary HOME
//! (`install` refuses to point a unit of the real home at a build). The
//! units go at the end, also after a failed assert. Every wait is for the
//! real condition with a deadline wide enough for a busy host.

use super::shared_world::{config, until, Broker, World};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

/// The seconds a wait for a start, a stop or an unload may take on a
/// busy host.
const WIDE: u64 = 120;

/// The units of one scenario. The drop stops and removes them even when
/// an assert fails, so the user manager of the host keeps nothing.
pub struct Units {
    name: String,
    root: PathBuf,
    run: PathBuf,
    bus: String,
}

impl Units {
    fn systemctl(&self, args: &[&str]) -> std::process::Output {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus)
            .env("XDG_RUNTIME_DIR", &self.run)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn state_of(&self, unit: &str) -> String {
        let out = self.systemctl(&["is-active", unit]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `LoadState` of the unit: `not-found` once the manager unloaded it.
    fn load_state(&self, unit: &str) -> String {
        let out = self.systemctl(&["show", "-P", "LoadState", unit]);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn status(&self, unit: &str) -> String {
        let out = self.systemctl(&["status", "--no-pager", "--full", "-n", "30", unit]);
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    fn service(&self) -> String {
        format!("paddock-device-{}.service", self.name)
    }

    fn socket(&self) -> String {
        format!("paddock-device-{}.socket", self.name)
    }
}

impl Drop for Units {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_paddock"))
            .args(["device", "uninstall", &self.name])
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_RUNTIME_DIR", &self.run)
            .env("DBUS_SESSION_BUS_ADDRESS", &self.bus)
            .env_remove("PADDOCK")
            .output();
        for unit in [self.socket(), self.service()] {
            let _ = self.systemctl(&["stop", &unit]);
            let _ = self.systemctl(&["reset-failed", &unit]);
        }
        let _ = std::fs::remove_file(self.run.join(format!("paddock/devices/{}.sock", self.name)));
        let _ = std::fs::remove_file(self.run.join(format!("paddock/devices/{}.lock", self.name)));
    }
}

/// A device name that no other scenario of this user has: unit names are
/// global for the user, and two threads of one process share the pid.
fn unique_name() -> String {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let n = COUNT.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("sd{}x{n}x{:x}", std::process::id(), nanos & 0xffff)
}

/// With a systemd user manager: the transient unit exists while the
/// instance runs, `systemctl --user stop` ends it and the next request
/// starts it again, and `paddock device install` writes the socket unit,
/// after which the first connection starts the service with no
/// `systemd-run` from a broker. False when the host has no systemd user
/// instance.
pub fn unit_owns_instance() -> bool {
    if !super::systemd_user() {
        println!(
            "skipped: no systemd user instance (DBUS_SESSION_BUS_ADDRESS and systemctl --user)"
        );
        return false;
    }
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let run = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(format!("/run/user/{uid}")));
    let name = unique_name();
    let mut w = World::new("");
    w.name = name.clone();
    w.run = run.clone();
    std::fs::create_dir_all(w.root.join("home")).unwrap();
    let s = w.script(
        "up.sh",
        "printf 'caller=%s ' \"$PADDOCK_CALLER\"\ntr '[:lower:]' '[:upper:]'",
    );
    std::fs::write(
        w.config(),
        config(&s, "").replace("[devices.up]", &format!("[devices.{name}]")),
    )
    .unwrap();
    let units = Units {
        name: name.clone(),
        root: w.root.clone(),
        run: run.clone(),
        bus: bus.clone(),
    };
    let service = units.service();
    let socket_unit = units.socket();
    // The real user manager: no PADDOCK_TEST_NO_USER_MANAGER, the session
    // bus of this host.
    let paddock = |args: &[&str]| {
        let mut c = w.paddock();
        c.env_remove("PADDOCK_TEST_NO_USER_MANAGER")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus)
            .env("HOME", w.root.join("home"))
            .args(args);
        c
    };
    let broker = |hold: u64| {
        let mut c = paddock(&[
            "device",
            "check",
            &name,
            "--send",
            "x",
            "--hold",
            &hold.to_string(),
        ]);
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        Broker(Some(c.spawn().unwrap()))
    };
    // What a failure needs: the instance log, the broker, the unit.
    let why = |b: &mut Broker, unit: &str| {
        if let Some(c) = b.0.as_mut() {
            let _ = c.kill();
        }
        format!(
            "log:\n{}\nbroker:\n{}\nunit:\n{}",
            w.log(),
            b.wait(),
            units.status(unit)
        )
    };
    // The state file of an instance that the transient unit holds: a new
    // pid, the unit's name, no owner. The start writes the file once and
    // whole; the wait is for that record, not for the pid alone, so a
    // state that another writer leaves for a moment cannot pass as a start.
    let unit_name = format!("paddock-device-{name}");
    let started = |old: u32| {
        until(
            WIDE,
            || w.state(),
            |s| {
                s.as_ref().is_some_and(|s| {
                    let p = s["pid"].as_u64().unwrap_or(0) as u32;
                    p > 0 && p != old && s["unit"] == unit_name
                })
            },
        )
    };
    let mut first = broker(20);
    let st = started(0);
    assert!(
        st.as_ref()
            .is_some_and(|s| s["pid"].as_u64().unwrap_or(0) > 0 && s["unit"] == unit_name),
        "no instance of {name} under its unit within {WIDE} s (state: {st:?}):\n{}",
        why(&mut first, &service)
    );
    let st = st.unwrap();
    let pid = st["pid"].as_u64().unwrap() as u32;
    assert_eq!(st["owner"], serde_json::Value::Null, "{st}");
    assert_eq!(
        units.state_of(&service),
        "active",
        "{}",
        units.status(&service)
    );
    // systemctl --user stop ends it; the next request starts it again.
    let out = first.wait();
    assert!(out.contains("(systemd-run)"), "{out}");
    let out = units.systemctl(&["stop", &service]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        until(WIDE, || w.state().is_none(), |gone| *gone),
        "the stop left the instance of {name} for {WIDE} s:\n{}",
        w.log()
    );
    // The unit is gone too (--collect) before the next start, so the next
    // systemd-run does not meet a unit that is still stopping. The broker
    // waits for the unload itself too; what the manager still had goes
    // into the message of a start that fails.
    let left = until(
        WIDE,
        || (units.state_of(&service), units.load_state(&service)),
        |(active, load)| {
            !matches!(active.as_str(), "active" | "deactivating" | "activating") && load != "loaded"
        },
    );
    let mut again = broker(2);
    let st = started(pid);
    assert!(
        st.as_ref().is_some_and(|s| {
            s["pid"].as_u64().unwrap_or(0) as u32 != pid && s["unit"] == unit_name
        }),
        "no new instance of {name} under its unit within {WIDE} s after the stop (old pid {pid}, state {st:?}; the unit was {} and {} before the start):\n{}",
        left.0,
        left.1,
        why(&mut again, &service)
    );
    let out = again.wait();
    assert!(out.contains('X'), "{out}");
    let _ = units.systemctl(&["stop", &service]);
    until(WIDE, || w.state().is_none(), |gone| *gone);
    // install writes the socket unit, and then the first connection starts
    // the service: no broker runs systemd-run.
    let out = paddock(&["device", "install", &name]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(text.contains(&socket_unit), "{text}");
    let unit_file = w
        .root
        .join(format!("cfg/systemd/user/paddock-device-{name}.socket"));
    assert!(unit_file.is_file(), "{}", unit_file.display());
    let unit_text = std::fs::read_to_string(&unit_file).unwrap();
    assert!(
        unit_text.contains(&format!("ListenStream=%t/paddock/devices/{name}.sock")),
        "{unit_text}"
    );
    assert_eq!(
        units.state_of(&socket_unit),
        "active",
        "{}",
        units.status(&socket_unit)
    );
    assert!(
        w.socket().exists(),
        "systemd made no socket at {}",
        w.socket().display()
    );
    let mut by_socket = broker(3);
    let pid = until(WIDE, || w.pid(), |p| *p > 0);
    assert!(
        pid > 0,
        "the socket started nothing within {WIDE} s:\n{}",
        why(&mut by_socket, &service)
    );
    let st = w.state().unwrap();
    assert_eq!(st["unit"], socket_unit, "{st}");
    // The service of the installed unit, not a transient one.
    let frag = units.systemctl(&["show", "-P", "FragmentPath", &service]);
    let frag = String::from_utf8_lossy(&frag.stdout).trim().to_string();
    assert!(
        !frag.is_empty() && !frag.contains("/transient/"),
        "the service runs from {frag:?}"
    );
    let out = by_socket.wait();
    assert!(out.contains("(socket unit)"), "{out}");
    assert!(out.contains('X'), "{out}");
    let out = paddock(&["device", "uninstall", &name]).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{text}");
    assert!(!unit_file.exists(), "the unit is still there");
    let load = units.systemctl(&["show", "-P", "LoadState", &socket_unit]);
    assert_eq!(
        String::from_utf8_lossy(&load.stdout).trim(),
        "not-found",
        "the user manager still has {socket_unit}"
    );
    drop(units);
    true
}
