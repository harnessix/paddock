//! Where an approved `exec` runs: a hardened, one-time unit.
//!
//! The executor (`paddock gatewayd`) never runs a command with the whole
//! host. By default the command sees the system read-only, the project
//! read-write, an empty home and its own `/tmp`. It can use the network,
//! but it cannot gain privileges, create namespaces or reach the user's
//! systemd manager and session bus. Only a few host
//! variables pass. `[exec]` in the user configuration widens this:
//!
//! ```toml
//! [exec]
//! home = "hidden"      # hidden (default), read-only, or full
//! network = true       # false: no network at all
//! paths = []           # more folders, read-write
//! ro_paths = []        # more folders, read-only
//! env = []             # more host variables to pass, by name
//! ```
//!
//! With a systemd user instance, the command runs in a transient unit
//! (`systemd-run --user`) with these properties. Without one, bwrap gives
//! the same view. On macOS, `sandbox-exec` runs it with a Seatbelt profile
//! of the same view (execbox/seatbelt.rs). A project `paddock.toml` cannot
//! set `[exec]`.

pub mod seatbelt;

use crate::env::HostEnv;
use anyhow::{bail, Context as _, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Host variables that every command gets.
pub const BASE_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TZ",
    "TERM",
    "USER",
    "LOGNAME",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Home {
    Hidden,
    ReadOnly,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecCfg {
    pub home: Home,
    pub network: bool,
    pub paths: Vec<PathBuf>,
    pub ro_paths: Vec<PathBuf>,
    pub env: Vec<String>,
    /// Keep the command line of an allowed exec of plain words in the audit
    /// log, for `paddock tools suggest`. Off by default: arguments can hold
    /// private text that no filter finds.
    pub remember_allowed: bool,
    /// Folders the command cannot see: by default the user's runtime
    /// folders, with the user manager, the session bus and agent sockets,
    /// where `systemd-run --user` would start a command outside every
    /// limit. Not part of `[exec]`.
    pub hide: Vec<PathBuf>,
    /// macOS: the command reaches the login keychain of the user through
    /// securityd, for gh and git credential helpers (`git-push`, `forge`).
    /// Not part of `[exec]`.
    pub keychain: bool,
}

/// The user's runtime folders on this host: `/run/user` and
/// `$XDG_RUNTIME_DIR` where it is elsewhere.
pub fn session_dirs() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/run/user")];
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if rt.is_absolute() && !rt.starts_with("/run/user") {
            v.push(rt);
        }
    }
    v.retain(|p| p.is_dir());
    v
}

impl Default for ExecCfg {
    fn default() -> Self {
        Self {
            home: Home::Hidden,
            network: true,
            paths: Vec::new(),
            ro_paths: Vec::new(),
            env: Vec::new(),
            remember_allowed: false,
            hide: session_dirs(),
            keychain: false,
        }
    }
}

