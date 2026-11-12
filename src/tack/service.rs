//! Installing, upgrading and removing tackd as a system service, and
//! reporting its state.
//!
//! - Linux: a root systemd unit, `/etc/systemd/system/paddock-tackd.service`,
//!   that runs a root-owned copy of paddock, `/usr/local/libexec/paddock/tackd`.
//! - macOS: a LaunchDaemon, `/Library/LaunchDaemons/io.paddock.tackd.plist`,
//!   that runs `/Library/PrivilegedHelperTools/io.paddock.tackd`.
//!
//! The service never runs the paddock binary that a user installed: a
//! user can replace that file, and root would then run the user's code.
//! `sudo paddock tack setup` copies it to a folder that only root writes
//! (every folder up to `/` is checked), checks that the copy runs and
//! reports this version, and only then renames it over the old copy:
//! an upgrade is atomic, and a failed one leaves the old tackd in place.
//! Running setup again after `paddock update` upgrades tackd and restarts
//! it. `sudo paddock tack remove` stops it and deletes every file it made.

use anyhow::{bail, Context as _, Result};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The first line of every file that setup writes.
pub const MARK: &str = "Written by paddock tack setup (sudo paddock tack remove deletes it).";
pub const UNIT_NAME: &str = "paddock-tackd.service";
pub const LABEL: &str = "io.paddock.tackd";

/// Where the service's files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub binary: PathBuf,
    /// The systemd unit or the launchd plist.
    pub unit: PathBuf,
    pub socket: PathBuf,
    pub audit: PathBuf,
    /// macOS: tackd's standard error (on Linux the journal holds it).
    pub stderr: Option<PathBuf>,
}

impl Paths {
    /// The paths on this host.
    pub fn system() -> Self {
        Self::under(Path::new("/"))
    }

    /// The layout a stand-in looks for tackd in: the system's, or the one
    /// under `$PADDOCK_TACK_TEST_ROOT` (an absolute path), which tests of
    /// the built binary set so that a tackd of the host stays out of them.
    /// Only the stand-in reads it: it runs as the user, so the variable
    /// gives nothing a user's own environment could not already do, and
    /// setup, remove and tackd itself always use the system's.
    pub fn for_standin(env: &crate::env::HostEnv) -> Self {
        match env.var(TEST_ROOT).map(Path::new) {
            Some(r) if r.is_absolute() => Self::under(r),
            _ => Self::system(),
        }
    }

    /// The same layout under `root`, for tests.
    pub fn under(root: &Path) -> Self {
        let p = |s: &str| root.join(s.trim_start_matches('/'));
        if cfg!(target_os = "macos") {
            Paths {
                binary: p("/Library/PrivilegedHelperTools/io.paddock.tackd"),
                unit: p("/Library/LaunchDaemons/io.paddock.tackd.plist"),
                socket: p("/var/run/paddock-tackd/tackd.sock"),
                audit: p("/var/log/paddock/tackd.log"),
                stderr: Some(p("/var/log/paddock/tackd.err")),
            }
        } else {
            Paths {
                binary: p("/usr/local/libexec/paddock/tackd"),
                unit: p("/etc/systemd/system/paddock-tackd.service"),
                socket: p("/run/paddock-tackd/tackd.sock"),
                audit: p("/var/log/paddock/tackd.log"),
                stderr: None,
            }
        }
    }
}

