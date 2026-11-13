//! The help text of a harness, read once for the flag probe: the check of
//! the auto-approval flags, and the flags that a harness gets only when its
//! help names them (`[launch] args_if_supported`).

use crate::build::Built;
use crate::env::HostEnv;
use crate::manifest::hex;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The variables that name a folder in the home. Without them, a harness
/// uses the defaults under the scratch `HOME`.
const HOME_VARS: [&str; 7] = [
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "GEMINI_CLI_HOME",
];

/// The output of `exe ARGS`, stdout then stderr. At most 20 seconds.
///
/// The probe runs on the host, outside a sandbox. It gets an empty scratch
/// home, so a harness that writes at start (history, settings, a login
/// cache) writes there and never in the home of the user.
pub(super) fn help_text(exe: &Path, args: &[String]) -> Result<String> {
    let scratch = tempfile::Builder::new()
        .prefix("paddock-probe-")
        .tempdir()
        .context("cannot create the scratch home of the help probe")?;
    let mut cmd = Command::new(exe);
    cmd.args(args).env("HOME", scratch.path());
    for var in HOME_VARS {
        cmd.env_remove(var);
    }
    let mut child = cmd
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

/// Make sure that the harness still accepts its auto-approval flags. Flags get
/// renamed between versions; without this check a harness could start in a
/// mode that waits for approvals that nobody gives.
pub fn probe_flags(env: &HostEnv, built: &Built) -> Result<()> {
    probe_args(
        env,
        &built.harness_exe,
        &built.profile.launch.yolo_args,
        &built.profile.launch.probe_args,
        "yolo_args",
    )
}

/// Make sure that the help output (`exe probe`) still mentions every flag
/// in `args`. `what` names the profile entry in the error.
pub fn probe_args(
    env: &HostEnv,
    exe: &Path,
    args: &[String],
    probe: &[String],
    what: &str,
) -> Result<()> {
    let flags: Vec<&String> = args
        .iter()
        .filter(|a| a.starts_with('-') && a.len() > 1)
        .collect();
    if flags.is_empty() {
        return Ok(());
    }
    let cache = probe_cache(env, exe, "", &flags, probe)?;
    if cache.is_file() {
        return Ok(());
    }
    let text = help_text(exe, probe)?;
    let missing: Vec<&str> = flags
        .iter()
        .filter(|f| !text.contains(f.as_str()))
        .map(|f| f.as_str())
        .collect();
    if !missing.is_empty() {
        bail!(
            "{} {} no longer mentions {}. The harness may have renamed a flag. \
             Update {what} in a profile override under {}",
            exe.display(),
            probe.join(" "),
            missing.join(", "),
            env.paddock_config().join("harness.d").display()
        );
    }
    crate::privfs::write(&cache, "ok")?;
    Ok(())
}

/// The cache file of one help probe. The key holds the size and the time
/// of the executable, so an update of the harness probes again. `kind`
/// keeps the caches of the two probes apart.
fn probe_cache(
    env: &HostEnv,
    exe: &Path,
    kind: &str,
    flags: &[&String],
    probe: &[String],
) -> Result<PathBuf> {
    let meta = std::fs::metadata(exe)?;
    let key = format!(
        "{kind}{}|{}|{}|{:?}|{:?}",
        exe.display(),
        meta.len(),
        meta.mtime(),
        flags,
        probe
    );
    Ok(env
        .paddock_state()
        .join("probe")
        .join(&hex(&Sha256::digest(key.as_bytes()))[..16]))
}

/// The flags of `args` (`[launch] args_if_supported`) that the help output
/// (`exe probe`) names, in their order. A flag that the help does not name
/// stays out, so an older harness starts as before. When the help cannot
/// be read, no flag passes. The result is cached for this executable.
pub fn supported_args(env: &HostEnv, exe: &Path, args: &[String], probe: &[String]) -> Vec<String> {
    if args.is_empty() {
        return Vec::new();
    }
    let all: Vec<&String> = args.iter().collect();
    let cache = match probe_cache(env, exe, "if_supported|", &all, probe) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("cannot probe {}: {e:#}", exe.display());
            return Vec::new();
        }
    };
    if let Ok(text) = std::fs::read_to_string(&cache) {
        return text.lines().map(str::to_string).collect();
    }
    let text = match help_text(exe, probe) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("cannot read the help of {}: {e:#}", exe.display());
            return Vec::new();
        }
    };
    let found = names_in_help(&text, args);
    if let Err(e) = crate::privfs::write(&cache, found.join("\n")) {
        log::warn!("cannot cache the probe of {}: {e:#}", exe.display());
    }
    found
}

/// The entries of `args` that `help` names as a whole word, so that
/// `--no-daemon` does not match `--no-daemon-x`.
pub fn names_in_help(help: &str, args: &[String]) -> Vec<String> {
    let word = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    args.iter()
        .filter(|a| a.starts_with('-') && a.len() > 1)
        .filter(|a| {
            help.match_indices(a.as_str()).any(|(i, _)| {
                let before = help[..i].chars().next_back();
                let after = help[i + a.len()..].chars().next();
                !before.is_some_and(word) && !after.is_some_and(word)
            })
        })
        .cloned()
        .collect()
}
