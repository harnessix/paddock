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
) -> Result<Command> {
    command(
        project, cache, cargo, python, argv, limits, env, gpu, ro_paths, None, None,
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
    c.args([
        "--chdir",
        "/work",
        "--clearenv",
        "--setenv",
        "HOME",
        "/home",
        "--setenv",
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
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
    Ok(c)
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
    }
}
