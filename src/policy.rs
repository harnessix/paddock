//! The policy engine runs last. It never adds access. It refuses a spec that
//! breaks an invariant and names the source of the offending entry.

use crate::env::HostEnv;
use crate::spec::{MountKind, SandboxSpec};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

mod git;

/// Paths under HOME that hold credentials or private data. They are never
/// mounted, and no mount may contain them.
pub const DENIED_HOME: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".password-store",
    ".netrc",
    ".git-credentials",
    ".gitconfig",
    ".config/git",
    ".config/gh",
    ".config/hub",
    ".docker",
    ".kube",
    ".aws",
    ".azure",
    ".config/gcloud",
    ".oci",
    ".terraform.d",
    ".config/rclone",
    ".local/share/keyrings",
    ".pki",
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    ".config/BraveSoftware",
    ".config/vivaldi",
    ".thunderbird",
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".npmrc",
    ".pypirc",
    ".config/op",
    ".config/Bitwarden",
];

/// Host sockets that give control over the host.
pub const DENIED_ABS: &[&str] = &[
    "/var/run/docker.sock",
    "/run/docker.sock",
    "/run/containerd",
    "/run/podman",
    "/var/run/libvirt",
    "/run/dbus",
    "/proc",
    "/sys",
    "/dev",
    "/boot",
    "/root",
];

/// Host variables that are never inherited: the sandbox sets them itself, or
/// they name a socket or control channel of the host session.
pub const HOST_ONLY_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "SHELL",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "_",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_ID",
    "XDG_SEAT",
    "XDG_VTNR",
    "XDG_SESSION_CLASS",
    "XDG_SESSION_TYPE",
    "DBUS_SESSION_BUS_ADDRESS",
    "DBUS_SYSTEM_BUS_ADDRESS",
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "DISPLAY",
    "XAUTHORITY",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "GPG_AGENT_INFO",
    "GNOME_KEYRING_CONTROL",
    "SESSION_MANAGER",
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "CONTAINER_HOST",
    "PODMAN_HOST",
    "KUBECONFIG",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "HYPRLAND_CMD",
    "SWAYSOCK",
    "I3SOCK",
    "NIRI_SOCKET",
    "KITTY_LISTEN_ON",
    "KITTY_PID",
    "KITTY_WINDOW_ID",
    "TMUX",
    "TMUX_PANE",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "STY",
    "WEZTERM_UNIX_SOCKET",
    "NVIM",
    "NVIM_LISTEN_ADDRESS",
    "VSCODE_IPC_HOOK_CLI",
    "VSCODE_GIT_IPC_HANDLE",
    "MANAGERPID",
    "INVOCATION_ID",
    "JOURNAL_STREAM",
    "SYSTEMD_EXEC_PID",
    "NOTIFY_SOCKET",
    "LISTEN_FDS",
    "LISTEN_PID",
    "MISE_SHELL",
    "LD_PRELOAD",
    "LD_AUDIT",
    "PADDOCK",
    "PADDOCK_ID",
    "PADDOCK_HARNESS",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    // The Claude app's control channel to the sessions it started. A
    // sandbox started from such a session must not talk to the host app.
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_SSE_PORT",
];

/// Variables that redirect package caches. Inheritance drops them so the
/// sandbox uses the shared cache mounts. `[env] pass` can still hand one over.
pub const CACHE_ENV: &[&str] = &[
    "XDG_CACHE_HOME",
    "CARGO_HOME",
    "CARGO_TARGET_DIR",
    "RUSTUP_HOME",
    "UV_CACHE_DIR",
    "PIP_CACHE_DIR",
    "npm_config_cache",
    "NPM_CONFIG_CACHE",
    "PNPM_STORE_DIR",
    "YARN_CACHE_FOLDER",
    "GOPATH",
    "GOCACHE",
    "GOMODCACHE",
];

