//! The local, model-free gate before a wake.

use anyhow::{Context, Result};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Call the work only after a successful check; a failed check is silent.
pub fn gate<T>(job: &super::Job, work: impl FnOnce() -> Result<T>) -> Result<Option<T>> {
    gate_cancel(job, || false, work)
}

pub fn gate_cancel<T>(
    job: &super::Job,
    cancelled: impl Fn() -> bool,
    work: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    let timeout = match &job.check_timeout {
        Some(value) => Duration::from_secs(crate::timer::parse_duration(value)?),
        None => Duration::from_secs(30),
    };
    if !passes_in(
        &job.check,
        timeout,
        job.project.as_deref().map(std::path::Path::new),
        &cancelled,
    )
    .unwrap_or(false)
    {
        return Ok(None);
    }
    work().map(Some)
}

/// A check passes only when it exits successfully with exactly `work` on stdout.
pub fn passes(command: &str, timeout: Duration) -> Result<bool> {
    passes_in(command, timeout, None, &|| false)
}

fn passes_in(
    command: &str,
    timeout: Duration,
    dir: Option<&std::path::Path>,
    cancelled: &dyn Fn() -> bool,
) -> Result<bool> {
    let mut cmd = Command::new("sh");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.args(["-c", command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().context("start turnout check")?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::last_os_error().into());
    }
    let mut output = Vec::new();
    let mut buf = [0; 4096];
    let started = Instant::now();
    let status = loop {
        for _ in 0..16 {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if output.len() < 1024 {
                        output.extend_from_slice(&buf[..n.min(1025 - output.len())]);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout || cancelled() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(
            Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())),
        );
    };
    // Do not wait for a descendant which kept the pipe open after the shell.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    for _ in 0..16 {
        match stdout.read(&mut buf) {
            Ok(n) if n > 0 => {
                if output.len() < 1024 {
                    output.extend_from_slice(&buf[..n.min(1025 - output.len())]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            _ => break,
        }
    }
    Ok(status.is_some_and(|s| s.success())
        && std::str::from_utf8(&output).is_ok_and(|s| s.trim() == "work"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_success_and_work() {
        let t = Duration::from_secs(2);
        assert!(passes("printf 'work\\n'", t).unwrap());
        assert!(!passes("printf work; exit 1", t).unwrap());
        assert!(!passes("echo no", t).unwrap());
        assert!(!passes("echo 'work extra'", t).unwrap());
    }
    #[test]
    fn times_out_the_process_group() {
        let now = Instant::now();
        assert!(!passes("sleep 10 & wait", Duration::from_millis(50)).unwrap());
        assert!(now.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn pause_cancels_a_running_check() {
        let started = Instant::now();
        let cancelled = || started.elapsed() >= Duration::from_millis(50);
        assert!(!passes_in("sleep 10", Duration::from_secs(5), None, &cancelled).unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn gate_calls_work_only_after_a_pass() {
        let text =
            "[turnout.x]\nharness='none'\ncheck='echo work'\ntrigger='timer'\nschedule='daily'\n";
        let mut job = super::super::parse(text).unwrap().remove("x").unwrap();
        assert_eq!(gate(&job, || Ok(42)).unwrap(), Some(42));
        job.check = "exit 1".into();
        assert_eq!(
            gate(&job, || panic!("work must not run")).unwrap(),
            None::<()>
        );
    }
}
