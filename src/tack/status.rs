//! `paddock tack status` and the tackd lines of `paddock doctor`: whether
//! tackd is installed, runs, which version, and whether its socket answers.
//! Without tackd, tack works in rootless mode, and the report says what
//! that does not protect.

use super::client;
use super::proto::{Answer, Request, Status};
use super::service::{self, Paths};
use serde::Serialize;
use std::time::Duration;

/// What tack promises without tackd, said the same way everywhere.
pub const ROOTLESS: &str = "rootless mode: tackd is not installed. Values stay out of shells, startup files and agent environments, but a program that gets a value is not protected from other processes of your user. sudo paddock tack setup installs tackd";

#[derive(Debug, Serialize)]
pub struct State {
    /// The service file that setup wrote is there.
    pub installed: bool,
    /// A service file is there that paddock did not write.
    pub foreign: bool,
    pub binary: bool,
    pub socket: String,
    /// tackd's own status, or why it did not answer.
    pub running: Result<Status, String>,
}

pub fn read(p: &Paths, timeout: Duration) -> State {
    let running = match client::ask(&p.socket, Request::Status {}, timeout) {
        Ok(Answer::Status(s)) => Ok(s),
        Ok(_) => Err("tackd answered a status request with something else".into()),
        Err(e) => Err(e.to_string()),
    };
    State {
        installed: service::ours(&p.unit),
        foreign: p.unit.exists() && !service::ours(&p.unit),
        binary: p.binary.exists(),
        socket: p.socket.display().to_string(),
        running,
    }
}

/// How to look at the service's log on this platform.
fn log_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "sudo launchctl print system/io.paddock.tackd; the log is /var/log/paddock/tackd.err"
    } else {
        "systemctl status paddock-tackd; journalctl -u paddock-tackd"
    }
}

/// The report lines, in the doctor format.
pub fn lines(s: &State, ours: &str) -> Vec<String> {
    let mut out = Vec::new();
    if s.foreign {
        out.push("  note     a tackd service file is there that paddock did not write: paddock leaves it alone".into());
    }
    match (&s.running, s.installed) {
        (Ok(st), _) => {
            out.push(format!(
                "  ok       tackd {} runs (pid {}, since {}): {} requests, {} allowed, {} denied, {} refused as malformed",
                st.version,
                st.pid,
                crate::storage::date(st.started),
                st.requests,
                st.allowed,
                st.denied,
                st.errors
            ));
            out.push(format!(
                "  ok       socket {} answers (protocol {:?})",
                s.socket, st.protocols
            ));
            if st.version != ours {
                out.push(format!(
                    "  CHECK    tackd is version {} and this paddock is {ours}: run sudo paddock tack setup to update tackd",
                    st.version
                ));
            }
            if let Some(e) = &st.audit_error {
                out.push(format!(
                    "  error    tackd cannot write its audit log {} ({e}): it allows nothing until it can",
                    st.audit
                ));
            }
            if !s.installed {
                out.push(
                    "  note     tackd answers, but its service file is not where setup puts it"
                        .into(),
                );
            }
        }
        (Err(e), true) => {
            out.push(format!(
                "  error    tackd is installed but does not answer: {e}"
            ));
            out.push(format!("           look at: {}", log_hint()));
            if !s.binary {
                out.push(
                    "  error    the tackd binary is missing: run sudo paddock tack setup".into(),
                );
            }
        }
        (Err(_), false) => out.push(format!("  off      {ROOTLESS}")),
    }
    out
}

/// The lines, with a note when this user's configuration is not where
/// tackd reads it.
pub fn lines_for(env: &crate::env::HostEnv, s: &State, ours: &str) -> Vec<String> {
    let mut out = lines(s, ours);
    if s.installed && env.paddock_config() != env.home.join(".config/paddock") {
        out.push(format!(
            "  note     your configuration is in {} (XDG_CONFIG_HOME); tackd reads ~/.config/paddock/config.toml: link that file there",
            super::show(env, &super::config::file(env))
        ));
    }
    out
}