/// Whether a host variable can be inherited. Besides the fixed list, any
/// variable whose value is the path of a socket, or whose name says it holds
/// one, is a host control channel and stays outside.
pub fn inheritable_var(name: &str, value: &str) -> bool {
    if !inheritable(name) {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    if upper.ends_with("_SOCK")
        || upper.ends_with("_SOCKET")
        || upper.contains("_IPC_")
        || upper.ends_with("_IPC")
    {
        return false;
    }
    let path = value.strip_prefix("unix:").unwrap_or(value);
    if path.starts_with('/') {
        use std::os::unix::fs::FileTypeExt;
        if std::fs::metadata(path)
            .map(|m| m.file_type().is_socket())
            .unwrap_or(false)
        {
            return false;
        }
    }
    true
}

/// High-privilege cloud and infrastructure credentials. Inheritance drops
/// them by default: they can create, delete or bill resources far beyond the
/// project. `[env] pass` or a profile's `env_pass` lets one through by name.
pub const CLOUD_SECRETS: &[&str] = &[
    // AWS
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_SECURITY_TOKEN",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
    // Azure
    "AZURE_CLIENT_SECRET",
    "AZURE_CLIENT_CERTIFICATE_PATH",
    "AZURE_CLIENT_CERTIFICATE_PASSWORD",
    "AZURE_PASSWORD",
    "AZURE_STORAGE_KEY",
    "AZURE_STORAGE_CONNECTION_STRING",
    "ARM_CLIENT_SECRET",
    "ARM_ACCESS_KEY",
    // Google Cloud
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CREDENTIALS",
    "GOOGLE_OAUTH_ACCESS_TOKEN",
    "CLOUDSDK_AUTH_ACCESS_TOKEN",
    "GOOGLE_GHA_CREDS_PATH",
    // Other clouds and CDNs
    "DIGITALOCEAN_ACCESS_TOKEN",
    "DIGITALOCEAN_TOKEN",
    "HCLOUD_TOKEN",
    "LINODE_TOKEN",
    "VULTR_API_KEY",
    "CLOUDFLARE_API_TOKEN",
    "CLOUDFLARE_API_KEY",
    "CF_API_TOKEN",
    "CF_API_KEY",
    "IBMCLOUD_API_KEY",
    "ALIBABA_CLOUD_ACCESS_KEY_ID",
    "ALIBABA_CLOUD_ACCESS_KEY_SECRET",
    "ALIBABA_CLOUD_SECURITY_TOKEN",
    "ALICLOUD_SECRET_KEY",
    "TENCENTCLOUD_SECRET_KEY",
    "HUAWEICLOUD_SDK_SK",
    "OCI_CLI_KEY_FILE",
    "SCW_SECRET_KEY",
    "OS_PASSWORD",
    // Infrastructure control planes
    "VAULT_TOKEN",
    "NOMAD_TOKEN",
    "CONSUL_HTTP_TOKEN",
    "TFE_TOKEN",
    "HCP_CLIENT_SECRET",
    "PULUMI_ACCESS_TOKEN",
    // Code forges. The host pushes and runs gh for the sandbox (git-push and
    // forge in broker.rs), so the token never needs to enter it.
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "GL_TOKEN",
    "GITEA_TOKEN",
];

pub fn is_cloud_secret(name: &str) -> bool {
    CLOUD_SECRETS.contains(&name) || name.starts_with("TF_TOKEN_")
}

pub fn inheritable(name: &str) -> bool {
    !HOST_ONLY_ENV.contains(&name)
        && !CACHE_ENV.contains(&name)
        && !name.starts_with("__MISE")
        && !name.starts_with("PADDOCK_")
        && !name.starts_with("BASH_FUNC_")
        && !name.contains(['=', '\0'])
        && !CODE_LOADER_ENV.contains(&name)
        && !name.starts_with("DYLD_")
        && !HISTORY_ENV.contains(&name)
}

/// Host variables that make a program load code: a shell start file, a
/// command before each prompt, a library. `DYLD_*` counts too.
pub const CODE_LOADER_ENV: &[&str] = &[
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
    "LD_PRELOAD",
    "LD_AUDIT",
];

/// Host variables that point at the shell history or the shell start files.
pub const HISTORY_ENV: &[&str] = &["HISTFILE", "ZDOTDIR"];

/// Host variables that the sandbox sets itself.
const SANDBOX_SETS: &[&str] = &[
    "HOME", "USER", "LOGNAME", "PATH", "SHELL", "PWD", "OLDPWD", "SHLVL", "_", "TMPDIR", "TMP",
    "TEMP",
];

/// Why a host variable never passes. No setting changes it, also not in
/// `inherit` mode, and `[env] pass` cannot name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Never {
    /// A socket or control channel of the host session.
    Socket,
    /// The sandbox sets its own value.
    Sandbox,
    /// It makes programs load code.
    Loader,
    /// It points at the shell history or the shell start files.
    History,
}

