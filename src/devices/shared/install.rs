//! `paddock device install|uninstall NAME`: hand the shared instance of a
//! device to the user manager, as `paddock gateway install` does for the
//! executor.
//!
//! Linux: two systemd user units. The `.socket` unit owns
//! `$XDG_RUNTIME_DIR/paddock/devices/NAME.sock` and starts the `.service`
//! unit on the first connection, so no broker ever runs `systemd-run` and
//! two brokers that start at once cannot start two instances.
//!
//! macOS: one LaunchAgent with a `Sockets` entry. launchd owns the socket,
//! keeps it when the job stops, makes it again at each login, and starts
//! `paddock deviced NAME` on the first connection, which takes the socket
//! with `launch_activate_socket` (units.rs).
//!
//! Without `install`, the first broker starts the instance itself:
//! `systemd-run --user` when a user manager runs, else a child of its own
//! paddock process, which ends with that run (shared.rs).

use crate::env::HostEnv;
use crate::units;
use anyhow::{bail, Result};
use std::path::PathBuf;

/// The socket key of the LaunchAgent, for `launch_activate_socket`.
pub const SOCKET_KEY: &str = "Listeners";

/// The command that deletes the unit, for its mark.
pub fn remove_cmd(name: &str) -> String {
    format!("paddock device uninstall {name}")
}

/// `paddock-device-NAME`: the name of the transient unit and the stem of
/// the installed units.
pub fn service_name(name: &str) -> String {
    format!("paddock-device-{name}")
}

/// `paddock-device-NAME.socket`.
pub fn socket_unit_name(name: &str) -> String {
    format!("{}.socket", service_name(name))
}

/// `paddock-device-NAME.service`.
pub fn service_unit_name(name: &str) -> String {
    format!("{}.service", service_name(name))
}

/// The label of the LaunchAgent of the shared instance of `name`.
pub fn label(name: &str) -> String {
    format!("com.harnessix.paddock.device.{name}")
}

/// The files that `install` writes on this host.
pub fn files(env: &HostEnv, name: &str) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![units::plist_file(env, &label(name))]
    } else {
        let dir = units::unit_dir(env);
        vec![
            dir.join(socket_unit_name(name)),
            dir.join(service_unit_name(name)),
        ]
    }
}

/// A unit or a LaunchAgent of paddock's is in place for `name`.
pub fn installed(env: &HostEnv, name: &str) -> bool {
    files(env, name).first().is_some_and(|f| units::ours(f))
}

/// The text of the two systemd user units: the socket that systemd owns
/// and the service that it starts on the first connection.
/// `env` are the variables of the instance ([`super::unit_env`]), so it
/// reads the configuration that `install` read.
pub fn unit_texts(
    name: &str,
    paddock: &std::path::Path,
    env: &[(String, String)],
) -> Vec<(String, String)> {
    let remove = remove_cmd(name);
    let socket = units::socket_unit(
        &remove,
        &format!("paddock shared device {name} socket"),
        &super::listen_stream(name),
    );
    let service = units::service_unit(
        &remove,
        &format!("paddock shared device {name}"),
        &socket_unit_name(name),
        &format!(
            "{} deviced {name} --socket {} --unit {}",
            paddock.display(),
            super::listen_stream(name),
            socket_unit_name(name)
        ),
        env,
    );
    vec![
        (socket_unit_name(name), socket),
        (service_unit_name(name), service),
    ]
}