/// The systemd unit. Every option is explained in the file itself, so an
/// administrator reading it knows why it is there.
pub fn unit_text(p: &Paths) -> String {
    format!(
        "# {MARK}
#
# tackd decides which program receives which tack secret, and starts the
# allowed programs with their secrets as the user who asked. It reads the
# [tack] table of that user, the process table and program files, runs the
# user's store as that user, and writes an audit log. It opens no network
# connection. paddock tack status shows its state.
#
# A program that tackd starts is its child: it inherits every namespace,
# system call filter and device rule of this unit. So this unit has no
# private file system view, no private network or /tmp, and no system call
# filter: the program must see and reach what it would from your shell.
# tackd's own limits are its capabilities and NoNewPrivileges below.
#
# A program at protect = \"uid\" runs as the helper account of its user,
# which setup makes; tackd keeps their registry, the ACL ledger, their
# homes and the exec wrapper folders in /var/lib/paddock-tackd.

[Unit]
Description=paddock tackd: secrets for allowlisted programs
After=local-fs.target

[Service]
# The root-owned copy of paddock that setup installed, not the user's.
Type=simple
ExecStart={bin} tack serve
# Start again after a crash, quickly: clients see a clear error meanwhile.
Restart=always
RestartSec=1
# A restart or an update stops tackd only: the programs that it started
# keep running (they live in the cgroup of the user who started them).
KillMode=process

# root, because it reads the configuration and the processes of every
# user and starts programs as them. Only these capabilities remain:
#   CAP_DAC_READ_SEARCH: read each user's configuration and program files.
#   CAP_SYS_PTRACE: read the program, root folder and namespaces of other
#   users' processes in /proc (it never attaches to a process).
#   CAP_SETUID, CAP_SETGID: become the user (and the user's own groups)
#   before a program or a store tool starts.
#   CAP_KILL: pass the stand-in's signals on to the program.
#   CAP_DAC_OVERRIDE: move the program into the caller's cgroup.
User=root
CapabilityBoundingSet=CAP_DAC_READ_SEARCH CAP_SYS_PTRACE CAP_SETUID CAP_SETGID CAP_KILL CAP_DAC_OVERRIDE
AmbientCapabilities=
# No setuid program can give it, or a program it starts, more rights.
NoNewPrivileges=yes
# The socket folder (0755 so every user reaches the socket) and the audit
# log folder (0700, root only).
RuntimeDirectory=paddock-tackd
RuntimeDirectoryMode=0755
LogsDirectory=paddock
LogsDirectoryMode=0700
UMask=0077
# No setuid files, no personality changes.
RestrictSUIDSGID=yes
LockPersonality=yes

[Install]
WantedBy=multi-user.target
",
        bin = p.binary.display()
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd plist.
pub fn plist_text(p: &Paths) -> String {
    let err = p
        .stderr
        .as_ref()
        .map(|e| {
            format!(
                "  <key>StandardErrorPath</key>\n  <string>{}</string>\n",
                xml(&e.display().to_string())
            )
        })
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!-- {MARK} -->
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>tack</string>
    <string>serve</string>
  </array>
  <key>UserName</key>
  <string>root</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>Umask</key>
  <integer>63</integer>
{err}</dict>
</plist>
",
        bin = xml(&p.binary.display().to_string())
    )
}

/// The service file for this platform.
pub fn service_text(p: &Paths) -> String {
    if cfg!(target_os = "macos") {
        plist_text(p)
    } else {
        unit_text(p)
    }
}

/// See [`Paths::for_standin`].
pub const TEST_ROOT: &str = "PADDOCK_TACK_TEST_ROOT";

/// Whether setup wrote this file.
pub fn ours(p: &Path) -> bool {
    std::fs::read_to_string(p)
        .map(|t| t.lines().take(3).any(|l| l.contains(MARK)))
        .unwrap_or(false)
}

/// What setup and remove do outside the file system: the service manager,
/// and running the new binary once. Tests use a fake.
pub trait Runner {
    fn run(&self, program: &str, args: &[&str]) -> Result<()>;
    /// The version that `bin --version` prints.
    fn version(&self, bin: &Path) -> Result<String>;
}

pub struct System;

impl Runner for System {
    fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        let out = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .with_context(|| format!("cannot run {program}"))?;
        if !out.status.success() {
            bail!(
                "{program} {} failed ({}): {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    fn version(&self, bin: &Path) -> Result<String> {
        let mut child = std::process::Command::new(bin)
            .arg("--version")
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("cannot run {}", bin.display()))?;
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(st) = child.try_wait()? {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    use std::io::Read;
                    o.read_to_string(&mut out)?;
                }
                if !st.success() {
                    bail!("{} --version failed ({st})", bin.display());
                }
                return Ok(out.trim().to_string());
            }
            if std::time::Instant::now() > end {
                let _ = child.kill();
                let _ = child.wait();
                bail!("{} --version did not end within 10s", bin.display());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Check that `dir` and every folder above it are `owner`'s and that no
/// one else can write them (a sticky folder is not accepted either), up to
/// and with `stop` (`/` on a host).
pub fn safe_folders(dir: &Path, owner: u32, stop: &Path) -> Result<()> {
    let mut d = Some(dir);
    while let Some(x) = d {
        let m = std::fs::symlink_metadata(x)
            .with_context(|| format!("cannot check {}", x.display()))?;
        if m.file_type().is_symlink() {
            bail!(
                "{} is a symbolic link: tackd's folders must be real folders that only root writes",
                x.display()
            );
        }
        if m.uid() != owner {
            bail!(
                "{} belongs to uid {}, not root: another user could replace tackd there. Fix its owner (sudo chown root {}) and run setup again",
                x.display(),
                m.uid(),
                x.display()
            );
        }
        if m.mode() & 0o022 != 0 {
            bail!(
                "{} can be written by its group or others: run sudo chmod go-w {} and run setup again",
                x.display(),
                x.display()
            );
        }
        if x == stop {
            break;
        }
        d = x.parent();
    }
    Ok(())
}

/// Create a folder chain with 0755, then check it.
fn make_folder(dir: &Path, owner: u32, stop: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut d = Some(dir);
    while let Some(x) = d {
        if x.exists() || x == stop {
            break;
        }
        missing.push(x.to_path_buf());
        d = x.parent();
    }
    for x in missing.iter().rev() {
        std::fs::create_dir(x).with_context(|| format!("cannot create {}", x.display()))?;
        std::fs::set_permissions(x, std::fs::Permissions::from_mode(0o755))?;
    }
    safe_folders(dir, owner, stop)
}

/// Write `bytes` to `target` atomically with `mode`: a new file next to
/// it, synced, renamed over it, and the folder synced.
pub fn atomic_write(target: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let dir = target.parent().context("no folder")?;
    let tmp = dir.join(format!(
        ".{}.new-{}",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("x"),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    let r = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, target)
            .with_context(|| format!("cannot replace {}", target.display()))?;
        std::fs::File::open(dir)?.sync_all()?;
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// What setup did, for the user.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
    pub upgraded_from: Option<String>,
}

/// Install or upgrade tackd. `source` is the running paddock; `owner` is
/// root (0) on a host and the test user in tests; `stop` is where folder
/// creation stops (the file system root on a host).
pub fn setup(
    p: &Paths,
    source: &Path,
    owner: u32,
    stop: &Path,
    runner: &dyn Runner,
) -> Result<Report> {
    let mut rep = Report::default();
    let version = env!("CARGO_PKG_VERSION");
    if p.unit.exists() && !ours(&p.unit) {
        bail!(
            "{} exists and paddock did not write it; nothing was changed",
            p.unit.display()
        );
    }
    let bin_dir = p.binary.parent().context("no binary folder")?;
    make_folder(bin_dir, owner, stop)?;
    let unit_dir = p.unit.parent().context("no unit folder")?;
    make_folder(unit_dir, owner, stop)?;
    // The log folder: systemd makes it on Linux (LogsDirectory); launchd
    // opens tackd's standard error there before tackd runs.
    if let Some(d) = p.audit.parent() {
        make_folder(d, owner, stop)?;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
    }
    if p.binary.exists() {
        rep.upgraded_from = runner.version(&p.binary).ok();
    }
    // The new copy, under a temporary name, checked before it replaces
    // the old one.
    let bytes =
        std::fs::read(source).with_context(|| format!("cannot read {}", source.display()))?;
    let tmp = bin_dir.join(format!(".tackd.new-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let staged = (|| -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        f.sync_all()?;
        drop(f);
        let got = runner.version(&tmp)?;
        // A development build prints `paddock 0.2.1-dev (commit ...)`.
        let dev = format!("{version}-dev");
        if !got.split_whitespace().any(|w| w == version || w == dev) {
            bail!("the copy reports {got:?}, not version {version}");
        }
        std::fs::rename(&tmp, &p.binary)?;
        std::fs::File::open(bin_dir)?.sync_all()?;
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.context(format!(
            "cannot install {} (the old tackd, if any, is unchanged)",
            p.binary.display()
        )));
    }
    rep.lines.push(format!(
        "installed {} (paddock {version})",
        p.binary.display()
    ));
    atomic_write(&p.unit, service_text(p).as_bytes(), 0o644)?;
    rep.lines.push(format!("wrote     {}", p.unit.display()));
    if cfg!(target_os = "macos") {
        let target = format!("system/{LABEL}");
        // Not loaded yet is fine; a loaded one is unloaded to restart it
        // with the new binary.
        let _ = runner.run("launchctl", &["bootout", &target]);
        let unit = p.unit.display().to_string();
        runner.run("launchctl", &["bootstrap", "system", &unit])?;
        runner.run("launchctl", &["enable", &target])?;
    } else {
        runner.run("systemctl", &["daemon-reload"])?;
        runner.run("systemctl", &["enable", UNIT_NAME])?;
        // restart: starts it, or restarts an older tackd with the new copy.
        runner.run("systemctl", &["restart", UNIT_NAME])?;
    }
    rep.lines.push(match &rep.upgraded_from {
        Some(old) => format!("restarted tackd (was {old})"),
        None => "started   tackd".into(),
    });
    Ok(rep)
}

/// Stop tackd and delete its files. `keep_audit` keeps the audit log.
/// Returns what it deleted, and what it left with the reason.
pub fn remove(p: &Paths, keep_audit: bool, runner: &dyn Runner) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let unit_ours = ours(&p.unit);
    if p.unit.exists() && !unit_ours {
        out.push(format!(
            "left      {} (paddock did not write it)",
            p.unit.display()
        ));
    }
    if unit_ours {
        if cfg!(target_os = "macos") {
            let _ = runner.run("launchctl", &["bootout", &format!("system/{LABEL}")]);
        } else {
            let _ = runner.run("systemctl", &["disable", "--now", UNIT_NAME]);
        }
        std::fs::remove_file(&p.unit)
            .with_context(|| format!("cannot delete {}", p.unit.display()))?;
        out.push(format!("deleted   {}", p.unit.display()));
        if !cfg!(target_os = "macos") {
            runner.run("systemctl", &["daemon-reload"])?;
            let _ = runner.run("systemctl", &["reset-failed", UNIT_NAME]);
        }
    }
    let mut files = vec![p.binary.clone(), p.socket.clone()];
    files.extend(p.stderr.clone());
    if !keep_audit {
        files.extend(super::audit::Log::new(p.audit.clone(), 0).files());
    }
    for f in files {
        if std::fs::symlink_metadata(&f).is_ok() {
            std::fs::remove_file(&f).with_context(|| format!("cannot delete {}", f.display()))?;
            out.push(format!("deleted   {}", f.display()));
        }
    }
    // The folders that only tackd uses, when they are empty.
    let mut dirs = vec![
        p.binary.parent().map(Path::to_path_buf),
        p.socket.parent().map(Path::to_path_buf),
    ];
    if !keep_audit {
        dirs.push(p.audit.parent().map(Path::to_path_buf));
    }
    for d in dirs.into_iter().flatten() {
        let ours = d.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n == "paddock" || n == "paddock-tackd"
        });
        if ours && std::fs::remove_dir(&d).is_ok() {
            out.push(format!("deleted   {}", d.display()));
        }
    }
    if keep_audit && p.audit.exists() {
        out.push(format!("kept      {} (--keep-audit)", p.audit.display()));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
