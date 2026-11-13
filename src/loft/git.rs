//! The loft's own history: a git folder that only the host sees, with
//! `tree/` as its work tree. The commands are hardened (hostgit): the
//! sandbox writes the tree, never the git folder.

use super::{Loft, NOTES};
use anyhow::{bail, Result};
use std::os::fd::AsRawFd;
use std::process::Command;
use std::time::Duration;

fn cmd(loft: &Loft) -> Command {
    let mut c = crate::hostgit::hardened(&loft.git_dir(), Some(&loft.tree()));
    c.current_dir(loft.tree())
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .env("GIT_AUTHOR_NAME", "paddock")
        .env("GIT_AUTHOR_EMAIL", "paddock@localhost")
        .env("GIT_COMMITTER_NAME", "paddock")
        .env("GIT_COMMITTER_EMAIL", "paddock@localhost");
    c
}

/// Run git in the loft: the exit code and the output.
pub fn run(loft: &Loft, args: &[&str]) -> Result<(i32, String)> {
    let mut c = cmd(loft);
    c.args(args);
    let (code, out, err) = crate::broker::run_capture(&mut c, Duration::from_secs(60))?;
    if code != 0 && code != 1 {
        bail!("git {} in the loft failed: {}", args[0], err.trim());
    }
    Ok((code, out))
}

fn ok(loft: &Loft, args: &[&str]) -> Result<String> {
    match run(loft, args)? {
        (0, out) => Ok(out),
        _ => bail!("git {} in the loft failed", args[0]),
    }
}

/// Make the history, with a first empty commit. The host request inbox
/// stays out of it.
pub fn init(loft: &Loft) -> Result<()> {
    crate::privfs::dir(&loft.tree())?;
    if loft.initialized() {
        return Ok(());
    }
    crate::privfs::dir(&loft.git_dir())?;
    ok(loft, &["init", "-q"])?;
    crate::privfs::write(
        &loft.git_dir().join("info/exclude"),
        format!("/{NOTES}/inbox/\n").as_bytes(),
    )?;
    ok(
        loft,
        &["commit", "-q", "--allow-empty", "-m", "Start the loft"],
    )?;
    Ok(())
}

/// Stage every change. `-f`: a `.gitignore` that an agent wrote in the
/// tree does not hide files from the loft's own history.
fn stage(loft: &Loft) -> Result<()> {
    let inbox = format!(":(exclude){NOTES}/inbox");
    ok(loft, &["add", "-A", "-f", "--", ".", &inbox])?;
    Ok(())
}

/// Commit what changed, as one commit. Returns false when nothing changed.
pub fn commit(loft: &Loft, message: &str) -> Result<bool> {
    if !loft.initialized() {
        bail!("the loft {} has no history yet", loft.root.display());
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(loft.root.join("lock"))?;
    // SAFETY: flock on an open file; the lock goes with the file.
    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
    stage(loft)?;
    if run(loft, &["diff", "--cached", "--quiet"])?.0 == 0 {
        return Ok(false);
    }
    ok(loft, &["commit", "-q", "--no-verify", "-m", message])?;
    Ok(true)
}

/// The changes since the last commit, as `git status --short`.
pub fn status(loft: &Loft) -> Result<String> {
    stage(loft)?;
    ok(loft, &["status", "--short", "--untracked-files=no"])
}

pub fn log(loft: &Loft, n: usize) -> Result<String> {
    ok(
        loft,
        &[
            "log",
            &format!("-n{n}"),
            "--format=%h %ad %s",
            "--date=format:%Y-%m-%d %H:%M",
            "--stat",
        ],
    )
}

/// The diff of the tree against the last commit, new files included.
pub fn diff(loft: &Loft) -> Result<String> {
    stage(loft)?;
    ok(loft, &["diff", "--cached", "--no-color", "--no-ext-diff"])
}

/// The commits that the end of a run wrote: `Run ID (HARNESS)`, as
/// `end_of_run` names them. By run id: the count and the commit time of
/// the newest. Empty when the loft has no history. Reading only.
pub fn run_commits(loft: &Loft) -> std::collections::HashMap<String, (u64, u64)> {
    let mut by_run = std::collections::HashMap::new();
    if !loft.initialized() {
        return by_run;
    }
    let out = match run(loft, &["log", "--no-color", "--format=%ct %s"]) {
        Ok((0, out)) => out,
        Ok(_) => return by_run,
        Err(e) => {
            log::debug!("cannot read the loft history: {e:#}");
            return by_run;
        }
    };
    for line in out.lines() {
        let Some((time, subject)) = line.split_once(' ') else {
            continue;
        };
        let Some(id) = run_of_subject(subject) else {
            continue;
        };
        let time = time.parse().unwrap_or(0);
        let e: &mut (u64, u64) = by_run.entry(id.to_string()).or_default();
        e.0 += 1;
        e.1 = e.1.max(time);
    }
    by_run
}

/// The run id of a subject `Run ID (HARNESS)`.
pub fn run_of_subject(subject: &str) -> Option<&str> {
    let rest = subject.strip_prefix("Run ")?;
    let (id, harness) = rest.split_once(" (")?;
    (!id.is_empty() && !id.contains(' ') && harness.ends_with(')')).then_some(id)
}
