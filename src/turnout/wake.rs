//! Serialize wakes and evaluate local triggers.
use super::{check, git, Job};
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::Path;

struct Lock(File);
impl Lock {
    fn take(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        let result = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            Ok(Some(Self(f)))
        } else {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(err.into())
            }
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn pending_path(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(format!("pending-{name}"))
}

struct Pending(std::path::PathBuf);
impl Pending {
    fn new(dir: &Path, name: &str) -> Result<Self> {
        let path = pending_path(dir, name);
        crate::privfs::write_atomic(&path, b"deferred\n")?;
        Ok(Self(path))
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Hold the job lock from trigger evaluation until the work is complete.
pub fn run<T>(
    state_dir: &Path,
    name: &str,
    job: &Job,
    now: u64,
    load: f64,
    work: impl FnOnce(Option<&str>, &super::pause::Generation) -> Result<T>,
) -> Result<Option<T>> {
    run_with_retry(state_dir, name, job, (now, load), || Ok(None), work)
}

/// A retry supplies a new clock and load after a deferred calendar wake.
pub fn run_with_retry<T>(
    state_dir: &Path,
    name: &str,
    job: &Job,
    initial: (u64, f64),
    mut retry: impl FnMut() -> Result<Option<(u64, f64)>>,
    work: impl FnOnce(Option<&str>, &super::pause::Generation) -> Result<T>,
) -> Result<Option<T>> {
    let Some(_lock) = Lock::take(&state_dir.join(format!("{name}.lock")))? else {
        return Ok(None);
    };
    if super::pause::paused(state_dir, name) {
        return Ok(None);
    }
    let generation = super::pause::generation(state_dir, name);
    let (mut now, mut load) = initial;
    // The marker survives a killed service and is picked up on the next
    // user-systemd startup. A normal return clears it.
    let existing = pending_path(state_dir, name);
    let mut pending = existing.exists().then(|| Pending(existing));
    loop {
        if super::pause::cancelled(state_dir, name, &generation) {
            return Ok(None);
        }
        let quiet = if let Some(quiet) = &job.quiet_hours {
            super::limits::is_quiet(
                super::limits::quiet_range(quiet)?,
                super::limits::local_minute(now)?,
            )
        } else {
            false
        };
        if !quiet && job.max_load.is_none_or(|max| load <= max) {
            break;
        }
        if job.trigger == "timer" && pending.is_none() {
            pending = Some(Pending::new(state_dir, name)?);
        }
        match retry()? {
            Some(next) => (now, load) = next,
            None => return Ok(None),
        }
    }
    let path = state_dir.join(format!("{name}.git.json"));
    let head = if job.trigger == "git" {
        let interval = job
            .poll_interval
            .as_deref()
            .map(crate::timer::parse_duration)
            .transpose()?
            .unwrap_or(60);
        let project = job.project.as_deref().context("missing project")?;
        let branch = job.branch.as_deref().context("missing branch")?;
        let Some(head) = git::poll(&path, Path::new(project), branch, interval, now)? else {
            return Ok(None);
        };
        Some(head)
    } else {
        None
    };
    let started = std::cell::Cell::new(false);
    let result = check::gate_cancel(
        job,
        || super::pause::cancelled(state_dir, name, &generation),
        || {
            if super::pause::cancelled(state_dir, name, &generation) {
                anyhow::bail!("job paused");
            }
            started.set(true);
            work(head.as_deref(), &generation)
        },
    );
    if started.get() && !super::pause::cancelled(state_dir, name, &generation) {
        if let Some(head) = head {
            git::complete(&path, &head)?;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_and_lock_gate_the_wake() {
        let d = tempfile::tempdir().unwrap();
        let text =
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n";
        let mut job = super::super::parse(text).unwrap().remove("x").unwrap();
        assert_eq!(
            run(d.path(), "x", &job, 10, 0.0, |_, _| Ok(1)).unwrap(),
            Some(1)
        );
        job.check = "exit 1".into();
        assert_eq!(
            run(d.path(), "x", &job, 11, 0.0, |_, _| panic!("must not run")).unwrap(),
            None::<()>
        );
        let _lock = Lock::take(&d.path().join("x.lock")).unwrap().unwrap();
        job.check = "echo work".into();
        assert_eq!(
            run(d.path(), "x", &job, 12, 0.0, |_, _| panic!(
                "already running"
            ))
            .unwrap(),
            None::<()>
        );
    }

    #[test]
    fn pause_and_load_defer_before_the_check() {
        let d = tempfile::tempdir().unwrap();
        let text = "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\nmax_load=2.0\n";
        let mut job = super::super::parse(text).unwrap().remove("x").unwrap();
        assert_eq!(
            run(d.path(), "x", &job, 1, 3.0, |_, _| panic!("load")).unwrap(),
            None::<()>
        );
        std::fs::write(d.path().join("paused-all"), "paused").unwrap();
        assert_eq!(
            run(d.path(), "x", &job, 1, 0.0, |_, _| panic!("paused")).unwrap(),
            None::<()>
        );
        std::fs::remove_file(d.path().join("paused-all")).unwrap();
        let minute = super::super::limits::local_minute(1).unwrap();
        let end = (minute + 1) % 1440;
        job.quiet_hours = Some(format!(
            "{:02}:{:02}-{:02}:{:02}",
            minute / 60,
            minute % 60,
            end / 60,
            end % 60
        ));
        assert_eq!(
            run(d.path(), "x", &job, 1, 0.0, |_, _| panic!("quiet hours")).unwrap(),
            None::<()>
        );
    }

    #[test]
    fn a_quick_resume_does_not_revive_an_inflight_check() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let ctl = d.path().join("systemctl");
        std::fs::write(&ctl, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = d.path().join("started");
        let mut job = super::super::parse(
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n",
        )
        .unwrap()
        .remove("x")
        .unwrap();
        job.check = format!("touch '{}'; sleep 2; echo work", started.display());
        std::thread::scope(|scope| {
            let handle =
                scope.spawn(|| run(d.path(), "x", &job, 1, 0.0, |_, _| panic!("cancelled work")));
            while !started.exists() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            super::super::pause::pause(d.path(), &["x".into()], false, &ctl).unwrap();
            super::super::pause::resume(d.path(), Some("x")).unwrap();
            assert_eq!(handle.join().unwrap().unwrap(), None::<()>);
        });
    }

    #[test]
    fn a_calendar_wake_waits_for_quiet_hours_and_load_to_clear() {
        let d = tempfile::tempdir().unwrap();
        let mut job = super::super::parse("[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\nmax_load=2.0\n").unwrap().remove("x").unwrap();
        let now = 1_800_000_000_u64;
        let minute = super::super::limits::local_minute(now).unwrap();
        let end = (minute + 1) % 1440;
        job.quiet_hours = Some(format!(
            "{:02}:{:02}-{:02}:{:02}",
            minute / 60,
            minute % 60,
            end / 60,
            end % 60
        ));
        let mut retries = 0;
        let result = run_with_retry(
            d.path(),
            "x",
            &job,
            (now, 3.0),
            || {
                retries += 1;
                Ok(Some((
                    now + 60 * retries,
                    if retries == 1 { 3.0 } else { 1.0 },
                )))
            },
            |_, _| Ok("worked"),
        )
        .unwrap();
        assert_eq!(result, Some("worked"));
        assert_eq!(retries, 2);
    }

    #[test]
    fn an_interrupted_deferred_wake_can_be_recovered() {
        let d = tempfile::tempdir().unwrap();
        let job = super::super::parse(
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n",
        )
        .unwrap()
        .remove("x")
        .unwrap();
        let pending = Pending::new(d.path(), "x").unwrap();
        std::mem::forget(pending); // Model a process killed before its cleanup.
        assert!(pending_path(d.path(), "x").exists());
        assert_eq!(
            run(d.path(), "x", &job, 1, 0.0, |_, _| Ok(7)).unwrap(),
            Some(7)
        );
        assert!(!pending_path(d.path(), "x").exists());
    }

    #[test]
    fn pausing_a_deferred_wake_clears_its_recovery_marker() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let ctl = d.path().join("systemctl");
        std::fs::write(&ctl, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let job = super::super::parse("[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\nmax_load=1.0\n").unwrap().remove("x").unwrap();
        let result = run_with_retry(
            d.path(),
            "x",
            &job,
            (1, 2.0),
            || {
                assert!(pending_path(d.path(), "x").exists());
                super::super::pause::pause(d.path(), &["x".into()], false, &ctl)?;
                super::super::pause::resume(d.path(), Some("x"))?;
                Ok(Some((2, 0.0)))
            },
            |_, _| panic!("cancelled work"),
        )
        .unwrap();
        assert_eq!(result, None::<()>);
        assert!(!pending_path(d.path(), "x").exists());
    }
}
