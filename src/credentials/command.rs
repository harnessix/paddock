//! A password manager command that prints a value (`from = "command"` of
//! `[credentials]`, `backend = "command"` of `[tack]`).

use super::secret;
use super::{COMMAND_TIMEOUT, MAX_VALUE};
use anyhow::{bail, Context as _, Result};

/// `prepare` changes the command before it starts (tackd: as the user).
pub(crate) fn run_command(
    cmd: &[String],
    prepare: &dyn Fn(&mut std::process::Command),
) -> Result<secret::Secret> {
    use std::process::{Command, Stdio};
    let mut c = Command::new(&cmd[0]);
    prepare(&mut c);
    let mut child = c
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot run {}", cmd[0]))?;
    let reader = crate::broker::Pipe::read(child.stdout.take().expect("piped"), MAX_VALUE + 1);
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} did not finish in {} seconds",
                cmd[0],
                COMMAND_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    // Locked and wiped from here on, whatever the outcome.
    // A process left behind can keep stdout open: the value is not
    // whole then.
    let deadline =
        (start + COMMAND_TIMEOUT).min(std::time::Instant::now() + crate::broker::PIPE_GRACE);
    let (out, ended) = reader.finish(deadline);
    let buf = secret::Secret::new(out);
    if !ended {
        bail!("{} left its output open after it ended", cmd[0]);
    }
    if !status.success() {
        bail!("{} failed ({status})", cmd[0]);
    }
    if buf.len() > MAX_VALUE {
        bail!("{} printed more than {MAX_VALUE} bytes", cmd[0]);
    }
    let mut v = std::str::from_utf8(buf.as_bytes()).context("the value is not UTF-8")?;
    v = v.strip_suffix('\n').unwrap_or(v);
    v = v.strip_suffix('\r').unwrap_or(v);
    if v.is_empty() {
        bail!("{} printed nothing", cmd[0]);
    }
    Ok(secret::Secret::from(v))
}
