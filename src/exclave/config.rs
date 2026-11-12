//! Peer-owned settings. Project files and remote requests cannot widen these.
use super::sandbox::Limits;
use crate::env::HostEnv;
use anyhow::{bail, Context, Result};

pub struct ServeConfig {
    pub limits: Limits,
    pub registries: Vec<String>,
    pub http_registries: Vec<String>,
    pub private_registries: Vec<String>,
    pub allow_egress: bool,
    pub env: std::collections::BTreeMap<String, String>,
    pub cache_root: std::path::PathBuf,
    pub cache_bytes: u64,
    pub allow_shared_disk: bool,
    pub gpu: bool,
    pub ro_paths: Vec<std::path::PathBuf>,
    /// Expose the peer user's `~/.rustup` and `~/.cargo/bin` read-only.
    pub rustup: bool,
}

impl ServeConfig {
    pub fn load(env: &HostEnv) -> Result<Self> {
        let text = match std::fs::read_to_string(env.paddock_config().join("config.toml")) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => bail!("cannot read exclave user configuration"),
        };
        let doc: toml::Value =
            toml::from_str(&text).map_err(|_| anyhow::anyhow!("invalid user configuration"))?;
        let mut result = Self {
            limits: Limits::default(),
            registries: Vec::new(),
            http_registries: Vec::new(),
            private_registries: Vec::new(),
            allow_egress: false,
            env: Default::default(),
            cache_root: env.paddock_data().join("exclave"),
            cache_bytes: 20 * 1024 * 1024 * 1024,
            allow_shared_disk: false,
            gpu: false,
            ro_paths: Vec::new(),
            rustup: true,
        };
        let Some(serve) = doc.get("exclave").and_then(|v| v.get("serve")) else {
            return Ok(result);
        };
        let table = serve.as_table().context("exclave serve must be a table")?;
        for (key, value) in table {
            match key.as_str() {
                "ro_paths" => {
                    for item in value
                        .as_array()
                        .context("exclave ro_paths must be a list")?
                    {
                        let path = std::path::PathBuf::from(
                            item.as_str().context("invalid exclave ro_path")?,
                        );
                        validate_ro_path(env, &path)?;
                        result.ro_paths.push(path);
                    }
                }
                "devices" => {
                    let devices = value.as_array().context("invalid peer devices")?;
                    for device in devices {
                        if device.as_str() != Some("gpu") {
                            bail!("invalid peer device");
                        }
                        result.gpu = true;
                    }
                }
                "cache_root" => {
                    let p = std::path::PathBuf::from(
                        value.as_str().context("invalid exclave cache root")?,
                    );
                    if !p.is_absolute() {
                        bail!("exclave cache root must be absolute");
                    }
                    result.cache_root = p;
                }
                "cache_bytes" => {
                    result.cache_bytes = value
                        .as_integer()
                        .and_then(|n| u64::try_from(n).ok())
                        .filter(|n| *n > 0)
                        .context("invalid exclave cache size")?;
                }
                "rustup" => {
                    result.rustup = value.as_bool().context("invalid exclave rustup setting")?;
                }
                "allow_shared_disk" => {
                    result.allow_shared_disk =
                        value.as_bool().context("invalid shared disk setting")?;
                }
                "env" => {
                    let vars = value.as_table().context("exclave env must be a table")?;
                    for (key, value) in vars {
                        if key.is_empty()
                            || !key.bytes().enumerate().all(|(i, c)| {
                                c == b'_'
                                    || c.is_ascii_alphabetic()
                                    || (i > 0 && c.is_ascii_digit())
                            })
                        {
                            bail!("invalid exclave environment name");
                        }
                        let value = value
                            .as_str()
                            .context("invalid exclave environment value")?;
                        if value.contains('\0') {
                            bail!("invalid exclave environment value");
                        }
                        result.env.insert(key.clone(), value.to_owned());
                    }
                }
                "allow_egress" => {
                    result.allow_egress =
                        value.as_bool().context("invalid exclave egress setting")?
                }
                "memory_bytes" => {
                    result.limits.memory_bytes = value
                        .as_integer()
                        .filter(|n| *n > 0)
                        .context("invalid exclave memory limit")?
                        as u64
                }
                "cpu_percent" => {
                    result.limits.cpu_percent = value
                        .as_integer()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|n| *n > 0)
                        .context("invalid exclave CPU limit")?
                }
                "seconds" => {
                    result.limits.seconds = value
                        .as_integer()
                        .filter(|n| *n > 0)
                        .context("invalid exclave time limit")?
                        as u64
                }
                "private_registries" => {
                    let items = value
                        .as_array()
                        .context("private registries must be a list")?;
                    result.private_registries = items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .context("invalid private registry")
                        })
                        .collect::<Result<_>>()?;
                    let (_, bad) =
                        crate::proxy::allow::RegistryHttpPolicy::parse(&result.private_registries);
                    if !bad.is_empty() {
                        bail!("invalid private registry");
                    }
                }
                "http_registries" => {
                    let items = value.as_array().context("HTTP registries must be a list")?;
                    result.http_registries = items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .context("invalid HTTP registry")
                        })
                        .collect::<Result<_>>()?;
                    let (_, bad) =
                        crate::proxy::allow::RegistryHttpPolicy::parse(&result.http_registries);
                    if !bad.is_empty() {
                        bail!("invalid HTTP registry");
                    }
                }
                "registries" => {
                    let items = value
                        .as_array()
                        .context("exclave registries must be a list")?;
                    result.registries = items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .context("invalid exclave registry")
                        })
                        .collect::<Result<_>>()?;
                    let (_, bad) = crate::proxy::allow::Allowlist::parse(&result.registries);
                    if !bad.is_empty() {
                        bail!("invalid exclave registry");
                    }
                }
                _ => bail!("unknown exclave serve setting"),
            }
        }
        Ok(result)
    }
}

