//! The user units that paddock writes: systemd user units on Linux,
//! LaunchAgents on macOS, and the socket that either one passes to the
//! process it starts.
//!
//! Two parts of paddock own such a unit. The gateway executor
//! (`paddock gateway install`, gatewayd/activation.rs) and the shared
//! device instances of `scope = "shared"` (`paddock device install`,
//! devices/shared/install.rs). Both write the same two shapes of file, so
//! the writers, the mark that says the file is paddock's, the socket that
//! systemd or launchd passes, and the two command runners live here.
//!
//! Every file that paddock writes starts with a mark ([`unit_mark`] or
//! [`plist_mark`]). `install` refuses to write over a file without it, and
//! `uninstall` leaves such a file alone, so a unit of the user's own is
//! never lost.
//!
//! The text of a unit and of a property list is platform-neutral code, so
//! its tests run everywhere.

use anyhow::{bail, Result};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

/// How the first line of every unit that paddock wrote begins. [`ours`]
/// looks for this, so a unit written by an older paddock, whose mark named
/// another remove command, still counts as paddock's.
pub const UNIT_MARK_PREFIX: &str = "# Written by paddock";
/// How the first line of every LaunchAgent that paddock wrote begins.
pub const PLIST_MARK_PREFIX: &str = "<!-- Written by paddock";

/// The first line of a systemd unit that paddock wrote, naming the command
/// that deletes it.
pub fn unit_mark(remove: &str) -> String {
    format!("{UNIT_MARK_PREFIX} ({remove} deletes it).")
}

/// The first line of a LaunchAgent that paddock wrote, naming the command
/// that deletes it.
pub fn plist_mark(remove: &str) -> String {
    format!("{PLIST_MARK_PREFIX} ({remove} deletes it). -->")
}

/// `PATH` of a process that launchd starts: launchd gives none.
pub const LAUNCHD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin";

/// The folders of paddock that the user set in the environment of
/// `install`. The process that systemd or launchd starts gets them too, so
/// it reads the same configuration as paddock.
pub const XDG_PASS: &[&str] = &[
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];

/// `~/.config/systemd/user`, where a systemd user unit goes.
pub fn unit_dir(env: &crate::env::HostEnv) -> PathBuf {
    env.config_dir.join("systemd").join("user")
}

/// `~/Library/LaunchAgents/LABEL.plist`.
pub fn plist_file(env: &crate::env::HostEnv, label: &str) -> PathBuf {
    env.home
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist"))
}

/// `gui/UID`, the launchd domain of the user's login session.
pub fn launchd_domain(uid: u32) -> String {
    format!("gui/{uid}")
}

/// XML text, with the five characters that may not stand alone replaced.
pub fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A `.socket` unit that listens on `listen` and starts its service on the
/// first connection. `listen` may use the specifiers of systemd, for
/// example `%t` for the runtime folder. `remove` is the paddock command
/// that deletes the unit, which goes into the mark.
pub fn socket_unit(remove: &str, description: &str, listen: &str) -> String {
    format!(
        "{}\n[Unit]\nDescription={description}\n\n\
         [Socket]\nListenStream={listen}\nSocketMode=0600\nDirectoryMode=0700\n\n\
         [Install]\nWantedBy=sockets.target\n",
        unit_mark(remove)
    )
}

/// A `.service` unit that runs `exec_start` and needs `socket_unit`.
/// `env` are more variables of the process ([`XDG_PASS`]): the user
/// manager does not have the folders that the user set in a shell, so
/// without them the process would read another configuration.
pub fn service_unit(
    remove: &str,
    description: &str,
    socket_unit: &str,
    exec_start: &str,
    env: &[(String, String)],
) -> String {
    let vars: String = env
        .iter()
        .map(|(k, v)| format!("Environment=\"{}\"\n", unit_quote(&format!("{k}={v}"))))
        .collect();
    format!(
        "{}\n[Unit]\nDescription={description}\nRequires={socket_unit}\n\n\
         [Service]\n{vars}ExecStart={exec_start}\n",
        unit_mark(remove)
    )
}