impl Never {
    pub fn name(self) -> &'static str {
        match self {
            Never::Socket => "socket",
            Never::Sandbox => "sandbox",
            Never::Loader => "loader",
            Never::History => "history",
        }
    }
}

/// Whether a host variable is on the never-pass list, and why. `value`:
/// its host value, when there is one; a value that is the path of a
/// socket makes any name a socket.
pub fn never_pass(name: &str, value: Option<&str>) -> Option<Never> {
    if CODE_LOADER_ENV.contains(&name)
        || name.starts_with("DYLD_")
        || name.starts_with("BASH_FUNC_")
    {
        return Some(Never::Loader);
    }
    if HISTORY_ENV.contains(&name) {
        return Some(Never::History);
    }
    if SANDBOX_SETS.contains(&name)
        || name.starts_with("PADDOCK")
        || name.starts_with("__MISE")
        || name.contains(['=', '\0'])
        || name.is_empty()
    {
        return Some(Never::Sandbox);
    }
    if HOST_ONLY_ENV.contains(&name) {
        return Some(Never::Socket);
    }
    if CACHE_ENV.contains(&name) {
        return None;
    }
    if !inheritable_var(name, value.unwrap_or("")) {
        return Some(Never::Socket);
    }
    None
}

/// The sentence that refuses a variable on the never-pass list: the
/// reason, then the safe way. The CLI, the screens and the sandbox use
/// the same words.
pub fn never_sentence(name: &str, why: Never) -> String {
    let safe = match (name, why) {
        ("SSH_AUTH_SOCK", _) => "To let the agent use ssh keys, add a key grant with via = \"agent\" and the public keys in [credentials.NAME]".to_string(),
        (_, Never::Socket) => "A socket of the host gives control over this computer, so no setting passes it".to_string(),
        (_, Never::Sandbox) => "The sandbox sets its own value. To change it, set the value in the project's own scripts".to_string(),
        (_, Never::Loader) => "It makes programs load code from this computer. Put the code in the project instead".to_string(),
        (_, Never::History) => "It points at your shell history or your shell start files. The sandbox keeps its own".to_string(),
    };
    let what = match why {
        Never::Socket => "it is a socket or control channel of this computer",
        Never::Sandbox => "the sandbox sets it",
        Never::Loader => "it makes programs load code",
        Never::History => "it points at your shell history",
    };
    format!("{name} stays on this computer: {what}. {safe}.")
}

/// Environment names that look like secrets. They pass only when a profile or
/// manifest names them explicitly.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "APIKEY",
        "CREDENTIAL",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|w| n.contains(w))
}

/// The home as written in `$HOME`, and its resolved path when `$HOME` goes
/// through a symlink. Mount sources are compared in both forms.
pub fn homes(env: &HostEnv) -> Vec<PathBuf> {
    let mut v = vec![env.home.clone()];
    if let Ok(real) = std::fs::canonicalize(&env.home) {
        if real != env.home {
            v.push(real);
        }
    }
    v
}