/// Write the units and turn the socket on.
#[cfg(not(target_os = "macos"))]
pub fn install(env: &HostEnv, name: &str) -> Result<Vec<PathBuf>> {
    crate::update::refuse_in_sandbox(env, "paddock device install")?;
    let paddock = crate::launchers::launcher_exe(env)?;
    let dir = units::unit_dir(env);
    let files: Vec<(PathBuf, String)> = unit_texts(name, &paddock, &super::unit_env(env))
        .into_iter()
        .map(|(n, text)| (dir.join(n), text))
        .collect();
    units::write_ours(&files)?;
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
    units::load_units(&paths)?;
    // An instance that a broker started itself holds the socket path;
    // systemd takes the path over now.
    if let Err(e) = super::stop_instance(env, name) {
        log::debug!("device {name}: {e}");
    }
    let _ = std::fs::remove_file(super::socket_path(env, name));
    units::systemctl(&["enable", "--now", &socket_unit_name(name)])?;
    Ok(files.into_iter().map(|(p, _)| p).collect())
}

/// Turn the socket off and remove the units that paddock wrote.
#[cfg(not(target_os = "macos"))]
pub fn uninstall(env: &HostEnv, name: &str) -> Result<Vec<PathBuf>> {
    let dir = units::unit_dir(env);
    let socket = socket_unit_name(name);
    let mut removed = Vec::new();
    if units::ours(&dir.join(&socket)) {
        units::unload_units(&[&socket, &service_unit_name(name)]);
    }
    for n in [socket, service_unit_name(name)] {
        let p = dir.join(&n);
        if units::ours(&p) {
            std::fs::remove_file(&p)?;
            removed.push(p);
        }
    }
    if !removed.is_empty() {
        let _ = units::systemctl(&["daemon-reload"]);
        let _ = std::fs::remove_file(super::socket_path(env, name));
    }
    Ok(removed)
}

/// One line for `paddock device show` and `paddock doctor`.
#[cfg(not(target_os = "macos"))]
pub fn status_line(env: &HostEnv, name: &str) -> String {
    let unit = socket_unit_name(name);
    if !installed(env, name) {
        return format!(
            "the first request starts the shared instance ({}). `paddock device install {name}` hands the socket to systemd",
            super::start_kind(env, name).name()
        );
    }
    if units::unit_active(&unit) {
        format!(
            "systemd owns the socket and starts the shared instance on the first request ({unit})"
        )
    } else {
        format!("{unit} is installed but not active: systemctl --user enable --now {unit}")
    }
}

