//! Persistent kill switch for scheduled jobs.
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation(Vec<u8>, Vec<u8>);

pub fn generation(dir: &Path, name: &str) -> Generation {
    Generation(
        std::fs::read(dir.join("generation-all")).unwrap_or_default(),
        std::fs::read(dir.join(format!("generation-job-{name}"))).unwrap_or_default(),
    )
}

pub fn cancelled(dir: &Path, name: &str, original: &Generation) -> bool {
    paused(dir, name) || &generation(dir, name) != original
}

fn advance(dir: &Path, name: Option<&str>) -> Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = dir.join(match name {
        Some(n) => format!("generation-job-{n}"),
        None => "generation-all".into(),
    });
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    crate::privfs::write_atomic(
        &path,
        format!(
            "{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
    )?;
    Ok(())
}

fn marker(dir: &Path, name: Option<&str>) -> std::path::PathBuf {
    dir.join(match name {
        Some(n) => format!("paused-{n}"),
        None => "paused-all".into(),
    })
}

/// Jobs that have ever acquired a wake lock, including jobs removed from configuration.
pub fn known_jobs(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.strip_suffix(".lock")?.to_owned();
            super::valid_name(&name).then_some(name)
        })
        .collect()
}
pub fn paused(dir: &Path, name: &str) -> bool {
    marker(dir, None).exists() || marker(dir, Some(name)).exists()
}
pub fn pause(dir: &Path, names: &[String], all: bool, systemctl: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    if all {
        advance(dir, None)?;
        std::fs::write(marker(dir, None), b"paused\n")?;
    } else {
        for n in names {
            advance(dir, Some(n))?;
            std::fs::write(marker(dir, Some(n)), b"paused\n")?;
        }
    }
    for n in names {
        match std::fs::remove_file(super::wake::pending_path(dir, n)) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        // A job may not be running. Stopping an absent unit is harmless.
        let output = Command::new(systemctl)
            .args([
                "--user",
                "stop",
                &format!("paddock-turnout-work-{n}.service"),
            ])
            .output()
            .context("run systemctl")?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            if !detail.contains("not loaded") && !detail.contains("not found") {
                anyhow::bail!("pause is set, but could not stop {n}: {}", detail.trim());
            }
        }
    }
    Ok(())
}
pub fn resume(dir: &Path, name: Option<&str>) -> Result<()> {
    let paths: Vec<_> = if let Some(name) = name {
        vec![marker(dir, Some(name))]
    } else {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("paused-"))
            })
            .collect()
    };
    for path in paths {
        match std::fs::remove_file(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn all_jobs_stay_paused_until_resume() {
        let d = tempfile::tempdir().unwrap();
        let fake = d.path().join("systemctl");
        std::fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = generation(d.path(), "one");
        std::fs::write(super::super::wake::pending_path(d.path(), "one"), "pending").unwrap();
        pause(d.path(), &["one".into(), "two".into()], true, &fake).unwrap();
        assert!(paused(d.path(), "one"));
        assert!(paused(d.path(), "two"));
        assert!(paused(d.path(), "new-job"));
        assert!(!super::super::wake::pending_path(d.path(), "one").exists());
        resume(d.path(), None).unwrap();
        assert!(!paused(d.path(), "one"));
        assert!(cancelled(d.path(), "one", &old));
        assert!(!cancelled(d.path(), "one", &generation(d.path(), "one")));
    }
}