pub fn denied_paths(env: &HostEnv) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = homes(env)
        .iter()
        .flat_map(|h| DENIED_HOME.iter().map(move |p| h.join(p)))
        .collect();
    v.extend(DENIED_ABS.iter().map(PathBuf::from));
    if let Some(rt) = &env.runtime_dir {
        // The whole runtime directory holds the session bus, keyrings and the
        // display sockets. Only the Wayland socket may pass, and only for GUI.
        v.push(rt.clone());
    }
    v
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// The mounts of a device sandbox (devices/sandbox/spec.rs) that the rules
/// below let pass. The user named them in the user configuration on the
/// host, never in a project file.
///
/// - The instance folder under `<run>/devices/`: the socket of the
///   supervisor lives there, as the kv socket of an agent sandbox lives in
///   its run folder.
/// - `home = "read-only"` or `"full"` of the profile: the home itself.
/// - A read-only regular file of `ro` under a protected home path, such as
///   a known hosts file in `~/.ssh`. A folder there stays refused.
fn device_mount_passes(env: &HostEnv, m: &crate::spec::Mount) -> bool {
    let Some(src) = &m.src else { return false };
    if m.origin == crate::devices::sandbox::spec::INSTANCE_ORIGIN {
        return src.starts_with(env.paddock_run().join("devices"));
    }
    let profile = m.origin.starts_with("[devices.") && m.origin.contains(".sandbox] ");
    if !profile {
        return false;
    }
    if m.origin.ends_with(".sandbox] home") {
        return *src == env.home && m.dst == env.home;
    }
    if m.origin.ends_with(".sandbox] ro") && m.kind == MountKind::Ro {
        return std::fs::symlink_metadata(src).is_ok_and(|md| md.is_file())
            && src.parent().is_some_and(|p| p != Path::new("/"));
    }
    false
}

