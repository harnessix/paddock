//! The terminal interface (`paddock-tui`): where paddock finds it, and how
//! paddock installs it. The user never builds it by hand.
//!
//! - A release tarball holds `paddock-tui` next to `paddock`
//!   (`.github/workflows/release.yml`). `paddock update` installs both.
//! - The first `paddock tui` on a host without it downloads the tarball of
//!   paddock's own version, with the same checks as an update: the GitHub
//!   hosts only, the checksum, and the `--version` of the program.
//! - The program goes next to the paddock binary. When a package owns that
//!   folder, or paddock cannot write there, it goes into paddock's data
//!   folder, in a folder for this version.
//! - A development build never downloads: no release has its version.
//!   Neither does a sandbox, a system without a release binary, or a host
//!   with `PADDOCK_NO_UPDATE_CHECK=1`. Each case gets one line with the one
//!   command to run.
//! - paddock never uses `npx`: npx runs code from npm and from the project
//!   folder, which an agent can write, outside the sandbox.

use super::{
    check_url, package_owner, refuse_in_sandbox, replace_binary, GitHub, Release, Source, Tarball,
    ARCHS, CURRENT, MAX_HEAD, MAX_REDIRECTS,
};
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The file name of the terminal interface.
pub const TUI: &str = "paddock-tui";

/// A developer names another program here, as an absolute path.
pub const OVERRIDE: &str = "PADDOCK_TUI";

/// The switch that turns off every request that the user did not ask for.
const NO_NETWORK: &str = "PADDOCK_NO_UPDATE_CHECK";

/// The script of the source tree that builds and installs the program.
const BUILD_SCRIPT: &str = "tui/scripts/install.sh";

/// The folders of paddock's own copies, one per version.
fn copies(env: &HostEnv) -> PathBuf {
    env.paddock_data().join("tui")
}

/// The copy of `version` that paddock keeps when it cannot write next to
/// its binary.
pub fn private_copy(env: &HostEnv, version: &str) -> PathBuf {
    copies(env).join(version).join(TUI)
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The installed program, in this order: `PADDOCK_TUI`, the folder of the
/// paddock binary `exe`, paddock's own copy of this version, PATH. An error
/// when `PADDOCK_TUI` names no program.
pub fn find(env: &HostEnv, exe: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(raw) = env.var(OVERRIDE).filter(|v| !v.is_empty()) {
        let p = PathBuf::from(raw);
        if !p.is_absolute() || !is_executable(&p) {
            bail!("{OVERRIDE}={raw} is not the absolute path of a program. Fix it or unset it");
        }
        return Ok(Some(p));
    }
    Ok(installed(env, exe).or_else(|| env.which(TUI)))
}

/// The program in one of the two folders that paddock installs it to.
pub fn installed(env: &HostEnv, exe: Option<&Path>) -> Option<PathBuf> {
    let sibling = exe.map(|e| e.with_file_name(TUI));
    sibling
        .into_iter()
        .chain([private_copy(env, CURRENT)])
        .find(|p| is_executable(p))
}

/// What paddock is, for the choice between a download and a refusal.
#[derive(Debug, Clone)]
pub struct Build<'a> {
    /// The version of Cargo.toml.
    pub version: &'a str,
    /// A release build, not a development build (src/buildinfo.rs).
    pub release: bool,
    /// `std::env::consts::OS`.
    pub os: &'static str,
    /// `std::env::consts::ARCH`.
    pub arch: &'a str,
    /// The paddock binary.
    pub exe: Option<&'a Path>,
}

impl<'a> Build<'a> {
    /// This program.
    pub fn this(exe: Option<&'a Path>) -> Self {
        Self {
            version: CURRENT,
            release: crate::BUILD.starts_with(crate::buildinfo::RELEASE),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            exe,
        }
    }

    fn homebrew(&self) -> bool {
        self.exe
            .is_some_and(|p| p.components().any(|c| c.as_os_str() == "Cellar"))
    }

    /// The command that builds the program from the source tree and puts
    /// it next to this paddock.
    fn build_command(&self) -> String {
        match self.exe.and_then(Path::parent) {
            Some(dir) => format!("{BUILD_SCRIPT} {}", shell_word(&dir.display().to_string())),
            None => BUILD_SCRIPT.to_string(),
        }
    }
}

/// `text` as one word of a shell command.
fn shell_word(text: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-+".contains(c);
    if !text.is_empty() && text.chars().all(plain) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// What `paddock tui` does on a host without the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Download it from this release.
    Download(Release),
    /// No download. The text says why, and the one command to run.
    Refuse(String),
}