/// Validate both names and resolved targets, including paths containing protected data.
pub fn validate_ro_path(env: &HostEnv, path: &std::path::Path) -> Result<()> {
    use std::path::{Component, Path};
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!("exclave ro_path must be an absolute, normalized path");
    }
    let real = std::fs::canonicalize(path).context("exclave ro_path must exist")?;
    let mut denied = crate::policy::denied_paths(env);
    denied.extend(crate::policy::homes(env));
    denied.push(Path::new("/srv").to_path_buf());
    for candidate in [path, real.as_path()] {
        if candidate == Path::new("/")
            || candidate.components().any(|c| match c {
                Component::Normal(n) => {
                    let name = n.to_string_lossy();
                    crate::detect::is_secret_name(&name)
                        || matches!(
                            name.as_ref(),
                            ".ssh"
                                | ".gnupg"
                                | ".aws"
                                | ".docker"
                                | ".kube"
                                | "credentials"
                                | "credentials.toml"
                        )
                }
                _ => false,
            })
            || denied
                .iter()
                .any(|d| candidate.starts_with(d) || d.starts_with(candidate))
        {
            bail!("exclave ro_path overlaps a protected path");
        }
    }
    // Mounting over the boundary's own files would invalidate its isolation.
    for reserved in [
        "/usr", "/bin", "/lib", "/lib64", "/proc", "/dev", "/sys", "/work", "/cache", "/home",
        "/etc", "/run",
    ] {
        let reserved = Path::new(reserved);
        if path.starts_with(reserved) || reserved.starts_with(path) {
            bail!("exclave ro_path overlaps a sandbox path");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn owner_paths_refuse_protected_and_symlinked_targets() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let tools = root.path().join("tools");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&tools).unwrap();
        let env = HostEnv::from_parts(home.clone(), "test".into(), 1, 1, Default::default());
        assert!(validate_ro_path(&env, &tools).is_ok());
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        std::fs::write(
            env.paddock_config().join("config.toml"),
            format!(
                "[exclave.serve]\nro_paths = [{}]\n",
                toml::Value::String(tools.display().to_string())
            ),
        )
        .unwrap();
        assert_eq!(ServeConfig::load(&env).unwrap().ro_paths, [tools]);
        assert!(validate_ro_path(&env, &home).is_err());
        assert!(validate_ro_path(&env, std::path::Path::new("/srv")).is_err());
        assert!(validate_ro_path(&env, std::path::Path::new("relative")).is_err());
        let link = root.path().join("link");
        symlink(&home, &link).unwrap();
        assert!(validate_ro_path(&env, &link).is_err());
        let credentials = root.path().join("credentials");
        std::fs::create_dir(&credentials).unwrap();
        assert!(validate_ro_path(&env, &credentials).is_err());
    }

    #[test]
    fn the_rustup_binds_are_on_by_default_and_the_owner_can_turn_them_off() {
        let root = tempfile::tempdir().unwrap();
        let env = HostEnv::from_parts(root.path().into(), "test".into(), 1, 1, Default::default());
        assert!(ServeConfig::load(&env).unwrap().rustup);
        std::fs::create_dir_all(env.paddock_config()).unwrap();
        let file = env.paddock_config().join("config.toml");
        std::fs::write(&file, "[exclave.serve]\nrustup = false\n").unwrap();
        assert!(!ServeConfig::load(&env).unwrap().rustup);
        std::fs::write(&file, "[exclave.serve]\nrustup = true\n").unwrap();
        assert!(ServeConfig::load(&env).unwrap().rustup);
        std::fs::write(&file, "[exclave.serve]\nrustup = \"no\"\n").unwrap();
        assert!(ServeConfig::load(&env).is_err());
    }
}