impl ExecCfg {
    /// `[exec]` of the user configuration, never of a project file.
    pub fn load(env: &HostEnv) -> Result<Self> {
        let file = env.paddock_config().join("config.toml");
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
        };
        Self::parse(env, &text).with_context(|| format!("{} [exec] is invalid", file.display()))
    }

    pub fn parse(env: &HostEnv, text: &str) -> Result<Self> {
        let doc: toml::Value = toml::from_str(text)?;
        let mut c = Self::default();
        let Some(t) = doc.get("exec").and_then(|e| e.as_table()) else {
            return Ok(c);
        };
        let paths = |k: &str| -> Result<Vec<PathBuf>> {
            let Some(v) = t.get(k) else {
                return Ok(Vec::new());
            };
            let list = v
                .as_array()
                .with_context(|| format!("{k} must be a list"))?;
            list.iter()
                .map(|p| {
                    let s = p.as_str().with_context(|| format!("{k} holds paths"))?;
                    let p = env.expand(s, &env.home);
                    if p == Path::new("/") || p == env.home {
                        bail!("{k}: {s} would open the whole system or home; use home = \"full\"");
                    }
                    Ok(p)
                })
                .collect()
        };
        for (k, v) in t {
            match k.as_str() {
                "home" => {
                    c.home = match v.as_str() {
                        Some("hidden") => Home::Hidden,
                        Some("read-only") => Home::ReadOnly,
                        Some("full") => Home::Full,
                        _ => bail!("home must be hidden, read-only or full"),
                    }
                }
                "network" => c.network = v.as_bool().context("network must be true or false")?,
                "remember_allowed" => {
                    c.remember_allowed = v
                        .as_bool()
                        .context("remember_allowed must be true or false")?
                }
                "paths" => c.paths = paths("paths")?,
                "ro_paths" => c.ro_paths = paths("ro_paths")?,
                "env" => {
                    c.env = v
                        .as_array()
                        .context("env must be a list of names")?
                        .iter()
                        .map(|n| {
                            n.as_str()
                                .filter(|n| {
                                    !n.is_empty()
                                        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                                })
                                .map(str::to_string)
                                .context("env holds variable names")
                        })
                        .collect::<Result<_>>()?
                }
                other => bail!("unknown key {other}"),
            }
        }
        Ok(c)
    }

    /// The variables that the command gets, from the signed host
    /// environment.
    pub fn pass_env(&self, host: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        host.iter()
            .filter(|(k, _)| BASE_ENV.contains(&k.as_str()) || self.env.iter().any(|e| e == *k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// How the command is isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// A transient systemd user unit.
    Systemd,
    /// bwrap, where there is no systemd user instance.
    Bwrap,
    /// `sandbox-exec` with a Seatbelt profile, on macOS.
    Seatbelt,
    /// No isolation: only for the unit tests of the broker.
    None,
}

/// The program that runs a command under Seatbelt.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The isolation that this host can give.
#[cfg(target_os = "macos")]
pub fn detect(env: &HostEnv) -> Result<Isolation> {
    let _ = env;
    if Path::new(SANDBOX_EXEC).is_file() {
        Ok(Isolation::Seatbelt)
    } else {
        bail!("exec needs {SANDBOX_EXEC} to run a command in isolation")
    }
}

/// The isolation that this host can give.
#[cfg(not(target_os = "macos"))]
pub fn detect(env: &HostEnv) -> Result<Isolation> {
    let systemd = env.which("systemd-run").is_some()
        && Command::new("systemctl")
            .args(["--user", "is-system-running"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                matches!(s.trim(), "running" | "degraded")
            })
            .unwrap_or(false);
    if systemd {
        return Ok(Isolation::Systemd);
    }
    let msg = "exec needs a systemd user instance or bwrap to run a command in isolation";
    match env.which("bwrap") {
        Some(bwrap) if bwrap_starts(&bwrap) => Ok(Isolation::Bwrap),
        Some(bwrap) => bail!(
            "{msg}: {} is on PATH but cannot start a sandbox here (for example inside another sandbox)",
            bwrap.display()
        ),
        None => bail!("{msg}"),
    }
}

/// bwrap can start a sandbox with a user namespace. It cannot inside a
/// sandbox that allows no nested namespace. paddock probes each bwrap once.
#[cfg(not(target_os = "macos"))]
fn bwrap_starts(bwrap: &Path) -> bool {
    static SEEN: std::sync::Mutex<BTreeMap<PathBuf, bool>> = std::sync::Mutex::new(BTreeMap::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    *seen.entry(bwrap.to_path_buf()).or_insert_with(|| {
        Command::new(bwrap)
            .args([
                "--unshare-user",
                "--disable-userns",
                "--ro-bind",
                "/",
                "/",
                "true",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// An isolated command, with the files that paddock made for it (the
/// Seatbelt profile, the empty home and `TMPDIR`). They are removed when
/// this is dropped, after the command.
pub struct Prepared {
    pub cmd: Command,
    scratch: Vec<PathBuf>,
}

impl std::ops::Deref for Prepared {
    type Target = Command;
    fn deref(&self) -> &Command {
        &self.cmd
    }
}

impl std::ops::DerefMut for Prepared {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.cmd
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        for p in &self.scratch {
            remove_scratch(p);
        }
    }
}

/// Remove a file or a folder of a command. `remove_dir_all` does not follow
/// a link that the command made inside its folder.
fn remove_scratch(p: &Path) {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// The command that runs `argv` in `project` with `cfg`, isolated. `run`
/// is the executor's run folder, where Seatbelt's files of the command go.
#[allow(clippy::too_many_arguments)]
pub fn command(
    how: Isolation,
    cfg: &ExecCfg,
    run: &Path,
    home: &Path,
    project: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout: u64,
    unit: &str,
) -> Result<Prepared> {
    command_in(
        how, cfg, run, home, project, project, argv, env, timeout, unit,
    )
}

/// Like `command`, with another working folder than the project: the
/// project stays writable, and `cwd` must be visible in the view of `cfg`.
#[allow(clippy::too_many_arguments)]
pub fn command_in(
    how: Isolation,
    cfg: &ExecCfg,
    run: &Path,
    home: &Path,
    project: &Path,
    cwd: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout: u64,
    unit: &str,
) -> Result<Prepared> {
    let cmd = match how {
        Isolation::Seatbelt => {
            return seatbelt_command(cfg, run, home, project, cwd, argv, env, unit)
        }
        _ => plain_command(how, cfg, home, project, cwd, argv, env, timeout, unit),
    };
    Ok(Prepared {
        cmd,
        scratch: Vec::new(),
    })
}

/// The shell line that goes to the working folder and runs the command:
/// `sh -c LINE paddock CWD ARGV...`.
pub const SEATBELT_SHELL: &str = r#"cd "$1" && shift && exec "$@""#;

/// The Seatbelt command: the profile in `<run>/exec/<unit>.sb` (mode 0600),
/// the empty home and `TMPDIR` in `<run>/exec/<unit>/`.
#[allow(clippy::too_many_arguments)]
fn seatbelt_command(
    cfg: &ExecCfg,
    run: &Path,
    home: &Path,
    project: &Path,
    cwd: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    unit: &str,
) -> Result<Prepared> {
    use std::os::unix::process::CommandExt;
    if unit.is_empty() || unit.contains('/') || unit.starts_with('.') {
        bail!("{unit:?} is not a name for a command's files");
    }
    let real = crate::backend::seatbelt::real;
    let base = run.join("exec");
    crate::privfs::dir(&base).with_context(|| format!("cannot create {}", base.display()))?;
    let dir = base.join(unit);
    let file = base.join(format!("{unit}.sb"));
    // A stale folder of an executor that was killed.
    remove_scratch(&dir);
    remove_scratch(&file);
    let mut p = Prepared {
        cmd: Command::new(SANDBOX_EXEC),
        scratch: vec![dir.clone(), file.clone()],
    };
    for d in ["home", "tmp"] {
        crate::privfs::dir(&dir.join(d))
            .with_context(|| format!("cannot create {}", dir.join(d).display()))?;
    }
    let real_list = |v: &[PathBuf]| v.iter().map(|d| real(d)).collect::<Vec<_>>();
    let cfg = ExecCfg {
        paths: real_list(&cfg.paths),
        ro_paths: real_list(&cfg.ro_paths),
        hide: real_list(&cfg.hide),
        ..cfg.clone()
    };
    let view = seatbelt::View {
        home: real(home),
        project: real(project),
        empty_home: real(&dir.join("home")),
        tmp: real(&dir.join("tmp")),
        user_cache: crate::backend::seatbelt::keychain::user_cache(),
        sockets: seatbelt::AGENT_SOCKETS
            .iter()
            .filter_map(|k| env.get(*k))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map(|p| real(&p))
            .collect(),
    };
    let profile = crate::seatbelt::render(&seatbelt::policy(&cfg, &view))?;
    crate::privfs::write(&file, profile)
        .with_context(|| format!("cannot write {}", file.display()))?;
    let mut vars = env.clone();
    vars.insert("HOME".into(), view.home_var(&cfg).display().to_string());
    vars.insert("TMPDIR".into(), format!("{}/", view.tmp.display()));
    vars.entry("PATH".into())
        .or_insert_with(|| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    // The shell starts in a folder that its profile can read: in the
    // folder of the executor, `getcwd` fails with EPERM at shell-init and
    // again at `cd`. PWD names the folder as the caller named it, so that
    // `pwd` prints /tmp/x and not /private/tmp/x.
    vars.insert("PWD".into(), cwd.display().to_string());
    p.cmd
        .current_dir(start_dir(cwd, &view.tmp))
        .arg("-f")
        .arg(&file)
        .args(["/bin/sh", "-c", SEATBELT_SHELL, "paddock"])
        .arg(cwd)
        .args(argv)
        .env_clear()
        .envs(vars)
        // Its own process group, so a timeout stops every process of it.
        .process_group(0);
    Ok(p)
}

/// Where the Seatbelt command starts: the working folder when it is one,
/// else `fallback` (its own TMPDIR), where `cd` then says what is wrong.
pub fn start_dir(cwd: &Path, fallback: &Path) -> PathBuf {
    if cwd.is_dir() {
        cwd.to_path_buf()
    } else {
        fallback.to_path_buf()
    }
}

#[allow(clippy::too_many_arguments)]
fn plain_command(
    how: Isolation,
    cfg: &ExecCfg,
    home: &Path,
    project: &Path,
    cwd: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout: u64,
    unit: &str,
) -> Command {
    let s = |p: &Path| p.display().to_string();
    match how {
        Isolation::None => {
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..]).current_dir(cwd).env_clear().envs(env);
            c
        }
        Isolation::Seatbelt => unreachable!("command_in builds the Seatbelt command"),
        Isolation::Systemd => {
            let mut c = Command::new("systemd-run");
            c.args(["--user", "--pipe", "--wait", "--collect", "--quiet"])
                .arg(format!("--unit={unit}"));
            let mut p: Vec<String> = vec![
                "ProtectSystem=strict".into(),
                "PrivateTmp=yes".into(),
                "NoNewPrivileges=yes".into(),
                "RestrictNamespaces=yes".into(),
                "RestrictSUIDSGID=yes".into(),
                "LockPersonality=yes".into(),
                "SystemCallFilter=@system-service".into(),
                format!("RuntimeMaxSec={}", timeout + 5),
                format!("WorkingDirectory={}", s(cwd)),
            ];
            for d in &cfg.hide {
                p.push(format!("TemporaryFileSystem={}", unit_path(d)));
            }
            match cfg.home {
                Home::Hidden => p.push("ProtectHome=tmpfs".into()),
                Home::ReadOnly => p.push("ProtectHome=read-only".into()),
                Home::Full => p.push(format!("ReadWritePaths={}", unit_path(home))),
            }
            p.push(format!("BindPaths={}", unit_path(project)));
            for d in &cfg.paths {
                p.push(format!("BindPaths={}", unit_path(d)));
            }
            for d in &cfg.ro_paths {
                p.push(format!("BindReadOnlyPaths={}", unit_path(d)));
            }
            if cfg.network {
                p.push("RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK".into());
            } else {
                p.push("PrivateNetwork=yes".into());
                p.push("RestrictAddressFamilies=AF_UNIX".into());
            }
            for prop in p {
                c.arg("-p").arg(prop);
            }
            // Only the names: systemd-run takes the values from its own
            // environment, so no token is on a host command line.
            for k in env.keys() {
                c.arg(format!("--setenv={k}"));
            }
            c.arg("--").args(argv);
            c.env_clear().envs(env);
            // systemd-run talks to the user manager over the runtime folder.
            for k in ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
                if let Ok(v) = std::env::var(k) {
                    c.env(k, v);
                }
            }
            c
        }
        Isolation::Bwrap => {
            let mut c = Command::new("bwrap");
            c.args([
                "--die-with-parent",
                "--new-session",
                "--unshare-pid",
                "--unshare-ipc",
                "--unshare-user",
                "--disable-userns",
            ]);
            c.args([
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--tmpfs",
                "/tmp",
            ]);
            for d in &cfg.hide {
                c.args(["--tmpfs", &s(d)]);
            }
            match cfg.home {
                Home::Hidden => {
                    c.args(["--tmpfs", &s(home)]);
                }
                Home::ReadOnly => {}
                Home::Full => {
                    c.args(["--bind", &s(home), &s(home)]);
                }
            }
            c.args(["--bind", &s(project), &s(project)]);
            for d in &cfg.paths {
                c.args(["--bind", &s(d), &s(d)]);
            }
            for d in &cfg.ro_paths {
                c.args(["--ro-bind", &s(d), &s(d)]);
            }
            if !cfg.network {
                c.arg("--unshare-net");
            }
            c.args(["--chdir", &s(cwd), "--"]).args(argv);
            c.env_clear().envs(env);
            c
        }
    }
}

/// A path in a list property of a unit (`BindPaths=`, `ReadWritePaths=`):
/// systemd splits these at white space and `:`, and takes a character after
/// `\` as it is.
fn unit_path(p: &Path) -> String {
    let mut out = String::new();
    for ch in p.display().to_string().chars() {
        if ch.is_whitespace() || matches!(ch, ':' | '\\' | '"' | '\'') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// systemd's own exit codes when a unit cannot start (for example 218 for
/// capabilities or 226 for a namespace), and `sandbox-exec`'s exit 65 for a
/// profile that it cannot apply, as text for the agent.
pub fn setup_failure(how: Isolation, code: i32, stderr: &str) -> Option<String> {
    match how {
        Isolation::Systemd => {
            ((200..=245).contains(&code) && stderr.trim().is_empty()).then(|| {
                format!(
                    "the isolated unit could not start (systemd exit code {code}); see journalctl --user"
                )
            })
        }
        Isolation::Seatbelt => {
            let first = stderr.trim_start().lines().next().unwrap_or("");
            (code == 65 && first.starts_with("sandbox-exec:")).then(|| {
                format!("the Seatbelt sandbox of the command could not start: {first}")
            })
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "execbox_tests.rs"]
mod tests;
