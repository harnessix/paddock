//! The help text of a harness, read once for the flag probe.

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The output of `exe ARGS`, stdout then stderr. At most 20 seconds.
pub(super) fn help_text(exe: &Path, args: &[String]) -> Result<String> {
    let mut child = Command::new(exe)
        .args(args)
        // Not the project: a harness may read settings from its working folder.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {}", exe.display()))?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            bail!(
                "{} {} did not finish in 20 seconds",
                exe.display(),
                args.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut text = String::new();
    child.stdout.take().unwrap().read_to_string(&mut text)?;
    child.stderr.take().unwrap().read_to_string(&mut text)?;
    Ok(text)
}
