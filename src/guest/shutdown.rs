//! The shutdown of the guest at the end of a session (`shutdown
//! {password}`), platform-neutral. The agent runs `sudo -S -p '' --
//! /sbin/shutdown -h now` with the guest password on stdin, the same root
//! step that `paddock macvm setup` uses to stop the golden
//! (`configure::shutdown_step`), in a process group that dies at
//! [`SHUTDOWN_TIMEOUT`].
//!
//! Why this way: the old shutdown asked System Events, and macOS asks the
//! person for the Automation permission first, which nobody answers in a
//! session. `shutdown -h now` needs root and no permission of TCC; the
//! password already reaches the agent for `data.prepare` at each start;
//! `sudo -S` reads it from stdin, so it is in no argv and no log. A
//! `launchctl reboot halt` needs root too, and it skips the orderly end
//! of the user processes that `shutdown` gives (SIGTERM, then the sync of
//! the disks). The main loop refuses the request while an app runs, as
//! it refuses every request with the password. Any failure is an error
//! reply, and the host then stops the VM through the framework.

use super::configure::{self, Plan, Ran};
use crate::macvm::text;
use std::time::Duration;

/// How long `sudo shutdown` may run. `shutdown -h now` returns at once
/// when it starts the halt; a `sudo` that waits for anything is killed
/// well inside the 30 s of the request (`proto::timeout`).
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// The command: the root shutdown step of the setup.
pub fn plan() -> Plan {
    configure::argv(&crate::macvm::configure::shutdown_step(), None)
}

/// Run the shutdown with `spawn` (`configure::run_plan` in the agent).
/// `Err` has the text for the reply. The password never goes into the
/// text: sudo prints only its own messages.
pub fn run(
    password: &str,
    spawn: impl FnOnce(&Plan, Option<&str>, Duration) -> std::io::Result<Ran>,
) -> Result<(), String> {
    if password.is_empty() {
        return Err(text::shutdown_no_password());
    }
    let p = plan();
    debug_assert!(p.password_on_stdin);
    let ran = spawn(&p, Some(password), SHUTDOWN_TIMEOUT)
        .map_err(|e| text::cannot_run_command(&p.argv.join(" "), &e.to_string()))?;
    log::info!(
        // not user text
        "shutdown: exit {:?}, timed out {}",
        ran.code,
        ran.timed_out
    );
    if ran.timed_out {
        return Err(text::shutdown_timed_out(SHUTDOWN_TIMEOUT.as_secs()));
    }
    if !ran.ok() {
        return Err(text::shutdown_failed(ran.code, &ran.output));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shutdown_is_sudo_with_the_password_on_stdin_and_a_limit() {
        let p = plan();
        assert_eq!(
            p.argv,
            [
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "--",
                "/sbin/shutdown",
                "-h",
                "now"
            ]
        );
        assert!(p.password_on_stdin);
        let mut seen = None;
        let r = run("pw-123", |plan, stdin, limit| {
            seen = Some((plan.clone(), stdin.map(str::to_string), limit));
            Ok(Ran {
                code: Some(0),
                ..Default::default()
            })
        });
        assert_eq!(r, Ok(()));
        let (plan, stdin, limit) = seen.unwrap();
        assert_eq!(plan, p);
        assert_eq!(stdin.as_deref(), Some("pw-123"));
        assert_eq!(limit, SHUTDOWN_TIMEOUT);
        assert!(!plan.argv.iter().any(|a| a.contains("pw-123")));
        let request = crate::macvm::proto::Request::Shutdown {
            password: "x".into(),
        };
        assert!(SHUTDOWN_TIMEOUT < crate::macvm::proto::timeout(&request));
    }

    #[test]
    fn no_password_runs_nothing() {
        let r = run("", |_, _, _| panic!("ran without a password"));
        assert!(r.unwrap_err().contains("no guest password"));
    }

    #[test]
    fn a_refused_password_a_hang_and_a_spawn_error_are_errors() {
        let r = run("pw", |_, _, _| {
            Ok(Ran {
                code: Some(1),
                timed_out: false,
                output: "Sorry, try again.\nsudo: 1 incorrect password attempt".into(),
            })
        });
        let e = r.unwrap_err();
        assert!(
            e.contains("exit 1") && e.contains("incorrect password"),
            "{e}"
        );
        assert!(!e.contains("pw;"), "{e}");
        let r = run("pw", |_, _, _| {
            Ok(Ran {
                code: None,
                timed_out: true,
                output: String::new(),
            })
        });
        assert!(r.unwrap_err().contains("15 s"));
        let r = run("pw", |_, _, _| Err(std::io::Error::other("no sudo")));
        assert!(r.unwrap_err().contains("no sudo"));
    }

    #[test]
    fn a_real_hang_is_killed_at_the_limit() {
        // The runner itself, with a command that waits as a sudo on a
        // prompt would: the limit holds.
        let p = Plan {
            argv: vec!["/bin/sh".into(), "-c".into(), "read x; sleep 30".into()],
            password_on_stdin: true,
        };
        let t = std::time::Instant::now();
        let r = configure::run_plan(&p, Some("pw"), Duration::from_millis(300)).unwrap();
        assert!(r.timed_out, "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    }
}
