//! The steps that install a program of a release: the tarball in a private
//! folder, the checksum, the `--version` of the program, and the atomic
//! replacement.

use super::{package_hint, refuse_in_sandbox, tui, verify_checksum, Release, Source};
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `bin --version` and check that it prints `paddock VERSION`.
pub fn check_version(bin: &Path, version: &str) -> Result<()> {
    check_program_version(bin, "paddock", version)
}

/// Run `bin --version` and check that it prints `NAME VERSION`.
pub fn check_program_version(bin: &Path, name: &str, version: &str) -> Result<()> {
    let mut cmd = Command::new(bin);
    cmd.arg("--version").env_clear().stdin(Stdio::null());
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = crate::busy::spawn(&mut cmd)
        .with_context(|| format!("cannot run the downloaded {}", bin.display()))?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the downloaded {name} did not answer --version");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    if let Some(s) = child.stdout.take() {
        s.take(4096).read_to_string(&mut out)?;
    }
    let want = format!("{name} {version}");
    if out.trim() != want {
        bail!(
            "the downloaded {name} prints {:?} for --version, not {want:?}. Nothing was changed",
            out.trim()
        );
    }
    Ok(())
}

/// Extract one member of a .tar.gz to memory with the system `tar`. Nothing
/// of the archive is written to the file system.
pub fn extract_member(tar: &Path, tarball: &Path, member: &str) -> Result<Vec<u8>> {
    let out = Command::new(tar)
        .args(["-xzOf"])
        .arg(tarball)
        .arg("--")
        .arg(member)
        .stdin(Stdio::null())
        .output()
        .context("cannot run tar")?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(
            "the tarball has no {member}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Replace `target` atomically: a new file next to it with mode 0755,
/// synced to disk, then renamed over it. A running paddock keeps the old
/// file open; the next start uses the new one.
pub fn replace_binary(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target
        .parent()
        .context("the paddock binary has no folder")?;
    let tmp = dir.join(format!(".paddock-update-{}", std::process::id()));
    let write = || -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&tmp)
            .with_context(|| format!("cannot write to {}", dir.display()))?;
        f.write_all(bytes)?;
        // The umask may have taken bits away.
        f.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, target)
            .with_context(|| format!("cannot replace {}", target.display()))?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    };
    let r = write();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// A private 0700 folder under paddock's cache, removed on drop.
pub struct PrivTemp(PathBuf);

impl PrivTemp {
    pub fn new(env: &HostEnv) -> Result<Self> {
        let p = env.paddock_cache().join("update").join(format!(
            "{}-{}",
            std::process::id(),
            crate::state::now()
        ));
        if p.exists() {
            std::fs::remove_dir_all(&p)?;
        }
        crate::privfs::dir(&p)?;
        Ok(Self(p))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The tarball of a release in a private folder, after the checksum check.
pub struct Tarball<'a> {
    rel: &'a Release,
    tmp: PrivTemp,
    file: PathBuf,
}

impl<'a> Tarball<'a> {
    /// Download the tarball of `rel` and its `.sha256`, and check the
    /// checksum.
    pub fn fetch(env: &HostEnv, source: &dyn Source, rel: &'a Release) -> Result<Self> {
        let tmp = PrivTemp::new(env)?;
        let tarball = source.download(&rel.tarball_url())?;
        let sums = source.download(&rel.checksum_url())?;
        verify_checksum(&tarball, &sums, &rel.tarball())?;
        let file = tmp.path().join(rel.tarball());
        crate::privfs::write(&file, &tarball)?;
        Ok(Self { rel, tmp, file })
    }

    /// The program `name` (`paddock` or `paddock-tui`) of the tarball,
    /// after a check that it prints the version of the release.
    pub fn program(&self, tar: &Path, name: &str) -> Result<Vec<u8>> {
        let member = format!("{}/{name}", self.rel.stem());
        let bin = extract_member(tar, &self.file, &member)?;
        let probe = self.tmp.path().join(name);
        crate::privfs::write(&probe, &bin)?;
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700))?;
        check_program_version(&probe, name, &self.rel.version)?;
        Ok(bin)
    }
}

/// Download, check and install `rel` over `target`, and its `paddock-tui`
/// next to `target`. `owner` is the package that owns `target`, if any:
/// then nothing is downloaded. Both programs pass every check before one
/// file changes.
pub fn install(
    env: &HostEnv,
    source: &dyn Source,
    rel: &Release,
    target: &Path,
    owner: Option<&str>,
    tar: &Path,
) -> Result<()> {
    refuse_in_sandbox(env, "paddock update")?;
    if let Some(o) = owner {
        bail!("{}", package_hint(o, &rel.version));
    }
    let got = Tarball::fetch(env, source, rel)?;
    let bin = got.program(tar, "paddock")?;
    let tui = got.program(tar, tui::TUI)?;
    replace_binary(&target.with_file_name(tui::TUI), &tui)?;
    replace_binary(target, &bin)
}
