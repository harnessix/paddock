//! MCP policy preparation for sandbox launches.
use crate::build::Built;
use crate::env::HostEnv;
use anyhow::{bail, Result};
use std::path::Path;

pub(super) fn validate(env: &HostEnv, built: &Built) -> Result<()> {
    if !built.spec.intent.shell
        && !built.manifest.mcp.servers.is_empty()
        && !matches!(built.profile.name.as_str(), "codex" | "gemini")
    {
        let configured = crate::mcp::servers(env, &built.profile, &built.spec.project);
        crate::mcp::diet::check_supported(&built.profile.name, &configured, &built.manifest.mcp)?;
    }
    Ok(())
}

pub(super) fn prepare(env: &HostEnv, built: &mut Built, backend: &str) -> Result<()> {
    if built.profile.name == "codex" && !built.spec.intent.shell {
        let linux = cfg!(target_os = "linux") || matches!(backend, "vm" | "docker" | "podman");
        let overrides = crate::mcp::diet::codex_overrides(
            env,
            &built.profile,
            &built.spec.project,
            &built.manifest.mcp,
            linux,
        )?;
        if !overrides.is_empty() {
            if !built
                .spec
                .command
                .first()
                .is_some_and(|c| c == &built.harness_exe.to_string_lossy())
            {
                bail!("cannot safely apply Codex MCP policies through this command wrapper");
            }
            let explicit_mcp = built.spec.command.windows(2).any(|args| {
                matches!(args[0].as_str(), "-c" | "--config") && args[1].contains("mcp_servers")
            }) || built.spec.command.iter().any(|arg| {
                (arg.starts_with("--config=") || arg.starts_with("-c"))
                    && arg.contains("mcp_servers")
            });
            if explicit_mcp {
                bail!("cannot safely combine a Codex MCP startup policy with an explicit MCP config override");
            }
            crate::mcp::diet::probe_codex_override(&built.harness_exe, env)?;
            let args = overrides
                .into_iter()
                .flat_map(|value| ["-c".to_string(), value]);
            built.spec.command.splice(1..1, args);
        }
    }
    Ok(())
}

pub(super) fn apply(env: &HostEnv, built: &Built, backend: &str) -> Result<()> {
    let spec = &built.spec;
    if built.profile.name == "codex" {
        if let Some(shadow) = &spec.shadow {
            shadow.copy_up(Path::new(".codex/config.toml"))?;
        }
        crate::mcp::diet::filter_codex(
            &spec.home_src,
            &built.manifest.mcp,
            cfg!(target_os = "linux") || matches!(backend, "vm" | "docker" | "podman"),
        )?;
    }
    if built.profile.name == "gemini" {
        if let Some(shadow) = &spec.shadow {
            shadow.copy_up(Path::new(".gemini/settings.json"))?;
        }
        let configured = crate::mcp::servers(env, &built.profile, &spec.project);
        crate::mcp::diet::filter_gemini(
            &spec.home_src,
            &configured,
            &built.manifest.mcp,
            cfg!(target_os = "linux") || matches!(backend, "vm" | "docker" | "podman"),
        )?;
    }
    Ok(())
}