/// The inside of a double-quoted unit value: `%` is a specifier, `\\` and
/// `"` are escapes.
fn unit_quote(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
}

/// One `ProgramArguments` entry and one socket of a LaunchAgent.
pub struct Agent<'a> {
    /// The paddock command that deletes the agent, for the mark.
    pub remove: &'a str,
    pub label: &'a str,
    /// The program and its arguments.
    pub argv: &'a [String],
    /// The key of the socket, which the program passes to
    /// [`passed`].
    pub socket_key: &'a str,
    pub socket: &'a Path,
    pub home: &'a Path,
    /// More variables of the process ([`XDG_PASS`]).
    pub env: &'a [(String, String)],
}

/// The LaunchAgent of `a`: launchd owns the socket, makes it at each login,
/// and starts the program on the first connection. The mark comes first, so
/// the file has no XML declaration: a comment may not come before one.
pub fn plist(a: &Agent<'_>) -> String {
    let s = |p: &Path| xml(&p.display().to_string());
    let args: String = a
        .argv
        .iter()
        .map(|v| format!("\t\t<string>{}</string>\n", xml(v)))
        .collect();
    let more: String = a
        .env
        .iter()
        .map(|(k, v)| {
            format!(
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                xml(k),
                xml(v)
            )
        })
        .collect();
    format!(
        "{}\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\t<string>{}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n\
\t<key>Sockets</key>\n\t<dict>\n\t\t<key>{}</key>\n\t\t<dict>\n\
\t\t\t<key>SockPathName</key>\n\t\t\t<string>{}</string>\n\
\t\t\t<key>SockPathMode</key>\n\t\t\t<integer>384</integer>\n\
\t\t\t<key>SockFamily</key>\n\t\t\t<string>Unix</string>\n\
\t\t\t<key>SockType</key>\n\t\t\t<string>stream</string>\n\
\t\t</dict>\n\t</dict>\n\
\t<key>RunAtLoad</key>\n\t<false/>\n\
\t<key>KeepAlive</key>\n\t<false/>\n\
\t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
\t<key>EnvironmentVariables</key>\n\t<dict>\n\
\t\t<key>HOME</key>\n\t\t<string>{}</string>\n\
\t\t<key>PATH</key>\n\t\t<string>{LAUNCHD_PATH}</string>\n\
{more}\t</dict>\n\
</dict>\n\
</plist>\n",
        plist_mark(a.remove),
        xml(a.label),
        xml(a.socket_key),
        s(a.socket),
        s(a.home),
    )
}

/// The file starts with paddock's mark (a unit or a property list).
pub fn ours(p: &Path) -> bool {
    std::fs::read_to_string(p)
        .map(|t| t.starts_with(UNIT_MARK_PREFIX) || t.starts_with(PLIST_MARK_PREFIX))
        .unwrap_or(false)
}

/// Write the files, or none of them when one is there that paddock did not
/// write.
pub fn write_ours(files: &[(PathBuf, String)]) -> Result<()> {
    for (p, _) in files {
        if p.exists() && !ours(p) {
            bail!(
                "{} exists and paddock did not write it; nothing was changed",
                p.display()
            );
        }
    }
    for (p, text) in files {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, text)?;
    }
    Ok(())
}

/// The variables of [`XDG_PASS`] that the user set, for a unit or a
/// property list.
pub fn xdg_env(env: &crate::env::HostEnv) -> Vec<(String, String)> {
    XDG_PASS
        .iter()
        .filter_map(|k| Some((k.to_string(), env.var(k)?.to_string())))
        .collect()
}

/// The listening socket that systemd passes (`LISTEN_FDS`). `key` is the
/// name of the launchd socket and is not used here: systemd names none.
#[cfg(not(target_os = "macos"))]
pub fn passed(key: &str) -> Result<Option<UnixListener>> {
    let _ = key;
    let pid_ok = std::env::var("LISTEN_PID")
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
        == Some(std::process::id());
    if pid_ok && std::env::var("LISTEN_FDS").ok().as_deref() == Some("1") {
        use std::os::unix::io::FromRawFd;
        // SAFETY: systemd passes the listening socket as fd 3.
        return Ok(Some(unsafe { UnixListener::from_raw_fd(3) }));
    }
    Ok(None)
}