/// Write the LaunchAgent and load it. launchd makes the socket now and at
/// each login.
#[cfg(target_os = "macos")]
pub fn install(env: &HostEnv, name: &str) -> Result<Vec<PathBuf>> {
    crate::update::refuse_in_sandbox(env, "paddock device install")?;
    let paddock = crate::launchers::launcher_exe(env)?;
    let socket = super::socket_path(env, name);
    if let Some(d) = socket.parent() {
        crate::privfs::dir(d)?;
    }
    // An instance that a broker started itself holds the socket path;
    // launchd takes the path over now.
    if let Err(e) = super::stop_instance(env, name) {
        log::debug!("device {name}: {e}");
    }
    let file = units::plist_file(env, &label(name));
    let extra = super::unit_env(env);
    let argv: Vec<String> = vec![
        paddock.display().to_string(),
        "deviced".to_string(),
        name.to_string(),
        "--socket".to_string(),
        socket.display().to_string(),
    ];
    let remove = remove_cmd(name);
    let lab = label(name);
    units::install_agent(
        env.uid,
        &file,
        &units::Agent {
            remove: &remove,
            label: &lab,
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
pub fn uninstall(env: &HostEnv, name: &str) -> Result<Vec<PathBuf>> {
    units::remove_agent(
        env.uid,
        &units::plist_file(env, &label(name)),
        &label(name),
        &super::socket_path(env, name),
    )
}

/// One line for `paddock device show` and `paddock doctor`.
#[cfg(target_os = "macos")]
pub fn status_line(env: &HostEnv, name: &str) -> String {
    let lab = label(name);
    if !installed(env, name) {
        return format!(
            "the first request starts the shared instance ({}). `paddock device install {name}` hands the socket to launchd",
            super::start_kind(env, name).name()
        );
    }
    if units::agent_loaded(env.uid, &lab) {
        format!(
            "launchd owns the socket and starts the shared instance on the first request ({lab})"
        )
    } else {
        format!("{lab} is installed but not loaded: paddock device install {name}")
    }
}

/// `install` and `uninstall` are for shared devices only: a `run` instance
/// lives and dies with its sandbox and needs no unit.
pub fn check_shared(def: &super::super::config::DeviceDef) -> Result<()> {
    if def.scope != super::super::config::Scope::Shared {
        bail!(
            "[devices.{}]: scope = \"{}\". Only a shared device has an instance of its own to install; a run instance ends with its sandbox. Set scope = \"shared\" to install one",
            def.name,
            def.scope.name()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_units_name_the_socket_and_the_instance() {
        let texts = unit_texts(
            "mac",
            Path::new("/home/demo/.local/bin/paddock"),
            &[("XDG_CONFIG_HOME".to_string(), "/home/demo/cfg".to_string())],
        );
        assert_eq!(texts[0].0, "paddock-device-mac.socket");
        assert_eq!(texts[1].0, "paddock-device-mac.service");
        let socket = &texts[0].1;
        assert!(
            socket.contains("ListenStream=%t/paddock/devices/mac.sock"),
            "{socket}"
        );
        assert!(socket.contains("SocketMode=0600"), "{socket}");
        assert!(
            socket.starts_with("# Written by paddock (paddock device uninstall mac deletes it)."),
            "{socket}"
        );
        let service = &texts[1].1;
        assert!(
            service.contains("Requires=paddock-device-mac.socket"),
            "{service}"
        );
        assert!(
            service.contains(
                "ExecStart=/home/demo/.local/bin/paddock deviced mac --socket %t/paddock/devices/mac.sock --unit paddock-device-mac.socket"
            ),
            "{service}"
        );
        assert!(
            service.contains("Environment=\"XDG_CONFIG_HOME=/home/demo/cfg\""),
            "{service}"
        );
    }

    #[test]
    fn the_launch_agent_of_a_shared_device_has_its_socket() {
        let socket = Path::new("/tmp/paddock-501/dev-mac.sock");
        let argv: Vec<String> = ["/opt/homebrew/bin/paddock", "deviced", "mac", "--socket"]
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once(socket.display().to_string()))
            .collect();
        let remove = remove_cmd("mac");
        let lab = label("mac");
        let p = units::plist(&units::Agent {
            remove: &remove,
            label: &lab,
            argv: &argv,
            socket_key: SOCKET_KEY,
            socket,
            home: Path::new("/Users/demo"),
            env: &[],
        });
        assert_eq!(lab, "com.harnessix.paddock.device.mac");
        for want in [
            "<key>Label</key>\n\t<string>com.harnessix.paddock.device.mac</string>",
            "<string>deviced</string>\n\t\t<string>mac</string>",
            "<key>Sockets</key>",
            "<key>Listeners</key>",
            "<key>SockPathName</key>\n\t\t\t<string>/tmp/paddock-501/dev-mac.sock</string>",
            "<key>RunAtLoad</key>\n\t<false/>",
        ] {
            assert!(p.contains(want), "{want} missing:\n{p}");
        }
    }

    #[test]
    fn install_is_for_shared_devices_only() {
        let def = crate::devices::DevicesCfg::parse(
            "[devices.d]\nshape = \"exec\"\ncommand = [\"./d\"]\nenabled = true\n",
        )
        .unwrap()
        .user
        .remove(0);
        let e = check_shared(&def).unwrap_err();
        assert!(format!("{e:#}").contains("Only a shared device"), "{e:#}");
        let shared = crate::devices::DevicesCfg::parse(
            "[devices.d]\nshape = \"exec\"\ncommand = [\"./d\"]\nenabled = true\nscope = \"shared\"\n",
        )
        .unwrap()
        .user
        .remove(0);
        assert!(check_shared(&shared).is_ok());
    }
}
