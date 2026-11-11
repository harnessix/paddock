//! User timer units. The executable and systemctl are injectable for tests.

use super::Job;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

fn quote(s: &str) -> Result<String> {
    if s.contains(['\n', '\r', '\0']) {
        bail!("unit value contains a control character");
    }
    Ok(format!(
        "\"{}\"",
        s.replace('%', "%%")
            .replace('$', "$$")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    ))
}

pub fn render(name: &str, job: &Job, executable: &Path) -> Result<(String, String)> {
    if !super::valid_name(name) {
        bail!("invalid turnout job name");
    }
    let exe = quote(
        executable
            .to_str()
            .context("executable path is not UTF-8")?,
    )?;
    let name = quote(name)?;
    let service = format!("[Unit]\nDescription=Paddock turnout job\n[Service]\nType=oneshot\nExecStart={exe} turnout run {name}\n");
    let timing = if job.trigger == "timer" {
        let schedule = job.schedule.as_deref().context("missing schedule")?;
        if schedule.contains(['\n', '\r', '\0']) {
            bail!("invalid schedule");
        }
        format!("OnCalendar={schedule}\n")
    } else {
        let seconds = job
            .poll_interval
            .as_deref()
            .map(crate::timer::parse_duration)
            .transpose()?
            .unwrap_or(60);
        if seconds < 60 {
            bail!("git poll interval must be at least 60 seconds");
        }
        format!("OnBootSec={seconds}s\nOnUnitActiveSec={seconds}s\n")
    };
    let jitter = job
        .jitter
        .as_deref()
        .map(crate::timer::parse_duration)
        .transpose()?
        .unwrap_or(60);
    let timer = format!("[Unit]\nDescription=Paddock turnout timer\n[Timer]\n{timing}RandomizedDelaySec={jitter}s\nPersistent=true\n[Install]\nWantedBy=timers.target\n");
    Ok((service, timer))
}

fn systemctl(program: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .arg("--user")
        .args(args)
        .status()
        .context("run systemctl")?;
    if !status.success() {
        bail!("systemctl {} failed", args.join(" "));
    }
    Ok(())
}

pub fn install(dir: &Path, systemctl_bin: &Path, exe: &Path, name: &str, job: &Job) -> Result<()> {
    let (service, timer) = render(name, job, exe)?;
    std::fs::create_dir_all(dir)?;
    let prefix = format!("paddock-turnout-{name}");
    let recovery = format!("paddock-turnout-recovery@{name}.service");
    let recovery_path = dir.join(&recovery);
    if job.trigger == "timer" {
        let executable = quote(exe.to_str().context("executable path is not UTF-8")?)?;
        let name = quote(name)?;
        std::fs::write(&recovery_path, format!("[Unit]\nDescription=Recover a deferred turnout wake\n[Service]\nType=oneshot\nExecStart={executable} turnout retry {name}\n[Install]\nWantedBy=default.target\n"))?;
    } else if recovery_path.exists() {
        systemctl(systemctl_bin, &["disable", "--now", &recovery])?;
        std::fs::remove_file(&recovery_path)?;
    }
    std::fs::write(dir.join(format!("{prefix}.service")), service)?;
    std::fs::write(dir.join(format!("{prefix}.timer")), timer)?;
    systemctl(systemctl_bin, &["daemon-reload"])?;
    if job.trigger == "timer" {
        systemctl(systemctl_bin, &["enable", &recovery])?;
    }
    systemctl(
        systemctl_bin,
        &["enable", "--now", &format!("{prefix}.timer")],
    )
}

pub fn remove(dir: &Path, systemctl_bin: &Path, name: &str) -> Result<()> {
    if !super::valid_name(name) {
        bail!("invalid turnout job name");
    }
    let prefix = format!("paddock-turnout-{name}");
    let timer = dir.join(format!("{prefix}.timer"));
    let recovery = format!("paddock-turnout-recovery@{name}.service");
    if dir.join(&recovery).exists() {
        systemctl(systemctl_bin, &["disable", "--now", &recovery])?;
        std::fs::remove_file(dir.join(recovery))?;
    }
    if timer.exists() {
        systemctl(
            systemctl_bin,
            &["disable", "--now", &format!("{prefix}.timer")],
        )?;
    }
    for ext in ["timer", "service"] {
        match std::fs::remove_file(dir.join(format!("{prefix}.{ext}"))) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    systemctl(systemctl_bin, &["daemon-reload"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn timer_and_git_units_use_fake_systemctl_and_remove_twice() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tmp.path().join("systemctl");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let text = "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\njitter='5m'\n";
        let mut job = super::super::parse(text).unwrap().remove("x").unwrap();
        let dir = tmp.path().join("units");
        for _ in 0..2 {
            install(&dir, &fake, Path::new("/tmp/paddock"), "x", &job).unwrap();
        }
        let timer = std::fs::read_to_string(dir.join("paddock-turnout-x.timer")).unwrap();
        let recovery =
            std::fs::read_to_string(dir.join("paddock-turnout-recovery@x.service")).unwrap();
        assert!(recovery.contains("turnout retry \"x\""));
        assert!(recovery.contains("WantedBy=default.target"));
        assert!(timer.contains("OnCalendar=daily\n"));
        assert!(timer.contains("RandomizedDelaySec=300s\nPersistent=true"));
        job.trigger = "git".into();
        job.poll_interval = Some("90s".into());
        let (_, timer) = render("x", &job, Path::new("/tmp/paddock")).unwrap();
        assert!(timer.contains("OnUnitActiveSec=90s"));
        install(&dir, &fake, Path::new("/tmp/paddock"), "x", &job).unwrap();
        assert!(!dir.join("paddock-turnout-recovery@x.service").exists());
        job.poll_interval = Some("59s".into());
        assert!(render("x", &job, Path::new("/tmp/paddock")).is_err());
        for _ in 0..2 {
            remove(&dir, &fake, "x").unwrap();
        }
        assert!(!dir.join("paddock-turnout-x.timer").exists());
        assert!(!dir.join("paddock-turnout-recovery@x.service").exists());
    }
}