/// The listening socket that launchd passes under `key`. None when launchd
/// did not start this process (`ESRCH`) or has no such socket (`ENOENT`).
#[cfg(target_os = "macos")]
pub fn passed(key: &str) -> Result<Option<UnixListener>> {
    use std::os::unix::io::FromRawFd;
    // <launch.h>, in libSystem.
    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const libc::c_char,
            fds: *mut *mut libc::c_int,
            cnt: *mut libc::size_t,
        ) -> libc::c_int;
    }
    let name = std::ffi::CString::new(key).map_err(|_| anyhow::anyhow!("a NUL in {key:?}"))?;
    let mut fds: *mut libc::c_int = std::ptr::null_mut();
    let mut cnt: libc::size_t = 0;
    // SAFETY: name is a C string; fds and cnt are valid out pointers.
    let rc = unsafe { launch_activate_socket(name.as_ptr(), &mut fds, &mut cnt) };
    match rc {
        0 => {}
        libc::ESRCH | libc::ENOENT => return Ok(None),
        e => {
            return Err(anyhow::Error::new(std::io::Error::from_raw_os_error(e))
                .context(format!("launchd did not pass the socket {key}")))
        }
    }
    let got: Vec<libc::c_int> = if fds.is_null() {
        Vec::new()
    } else {
        // SAFETY: launchd gives an array of cnt descriptors, which the
        // caller frees.
        let v = unsafe { std::slice::from_raw_parts(fds, cnt) }.to_vec();
        unsafe { libc::free(fds.cast()) };
        v
    };
    match got.as_slice() {
        [fd] => Ok(Some(unsafe { UnixListener::from_raw_fd(*fd) })),
        other => {
            for fd in other {
                // SAFETY: each descriptor is ours, and not used again.
                unsafe { libc::close(*fd) };
            }
            bail!("launchd passed {} sockets for {key}, not one", other.len())
        }
    }
}

/// `systemctl --user ARGS`.
#[cfg(not(target_os = "macos"))]
pub fn systemctl(args: &[&str]) -> Result<()> {
    use anyhow::Context as _;
    let st = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()
        .context("cannot run systemctl --user")?;
    if !st.success() {
        bail!("systemctl --user {} failed with {st}", args.join(" "));
    }
    Ok(())
}

/// Make the user manager load the units in `files`, which paddock just
/// wrote: a reload, and a `systemctl --user link` for each file that the
/// manager does not find where paddock wrote it. That happens when
/// `XDG_CONFIG_HOME` of this shell is not the one of the user manager,
/// whose unit folder is the only one it reads. A unit of the same name of
/// the user's own makes the link fail, which is the error.
#[cfg(not(target_os = "macos"))]
pub fn load_units(files: &[PathBuf]) -> Result<()> {
    systemctl(&["daemon-reload"])?;
    let mut linked = false;
    for f in files {
        let Some(name) = f.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if fragment_path(name).is_some_and(|p| same_file(&p, f)) {
            continue;
        }
        let file = f.display().to_string();
        systemctl(&["link", &file])?;
        linked = true;
    }
    if linked {
        systemctl(&["daemon-reload"])?;
    }
    Ok(())
}

/// Undo [`load_units`] before the files go: `disable` removes every link
/// to a unit in the folder of the user manager, the one of `enable` and
/// the one of `link`. Best effort.
#[cfg(not(target_os = "macos"))]
pub fn unload_units(names: &[&str]) {
    for n in names {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", n])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// The file that the user manager loaded `unit` from. None when it has
/// no such unit.
#[cfg(not(target_os = "macos"))]
fn fragment_path(unit: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "show", "-P", "FragmentPath", unit])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !p.is_empty()).then(|| PathBuf::from(p))
}

#[cfg(not(target_os = "macos"))]
fn same_file(a: &Path, b: &Path) -> bool {
    let c = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    c(a) == c(b)
}