/// Whether paddock downloads the program. `asked` is true when the user
/// asked for the install by name (`paddock update --tui`): then the switch
/// `PADDOCK_NO_UPDATE_CHECK` does not stop it.
pub fn plan(env: &HostEnv, build: &Build, asked: bool) -> Plan {
    const MISSING: &str = "The paddock TUI is not installed.";
    if env.var("PADDOCK") == Some("1") {
        return Plan::Refuse(format!(
            "{MISSING} This is a paddock sandbox (PADDOCK=1), and paddock installs nothing from inside one. Ask the user to run this in a terminal on the host: paddock tui"
        ));
    }
    if !build.release {
        return Plan::Refuse(format!(
            "{MISSING} This paddock is a development build, and no release has its TUI, so paddock does not download one. Build and install it in the paddock source tree with: {}",
            build.build_command()
        ));
    }
    if build.os != "linux" {
        let how = if build.homebrew() {
            "Homebrew installs it together with paddock. Run: brew reinstall paddock".to_string()
        } else {
            format!(
                "The releases have it for Linux only. Build and install it in the paddock source tree with: {}",
                build.build_command()
            )
        };
        return Plan::Refuse(format!("{MISSING} {how}"));
    }
    if !ARCHS.contains(&build.arch) {
        return Plan::Refuse(format!(
            "{MISSING} The releases have it for {} only, not for {}. Build and install it in the paddock source tree with: {}",
            ARCHS.join(" and "),
            build.arch,
            build.build_command()
        ));
    }
    if !asked && env.var(NO_NETWORK) == Some("1") {
        return Plan::Refuse(format!(
            "{MISSING} {NO_NETWORK}=1 is set, so paddock does not download it on its own. Install it with: paddock update --tui"
        ));
    }
    match Release::for_target(&format!("v{}", build.version), build.arch, build.os) {
        Ok(rel) => Plan::Download(rel),
        Err(e) => Plan::Refuse(format!("{MISSING} {e}")),
    }
}

/// `12 MB` for a size in bytes. A size under one megabyte is 1 MB.
pub fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1_000_000).max(1))
}

/// The one line before the download.
pub fn installing_line(version: &str, size: Option<u64>) -> String {
    match size {
        Some(n) => format!("Installing the paddock TUI {version} ({})…", megabytes(n)),
        None => format!("Installing the paddock TUI {version}…"),
    }
}

/// The one line after a download that failed.
pub fn failure_line(err: &anyhow::Error) -> String {
    format!(
        "paddock could not install the TUI: {err:#}. When the host is online, run this again: paddock tui"
    )
}

/// Download the `paddock-tui` of `rel`, check it, and install it for the
/// paddock binary `exe`. `say` gets the one line before the download. The
/// answer is the installed file.
pub fn install(
    env: &HostEnv,
    source: &dyn Source,
    rel: &Release,
    exe: &Path,
    tar: &Path,
    say: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    refuse_in_sandbox(env, "the install of the paddock TUI")?;
    say(&installing_line(
        &rel.version,
        source.size(&rel.tarball_url()),
    ));
    let got = Tarball::fetch(env, source, rel)?;
    let bytes = got.program(tar, TUI).with_context(|| {
        format!(
            "release {} has no usable {TUI}. `paddock update` installs the latest release, which has one",
            rel.version
        )
    })?;
    place(env, exe, &rel.version, &bytes)
}

/// The program for `paddock tui` on a host without it: the download of
/// [`plan`], or the one line that says why not and what to run. `say` gets
/// the one line before the download.
pub fn obtain(
    env: &HostEnv,
    build: &Build,
    source: &dyn Source,
    asked: bool,
    say: &mut dyn FnMut(&str),
) -> std::result::Result<PathBuf, String> {
    let rel = match plan(env, build, asked) {
        Plan::Download(rel) => rel,
        Plan::Refuse(line) => return Err(line),
    };
    let mut run = || -> Result<PathBuf> {
        let exe = build.exe.context("cannot find the paddock executable")?;
        let tar = env.which("tar").context("the install needs tar")?;
        install(env, source, &rel, exe, &tar, say)
    };
    run().map_err(|e| failure_line(&e))
}

/// Write the program next to `exe`, or into paddock's data folder when a
/// package owns `exe` or its folder cannot be written.
fn place(env: &HostEnv, exe: &Path, version: &str, bytes: &[u8]) -> Result<PathBuf> {
    if package_owner(env, exe).is_none() {
        let sibling = exe.with_file_name(TUI);
        match replace_binary(&sibling, bytes) {
            Ok(()) => return Ok(sibling),
            Err(e) => log::debug!("the TUI goes into the data folder: {e:#}"),
        }
    }
    let own = private_copy(env, version);
    let dir = own.parent().context("no folder for the TUI")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    replace_binary(&own, bytes)?;
    // The copies of other versions are of no use to this paddock.
    if let Ok(entries) = std::fs::read_dir(copies(env)) {
        for e in entries.flatten() {
            if e.file_name() != version {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(own)
}

/// The status, the `Location` and the `Content-Length` of the head of an
/// HTTP response.
pub fn parse_head(raw: &[u8]) -> Result<(u16, Option<String>, Option<u64>)> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("the server closed the connection before the HTTP head ended")?;
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut resp = httparse::Response::new(&mut headers);
    resp.parse(&raw[..end + 4])?;
    let header = |name: &str| {
        resp.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(h.value).trim().to_string())
    };
    Ok((
        resp.code.context("no HTTP status")?,
        header("location"),
        header("content-length").and_then(|l| l.parse().ok()),
    ))
}

impl GitHub {
    /// The size of a download, from a HEAD request. Redirects are followed
    /// to the allowed hosts only. None when the server does not tell it.
    pub(super) fn length(&self, raw: &str) -> Option<u64> {
        let mut url = check_url(raw).ok()?;
        for _ in 0..=MAX_REDIRECTS {
            let (head, _) = self
                .exchange("HEAD", &url, "application/octet-stream", MAX_HEAD)
                .ok()?;
            let (status, location, length) = parse_head(&head).ok()?;
            match status {
                200 => return length,
                301 | 302 | 303 | 307 | 308 => {
                    url = check_url(url.join(&location?).ok()?.as_str()).ok()?;
                }
                _ => return None,
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "tui_tests.rs"]
mod tests;
