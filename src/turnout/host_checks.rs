//! Sequential cargo integration suites run inside the wake's service cgroup.
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Worktree {
    project: PathBuf,
    pub path: PathBuf,
}
impl Worktree {
    pub fn reserve(project: &Path, parent: &Path) -> Result<Self> {
        crate::privfs::dir(parent)?;
        let path = parent.join(format!(
            "checkout-{}-{}",
            std::process::id(),
            crate::state::now()
        ));
        Ok(Self {
            project: project.to_owned(),
            path,
        })
    }
    pub fn add(project: &Path, path: &Path, head: &str) -> Result<()> {
        if !matches!(head.len(), 40 | 64) || !head.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("invalid commit id");
        }
        let output = Command::new("git")
            .current_dir(project)
            .args(["worktree", "add", "--detach", "--"])
            .arg(path)
            .arg(head)
            .output()
            .context("create checks worktree")?;
        if !output.status.success() {
            anyhow::bail!(
                "cannot create checks worktree: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn create(project: &Path, parent: &Path, head: &str) -> Result<Self> {
        let tree = Self::reserve(project, parent)?;
        Self::add(project, &tree.path, head)?;
        Ok(tree)
    }
}
impl Drop for Worktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .current_dir(&self.project)
            .args(["worktree", "remove", "--force", "--"])
            .arg(&self.path)
            .output();
    }
}

fn run_suite(
    cargo: &Path,
    project: &Path,
    suite: &str,
    timeout: Duration,
    output: &mut File,
) -> Result<(bool, bool)> {
    writeln!(output, "\n--- {suite} ---")?;
    let mut child = Command::new(cargo)
        .current_dir(project)
        .args(["test", "--test", suite])
        .env("CARGO_BUILD_JOBS", "4")
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone()?))
        .stderr(Stdio::from(output.try_clone()?))
        .process_group(0)
        .spawn()
        .with_context(|| format!("start suite {suite}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.success(), false));
        }
        if start.elapsed() >= timeout {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Ok((false, true));
        }
        std::thread::sleep(Duration::from_millis(20).min(timeout.saturating_sub(start.elapsed())));
    }
}

/// Return a report with suite outcomes and the captured output path.
pub fn suites(
    cargo: &Path,
    project: &Path,
    suites: &[String],
    timeout: Duration,
    output_path: &Path,
) -> Result<String> {
    if let Some(parent) = output_path.parent() {
        crate::privfs::dir(parent)?;
    }
    crate::privfs::write_new(output_path, b"")?;
    let mut output = crate::privfs::append(output_path)?;
    let mut results = Vec::new();
    for suite in suites {
        let (ok, timed_out) = run_suite(cargo, project, suite, timeout, &mut output)?;
        let text = std::fs::read_to_string(output_path)?;
        let section = text
            .rsplit(&format!("\n--- {suite} ---\n"))
            .next()
            .unwrap_or("");
        let counts = section
            .lines()
            .filter(|line| line.starts_with("test result:"))
            .collect::<Vec<_>>()
            .join("; ");
        let mut names = Vec::new();
        let mut in_failures = false;
        for line in section.lines() {
            if line == "failures:" {
                in_failures = true;
                continue;
            }
            if in_failures {
                if line.starts_with("    ") {
                    names.push(line.trim().to_owned());
                } else if !line.is_empty() {
                    in_failures = false;
                }
            }
        }
        let result = if timed_out {
            "timed out"
        } else if ok {
            "passed"
        } else {
            "failed"
        };
        results.push(format!(
            "{suite}: {result}{}{}",
            if counts.is_empty() {
                String::new()
            } else {
                format!(" ({counts})")
            },
            if names.is_empty() {
                String::new()
            } else {
                format!("; failures: {}", names.join(", "))
            }
        ));
    }
    Ok(format!(
        "{}\nOutput: {}",
        results.join("\n"),
        output_path.display()
    ))
}

