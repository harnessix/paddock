//! Socket activation of the executor (`paddock gateway install|remove|status`).
//!
//! Linux: two systemd user units. systemd owns the socket and starts
//! `paddock gatewayd` on the first connection (`LISTEN_FDS`).
//!
//! macOS: one LaunchAgent with a `Sockets` entry. launchd owns the socket
//! and starts `paddock gatewayd` on the first connection, which takes the
//! socket with `launch_activate_socket`.
//!
//! Without `install`, paddock starts the executor itself.
//!
//! The writers themselves are in units.rs, which the shared device
//! instances of `paddock device install` use too.

use crate::env::HostEnv;
use crate::units;
use anyhow::Result;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

pub const SOCKET_UNIT: &str = "paddock-gatewayd.socket";
pub const SERVICE_UNIT: &str = "paddock-gatewayd.service";
/// The command that deletes the units and the LaunchAgent, for their mark.
pub const REMOVE_CMD: &str = "paddock gateway remove";

/// The label of the LaunchAgent.
pub const LABEL: &str = "com.harnessix.paddock.gatewayd";
/// The socket key of the LaunchAgent, for `launch_activate_socket`.
pub const SOCKET_KEY: &str = "Listeners";

pub fn unit_dir(env: &HostEnv) -> PathBuf {
    units::unit_dir(env)
}

pub fn socket_unit() -> String {
    units::socket_unit(
        REMOVE_CMD,
        "paddock gateway executor socket",
        "%t/paddock/gatewayd.sock",
    )
}

/// `extra` are more variables of the executor ([`units::XDG_PASS`]).
pub fn service_unit(paddock: &std::path::Path, extra: &[(String, String)]) -> String {
    units::service_unit(
        REMOVE_CMD,
        "paddock gateway executor",
        SOCKET_UNIT,
        &format!("{} gatewayd", paddock.display()),
        extra,
    )
}

/// `~/Library/LaunchAgents/com.harnessix.paddock.gatewayd.plist`.
pub fn plist_file(env: &HostEnv) -> PathBuf {
    units::plist_file(env, LABEL)
}

/// The LaunchAgent of the executor. `extra` are more variables of the
/// executor ([`units::XDG_PASS`]).
pub fn plist(
    paddock: &std::path::Path,
    socket: &std::path::Path,
    home: &std::path::Path,
    extra: &[(String, String)],
) -> String {
    let argv = [paddock.display().to_string(), "gatewayd".to_string()];
    units::plist(&units::Agent {
        remove: REMOVE_CMD,
        label: LABEL,
        argv: &argv,
        socket_key: SOCKET_KEY,
        socket,
        home,
        env: extra,
    })
}

/// The listening socket that systemd or launchd passes.
pub fn passed() -> Result<Option<UnixListener>> {
    units::passed(SOCKET_KEY)
}

/// Write the units and turn the socket on.
#[cfg(not(target_os = "macos"))]
pub fn install(env: &HostEnv) -> Result<Vec<PathBuf>> {
    crate::update::refuse_in_sandbox(env, "paddock gateway install")?;
    let paddock = crate::launchers::launcher_exe(env)?;
    let dir = unit_dir(env);
    let files = [
        (dir.join(SOCKET_UNIT), socket_unit()),
        (
            dir.join(SERVICE_UNIT),
            service_unit(&paddock, &units::xdg_env(env)),
        ),
    ];
    units::write_ours(&files)?;
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
    units::load_units(&paths)?;
    // An executor that paddock started itself holds the socket path until it
    // is idle; systemd takes the path over after that.
    let _ = std::fs::remove_file(super::socket_path(env));
    units::systemctl(&["enable", "--now", SOCKET_UNIT])?;
    Ok(files.into_iter().map(|(p, _)| p).collect())
}

/// Turn the socket off and remove the units that paddock wrote.
#[cfg(not(target_os = "macos"))]
pub fn remove(env: &HostEnv) -> Result<Vec<PathBuf>> {
    let dir = unit_dir(env);
    let mut removed = Vec::new();
    if units::ours(&dir.join(SOCKET_UNIT)) {
        units::unload_units(&[SOCKET_UNIT, SERVICE_UNIT]);
    }
    for name in [SOCKET_UNIT, SERVICE_UNIT] {
        let p = dir.join(name);
        if units::ours(&p) {
            std::fs::remove_file(&p)?;
            removed.push(p);
        }
    }
    if !removed.is_empty() {
        let _ = units::systemctl(&["daemon-reload"]);
    }
    Ok(removed)
}