/// `systemctl --user is-active --quiet UNIT`.
#[cfg(not(target_os = "macos"))]
pub fn unit_active(unit: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `launchctl ARGS`. Its output goes to paddock's log, not the terminal.
#[cfg(target_os = "macos")]
pub fn launchctl(args: &[&str]) -> Result<()> {
    use anyhow::Context as _;
    let out = std::process::Command::new("/bin/launchctl")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .context("cannot run /bin/launchctl")?;
    if !out.status.success() {
        bail!(
            "launchctl {} failed with {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// True when launchd knows the service `gui/UID/LABEL`.
#[cfg(target_os = "macos")]
pub fn agent_loaded(uid: u32, label: &str) -> bool {
    let service = format!("{}/{label}", launchd_domain(uid));
    std::process::Command::new("/bin/launchctl")
        .args(["print", &service])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Write the LaunchAgent `a` at `file` and load it, replacing an agent of
/// an earlier install.
#[cfg(target_os = "macos")]
pub fn install_agent(uid: u32, file: &Path, a: &Agent<'_>) -> Result<()> {
    write_ours(&[(file.to_path_buf(), plist(a))])?;
    let domain = launchd_domain(uid);
    let service = format!("{domain}/{}", a.label);
    if let Err(e) = launchctl(&["bootout", &service]) {
        log::debug!("{e:#}");
    }
    // A process that paddock started itself holds the socket path until it
    // is idle; launchd takes the path over now.
    let _ = std::fs::remove_file(a.socket);
    let path = file.display().to_string();
    launchctl(&["bootstrap", &domain, &path])?;
    launchctl(&["enable", &service])?;
    Ok(())
}

/// Unload the LaunchAgent `label` and delete the file that paddock wrote.
/// Nothing happens when the file is not paddock's.
#[cfg(target_os = "macos")]
pub fn remove_agent(uid: u32, file: &Path, label: &str, socket: &Path) -> Result<Vec<PathBuf>> {
    if !ours(file) {
        return Ok(Vec::new());
    }
    let service = format!("{}/{label}", launchd_domain(uid));
    if let Err(e) = launchctl(&["bootout", &service]) {
        log::debug!("{e:#}");
    }
    std::fs::remove_file(file)?;
    // launchd leaves its socket file behind.
    let _ = std::fs::remove_file(socket);
    Ok(vec![file.to_path_buf()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_socket_unit_names_its_path_and_its_service() {
        let s = socket_unit(
            "paddock gateway remove",
            "paddock gateway executor socket",
            "%t/paddock/gatewayd.sock",
        );
        assert!(s.starts_with(UNIT_MARK_PREFIX), "{s}");
        assert!(
            s.starts_with("# Written by paddock (paddock gateway remove"),
            "{s}"
        );
        assert!(s.contains("ListenStream=%t/paddock/gatewayd.sock"), "{s}");
        assert!(s.contains("SocketMode=0600"), "{s}");
        assert!(s.contains("WantedBy=sockets.target"), "{s}");
        let svc = service_unit(
            "paddock gateway remove",
            "paddock gateway executor",
            "paddock-gatewayd.socket",
            "/home/demo/.local/bin/paddock gatewayd",
            &[
                (
                    "XDG_CONFIG_HOME".to_string(),
                    "/home/demo/my cfg".to_string(),
                ),
                ("XDG_STATE_HOME".to_string(), "/s/50%\"q".to_string()),
            ],
        );
        assert!(
            svc.contains("Environment=\"XDG_CONFIG_HOME=/home/demo/my cfg\"\n"),
            "{svc}"
        );
        assert!(
            svc.contains("Environment=\"XDG_STATE_HOME=/s/50%%\\\"q\"\n"),
            "{svc}"
        );
        assert!(svc.contains("Requires=paddock-gatewayd.socket"), "{svc}");
        assert!(
            svc.contains("ExecStart=/home/demo/.local/bin/paddock gatewayd"),
            "{svc}"
        );
    }

    #[test]
    fn a_launch_agent_owns_the_socket_and_starts_on_demand() {
        let argv = ["/opt/homebrew/bin/paddock".to_string(), "gatewayd".into()];
        let env = [("XDG_CONFIG_HOME".to_string(), "/Users/demo/cfg".to_string())];
        let p = plist(&Agent {
            remove: "paddock gateway remove",
            label: "com.harnessix.paddock.gatewayd",
            argv: &argv,
            socket_key: "Listeners",
            socket: Path::new("/Users/demo/.local/state/paddock/gatewayd.sock"),
            home: Path::new("/Users/demo"),
            env: &env,
        });
        assert!(p.starts_with(PLIST_MARK_PREFIX), "{p}");
        assert!(!p.contains("<?xml"), "{p}");
        for want in [
            "<key>Label</key>\n\t<string>com.harnessix.paddock.gatewayd</string>",
            "<string>/opt/homebrew/bin/paddock</string>\n\t\t<string>gatewayd</string>",
            "<key>Sockets</key>",
            "<key>Listeners</key>",
            "<key>SockPathName</key>\n\t\t\t<string>/Users/demo/.local/state/paddock/gatewayd.sock</string>",
            "<key>SockPathMode</key>\n\t\t\t<integer>384</integer>",
            "<key>SockFamily</key>\n\t\t\t<string>Unix</string>",
            "<key>RunAtLoad</key>\n\t<false/>",
            "<key>KeepAlive</key>\n\t<false/>",
            "<key>HOME</key>\n\t\t<string>/Users/demo</string>",
            "<key>XDG_CONFIG_HOME</key>\n\t\t<string>/Users/demo/cfg</string>\n\t</dict>",
        ] {
            assert!(p.contains(want), "{want} missing:\n{p}");
        }
        assert_eq!(launchd_domain(501), "gui/501");
    }

    #[test]
    fn xml_characters_in_a_path_stay_one_string() {
        let argv = ["/Users/a&b/paddock".to_string()];
        let p = plist(&Agent {
            remove: "paddock device uninstall x",
            label: "x",
            argv: &argv,
            socket_key: "Listeners",
            socket: Path::new("/s"),
            home: Path::new("/h"),
            env: &[],
        });
        assert!(p.contains("<string>/Users/a&amp;b/paddock</string>"), "{p}");
        assert_eq!(xml("a<b>c\"d'e&f"), "a&lt;b&gt;c&quot;d&apos;e&amp;f");
    }

    #[test]
    fn paddock_writes_over_its_own_files_only() {
        let t = tempfile::tempdir().unwrap();
        let mine = t.path().join("LaunchAgents").join("x.plist");
        let argv = ["/p".to_string()];
        let a = Agent {
            remove: "paddock device uninstall x",
            label: "x",
            argv: &argv,
            socket_key: "Listeners",
            socket: Path::new("/s"),
            home: Path::new("/h"),
            env: &[],
        };
        write_ours(&[(mine.clone(), plist(&a))]).unwrap();
        assert!(ours(&mine));
        // Again: paddock's own file is replaced.
        write_ours(&[(mine.clone(), plist(&a))]).unwrap();
        let unit = t.path().join("x.socket");
        write_ours(&[(
            unit.clone(),
            socket_unit("paddock gateway remove", "d", "/s"),
        )])
        .unwrap();
        assert!(ours(&unit));
        // A file of the user: refused, and the other file is not written.
        let foreign = t.path().join("foreign.plist");
        std::fs::write(&foreign, "<?xml version=\"1.0\"?>\n<plist/>\n").unwrap();
        let other = t.path().join("other");
        let e =
            write_ours(&[(other.clone(), "x".into()), (foreign.clone(), "y".into())]).unwrap_err();
        assert!(
            format!("{e:#}").contains("paddock did not write it"),
            "{e:#}"
        );
        assert!(!other.exists());
        assert!(!ours(&foreign));
        assert_eq!(
            std::fs::read_to_string(&foreign).unwrap(),
            "<?xml version=\"1.0\"?>\n<plist/>\n"
        );
    }
}
