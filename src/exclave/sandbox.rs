//! Construction of the peer execution boundary.
//!
//! A transient systemd scope accounts for the entire process tree. The
//! command is refused if that boundary cannot be started; per-process rlimits
//! do not limit a build that forks.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

// The transient scope must actually have the controllers enabled. Checking
// from inside the scope avoids relying on a successful systemd-run exit alone.
const VERIFY_LIMITS: &str = r#"
mem_limit=$1; cpu_limit=$2; shift 2
cg=
while IFS=: read -r id controllers path; do
    if [ "$id" = 0 ] && [ -z "$controllers" ]; then cg=$path; break; fi
done < /proc/self/cgroup
case "$cg" in /*) ;; *) echo 'exclave: cgroup v2 unavailable' >&2; exit 125;; esac
base=/sys/fs/cgroup$cg
if ! read -r memory < "$base/memory.max" || ! read -r swap < "$base/memory.swap.max" || ! read -r quota period < "$base/cpu.max"; then
    echo 'exclave: resource controllers unavailable' >&2; exit 125
fi
case "$memory:$swap:$quota:$period" in *[!0-9:]*) echo 'exclave: resource limits unavailable' >&2; exit 125;; esac
if [ -z "$memory" ] || [ -z "$swap" ] || [ -z "$quota" ] || [ -z "$period" ] || [ "$memory" -gt "$mem_limit" ] || [ "$swap" -ne 0 ] || [ "$quota" -gt $((period * cpu_limit / 100)) ]; then
    echo 'exclave: resource limits not enforced' >&2; exit 125
fi
exec "$@"
"#;

const SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The peer user's rustup toolchains and proxies, as `detect::Rustup` gives
/// them to a sandbox: both read-only, the proxies first on PATH. Without
/// them `cargo` exists only where the system installs it in /usr. The peer
/// owner turns this off with `[exclave.serve] rustup = false`.
///
/// The binds need no `ro_paths` entry, so a link must not widen them: a
/// source that resolves to the home, an ancestor of it, a home bin folder or
/// a folder that holds Cargo credentials is refused, and so is the run.
fn rustup_toolchain(home: &Path) -> Result<Option<(std::path::PathBuf, std::path::PathBuf)>> {
    let (Ok(rustup), Ok(bin)) = (
        std::fs::canonicalize(home.join(".rustup")),
        std::fs::canonicalize(home.join(".cargo/bin")),
    ) else {
        return Ok(None);
    };
    if !(rustup.join("toolchains").is_dir() && bin.join("cargo").is_file()) {
        return Ok(None);
    }
    let real_home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let mut protected = vec![real_home.clone()];
    for inside in [
        ".local/bin",
        "bin",
        ".cargo/credentials.toml",
        ".cargo/credentials",
    ] {
        protected.push(real_home.join(inside));
        if let Ok(p) = std::fs::canonicalize(home.join(inside)) {
            protected.push(p);
        }
    }
    for (name, source) in [("~/.rustup", &rustup), ("~/.cargo/bin", &bin)] {
        if protected.iter().any(|p| p.starts_with(source)) {
            bail!(
                "exclave: {name} resolves to {}, which holds the home, a home bin folder or Cargo credentials; fix the link or set [exclave.serve] rustup = false",
                source.display()
            );
        }
    }
    Ok(Some((rustup, bin)))
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub memory_bytes: u64,
    pub cpu_percent: u32,
    pub seconds: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            cpu_percent: 200,
            seconds: 3600,
        }
    }
}

/// Render an offline peer job. Project and cache are the only writable binds.
/// No inherited environment, peer home, session sockets or network enter it.
#[allow(clippy::too_many_arguments)]
pub fn offline(
    project: &Path,
    cache: &Path,
    cargo: &Path,
    python: &Path,
    argv: &[String],
    limits: &Limits,
    env: &BTreeMap<String, String>,
    gpu: bool,
    ro_paths: &[std::path::PathBuf],
    rustup_home: Option<&Path>,
) -> Result<Command> {
    command(
        project,
        cache,
        cargo,
        python,
        argv,
        limits,
        env,
        gpu,
        ro_paths,
        rustup_home,
        None,
        None,
    )
}

/// With a host-owned, allowlisted registry proxy socket.
#[allow(clippy::too_many_arguments)]
pub fn registries(
    project: &Path,
    cache: &Path,
    cargo: &Path,
    python: &Path,
    argv: &[String],
    limits: &Limits,
    socket: &Path,
    executable: &Path,
    env: &BTreeMap<String, String>,
    gpu: bool,
    ro_paths: &[std::path::PathBuf],
    rustup_home: Option<&Path>,
) -> Result<Command> {
    command(
        project,
        cache,
        cargo,
        python,
        argv,
        limits,
        env,
        gpu,
        ro_paths,
        rustup_home,
        Some((socket, executable)),
        None,
    )
}

/// Egress remains subject to the peer's network isolation and LAN blocks.
#[allow(clippy::too_many_arguments)]
pub fn egress(
    env: &crate::env::HostEnv,
    project: &Path,
    cache: &Path,
    cargo: &Path,
    python: &Path,
    argv: &[String],
    limits: &Limits,
    resolv: &Path,
    vars: &BTreeMap<String, String>,
    gpu: bool,
    ro_paths: &[std::path::PathBuf],
    rustup: bool,
) -> Result<Command> {
    let spec = crate::spec::SandboxSpec::new(
        "exclave".into(),
        "exclave",
        project,
        cache.to_path_buf(),
        &env.home,
        env.uid,
        env.gid,
        &env.user,
    );
    let mut clean = env.clone();
    for key in crate::proxy::PROXY_VARS {
        clean.vars.remove(*key);
    }
    let (pasta, args) = crate::backend::bwrap::pasta_prefix(&clean, &spec)?;
    command(
        project,
        cache,
        cargo,
        python,
        argv,
        limits,
        vars,
        gpu,
        ro_paths,
        rustup.then_some(env.home.as_path()),
        None,
        Some((pasta, args, resolv.to_path_buf())),
    )
}

#[allow(clippy::too_many_arguments)]
fn command(
    project: &Path,
    cache: &Path,
    cargo: &Path,
    python: &Path,
    argv: &[String],
    limits: &Limits,
    vars: &BTreeMap<String, String>,
    gpu: bool,
    ro_paths: &[std::path::PathBuf],
    rustup_home: Option<&Path>,
    proxy: Option<(&Path, &Path)>,
    egress: Option<(std::path::PathBuf, Vec<String>, std::path::PathBuf)>,
) -> Result<Command> {
    if argv.is_empty() || argv[0].is_empty() {
        bail!("empty exclave command");
    }
    if !project.is_absolute()
        || !cache.is_absolute()
        || !cargo.is_absolute()
        || !python.is_absolute()
        || !project.is_dir()
        || !cache.is_dir()
        || !cargo.is_dir()
        || !python.is_dir()
    {
        bail!("exclave project and cache must be existing absolute directories");
    }
    if limits.memory_bytes == 0 || limits.cpu_percent == 0 || limits.seconds == 0 {
        bail!("exclave limits must be positive");
    }
    let venv = project.join(".venv");
    match std::fs::symlink_metadata(&venv) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&venv)?,
        _ => bail!("project .venv must be a directory"),
    }
    let mut c = Command::new("systemd-run");
    c.args(["--user", "--scope", "--quiet", "--collect", "-p"])
        .arg(format!("MemoryMax={}", limits.memory_bytes))
        .args(["-p", "MemorySwapMax=0"])
        .arg("-p")
        .arg(format!("CPUQuota={}%", limits.cpu_percent))
        .arg("-p")
        .arg(format!("RuntimeMaxSec={}", limits.seconds))
        .arg("--")
        .args([
            "nice",
            "-n",
            "10",
            "/bin/sh",
            "-c",
            VERIFY_LIMITS,
            "exclave-check",
        ])
        .arg(limits.memory_bytes.to_string())
        .arg(limits.cpu_percent.to_string());
    if let Some((program, args, _)) = &egress {
        c.arg(program).args(args);
    }
    c.args([
        "bwrap",
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--ro-bind",
        "/usr",
        "/usr",
    ]);
    if egress.is_some() {
        c.arg("--share-net");
    }
    for (source, dest) in [("/bin", "/bin"), ("/lib", "/lib"), ("/lib64", "/lib64")] {
        if let Ok(link) = std::fs::read_link(source) {
            c.arg("--symlink").arg(link).arg(dest);
        } else if Path::new(source).is_dir() {
            c.args(["--ro-bind", source, dest]);
        }
    }
    c.args(["--proc", "/proc", "--dev", "/dev"]);
    if gpu {
        bind_gpu(&mut c, Path::new("/dev"), Path::new("/sys"), true)?;
    }
    c.args([
        "--tmpfs",
        "/tmp",
        "--dir",
        "/work",
        "--dir",
        "/cache",
        "--dir",
        "/cache/target",
        "--dir",
        "/cache/cargo",
        "--dir",
        "/etc",
        "--dir",
        "/etc/ssl",
        "--dir",
        "/etc/ssl/certs",
        "--tmpfs",
        "/home",
    ])
    .arg("--bind")
    .arg(project)
    .arg("/work")
    .arg("--bind")
    .arg(cache)
    .arg("/cache/target")
    .arg("--bind")
    .arg(cargo)
    .arg("/cache/cargo")
    .arg("--bind")
    .arg(python)
    .arg("/cache/python")
    .arg("--bind")
    .arg(python.join("venv"))
    .arg("/work/.venv");
    for path in ro_paths {
        c.arg("--ro-bind").arg(path).arg(path);
    }
    let mut path = String::from(SYSTEM_PATH);
    let toolchain = match rustup_home {
        Some(home) => rustup_toolchain(home)?,
        None => None,
    };
    if let Some((rustup, bin)) = toolchain {
        c.arg("--ro-bind")
            .arg(rustup)
            .arg("/home/.rustup")
            .arg("--ro-bind")
            .arg(bin)
            .arg("/home/.cargo/bin")
            .args(["--setenv", "RUSTUP_HOME", "/home/.rustup"]);
        path = format!("/home/.cargo/bin:{SYSTEM_PATH}");
    }
    c.args([
        "--chdir",
        "/work",
        "--clearenv",
        "--setenv",
        "HOME",
        "/home",
        "--setenv",
        "PATH",
        &path,
        "--setenv",
        "CARGO_TARGET_DIR",
        "/cache/target",
        "--setenv",
        "CARGO_HOME",
        "/cache/cargo",
        "--setenv",
        "PIP_CACHE_DIR",
        "/cache/python/pip",
        "--setenv",
        "UV_CACHE_DIR",
        "/cache/python/uv",
        "--setenv",
        "UV_PROJECT_ENVIRONMENT",
        "/work/.venv",
    ]);
    for (name, value) in vars {
        c.arg("--setenv").arg(name).arg(value);
    }
    if let Some(bundle) = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/cert.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
    ]
    .iter()
    .filter_map(|p| std::fs::canonicalize(p).ok())
    .find(|p| p.is_file())
    {
        c.arg("--ro-bind")
            .arg(bundle)
            .arg("/etc/ssl/certs/ca-certificates.crt")
            .args([
                "--setenv",
                "SSL_CERT_FILE",
                "/etc/ssl/certs/ca-certificates.crt",
                "--setenv",
                "CURL_CA_BUNDLE",
                "/etc/ssl/certs/ca-certificates.crt",
            ]);
    }
    if let Some((_, _, resolv)) = egress {
        c.args(["--ro-bind"]).arg(resolv).arg("/etc/resolv.conf");
    }
    if let Some((socket, executable)) = proxy {
        c.args(["--dir", "/run", "--dir", "/run/paddock", "--ro-bind"]).arg(socket).arg("/run/paddock/proxy.sock")
            .arg("--ro-bind").arg(executable).arg("/run/paddock/paddock")
            .args(["--setenv", "HTTP_PROXY", "http://127.0.0.1:3131", "--setenv", "HTTPS_PROXY", "http://127.0.0.1:3131", "--setenv", "http_proxy", "http://127.0.0.1:3131", "--setenv", "https_proxy", "http://127.0.0.1:3131", "--", "/bin/sh", "-c", " /run/paddock/paddock relay --listen 127.0.0.1:3131 --socket /run/paddock/proxy.sock --ready-file /tmp/.relay-ready >/dev/null 2>&1 & i=0; while [ ! -e /tmp/.relay-ready ]; do i=$((i+1)); [ $i -lt 100 ] || exit 125; sleep 0.05; done; exec \"$@\" ", "exclave"]);
    } else {
        c.arg("--");
    }
    c.args(argv);
    // systemd-run needs the user's manager address, but the sandbox starts
    // with a separate cleared environment above.
    let _ = std::env::var_os("PATH").context("PATH is required for peer execution")?;
    match manager_runtime_dir(
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .as_deref(),
        // SAFETY: getuid has no failure case.
        unsafe { libc::getuid() },
    ) {
        ManagerDir::Keep => {}
        ManagerDir::Use(dir) => {
            c.env("XDG_RUNTIME_DIR", dir);
        }
        ManagerDir::Unset => {
            c.env_remove("XDG_RUNTIME_DIR");
        }
    }
    Ok(c)
}

/// The XDG_RUNTIME_DIR that `systemd-run --user --scope` gets.
#[derive(Debug, PartialEq, Eq)]
enum ManagerDir {
    Keep,
    Use(std::path::PathBuf),
    Unset,
}

/// `systemd-run --user --scope` connects only to the private socket of the
/// user manager, `$XDG_RUNTIME_DIR/systemd/private`. It falls back to
/// DBUS_SESSION_BUS_ADDRESS only when XDG_RUNTIME_DIR is not set, not when
/// the socket is missing (systemd 262, bus_connect_transport_systemd). A
/// peer whose runtime folder is not the one of the manager (a scratch
/// folder, or the /tmp fallback of a session without pam_systemd) failed
/// with "Failed to connect to user scope bus via local transport". Then
/// systemd-run gets /run/user/UID when the manager listens there, else no
/// XDG_RUNTIME_DIR, so that it uses the session bus. Only systemd-run and
/// the commands before bwrap see this value: bwrap clears the environment.
fn manager_runtime_dir(current: Option<&Path>, uid: u32) -> ManagerDir {
    manager_runtime_dir_in(current, &Path::new("/run/user").join(uid.to_string()))
}

fn manager_runtime_dir_in(current: Option<&Path>, standard: &Path) -> ManagerDir {
    let private = |d: &Path| d.join("systemd/private").exists();
    match current {
        None => ManagerDir::Keep,
        Some(d) if !d.as_os_str().is_empty() && private(d) => ManagerDir::Keep,
        Some(_) if private(standard) => ManagerDir::Use(standard.to_path_buf()),
        Some(_) => ManagerDir::Unset,
    }
}

fn bind_gpu(c: &mut Command, dev: &Path, sys: &Path, require_device: bool) -> Result<()> {
    let nodes = gpu_nodes(dev, require_device)?;
    if nodes.is_empty() {
        bail!("no NVIDIA devices available");
    }
    for node in nodes {
        c.arg("--dev-bind").arg(&node).arg(&node);
    }
    c.arg("--ro-bind").arg(sys).arg("/sys");
    Ok(())
}

fn gpu_nodes(root: &Path, require_device: bool) -> Result<Vec<std::path::PathBuf>> {
    use std::os::unix::fs::FileTypeExt;
    let mut nodes = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let numbered = name
            .strip_prefix("nvidia")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
        if numbered
            || matches!(
                name.as_ref(),
                "nvidiactl" | "nvidia-uvm" | "nvidia-uvm-tools" | "nvidia-modeset"
            )
        {
            let kind = entry.file_type()?;
            if kind.is_symlink()
                || (require_device && !kind.is_char_device())
                || (!require_device && !kind.is_file())
            {
                continue;
            }
            nodes.push(entry.path());
        }
    }
    nodes.sort();
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_run_gets_the_runtime_folder_of_the_user_manager() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("scratch");
        let standard = dir.path().join("standard");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::create_dir_all(standard.join("systemd")).unwrap();
        // No XDG_RUNTIME_DIR: systemd-run uses the session bus by itself.
        assert_eq!(manager_runtime_dir_in(None, &standard), ManagerDir::Keep);
        // A scratch folder: the folder where the manager listens.
        std::fs::write(standard.join("systemd/private"), "").unwrap();
        assert_eq!(
            manager_runtime_dir_in(Some(&scratch), &standard),
            ManagerDir::Use(standard.clone())
        );
        // The folder of the manager stays.
        std::fs::create_dir_all(scratch.join("systemd")).unwrap();
        std::fs::write(scratch.join("systemd/private"), "").unwrap();
        assert_eq!(
            manager_runtime_dir_in(Some(&scratch), &standard),
            ManagerDir::Keep
        );
        // No private socket anywhere: the session bus.
        std::fs::remove_file(scratch.join("systemd/private")).unwrap();
        std::fs::remove_file(standard.join("systemd/private")).unwrap();
        assert_eq!(
            manager_runtime_dir_in(Some(&scratch), &standard),
            ManagerDir::Unset
        );
    }

    #[test]
    fn fake_gpu_nodes_are_selected_by_name_without_following_links() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "nvidia0",
            "nvidiactl",
            "nvidia-uvm",
            "nvidia-uvm-tools",
            "nvidia-modeset",
            "nvidia-other",
            "not-nvidia",
        ] {
            std::fs::write(dir.path().join(name), "fake").unwrap();
        }
        std::os::unix::fs::symlink("nvidia0", dir.path().join("nvidia1")).unwrap();
        let nodes = gpu_nodes(dir.path(), false).unwrap();
        assert_eq!(nodes.len(), 5);
        assert!(gpu_nodes(dir.path(), true).unwrap().is_empty());
        let sys = dir.path().join("sys");
        std::fs::create_dir(&sys).unwrap();
        std::fs::write(sys.join("fake-pci"), "gpu").unwrap();
        let mut cmd = Command::new("bwrap");
        bind_gpu(&mut cmd, dir.path(), &sys, false).unwrap();
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args
            .windows(3)
            .any(|a| a[0] == "--ro-bind" && a[1] == sys.as_os_str() && a[2] == "/sys"));
        assert_eq!(args.iter().filter(|a| **a == "--dev-bind").count(), 5);
    }

    #[test]
    fn refuses_to_start_when_limits_are_not_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let status = Command::new("/bin/sh")
            .args([
                "-c",
                VERIFY_LIMITS,
                "exclave-check",
                "1",
                "1",
                "/usr/bin/touch",
            ])
            .arg(&marker)
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(125));
        assert!(!marker.exists());
    }

    #[test]
    fn sandbox_limits_the_tree_and_does_not_share_network() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let cache = dir.path().join("cache");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(&cache).unwrap();
        let cargo = dir.path().join("cargo");
        std::fs::create_dir(&cargo).unwrap();
        let python = dir.path().join("python");
        std::fs::create_dir(&python).unwrap();
        std::fs::create_dir(python.join("venv")).unwrap();
        let tools = dir.path().join("tools");
        std::fs::create_dir(&tools).unwrap();
        std::fs::write(tools.join("uv"), "fake tool").unwrap();
        let link = dir.path().join("uv");
        std::os::unix::fs::symlink(tools.join("uv"), &link).unwrap();
        assert_eq!(std::fs::read(&link).unwrap(), b"fake tool");
        let vars = BTreeMap::from([(
            "PIP_INDEX_URL".into(),
            "https://mirror.example/simple".into(),
        )]);
        let cmd = offline(
            &project,
            &cache,
            &cargo,
            &python,
            &["true".into()],
            &Limits::default(),
            &vars,
            false,
            std::slice::from_ref(&tools),
            Some(&dir.path().join("home")),
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "MemoryMax=4294967296",
            "MemorySwapMax=0",
            "CPUQuota=200%",
            "RuntimeMaxSec=3600",
            "nice",
            "10",
            "--unshare-all",
            "--clearenv",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        ] {
            assert!(args.iter().any(|s| s == expected), "missing {expected}");
        }
        assert!(!args.iter().any(|s| s == "--share-net"));
        assert!(!args.iter().any(|s| s == "--dev-bind"));
        assert!(!args.iter().any(|s| s == "/sys"));
        let tools = tools.to_str().unwrap();
        assert!(args.windows(3).any(|a| a == ["--ro-bind", tools, tools]));
        assert!(args
            .windows(3)
            .any(|a| a == ["--setenv", "PIP_INDEX_URL", "https://mirror.example/simple"]));
        assert!(!args.iter().any(|s| s == "RUSTUP_HOME"));
        // A peer user with rustup gets its toolchain read-only, as a sandbox does.
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
        std::fs::create_dir_all(home.join(".cargo/bin")).unwrap();
        std::fs::write(home.join(".cargo/bin/cargo"), "proxy").unwrap();
        let cmd = offline(
            &project,
            &cache,
            &cargo,
            &python,
            &["true".into()],
            &Limits::default(),
            &vars,
            false,
            &[],
            Some(&home),
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let rustup = std::fs::canonicalize(home.join(".rustup")).unwrap();
        let bin = std::fs::canonicalize(home.join(".cargo/bin")).unwrap();
        let (rustup, bin) = (rustup.to_str().unwrap(), bin.to_str().unwrap());
        assert!(args
            .windows(3)
            .any(|a| a == ["--ro-bind", rustup, "/home/.rustup"]));
        assert!(args
            .windows(3)
            .any(|a| a == ["--ro-bind", bin, "/home/.cargo/bin"]));
        assert!(args
            .windows(3)
            .any(|a| a == ["--setenv", "RUSTUP_HOME", "/home/.rustup"]));
        assert!(args.windows(3).any(|a| a
            == [
                "--setenv",
                "PATH",
                "/home/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
            ]));
    }

    fn offline_args(home: Option<&Path>) -> Result<Vec<String>> {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python");
        std::fs::create_dir_all(python.join("venv")).unwrap();
        let cmd = offline(
            dir.path(),
            dir.path(),
            dir.path(),
            &python,
            &["true".into()],
            &Limits::default(),
            &BTreeMap::new(),
            false,
            &[],
            home,
        )?;
        Ok(cmd
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect())
    }

    fn path_of(args: &[String]) -> &str {
        let i = args
            .windows(2)
            .position(|a| a == ["--setenv", "PATH"])
            .expect("PATH is set");
        &args[i + 2]
    }

    #[test]
    fn the_peer_owner_can_turn_off_the_rustup_binds() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
        std::fs::create_dir_all(home.join(".cargo/bin")).unwrap();
        std::fs::write(home.join(".cargo/bin/cargo"), "proxy").unwrap();
        let on = offline_args(Some(&home)).unwrap();
        assert_eq!(path_of(&on), format!("/home/.cargo/bin:{SYSTEM_PATH}"));
        let off = offline_args(None).unwrap();
        assert_eq!(path_of(&off), SYSTEM_PATH);
        assert!(!off
            .iter()
            .any(|s| s == "RUSTUP_HOME" || s == "/home/.rustup" || s == "/home/.cargo/bin"));
    }

    #[test]
    fn a_rustup_link_to_the_home_or_a_home_bin_folder_is_refused() {
        // Each case links one source into a folder that must not reach every
        // run. The other source stays a plain toolchain folder.
        type Target = fn(&Path) -> std::path::PathBuf;
        let cases: [(&str, Target); 5] = [
            (".rustup", |h| h.to_path_buf()),
            (".rustup", |h| h.parent().unwrap().to_path_buf()),
            (".cargo/bin", |h| h.join(".local/bin")),
            (".cargo/bin", |h| h.join("bin")),
            (".cargo/bin", |h| h.join(".cargo")),
        ];
        for (source, target) in cases {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            std::fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
            std::fs::create_dir_all(home.join(".cargo/bin")).unwrap();
            std::fs::create_dir_all(home.join(".local/bin")).unwrap();
            std::fs::create_dir_all(home.join("bin")).unwrap();
            std::fs::create_dir_all(home.join("toolchains")).unwrap();
            std::fs::create_dir_all(dir.path().join("toolchains")).unwrap();
            std::fs::write(home.join(".cargo/credentials.toml"), "token").unwrap();
            for bin in [".local/bin", "bin", ".cargo"] {
                std::fs::write(home.join(bin).join("cargo"), "proxy").unwrap();
            }
            std::fs::write(home.join(".cargo/bin/cargo"), "proxy").unwrap();
            let target = target(&home);
            let link = home.join(source);
            std::fs::remove_dir_all(&link).unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let err =
                offline_args(Some(&home)).expect_err(&format!("{source} -> {}", target.display()));
            assert!(
                err.to_string().contains("rustup = false"),
                "{source} -> {}: {err}",
                target.display()
            );
        }
        // The same home with plain folders binds both.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".rustup/toolchains")).unwrap();
        std::fs::create_dir_all(home.join(".cargo/bin")).unwrap();
        std::fs::write(home.join(".cargo/bin/cargo"), "proxy").unwrap();
        std::fs::write(home.join(".cargo/credentials.toml"), "token").unwrap();
        let args = offline_args(Some(&home)).unwrap();
        assert_eq!(path_of(&args), format!("/home/.cargo/bin:{SYSTEM_PATH}"));
    }
}