/// One line for `paddock doctor` and `paddock gateway status`.
#[cfg(not(target_os = "macos"))]
pub fn status_line(env: &HostEnv) -> String {
    let dir = unit_dir(env);
    if !units::ours(&dir.join(SOCKET_UNIT)) {
        return "  ok       the gateway executor starts when a request needs it (paddock gateway install hands this to systemd)".into();
    }
    if units::unit_active(SOCKET_UNIT) {
        "  ok       systemd starts the gateway executor on the first request (socket activation)"
            .into()
    } else {
        format!("  CHECK    {SOCKET_UNIT} is installed but not active: systemctl --user enable --now {SOCKET_UNIT}")
    }
}

/// Write the LaunchAgent and load it. launchd makes the socket now and at
/// each login.
#[cfg(target_os = "macos")]
pub fn install(env: &HostEnv) -> Result<Vec<PathBuf>> {
    crate::update::refuse_in_sandbox(env, "paddock gateway install")?;
    let paddock = crate::launchers::launcher_exe(env)?;
    let socket = super::socket_path(env);
    if let Some(d) = socket.parent() {
        crate::privfs::dir(d)?;
    }
    let file = plist_file(env);
    let extra = units::xdg_env(env);
    let argv = [paddock.display().to_string(), "gatewayd".to_string()];
    units::install_agent(
        env.uid,
        &file,
        &units::Agent {
            remove: REMOVE_CMD,
            label: LABEL,
            argv: &argv,
            socket_key: SOCKET_KEY,
            socket: &socket,
            home: &env.home,
            env: &extra,
        },
    )?;
    Ok(vec![file])
}

/// Unload the LaunchAgent and delete the file that paddock wrote.
#[cfg(target_os = "macos")]
pub fn remove(env: &HostEnv) -> Result<Vec<PathBuf>> {
    units::remove_agent(env.uid, &plist_file(env), LABEL, &super::socket_path(env))
}

/// One line for `paddock doctor` and `paddock gateway status`.
#[cfg(target_os = "macos")]
pub fn status_line(env: &HostEnv) -> String {
    if !units::ours(&plist_file(env)) {
        return "  ok       the gateway executor starts when a request needs it (paddock gateway install hands this to launchd)".into();
    }
    if units::agent_loaded(env.uid, LABEL) {
        "  ok       launchd starts the gateway executor on the first request (socket activation)"
            .into()
    } else {
        format!("  CHECK    {LABEL} is installed but not loaded: paddock gateway install")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_units_name_the_same_socket_as_paddock() {
        let s = socket_unit();
        assert!(s.contains("ListenStream=%t/paddock/gatewayd.sock"));
        assert!(s.contains("SocketMode=0600"));
        let svc = service_unit(Path::new("/home/demo/.local/bin/paddock"), &[]);
        assert!(svc.contains("ExecStart=/home/demo/.local/bin/paddock gatewayd"));
    }

    #[test]
    fn the_launch_agent_of_the_executor_keeps_its_shape() {
        let p = plist(
            Path::new("/opt/homebrew/bin/paddock"),
            Path::new("/Users/demo/.local/state/paddock/gatewayd.sock"),
            Path::new("/Users/demo"),
            &[("XDG_CONFIG_HOME".into(), "/Users/demo/cfg".into())],
        );
        assert!(p.starts_with(units::PLIST_MARK_PREFIX), "{p}");
        for want in [
            "<key>Label</key>\n\t<string>com.harnessix.paddock.gatewayd</string>",
            "<string>/opt/homebrew/bin/paddock</string>\n\t\t<string>gatewayd</string>",
            "<key>Listeners</key>",
            "<key>SockPathName</key>\n\t\t\t<string>/Users/demo/.local/state/paddock/gatewayd.sock</string>",
            "<key>XDG_CONFIG_HOME</key>\n\t\t<string>/Users/demo/cfg</string>\n\t</dict>",
        ] {
            assert!(p.contains(want), "{want} missing:\n{p}");
        }
    }

    /// A unit of an older paddock, whose mark named another command, is
    /// still paddock's: an upgrade can still remove it.
    #[test]
    fn a_mark_of_an_older_paddock_still_counts() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("old.socket");
        std::fs::write(
            &f,
            "# Written by paddock (paddock gateway remove deletes it).\n[Socket]\n",
        )
        .unwrap();
        assert!(units::ours(&f));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn remove_leaves_units_that_paddock_did_not_write() {
        let t = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
        let dir = unit_dir(&env);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SOCKET_UNIT), "[Socket]\nmine\n").unwrap();
        assert!(remove(&env).unwrap().is_empty());
        assert!(dir.join(SOCKET_UNIT).exists());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn remove_leaves_a_launch_agent_that_paddock_did_not_write() {
        let t = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(t.path().join("home"), "u".into(), 1, 1, Default::default());
        let f = plist_file(&env);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "<plist/>\n").unwrap();
        assert!(remove(&env).unwrap().is_empty());
        assert!(f.exists());
    }
}