/// The tackd block of `paddock doctor`.
pub fn doctor_lines(env: &crate::env::HostEnv) -> Vec<String> {
    let st = read(&Paths::system(), Duration::from_secs(3));
    lines_for(env, &st, env!("CARGO_PKG_VERSION"))
}

/// A line for `paddock update` when tackd runs another version, or None.
pub fn update_hint() -> Option<String> {
    let p = Paths::system();
    service::ours(&p.unit).then(|| {
        "tackd still runs the version that it was installed with: run sudo paddock tack setup to update it".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(version: &str) -> Status {
        Status {
            version: version.into(),
            protocols: vec![1],
            pid: 9,
            started: 1_700_000_000,
            requests: 5,
            allowed: 2,
            denied: 2,
            errors: 1,
            audit: "/var/log/paddock/tackd.log".into(),
            audit_error: None,
        }
    }

    fn state(installed: bool, running: Result<Status, String>) -> State {
        State {
            installed,
            foreign: false,
            binary: installed,
            socket: "/run/paddock-tackd/tackd.sock".into(),
            running,
        }
    }

    #[test]
    fn not_installed_reports_the_rootless_mode_plainly() {
        let l = lines(&state(false, Err("no socket".into())), "1.0.0");
        assert_eq!(l.len(), 1);
        assert!(
            l[0].contains("rootless mode") && l[0].contains("not protected"),
            "{l:?}"
        );
    }

    #[test]
    fn a_running_tackd_shows_version_counters_and_socket() {
        let l = lines(&state(true, Ok(st("1.0.0"))), "1.0.0");
        assert!(
            l[0].starts_with("  ok       tackd 1.0.0 runs (pid 9"),
            "{l:?}"
        );
        assert!(l[0].contains("2 allowed, 2 denied, 1 refused"), "{l:?}");
        assert!(l[1].contains("socket /run/paddock-tackd/tackd.sock answers"));
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn an_old_tackd_asks_for_setup_again() {
        let l = lines(&state(true, Ok(st("0.9.0"))), "1.0.0");
        assert!(
            l.iter()
                .any(|x| x.contains("CHECK") && x.contains("sudo paddock tack setup")),
            "{l:?}"
        );
    }

    #[test]
    fn installed_but_silent_points_at_the_service_log() {
        let mut s = state(true, Err("tackd does not answer".into()));
        s.binary = false;
        let l = lines(&s, "1.0.0");
        assert!(l[0].contains("installed but does not answer"), "{l:?}");
        assert!(l[1].contains("look at:"), "{l:?}");
        assert!(l[2].contains("binary is missing"), "{l:?}");
    }

    #[test]
    fn an_audit_log_that_fails_is_an_error() {
        let mut s = st("1.0.0");
        s.audit_error = Some("read-only file system".into());
        let l = lines(&state(true, Ok(s)), "1.0.0");
        assert!(
            l.iter()
                .any(|x| x.contains("error") && x.contains("allows nothing")),
            "{l:?}"
        );
    }

    #[test]
    fn a_configuration_outside_the_default_folder_is_named() {
        let t = tempfile::tempdir().unwrap();
        let mut vars = std::collections::BTreeMap::new();
        vars.insert(
            "XDG_CONFIG_HOME".to_string(),
            t.path().join("cfg").display().to_string(),
        );
        let env =
            crate::env::HostEnv::from_parts(t.path().to_path_buf(), "u".into(), 1000, 1000, vars);
        let l = lines_for(&env, &state(true, Ok(st("1.0.0"))), "1.0.0");
        assert!(l.last().unwrap().contains("XDG_CONFIG_HOME"), "{l:?}");
        let plain = crate::env::HostEnv::from_parts(
            t.path().to_path_buf(),
            "u".into(),
            1000,
            1000,
            Default::default(),
        );
        assert_eq!(
            lines_for(&plain, &state(true, Ok(st("1.0.0"))), "1.0.0").len(),
            2
        );
    }

    #[test]
    fn a_missing_socket_reads_as_not_running() {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::under(t.path());
        let s = read(&p, Duration::from_millis(200));
        assert!(!s.installed && !s.binary);
        let e = s.running.unwrap_err();
        assert!(e.contains("not running"), "{e}");
    }
}