/// Run a pinned checkout in a budgeted service, and retain its output.
pub fn perform(
    env: &crate::env::HostEnv,
    job: &super::Job,
    head: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<String> {
    let project = Path::new(job.project.as_deref().context("missing project")?);
    let tree = Worktree::reserve(project, &env.paddock_cache().join("turnout/worktrees"))?;
    let state = env.paddock_state().join("turnout");
    crate::privfs::dir(&state)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let output = state.join(format!("host-checks-{}-{stamp}.log", std::process::id()));
    let summary = state.join(format!(
        "host-checks-{}-{stamp}.summary",
        std::process::id()
    ));
    let to_text = |p: &Path| {
        p.to_str()
            .map(str::to_owned)
            .context("turnout paths must be UTF-8")
    };
    let checkout = to_text(&tree.path)?;
    let out = to_text(&output)?;
    let summary_name = to_text(&summary)?;
    let result = super::budget::run(
        Path::new("systemd-run"),
        Path::new("systemctl"),
        cancelled,
        &std::env::current_exe()?,
        "host-checks",
        job,
        &[
            "--checkout",
            &checkout,
            "--commit",
            head,
            "--output",
            &out,
            "--summary",
            &summary_name,
        ],
    );
    if let Err(e) = result {
        anyhow::bail!("{e:#}\nOutput: {}", output.display());
    }
    let text = std::fs::read_to_string(&summary)
        .with_context(|| format!("missing suite results; output: {}", output.display()))?;
    let _ = std::fs::remove_file(summary);
    if text
        .lines()
        .any(|line| line.contains(": failed") || line.contains(": timed out"))
    {
        anyhow::bail!("{text}");
    }
    Ok(text)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn runs_fake_suites_sequentially_and_records_results() {
        let d = tempfile::tempdir().unwrap();
        let cargo = d.path().join("cargo");
        std::fs::write(&cargo, "#!/bin/sh\necho jobs=$CARGO_BUILD_JOBS\nif [ \"$3\" = bad ]; then echo 'test result: FAILED. 1 passed; 1 failed'; exit 1; fi\necho 'test result: ok. 3 passed; 0 failed'\n").unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log = d.path().join("out.log");
        let result = suites(
            &cargo,
            d.path(),
            &["good".into(), "bad".into()],
            Duration::from_secs(2),
            &log,
        )
        .unwrap();
        assert!(result.contains("good: passed (test result: ok. 3 passed"));
        assert!(result.contains("bad: failed (test result: FAILED. 1 passed"));
        let output = std::fs::read_to_string(log).unwrap();
        assert_eq!(output.matches("jobs=4").count(), 2);
        assert!(output.find("--- good ---").unwrap() < output.find("--- bad ---").unwrap());
    }

    #[test]
    fn a_slow_suite_is_stopped_and_reported() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("test"), "sleep 10\n").unwrap();
        let start = Instant::now();
        let text = suites(
            Path::new("/bin/sh"),
            d.path(),
            &["slow".into()],
            Duration::from_millis(60),
            &d.path().join("output"),
        )
        .unwrap();
        assert!(text.contains("slow: timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn branch_wake_runs_and_mails_fake_suite_results() {
        use crate::{env::HostEnv, mail};
        use std::collections::BTreeMap;
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .output()
                .unwrap()
                .status
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["commit", "--allow-empty", "-m", "first"]);
        let fake = d.path().join("cargo");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'test result: ok. 2 passed; 0 failed'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = HostEnv::from_parts(d.path().join("home"), "test".into(), 1, 1, BTreeMap::new());
        let text = format!("[turnout.host-checks]\nharness='none'\ncheck='echo work'\ntrigger='git'\nproject='{}'\nbranch='main'\nsuites=['one','two']\n", repo.display());
        let job = super::super::parse(&text)
            .unwrap()
            .remove("host-checks")
            .unwrap();
        let work = |head: Option<&str>, _: &super::super::pause::Generation| {
            let tree = Worktree::create(&repo, &d.path().join("trees"), head.unwrap())?;
            suites(
                &fake,
                &tree.path,
                &job.suites,
                Duration::from_secs(2),
                &d.path().join("suites.log"),
            )
        };
        super::super::report::wake(&env, "host-checks", &job, 100, 0.0, work).unwrap();
        let messages = mail::read(&env, "turnout-host-checks", mail::Dir::Up);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].text.contains("one: passed"));
        assert!(messages[0].text.contains("two: passed"));
        assert!(messages[0].text.contains("2 passed"));
        assert!(messages[0].text.contains("suites.log"));
        super::super::report::wake(&env, "host-checks", &job, 160, 0.0, |_, _| {
            panic!("unchanged commit")
        })
        .unwrap();
        assert_eq!(
            mail::read(&env, "turnout-host-checks", mail::Dir::Up).len(),
            1
        );
    }
}
