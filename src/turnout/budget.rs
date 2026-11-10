//! Run admitted work in a transient user service with host-enforced limits.
use super::Job;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn run(
    systemd_run: &Path,
    systemctl: &Path,
    cancelled: &dyn Fn() -> bool,
    executable: &Path,
    name: &str,
    job: &Job,
    extra: &[&str],
) -> Result<()> {
    if cancelled() {
        bail!("job paused");
    }
    let seconds = job
        .max_runtime
        .as_deref()
        .map(crate::timer::parse_duration)
        .transpose()?
        .unwrap_or(1800);
    let memory = job.max_mem.as_deref().unwrap_or("2G");
    let mut command = Command::new(systemd_run);
    command.args(["--user", "--quiet", "--wait", "--pipe"]);
    for key in [
        "HOME",
        "PATH",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ] {
        if let Some(value) = std::env::var_os(key) {
            let mut arg = std::ffi::OsString::from(format!("--setenv={key}="));
            arg.push(value);
            command.arg(arg);
        }
    }
    let mut child = command
        .arg(format!("--unit=paddock-turnout-work-{name}"))
        .arg(format!("--property=RuntimeMaxSec={seconds}s"))
        .arg(format!("--property=MemoryMax={memory}"))
        .arg("--property=TimeoutStopSec=5s")
        .arg(executable)
        .args(["turnout", "worker", name])
        .args(extra)
        .spawn()
        .context("start budgeted turnout work")?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancelled() {
            let _ = Command::new(systemctl)
                .args([
                    "--user",
                    "stop",
                    &format!("paddock-turnout-work-{name}.service"),
                ])
                .status();
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        let unit = format!("paddock-turnout-work-{name}.service");
        let reason = Command::new(systemctl)
            .args(["--user", "show", "--property=Result", "--value", &unit])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .filter(|s| !s.is_empty());
        let _ = Command::new(systemctl)
            .args(["--user", "reset-failed", &unit])
            .status();
        bail!(
            "budgeted work ended with {status}; systemd result: {}",
            reason.as_deref().unwrap_or("unknown")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn passes_limits_to_fake_systemd_run() {
        let d = tempfile::tempdir().unwrap();
        let fake = d.path().join("systemd-run");
        let log = d.path().join("args");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let text = "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\nmax_runtime='45s'\nmax_mem='512M'\n";
        let job = super::super::parse(text).unwrap().remove("x").unwrap();
        run(
            &fake,
            &fake,
            &|| false,
            Path::new("/fake/paddock"),
            "x",
            &job,
            &[],
        )
        .unwrap();
        let args = std::fs::read_to_string(log).unwrap();
        assert!(args.contains("--property=RuntimeMaxSec=45s\n"));
        assert!(args.contains("--property=MemoryMax=512M\n"));
        assert!(args.contains("--property=TimeoutStopSec=5s\n"));
        assert!(args.contains("--unit=paddock-turnout-work-x\n"));
    }

    #[test]
    fn pause_stops_an_active_fake_service() {
        let d = tempfile::tempdir().unwrap();
        let fake = d.path().join("systemd-run");
        let ctl = d.path().join("systemctl");
        let stopped = d.path().join("stopped");
        let started = d.path().join("started");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.02; done\nexit 1\n",
                started.display(),
                stopped.display()
            ),
        )
        .unwrap();
        std::fs::write(&ctl, format!("#!/bin/sh\ntouch '{}'\n", stopped.display())).unwrap();
        for path in [&fake, &ctl] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let job = super::super::parse(
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n",
        )
        .unwrap()
        .remove("x")
        .unwrap();
        let flag = d.path().join("cancelled");
        let start_signal = started.clone();
        let cancel_flag = flag.clone();
        let cancel_thread = std::thread::spawn(move || {
            while !start_signal.exists() {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::fs::write(cancel_flag, "cancelled").unwrap();
        });
        assert!(run(
            &fake,
            &ctl,
            &|| flag.exists(),
            Path::new("/fake/paddock"),
            "x",
            &job,
            &[]
        )
        .is_err());
        cancel_thread.join().unwrap();
        assert!(stopped.exists());
    }
}
