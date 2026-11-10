//! Poll a local branch at a bounded rate, with an injectable clock.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Default, Serialize, Deserialize)]
struct State {
    last_poll: Option<u64>,
    completed: Option<String>,
}

fn read(path: &Path) -> Result<State> {
    match std::fs::read(path) {
        Ok(data) => Ok(serde_json::from_slice(&data).context("invalid git trigger state")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}
fn write(path: &Path, state: &State) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(state)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// A caller must serialize poll and complete for the same job.
pub fn poll(
    path: &Path,
    project: &Path,
    branch: &str,
    interval: u64,
    now: u64,
) -> Result<Option<String>> {
    if interval < 60 {
        bail!("git poll interval must be at least 60 seconds");
    }
    let mut state = read(path)?;
    if state
        .last_poll
        .is_some_and(|last| now.saturating_sub(last) < interval)
    {
        return Ok(None);
    }
    state.last_poll = Some(now);
    write(path, &state)?;
    let output = Command::new("git")
        .current_dir(project)
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("refs/heads/{branch}^{{commit}}"),
        ])
        .output()
        .context("run git")?;
    if !output.status.success() {
        bail!("cannot read branch {branch}");
    }
    let head = String::from_utf8(output.stdout)?.trim().to_owned();
    if state.completed.as_deref() == Some(&head) {
        Ok(None)
    } else {
        Ok(Some(head))
    }
}

pub fn complete(path: &Path, head: &str) -> Result<()> {
    let mut state = read(path)?;
    state.completed = Some(head.to_owned());
    write(path, &state)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap()
            .status
            .success());
    }
    #[test]
    fn polls_at_the_minimum_interval_and_only_returns_new_commits() {
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-b", "main"]);
        git(d.path(), &["commit", "--allow-empty", "-m", "first"]);
        let state = d.path().join("state");
        let head = poll(&state, d.path(), "main", 60, 100).unwrap().unwrap();
        assert!(poll(&state, d.path(), "main", 60, 159).unwrap().is_none());
        complete(&state, &head).unwrap();
        assert!(poll(&state, d.path(), "main", 60, 160).unwrap().is_none());
        git(d.path(), &["commit", "--allow-empty", "-m", "second"]);
        assert!(poll(&state, d.path(), "main", 60, 219).unwrap().is_none());
        assert_ne!(
            poll(&state, d.path(), "main", 60, 220).unwrap().unwrap(),
            head
        );
        assert!(poll(&state, d.path(), "main", 59, 280).is_err());
    }
}