pub fn enforce(env: &HostEnv, spec: &SandboxSpec) -> Result<()> {
    let denied = denied_paths(env);
    let homes = homes(env);
    // The display socket is not a mount entry: gui.rs sets it only for the
    // nested compositor (window mode) or the approved host mode.

    for m in &spec.mounts {
        let Some(src) = &m.src else { continue };
        if src == Path::new("/") {
            bail!("policy: refusing to mount / (from {})", m.origin);
        }
        if device_mount_passes(env, m) {
            continue;
        }
        let real = std::fs::canonicalize(src).unwrap_or_else(|_| src.clone());
        for cand in [src, &real] {
            if cand == Path::new("/") {
                bail!("policy: refusing to mount / (from {})", m.origin);
            }
            if homes.iter().any(|h| h.starts_with(cand)) {
                bail!(
                    "policy: refusing to mount {}: it is the home directory or contains it (from {})",
                    cand.display(),
                    m.origin
                );
            }
            for d in &denied {
                if overlaps(cand, d) {
                    bail!(
                        "policy: refusing to mount {}: it overlaps the protected path {} (from {})",
                        cand.display(),
                        d.display(),
                        m.origin
                    );
                }
            }
        }
    }

    git::check(spec)?;

    for (k, v) in &spec.env {
        let explicit = ["profile", "manifest", "host env", "network", "credentials"]
            .iter()
            .any(|o| v.origin.starts_with(o));
        if looks_secret(k) && !explicit {
            bail!("policy: environment variable {k} looks like a secret and was not requested explicitly (from {})", v.origin);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Mount;
    use std::collections::BTreeMap;
    use std::process::Command;

    fn fake_env(home: &Path) -> HostEnv {
        HostEnv::from_parts(
            home.to_path_buf(),
            "tester".into(),
            4242,
            4242,
            BTreeMap::new(),
        )
    }

    fn spec(env: &HostEnv, project: &Path) -> SandboxSpec {
        SandboxSpec::new(
            "t".into(),
            "x",
            project,
            env.home.join("sandbox"),
            &env.home,
            1,
            1,
            "tester",
        )
    }

    #[test]
    fn refuses_home_and_denied_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let env = fake_env(tmp.path());
        let project = tmp.path().join("p");
        std::fs::create_dir_all(&project).unwrap();

        let mut s = spec(&env, &project);
        s.add_mount(Mount::same(tmp.path(), false, "test"));
        assert!(enforce(&env, &s).is_err());

        let mut s = spec(&env, &project);
        s.add_mount(Mount::same(tmp.path().join(".config"), false, "test"));
        assert!(enforce(&env, &s)
            .unwrap_err()
            .to_string()
            .contains("protected path"));

        let mut s = spec(&env, &project);
        s.add_mount(Mount::same("/var/run/docker.sock", true, "test"));
        assert!(enforce(&env, &s).is_err());

        let mut s = spec(&env, &project);
        s.add_mount(Mount::same(&project, true, "test"));
        assert!(enforce(&env, &s).is_ok());
    }

    #[test]
    fn a_symlinked_home_still_protects_its_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("home/.ssh")).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
        let env = fake_env(&tmp.path().join("link/home"));
        let project = tmp.path().join("p");
        std::fs::create_dir_all(&project).unwrap();
        for src in [real.join("home"), real.join("home/.ssh")] {
            let mut s = spec(&env, &project);
            s.add_mount(Mount::same(&src, false, "test"));
            assert!(enforce(&env, &s).is_err(), "{}", src.display());
        }
    }

    #[test]
    fn claude_app_control_channel_stays_outside() {
        assert!(!inheritable_var("CLAUDE_CODE_MESSAGING_TOKEN", "abc"));
        assert!(!inheritable_var(
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "/run/user/1000/cc-socks/1.sock"
        ));
        assert!(inheritable_var("EDITOR", "nvim"));
        assert!(!inheritable_var("CARGO_HOME", "/host/cargo"));
        assert!(!inheritable_var("UV_CACHE_DIR", "/host/uv"));
        assert!(!inheritable_var("XDG_CACHE_HOME", "/host/cache"));
    }

    #[test]
    fn refuses_writable_git_hooks() {
        let tmp = tempfile::tempdir().unwrap();
        let env = fake_env(tmp.path());
        let project = tmp.path().join("repo");
        std::fs::create_dir_all(&project).unwrap();
        let ok = Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&project)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut s = spec(&env, &project);
        s.add_mount(Mount::same(&project, true, "test project"));
        // The git directory must be a mount point of its own first.
        let e = enforce(&env, &s).unwrap_err().to_string();
        assert!(e.contains("mount point"), "{e}");
        s.add_mount(Mount::same(project.join(".git"), true, "test git dir"));
        let e = enforce(&env, &s).unwrap_err().to_string();
        assert!(e.contains("hooks"), "{e}");
        // What the git detector adds makes the spec pass.
        let detector_env = fake_env(tmp.path());
        let cx = crate::detect::Context {
            env: &detector_env,
            project: &project,
            manifest: &crate::manifest::Manifest::default(),
        };
        let mut s = spec(&env, &project);
        s.add_mount(Mount::same(&project, true, "test project"));
        crate::detect::Detector::apply(&crate::detect::Git, &cx, &mut s).unwrap();
        enforce(&env, &s).unwrap();
    }

    #[test]
    fn refuses_unrequested_secret_env() {
        let tmp = tempfile::tempdir().unwrap();
        let env = fake_env(tmp.path());
        let mut s = spec(&env, tmp.path());
        s.set_env("GITHUB_TOKEN", "x", "base");
        assert!(enforce(&env, &s).is_err());
        s.set_env("GITHUB_TOKEN", "x", "profile copilot env_pass");
        assert!(enforce(&env, &s).is_ok());
    }
}
